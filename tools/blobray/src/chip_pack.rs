//! Reusable chip-specific address, register-catalog and compiled knowledge.

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

use serde::Deserialize;

use crate::{
    Result,
    error::BlobrayError,
    interfaces::{CapabilityRuleSet, SemanticCatalogs},
    target::TargetSpec,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ChipPack {
    pub(crate) path: PathBuf,
    pub(crate) id: String,
    pub(crate) applicability: open_radio_vendor_review::Applicability,
    pub(crate) memory_map: Option<PathBuf>,
    pub(crate) svd_paths: Vec<PathBuf>,
    pub(crate) register_model: Option<PathBuf>,
    pub(crate) knowledge_provider: Option<String>,
    pub(crate) knowledge_packs: Vec<PathBuf>,
    pub(crate) knowledge_operations: usize,
    pub(crate) capability_packs: Vec<PathBuf>,
    pub(crate) capability_rules: usize,
    pub(crate) interface_template_packs: Vec<PathBuf>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct ChipDocument {
    schema: u32,
    id: String,
    #[serde(default)]
    applicability: open_radio_vendor_review::Applicability,
    #[serde(default)]
    memory_map: Option<String>,
    #[serde(default)]
    svd: Vec<String>,
    #[serde(default)]
    register_model: Option<String>,
    #[serde(default)]
    knowledge_provider: Option<String>,
    #[serde(default)]
    knowledge_packs: Vec<String>,
    #[serde(default)]
    capability_packs: Vec<String>,
    #[serde(default)]
    interface_template_packs: Vec<String>,
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
        validate_applicability(&path, &document.applicability)?;
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
        let register_model = document
            .register_model
            .as_deref()
            .map(|value| resolve_relative(base, value, "register-model"))
            .transpose()?;
        let knowledge_packs =
            resolve_unique_paths(base, &document.knowledge_packs, "knowledge-packs")?;
        let knowledge_operations = SemanticCatalogs::load(&knowledge_packs)?.len();
        let capability_packs =
            resolve_unique_paths(base, &document.capability_packs, "capability-packs")?;
        let capability_rules = CapabilityRuleSet::load(&capability_packs)?.len();
        let interface_template_packs = resolve_unique_paths(
            base,
            &document.interface_template_packs,
            "interface-template-packs",
        )?;

        Ok(Self {
            path,
            id: document.id,
            applicability: document.applicability,
            memory_map,
            svd_paths,
            register_model,
            knowledge_provider: document.knowledge_provider,
            knowledge_packs,
            knowledge_operations,
            capability_packs,
            capability_rules,
            interface_template_packs,
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

fn validate_applicability(
    path: &Path,
    applicability: &open_radio_vendor_review::Applicability,
) -> Result<()> {
    if !applicability.ecosystems.is_empty()
        || !applicability.artifact_lineages.is_empty()
        || !applicability.artifacts.is_empty()
    {
        return Err(crate::Error::invalid(format!(
            "chip pack {} applicability may contain only chips and chip-revisions",
            path.display()
        )));
    }
    for (name, values) in [
        ("chips", &applicability.chips),
        ("chip-revisions", &applicability.chip_revisions),
    ] {
        let mut unique = BTreeSet::new();
        if values
            .iter()
            .any(|value| value.trim().is_empty() || !unique.insert(value))
        {
            return Err(crate::Error::invalid(format!(
                "chip pack {} applicability {name} must be non-empty and unique",
                path.display()
            )));
        }
    }
    Ok(())
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
        fs::write(root.join("registers.toml"), "schema = 2\n").unwrap();
        fs::write(
            root.join("capabilities.toml"),
            "schema = 1\nid = \"fixture.capabilities\"\n[[rules]]\nid = \"fixture.radio.ready\"\nprotocol = \"radio\"\nscope = \"runtime\"\nsummary = \"Reviewed radio boundary\"\n[[rules.requirements]]\nkind = \"operation\"\nvalue = \"radio.ready\"\n",
        )
        .unwrap();
        fs::write(
            root.join("chip.toml"),
            "schema = 3\nid = \"chip\"\nmemory-map = \"memory.toml\"\nsvd = [\"chip.svd\"]\nregister-model = \"registers.toml\"\nknowledge-provider = \"chip-knowledge-v1\"\ncapability-packs = [\"capabilities.toml\"]\n",
        )
        .unwrap();

        let pack = ChipPack::load(&root.join("chip.toml")).unwrap();
        assert_eq!(pack.memory_map, Some(root.join("memory.toml")));
        assert_eq!(pack.svd_paths, [root.join("chip.svd")]);
        assert_eq!(pack.register_model, Some(root.join("registers.toml")));
        assert_eq!(
            pack.knowledge_provider.as_deref(),
            Some("chip-knowledge-v1")
        );
        assert_eq!(pack.capability_rules, 1);

        fs::write(
            root.join("invalid.toml"),
            "schema = 3\nid = \"chip\"\nmemory-map = \"/private/map.toml\"\n",
        )
        .unwrap();
        assert!(ChipPack::load(&root.join("invalid.toml")).is_err());
        fs::remove_dir_all(root).unwrap();
    }
}
