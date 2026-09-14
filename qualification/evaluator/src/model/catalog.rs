//! Canonical capability declarations selected by qualification programs.

use super::{
    BTreeMap, BTreeSet, CapabilityDocument, ManifestDocument, Path, PathBuf, QUALIFICATION_SCHEMA,
    Result, Sha256, fs, slug, validate_relative_path,
};
use serde::Deserialize;
use sha2::Digest as _;

pub(crate) const CAPABILITY_CATALOG_SCHEMA: u16 = 1;

#[derive(Clone, Debug)]
pub(crate) struct SourceIdentity {
    pub(crate) id: String,
    pub(crate) schema: u16,
    pub(crate) path: PathBuf,
    pub(crate) sha256: String,
}

#[derive(Clone, Debug)]
pub(crate) enum CapabilityOrigin {
    Program,
    Catalog { id: String, path: PathBuf },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum CapabilityLevel {
    NativeSilicon,
    LowerPrimitive,
    ComposedProduct,
}

impl CapabilityLevel {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::NativeSilicon => "native-silicon",
            Self::LowerPrimitive => "lower-primitive",
            Self::ComposedProduct => "composed-product",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub(crate) struct CapabilityScope {
    pub(crate) chip: String,
    pub(crate) role: String,
    pub(crate) phy: String,
    pub(crate) security: Vec<String>,
    pub(crate) composition: String,
    pub(crate) level: CapabilityLevel,
    pub(crate) activation_boundary: String,
    pub(crate) limitations: String,
}

impl CapabilityScope {
    fn validate(&self, capability: &str) -> Result<()> {
        slug(&self.chip, "catalog scope chip")?;
        slug(&self.role, "catalog scope role")?;
        slug(&self.phy, "catalog scope PHY")?;
        slug(&self.composition, "catalog scope composition")?;
        if self.security.is_empty() {
            return Err(format!("catalog capability {capability} has no security scope").into());
        }
        let mut security = BTreeSet::new();
        for value in &self.security {
            let value = slug(value, "catalog security scope")?;
            if !security.insert(value.clone()) {
                return Err(format!(
                    "catalog capability {capability} repeats security scope {value}"
                )
                .into());
            }
        }
        if self.activation_boundary.trim().is_empty() || self.limitations.trim().is_empty() {
            return Err(format!(
                "catalog capability {capability} requires activation-boundary and limitations"
            )
            .into());
        }
        Ok(())
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct CatalogDocument {
    schema: u16,
    id: String,
    capabilities: Vec<CapabilityDocument>,
}

impl ManifestDocument {
    pub(super) fn resolve_catalogs(
        mut self,
        root: &Path,
        program_path: &Path,
        program_input: &str,
    ) -> Result<Self> {
        if self.schema != QUALIFICATION_SCHEMA {
            return Err(format!(
                "unsupported qualification schema {} (expected {QUALIFICATION_SCHEMA})",
                self.schema
            )
            .into());
        }
        self.program_source = Some(SourceIdentity {
            id: self.target.clone(),
            schema: self.schema,
            path: display_path(root, program_path),
            sha256: sha256(program_input.as_bytes()),
        });

        let mut origins = BTreeMap::new();
        let mut inline_ids = BTreeSet::new();
        for capability in &self.capabilities {
            let id = slug(&capability.id, "capability id")?;
            if !inline_ids.insert(id.clone()) {
                return Err(format!("qualification manifest repeats capability {id}").into());
            }
            if capability.catalog_scope.is_some() {
                return Err(format!(
                    "inline capability {id} cannot declare catalog-scope metadata"
                )
                .into());
            }
            origins.insert(id, CapabilityOrigin::Program);
        }

        if self.catalogs.is_empty() && self.catalog_capabilities.is_empty() {
            self.capability_origins = origins;
            return Ok(self);
        }
        if self.catalogs.is_empty() || self.catalog_capabilities.is_empty() {
            return Err(
                "catalogs and catalog-capabilities must either both be present or both be absent"
                    .into(),
            );
        }

        let mut catalog_paths = BTreeSet::new();
        let mut catalog_ids = BTreeSet::new();
        let mut declarations = BTreeMap::<String, (CapabilityDocument, String, PathBuf)>::new();
        for relative in &self.catalogs {
            validate_relative_path(relative)?;
            if !catalog_paths.insert(relative.clone()) {
                return Err(format!(
                    "qualification program repeats catalog {}",
                    relative.display()
                )
                .into());
            }
            let input = read_contained_file(root, relative, "capability catalog")?;
            let document: CatalogDocument = toml_edit::de::from_str(&input).map_err(|error| {
                format!(
                    "cannot parse capability catalog {}: {error}",
                    relative.display()
                )
            })?;
            if document.schema != CAPABILITY_CATALOG_SCHEMA {
                return Err(format!(
                    "unsupported capability catalog schema {} in {} (expected {CAPABILITY_CATALOG_SCHEMA})",
                    document.schema,
                    relative.display()
                )
                .into());
            }
            let catalog_id = slug(&document.id, "capability catalog id")?;
            if !catalog_ids.insert(catalog_id.clone()) {
                return Err(format!("duplicate capability catalog id {catalog_id}").into());
            }
            if document.capabilities.is_empty() {
                return Err(format!("capability catalog {catalog_id} is empty").into());
            }
            self.catalog_sources.push(SourceIdentity {
                id: catalog_id.clone(),
                schema: document.schema,
                path: relative.clone(),
                sha256: sha256(input.as_bytes()),
            });
            for capability in document.capabilities {
                let id = slug(&capability.id, "capability id")?;
                let scope = capability.catalog_scope.as_ref().ok_or_else(|| {
                    format!("catalog capability {id} requires catalog-scope metadata")
                })?;
                scope.validate(&id)?;
                if inline_ids.contains(&id) {
                    return Err(format!(
                        "capability {id} is declared both inline and in catalog {catalog_id}"
                    )
                    .into());
                }
                if let Some((_, previous, _)) = declarations.insert(
                    id.clone(),
                    (capability, catalog_id.clone(), relative.clone()),
                ) {
                    return Err(format!(
                        "capability {id} is declared by both catalog {previous} and {catalog_id}"
                    )
                    .into());
                }
            }
        }

        let selected = self
            .catalog_capabilities
            .iter()
            .map(|id| slug(id, "catalog capability reference"))
            .collect::<Result<Vec<_>>>()?;
        let mut referenced = BTreeSet::new();
        for id in &selected {
            if !referenced.insert(id.clone()) {
                return Err(
                    format!("qualification program repeats catalog capability {id}").into(),
                );
            }
            if !declarations.contains_key(id) {
                return Err(format!("unknown catalog capability {id}").into());
            }
        }

        let mut resolved = BTreeSet::new();
        for id in selected {
            resolve_capability(&id, &declarations, &mut resolved);
        }
        for id in resolved {
            let (capability, catalog_id, path) = declarations
                .get(&id)
                .expect("resolved IDs originate in the declaration index");
            self.capabilities.push(capability.clone());
            self.catalog_scopes.insert(
                id.clone(),
                capability
                    .catalog_scope
                    .clone()
                    .expect("catalog declarations require structured scope"),
            );
            origins.insert(
                id,
                CapabilityOrigin::Catalog {
                    id: catalog_id.clone(),
                    path: path.clone(),
                },
            );
        }
        self.capability_origins = origins;
        Ok(self)
    }
}

fn resolve_capability(
    id: &str,
    declarations: &BTreeMap<String, (CapabilityDocument, String, PathBuf)>,
    resolved: &mut BTreeSet<String>,
) {
    if !resolved.insert(id.to_owned()) {
        return;
    }
    let capability = &declarations[id].0;
    for dependency in &capability.depends_on {
        if declarations.contains_key(dependency) {
            resolve_capability(dependency, declarations, resolved);
        }
    }
}

fn read_contained_file(root: &Path, relative: &Path, kind: &str) -> Result<String> {
    let mut path = fs::canonicalize(root)?;
    let components = relative.components().collect::<Vec<_>>();
    for (index, component) in components.iter().enumerate() {
        let std::path::Component::Normal(name) = component else {
            return Err(format!("{kind} must be a contained repository-relative path").into());
        };
        path.push(name);
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| format!("cannot inspect {kind} {}: {error}", relative.display()))?;
        let final_component = index + 1 == components.len();
        if metadata.file_type().is_symlink()
            || (final_component && !metadata.file_type().is_file())
            || (!final_component && !metadata.file_type().is_dir())
        {
            return Err(format!(
                "{kind} path must contain only regular directories and end in a regular file: {}",
                relative.display()
            )
            .into());
        }
    }
    fs::read_to_string(&path)
        .map_err(|error| format!("cannot read {kind} {}: {error}", relative.display()).into())
}

fn display_path(root: &Path, path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_owned()
    } else {
        root.join(path)
    };
    absolute
        .strip_prefix(root)
        .map_or_else(|_| absolute.clone(), Path::to_path_buf)
}

fn sha256(input: &[u8]) -> String {
    format!("{:x}", Sha256::digest(input))
}

#[cfg(test)]
mod tests;
