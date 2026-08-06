//! Reusable platform composition kept above generic target/backend analysis.

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

use toml_edit::{DocumentMut, Item};

use crate::{Result, interfaces::SemanticCatalogs, target::TargetSpec};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PlatformPack {
    pub(crate) path: PathBuf,
    pub(crate) id: String,
    pub(crate) architecture: String,
    pub(crate) calling_convention: String,
    pub(crate) harness: Option<String>,
    pub(crate) semantic_catalogs: Vec<PathBuf>,
    pub(crate) semantic_operations: usize,
}

impl PlatformPack {
    pub(crate) fn load(path: &Path) -> Result<Self> {
        let path = path
            .canonicalize()
            .map_err(|error| format!("cannot resolve platform pack {}: {error}", path.display()))?;
        let input = fs::read_to_string(&path)?;
        let document = input.parse::<DocumentMut>()?;
        reject_unknown_keys(&document, &path)?;
        if document.get("schema").and_then(Item::as_integer) != Some(1) {
            return Err(format!("{} requires platform pack schema = 1", path.display()).into());
        }
        let id = required_string(&document, "id", &path)?;
        validate_id(&id, "platform pack")?;
        let architecture = required_string(&document, "architecture", &path)?;
        let calling_convention = required_string(&document, "calling-convention", &path)?;
        let harness = optional_string(&document, "harness");
        if harness
            .as_ref()
            .is_some_and(|value| value.chars().any(char::is_whitespace))
        {
            return Err(format!("{} harness must be one token", path.display()).into());
        }
        let base = path.parent().unwrap_or_else(|| Path::new("."));
        let semantic_catalogs = string_array(&document, "semantic-catalogs", &path)?
            .into_iter()
            .map(|value| {
                let relative = Path::new(&value);
                if relative.is_absolute() {
                    return Err(format!(
                        "{} semantic catalog paths must be relative to the platform pack",
                        path.display()
                    )
                    .into());
                }
                let resolved = base.join(relative);
                resolved.canonicalize().map_err(|error| {
                    format!(
                        "cannot resolve semantic catalog {} from {}: {error}",
                        relative.display(),
                        path.display()
                    )
                    .into()
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let unique = semantic_catalogs.iter().collect::<BTreeSet<_>>();
        if unique.len() != semantic_catalogs.len() {
            return Err(format!(
                "{} contains duplicate semantic catalog paths",
                path.display()
            )
            .into());
        }
        let semantic_operations = SemanticCatalogs::load(&semantic_catalogs)?.len();
        Ok(Self {
            path,
            id,
            architecture,
            calling_convention,
            harness,
            semantic_catalogs,
            semantic_operations,
        })
    }

    pub(crate) fn apply_to_target(&self, target: &mut TargetSpec) -> Result<()> {
        if self.architecture != target.architecture.label()
            || self.calling_convention != target.calling_convention.label()
        {
            return Err(format!(
                "platform pack {:?} requires {}/{}, but target {} uses {}/{}",
                self.id,
                self.architecture,
                self.calling_convention,
                target.id,
                target.architecture.label(),
                target.calling_convention.label(),
            )
            .into());
        }
        if let Some(harness) = &self.harness {
            if target.harness.is_some() {
                return Err(format!(
                    "target {} already has a platform harness before applying pack {:?}",
                    target.id, self.id
                )
                .into());
            }
            target.harness = Some(harness.clone());
        }
        Ok(())
    }
}

fn reject_unknown_keys(document: &DocumentMut, path: &Path) -> Result<()> {
    for (key, _) in document.iter() {
        if !matches!(
            key,
            "schema"
                | "id"
                | "architecture"
                | "calling-convention"
                | "harness"
                | "semantic-catalogs"
        ) {
            return Err(format!("{} has unknown platform pack key {key:?}", path.display()).into());
        }
    }
    Ok(())
}

fn required_string(document: &DocumentMut, key: &str, path: &Path) -> Result<String> {
    optional_string(document, key)
        .ok_or_else(|| format!("{} requires string {key:?}", path.display()).into())
}

fn optional_string(document: &DocumentMut, key: &str) -> Option<String> {
    document.get(key).and_then(Item::as_str).map(str::to_owned)
}

fn string_array(document: &DocumentMut, key: &str, path: &Path) -> Result<Vec<String>> {
    let Some(item) = document.get(key) else {
        return Ok(Vec::new());
    };
    let values = item
        .as_array()
        .ok_or_else(|| format!("{} {key:?} must be an array", path.display()))?;
    values
        .iter()
        .enumerate()
        .map(|(index, value)| {
            value
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| format!("{} {key}[{index}] must be a string", path.display()).into())
        })
        .collect()
}

fn validate_id(value: &str, kind: &str) -> Result<()> {
    if value.is_empty()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err(format!("invalid {kind} id {value:?}").into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_loads_relative_catalogs_and_applies_its_harness() {
        let root = std::env::temp_dir().join(format!(
            "vendor-workbench-platform-pack-{}",
            std::process::id()
        ));
        if root.exists() {
            fs::remove_dir_all(&root).unwrap();
        }
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("semantics.toml"),
            "schema = 1\nid = \"fixture\"\n[[operations]]\nid = \"time.wait\"\ndomain = \"time\"\nsummary = \"Wait\"\nargument-roles = [\"duration\"]\neffects = [\"time.wait\"]\n",
        )
        .unwrap();
        fs::write(
            root.join("platform.toml"),
            "schema = 1\nid = \"fixture-platform\"\narchitecture = \"riscv32\"\ncalling-convention = \"riscv-ilp32\"\nharness = \"esp32s31-radio-v1\"\nsemantic-catalogs = [\"semantics.toml\"]\n",
        )
        .unwrap();
        let target_path = root.join("target.spec");
        fs::write(
            &target_path,
            "schema 1\ntarget fixture\narchitecture riscv32\ncalling-convention riscv-ilp32\nendianness little\npointer-width 32\nrust-target riscv32imac-unknown-none-elf\n",
        )
        .unwrap();

        let pack = PlatformPack::load(&root.join("platform.toml")).unwrap();
        let mut target = TargetSpec::load(&target_path).unwrap();
        pack.apply_to_target(&mut target).unwrap();
        assert_eq!(target.harness.as_deref(), Some("esp32s31-radio-v1"));
        assert_eq!(pack.semantic_operations, 1);

        fs::remove_dir_all(root).unwrap();
    }
}
