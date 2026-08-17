//! Reusable chip-specific address, register-catalog and compiled knowledge.

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

use serde::Deserialize;

use crate::{Result, error::BlobrayError, interfaces::SemanticCatalogs, target::TargetSpec};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ChipPack {
    pub(crate) path: PathBuf,
    pub(crate) id: String,
    pub(crate) memory_map: Option<PathBuf>,
    pub(crate) svd_paths: Vec<PathBuf>,
    pub(crate) knowledge_provider: Option<String>,
    pub(crate) knowledge_packs: Vec<PathBuf>,
    pub(crate) knowledge_operations: usize,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct ChipDocument {
    schema: u32,
    id: String,
    #[serde(default)]
    memory_map: Option<String>,
    #[serde(default)]
    svd: Vec<String>,
    #[serde(default)]
    knowledge_provider: Option<String>,
    #[serde(default)]
    knowledge_packs: Vec<String>,
}

impl ChipPack {
    #[tracing::instrument(name = "load_chip_pack", fields(path = %path.display()))]
    pub(crate) fn load(path: &Path) -> Result<Self> {
        let path = path
            .canonicalize()
            .map_err(|error| BlobrayError::manifest("chip pack", path, error))?;
        let input = fs::read_to_string(&path)?;
        let document: ChipDocument = toml_edit::de::from_str(&input).map_err(|error| {
            BlobrayError::manifest_source("chip pack", &path, &input, &error, error.span())
        })?;
        if document.schema != 3 {
            return Err(crate::Error::invalid(format!(
                "chip pack {} requires schema = 3",
                path.display()
            )));
        }
        validate_id(&document.id)?;
        if document
            .knowledge_provider
            .as_ref()
            .is_some_and(|provider| {
                provider.is_empty() || provider.chars().any(char::is_whitespace)
            })
        {
            return Err(crate::Error::invalid(format!(
                "chip pack {} knowledge-provider must be one non-empty token",
                path.display()
            )));
        }

        let base = path.parent().unwrap_or_else(|| Path::new("."));
        let memory_map = document
            .memory_map
            .as_deref()
            .map(|value| resolve_relative(base, value, "memory-map"))
            .transpose()?;
        let svd_paths = resolve_unique_paths(base, &document.svd, "svd")?;
        let knowledge_packs =
            resolve_unique_paths(base, &document.knowledge_packs, "knowledge-packs")?;
        let knowledge_operations = SemanticCatalogs::load(&knowledge_packs)?.len();

        Ok(Self {
            path,
            id: document.id,
            memory_map,
            svd_paths,
            knowledge_provider: document.knowledge_provider,
            knowledge_packs,
            knowledge_operations,
        })
    }

    pub(crate) fn apply_to_target(&self, target: &mut TargetSpec) -> Result<()> {
        if let Some(provider) = &self.knowledge_provider {
            if target.knowledge_provider.is_some() {
                return Err(crate::Error::invalid(format!(
                    "target {} already has a knowledge provider before applying chip pack {:?}",
                    target.id, self.id
                )));
            }
            target.knowledge_provider = Some(provider.clone());
        }
        Ok(())
    }
}

fn resolve_unique_paths(base: &Path, values: &[String], key: &str) -> Result<Vec<PathBuf>> {
    let mut unique = BTreeSet::new();
    values
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let path = resolve_relative(base, value, &format!("{key}[{index}]"))?;
            if !unique.insert(path.clone()) {
                return Err(crate::Error::invalid(format!(
                    "chip pack contains duplicate {key} path {}",
                    path.display()
                )));
            }
            Ok(path)
        })
        .collect()
}

fn resolve_relative(base: &Path, value: &str, key: &str) -> Result<PathBuf> {
    let relative = Path::new(value);
    if value.is_empty() || relative.is_absolute() {
        return Err(crate::Error::invalid(format!(
            "chip pack {key} must be a non-empty relative path"
        )));
    }
    Ok(base.join(relative))
}

fn validate_id(value: &str) -> Result<()> {
    if value.is_empty()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err(crate::Error::invalid(format!(
            "invalid chip pack id {value:?}"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owns_chip_resources_and_rejects_project_independent_absolute_paths() {
        let root = std::env::temp_dir().join(format!("blobray-chip-pack-{}", std::process::id()));
        if root.exists() {
            fs::remove_dir_all(&root).unwrap();
        }
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("memory.toml"), "schema = 1\n").unwrap();
        fs::write(root.join("chip.svd"), "<device/>").unwrap();
        fs::write(
            root.join("chip.toml"),
            "schema = 3\nid = \"chip\"\nmemory-map = \"memory.toml\"\nsvd = [\"chip.svd\"]\nknowledge-provider = \"chip-knowledge-v1\"\n",
        )
        .unwrap();

        let pack = ChipPack::load(&root.join("chip.toml")).unwrap();
        assert_eq!(pack.memory_map, Some(root.join("memory.toml")));
        assert_eq!(pack.svd_paths, [root.join("chip.svd")]);
        assert_eq!(
            pack.knowledge_provider.as_deref(),
            Some("chip-knowledge-v1")
        );

        fs::write(
            root.join("invalid.toml"),
            "schema = 3\nid = \"chip\"\nmemory-map = \"/private/map.toml\"\n",
        )
        .unwrap();
        assert!(ChipPack::load(&root.join("invalid.toml")).is_err());
        fs::remove_dir_all(root).unwrap();
    }
}
