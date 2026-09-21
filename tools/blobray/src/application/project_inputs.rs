//! Shared project source declarations and operation-specific input resolution.
use crate::artifact::{self, ArtifactContainerKind};
use crate::{
    Result,
    project::ProjectSpec,
    project_ir::ProjectIrProfile,
    run_spec::{InputRole, RunSpec},
};
use std::{collections::BTreeSet, fs, path::PathBuf};

pub(crate) fn known_sources(project: &ProjectSpec) -> BTreeSet<String> {
    let mut sources = project
        .ir_profiles
        .iter()
        .flat_map(|profile| profile.sources.iter().cloned())
        .collect::<BTreeSet<_>>();
    if let Some(workspace) = &project.verification {
        sources.extend(
            workspace
                .suites
                .iter()
                .flat_map(|suite| suite.vendor.iter().map(|vendor| vendor.source.to_string())),
        );
        sources.extend(
            workspace
                .suites
                .iter()
                .flat_map(|suite| suite.auxiliary_sources.iter().map(ToString::to_string)),
        );
    }
    sources.extend(
        project
            .analysis_symbol_families
            .iter()
            .map(|family| family.source.clone()),
    );
    sources
}

pub(crate) fn required_roles(project: &ProjectSpec) -> BTreeSet<InputRole> {
    let required_sources = project
        .ir_profiles
        .iter()
        .flat_map(|profile| profile.sources.iter())
        .chain(
            project
                .analysis_symbol_families
                .iter()
                .filter(|family| {
                    family.disposition == crate::project::AnalysisSymbolFamilyDisposition::Required
                })
                .map(|family| &family.source),
        )
        .cloned()
        .chain(
            project
                .verification
                .iter()
                .flat_map(|workspace| workspace.suites.iter())
                .flat_map(|suite| {
                    suite
                        .vendor
                        .iter()
                        .map(|vendor| vendor.source.to_string())
                        .chain(suite.auxiliary_sources.iter().map(ToString::to_string))
                }),
        )
        .collect::<BTreeSet<_>>();
    let mut roles = required_sources
        .iter()
        .map(|source| InputRole::SourceArtifact(source.parse().expect("validated project source")))
        .collect::<BTreeSet<_>>();
    if let Some(workspace) = &project.verification {
        for suite in &workspace.suites {
            roles.insert(suite.rust_artifact_role.clone());
            if let Some(role) = &suite.rust_companion_role {
                roles.insert(role.clone());
            }
        }
    }
    roles
}

/// Executable comparison currently accepts exactly one input for each role.
/// Repeated source bindings are valid configuration, but cannot be truncated.
pub(crate) fn single_input(
    run: &RunSpec,
    role: &InputRole,
    operation: &str,
) -> Result<Option<PathBuf>> {
    let mut inputs = run.inputs().iter().filter(|input| &input.role == role);
    let first = inputs.next().map(|input| input.path.clone());
    if inputs.next().is_some() {
        return Err(crate::Error::invalid(format!(
            "{operation} supports exactly one {role} input; the run spec binds multiple artifacts"
        )));
    }
    Ok(first)
}

#[derive(Debug)]
pub(crate) struct ResolvedInputs {
    pub(crate) artifacts: Vec<(String, PathBuf)>,
    pub(crate) inventories: Vec<(String, PathBuf)>,
    pub(crate) companions: Vec<PathBuf>,
    pub(crate) source_companions: Vec<(String, PathBuf)>,
}

pub(crate) fn resolve_inputs(
    profile: &ProjectIrProfile,
    run_spec: &RunSpec,
) -> Result<ResolvedInputs> {
    let bound = run_spec
        .inputs()
        .iter()
        .filter_map(|input| {
            let InputRole::SourceArtifact(source) = &input.role else {
                return None;
            };
            Some((source.as_str(), &input.path))
        })
        .fold(
            std::collections::BTreeMap::<_, Vec<_>>::new(),
            |mut bound, (source, path)| {
                bound.entry(source).or_default().push(path);
                bound
            },
        );
    let artifacts = if profile.sources.is_empty() {
        run_spec
            .inputs()
            .iter()
            .filter_map(|input| {
                let InputRole::SourceArtifact(source) = &input.role else {
                    return None;
                };
                Some((source.to_string(), input.path.clone()))
            })
            .collect::<Vec<_>>()
    } else {
        profile
            .sources
            .iter()
            .flat_map(|source| match bound.get(source.as_str()) {
                Some(paths) => paths
                    .iter()
                    .map(|path| Ok((source.clone(), (*path).clone())))
                    .collect::<Vec<_>>(),
                None => vec![Err(crate::Error::invalid(format!(
                    "IR profile {:?} requests missing run-spec role source-artifact:{source}",
                    profile.id
                )))],
            })
            .collect::<Result<Vec<_>>>()?
    };
    if artifacts.is_empty() {
        return Err(crate::Error::invalid(format!(
            "IR profile {:?} has no source-artifact bindings in the run spec",
            profile.id
        )));
    }

    let companions = run_spec
        .inputs()
        .iter()
        .filter(|input| input.role == InputRole::Companion)
        .map(|input| input.path.clone())
        .collect::<Vec<_>>();
    if artifacts.len() > 1 && !companions.is_empty() {
        return Err(crate::Error::invalid(format!(
            "IR profile {:?} selects multiple sources but the run spec has a global companion",
            profile.id
        )));
    }
    let source_companions = run_spec
        .inputs()
        .iter()
        .filter_map(|input| match &input.role {
            InputRole::SourceCompanion(source)
                if artifacts.iter().any(|(owner, _)| owner == source.as_str()) =>
            {
                Some((source.to_string(), input.path.clone()))
            }
            _ => None,
        })
        .collect();
    Ok(ResolvedInputs {
        artifacts,
        source_companions,
        inventories: run_spec
            .inputs()
            .iter()
            .filter_map(|input| match &input.role {
                InputRole::SourceInventory(source)
                    if profile.sources.is_empty()
                        || profile
                            .sources
                            .iter()
                            .any(|candidate| candidate == source.as_str()) =>
                {
                    Some((source.to_string(), input.path.clone()))
                }
                _ => None,
            })
            .collect(),
        companions,
    })
}

