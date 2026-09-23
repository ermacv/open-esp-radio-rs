//! Run-owned output declarations and exact producer lookup.
//!
//! This preflight prevents known conflicting destinations before analysis or CAS
//! restore. It is not a pinned output capability or an atomic publication layer.

use super::{
    pass_spec::{CachePass, PassKind},
    work::ResolvedWork,
};
use crate::{Result, function_workspace::ReviewedEventReplay, project::ProjectSpec};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

pub(super) struct ReplayDeclarations(std::result::Result<Vec<ReviewedEventReplay>, String>);
impl ReplayDeclarations {
    pub(super) fn capture(project: &ProjectSpec) -> Self {
        Self(
            (|| -> Result<Vec<ReviewedEventReplay>> {
                let Some(functions) = &project.functions else {
                    return Ok(Vec::new());
                };
                let pack = crate::function_workspace::FunctionPack::load_reviewed(&functions.pack)?;
                let replays = pack
                    .event_routes
                    .iter()
                    .filter_map(crate::function_workspace::ReviewedEventRoute::replay)
                    .collect::<Vec<_>>();
                ensure_unique_replay_outputs(replays.iter().copied())?;
                let mut unique = BTreeMap::new();
                for replay in replays {
                    unique
                        .entry((
                            replay.manifest.clone(),
                            replay.source.clone(),
                            replay.evidence.clone(),
                        ))
                        .or_insert_with(|| replay.clone());
                }
                Ok(unique.into_values().collect())
            })()
            .map_err(|error| error.to_string()),
        )
    }
    pub(super) fn records(&self) -> Result<&[ReviewedEventReplay]> {
        self.0
            .as_deref()
            .map_err(|reason| crate::Error::invalid(reason.clone()))
    }
}