pub(crate) struct ResolvedBinding {
    pub(crate) role: InputRole,
    pub(crate) path: PathBuf,
    pub(crate) container: ArtifactContainerKind,
}

pub(crate) fn resolve_bindings(
    bindings: Vec<crate::run_spec::RunInput>,
    known_sources: &BTreeSet<String>,
    required_roles: &BTreeSet<InputRole>,
) -> Result<Vec<ResolvedBinding>> {
    let mut roles = BTreeSet::new();
    let mut resolved = Vec::with_capacity(bindings.len());
    let mut identities = BTreeSet::new();
    for binding in bindings {
        if !binding.role.is_repeatable() && !roles.insert(binding.role.clone()) {
            return Err(crate::Error::invalid(format!(
                "duplicate project input role {}",
                binding.role
            )));
        }
        if let Some(source) = binding.role.qualified_source_id()
            && !known_sources.is_empty()
            && !known_sources.contains(source)
        {
            return Err(crate::Error::invalid(format!(
                "input role {} references source {source:?}, which is absent from project analysis and verification suites",
                binding.role
            )));
        }
        let path = fs::canonicalize(&binding.path).map_err(|error| {
            crate::Error::invalid(format!(
                "cannot resolve project input {}: {error}",
                binding.path.display()
            ))
        })?;
        if !identities.insert((binding.role.clone(), path.clone())) {
            return Err(crate::Error::invalid(format!(
                "duplicate artifact binding {}={}",
                binding.role,
                path.display()
            )));
        }
        let inventory = artifact::inspect_artifact(&path)?;
        resolved.push(ResolvedBinding {
            role: binding.role,
            path,
            container: inventory.container,
        });
    }
    for role in required_roles {
        if !resolved.iter().any(|binding| &binding.role == role) {
            return Err(crate::Error::invalid(format!(
                "project requires --bind {role}=PATH"
            )));
        }
    }
    Ok(resolved)
}

/// Address-based reference companions require a linked primary image.
/// Primary archive sets instead resolve relocations between their own members.
pub(crate) fn validate_ir_context(inputs: &ResolvedInputs) -> Result<()> {
    for (source, path) in &inputs.artifacts {
        if inputs.companions.is_empty()
            && !inputs
                .source_companions
                .iter()
                .any(|(owner, _)| owner == source)
        {
            continue;
        }
        let inventory = artifact::inspect_artifact(path)?;
        if inventory.container != ArtifactContainerKind::Elf32
            || !inventory.objects.iter().any(|object| {
                matches!(
                    object.kind,
                    artifact::ArtifactObjectKind::Executable
                        | artifact::ArtifactObjectKind::Dynamic
                )
            })
        {
            return Err(crate::Error::invalid(format!(
                "IR reference resolution for source {source:?} with companions requires a linked ELF primary: {}; bind relocatable code archives as an ordered source-artifact set",
                path.display()
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn single_executable_operations_reject_an_ordered_set_without_truncating_it() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("local.toml");
        fs::write(&path, "schema = 1\n[[inputs]]\nrole = \"source-artifact:vendor\"\npath = \"z.a\"\n[[inputs]]\nrole = \"source-artifact:vendor\"\npath = \"a.a\"\n").unwrap();
        let run = RunSpec::load(&path).unwrap();
        let error = single_input(
            &run,
            &InputRole::SourceArtifact("vendor".parse().unwrap()),
            "bounded comparison",
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("bounded comparison"));
        assert!(error.contains("exactly one"));
        assert!(run.inputs()[0].path.ends_with("z.a"));
        assert!(run.inputs()[1].path.ends_with("a.a"));
    }
}