pub(super) fn ensure_unique_replay_outputs<'a>(
    replays: impl IntoIterator<Item = &'a ReviewedEventReplay>,
) -> Result<()> {
    let mut owners = BTreeMap::new();
    for replay in replays {
        let identity = (replay.manifest.clone(), replay.source.clone());
        if let Some(previous) = owners.insert(replay.evidence.clone(), identity.clone())
            && previous != identity
        {
            return Err(crate::Error::invalid(format!(
                "event replay evidence output {} is assigned to both manifest/source {:?} and {:?}",
                replay.evidence.display(),
                previous,
                identity
            )));
        }
    }
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct OutputOwner {
    pub(super) pass: PassKind,
    pub(super) work: String,
}
impl OutputOwner {
    fn new(key: &str) -> Result<Self> {
        Ok(Self {
            pass: CachePass::parse(key)?.spec.kind,
            work: key.to_owned(),
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OutputKind {
    File,
    Bundle,
}

struct Claim {
    declared: PathBuf,
    location: PathBuf,
    kind: OutputKind,
    owner: OutputOwner,
}

pub(super) struct OutputCatalog {
    claims: Vec<Claim>,
    lookup: BTreeMap<PathBuf, OutputOwner>,
    work_outputs: BTreeMap<String, Vec<PathBuf>>,
    unavailable: BTreeMap<String, String>,
    protected: Vec<PathBuf>,
}

impl OutputCatalog {
    pub(super) fn capture(
        project: &ProjectSpec,
        protected: &[PathBuf],
        replays: &ReplayDeclarations,
    ) -> Result<Self> {
        let mut catalog = Self {
            claims: vec![],
            lookup: BTreeMap::new(),
            work_outputs: BTreeMap::new(),
            unavailable: BTreeMap::new(),
            protected: protected.to_vec(),
        };
        if let Some(spec) = &project.symbol_inventory {
            catalog.file("symbol-inventory", &spec.output)?;
        }
        if let Some(spec) = &project.registers {
            catalog.file("mmio-discovery", &spec.facts)?;
            if let Some(path) = &spec.review_output {
                catalog.file("register-review", path)?;
            }
        }
        if let Some(spec) = &project.interfaces {
            catalog.file("interface-discovery", &spec.facts)?;
            if let Some(path) = &spec.capability_context {
                catalog.file("interface-capability-context", path)?;
            }
        }
        for profile in &project.ir_profiles {
            let key = format!("linked-ir:{}", profile.id);
            catalog.claim(&key, &profile.output, OutputKind::Bundle)?;
            for path in crate::artifacts::bundle_files(&profile.output).chain(std::iter::once(
                profile.output.join(crate::application::coverage::FILE),
            )) {
                catalog.file(&key, &path)?;
            }
        }
        match replays.records() {
            Ok(replays) => {
                for replay in replays {
                    catalog.file("event-replays", &replay.evidence)?;
                }
            }
            Err(error) => {
                catalog
                    .unavailable
                    .insert("event-replays".into(), error.to_string());
            }
        }
        if let Some(spec) = &project.review {
            catalog.file("review-scopes", &spec.output)?;
        }
        if let Some(spec) = &project.navigation_index {
            catalog.file("navigation-index", &spec.output)?;
        }
        if let Some(path) = project
            .code
            .as_ref()
            .and_then(|spec| spec.review_output.as_ref())
        {
            catalog.file("code-boundary-review", path)?;
        }
        if let Some(path) = project
            .functions
            .as_ref()
            .and_then(|spec| spec.review_output.as_ref())
        {
            catalog.file("function-review", path)?;
        }
        Ok(catalog)
    }

    fn file(&mut self, key: &str, path: &Path) -> Result<()> {
        self.claim(key, path, OutputKind::File)?;
        self.work_outputs
            .entry(key.to_owned())
            .or_default()
            .push(path.to_owned());
        Ok(())
    }

    fn claim(&mut self, key: &str, path: &Path, kind: OutputKind) -> Result<()> {
        let owner = OutputOwner::new(key)?;
        let location = resolve_location(path)?;
        self.check_protected(path, &location)?;
        for prior in &self.claims {
            let nested_member = prior.owner == owner
                && prior.kind == OutputKind::Bundle
                && kind == OutputKind::File
                && location != prior.location
                && location.starts_with(&prior.location);
            if !nested_member
                && (overlaps(&location, &prior.location)
                    || same_existing_file(path, &prior.declared)?)
            {
                return Err(crate::Error::invalid(format!(
                    "conflicting output ownership: {} ({}) and {} ({})",
                    prior.declared.display(),
                    prior.owner.work,
                    path.display(),
                    owner.work
                )));
            }
        }
        self.lookup.insert(location.clone(), owner.clone());
        self.claims.push(Claim {
            declared: path.to_owned(),
            location,
            kind,
            owner,
        });
        Ok(())
    }

    fn check_protected(&self, path: &Path, location: &Path) -> Result<()> {
        for input in &self.protected {
            let source = resolve_location(input).map_err(|error| {
                crate::Error::invalid(format!(
                    "analysis input {} cannot be resolved for output ownership: {error}",
                    input.display()
                ))
            })?;
            if overlaps(location, &source) || same_existing_file(path, input)? {
                return Err(crate::Error::invalid(format!(
                    "output {} conflicts with protected analysis input {}",
                    path.display(),
                    input.display()
                )));
            }
        }
        Ok(())
    }

    /// Exact file or declared bundle root only. A parent directory, sibling or
    /// undeclared child never inherits a producer by path prefix.
    pub(super) fn producer(&self, input: &Path) -> Result<Option<&OutputOwner>> {
        Ok(self.lookup.get(&resolve_location(input)?))
    }

    pub(super) fn validate(&self, work: &ResolvedWork) -> Result<()> {
        if let Some(reason) = self.unavailable.get(work.key()) {
            return Err(crate::Error::invalid(reason.clone()));
        }
        let declared = self
            .work_outputs
            .get(work.key())
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        if declared != work.outputs() {
            return Err(crate::Error::invalid(format!(
                "work {:?} changed its declared output set or order",
                work.key()
            )));
        }
        for claim in self
            .claims
            .iter()
            .filter(|claim| claim.owner.work == work.key())
        {
            let current = resolve_location(&claim.declared)?;
            if current != claim.location {
                return Err(crate::Error::invalid(format!(
                    "output binding changed after ownership capture: {}",
                    claim.declared.display()
                )));
            }
            self.check_protected(&claim.declared, &current)?;
            for other in &self.claims {
                if other.owner != claim.owner
                    && same_existing_file(&claim.declared, &other.declared)?
                {
                    return Err(crate::Error::invalid(format!(
                        "output alias changed after ownership capture: {} and {}",
                        claim.declared.display(),
                        other.declared.display()
                    )));
                }
            }
        }
        Ok(())
    }
}

fn overlaps(a: &Path, b: &Path) -> bool {
    a.starts_with(b) || b.starts_with(a)
}

/// Resolve existing symlinks before interpreting `..`, retaining absent tails.
/// Unresolvable links and non-directory ancestors are explicit errors.
fn resolve_location(path: &Path) -> Result<PathBuf> {
    use std::path::Component;
    let absolute = std::path::absolute(path)?;
    let mut resolved = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => resolved.push(component.as_os_str()),
            Component::CurDir => (),
            Component::ParentDir => {
                match std::fs::metadata(&resolved) {
                    Ok(metadata) if !metadata.is_dir() => {
                        return Err(crate::Error::invalid(format!(
                            "output path ancestor {} is not a directory",
                            resolved.display()
                        )));
                    }
                    Ok(_) => (),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
                    Err(error) => return Err(error.into()),
                }
                resolved.pop();
            }
            Component::Normal(part) => {
                resolved.push(part);
                match std::fs::symlink_metadata(&resolved) {
                    Ok(metadata) if metadata.file_type().is_symlink() => {
                        resolved = std::fs::canonicalize(&resolved)?;
                    }
                    Ok(_) => (),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
                    Err(error) => return Err(error.into()),
                }
            }
        }
    }
    Ok(resolved)
}

fn same_existing_file(a: &Path, b: &Path) -> Result<bool> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let identity = |path: &Path| -> Result<Option<(u64, u64)>> {
            match std::fs::metadata(path) {
                Ok(value) if value.is_file() => Ok(Some((value.dev(), value.ino()))),
                Ok(_) => Ok(None),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(error) => Err(error.into()),
            }
        };
        Ok(match (identity(a)?, identity(b)?) {
            (Some(a), Some(b)) => a == b,
            _ => false,
        })
    }
    #[cfg(not(unix))]
    {
        let _ = (a, b);
        Ok(false)
    }
}

#[cfg(test)]
mod tests;
