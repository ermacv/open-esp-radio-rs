//! Reusable platform composition kept above generic target/backend analysis.

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

use toml_edit::{Item, Table};

use crate::{
    Result,
    error::{ManifestContext, WorkbenchError},
    interfaces::SemanticCatalogs,
    target::TargetSpec,
};

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
    #[tracing::instrument(name = "load_platform_pack", fields(path = %path.display()))]
    pub(crate) fn load(path: &Path) -> Result<Self> {
        let path = path
            .canonicalize()
            .map_err(|error| WorkbenchError::manifest("platform pack", path, error))?;
        let input = fs::read_to_string(&path)?;
        let document = toml_edit::Document::parse(input.as_str()).map_err(|error| {
            WorkbenchError::manifest_source("platform pack", &path, &input, &error, error.span())
        })?;
        let source = ManifestContext::new("platform pack", &path, &input);
        Self::parse(&path, &document, source)
    }

    fn parse(path: &Path, document: &Table, source: ManifestContext<'_>) -> Result<Self> {
        reject_unknown_keys(document, source)?;
        if document.get("schema").and_then(Item::as_integer) != Some(1) {
            return Err(source.item(document.get("schema"), "platform pack requires schema = 1"));
        }
        let id = required_string(document, "id", source)?;
        validate_id(&id, "platform pack")
            .map_err(|message| source.table_key(document, "id", message))?;
        let architecture = required_string(document, "architecture", source)?;
        let calling_convention = required_string(document, "calling-convention", source)?;
        let harness = optional_string(document, "harness", source)?;
        if harness
            .as_ref()
            .is_some_and(|value| value.chars().any(char::is_whitespace))
        {
            return Err(source.table_key(document, "harness", "harness must be one token"));
        }
        let base = path.parent().unwrap_or_else(|| Path::new("."));
        let catalogs = string_array(document, "semantic-catalogs", source)?;
        let values = document
            .get("semantic-catalogs")
            .and_then(Item::as_array)
            .expect("string_array accepted semantic-catalogs");
        let mut unique = BTreeSet::new();
        let mut semantic_catalogs = Vec::with_capacity(catalogs.len());
        for (index, value) in catalogs.into_iter().enumerate() {
            let relative = Path::new(&value);
            let span = values.get(index).and_then(toml_edit::Value::span);
            if relative.is_absolute() {
                return Err(source.error(
                    span,
                    "semantic catalog paths must be relative to the platform pack",
                ));
            }
            let resolved = base.join(relative);
            let resolved = resolved.canonicalize().map_err(|error| {
                source.error(
                    span.clone(),
                    format!(
                        "cannot resolve semantic catalog {}: {error}",
                        relative.display()
                    ),
                )
            })?;
            if !unique.insert(resolved.clone()) {
                return Err(source.error(span, "duplicate semantic catalog path"));
            }
            semantic_catalogs.push(resolved);
        }
        let semantic_operations = SemanticCatalogs::load(&semantic_catalogs)?.len();
        Ok(Self {
            path: path.to_owned(),
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
            return Err(crate::Error::invalid(format!(
                "platform pack {:?} requires {}/{}, but target {} uses {}/{}",
                self.id,
                self.architecture,
                self.calling_convention,
                target.id,
                target.architecture.label(),
                target.calling_convention.label(),
            )));
        }
        if let Some(harness) = &self.harness {
            if target.harness.is_some() {
                return Err(crate::Error::invalid(format!(
                    "target {} already has a platform harness before applying pack {:?}",
                    target.id, self.id
                )));
            }
            target.harness = Some(harness.clone());
        }
        Ok(())
    }
}

fn reject_unknown_keys(document: &Table, source: ManifestContext<'_>) -> Result<()> {
    for (key, item) in document.iter() {
        if !matches!(
            key,
            "schema"
                | "id"
                | "architecture"
                | "calling-convention"
                | "harness"
                | "semantic-catalogs"
        ) {
            return Err(source.item(Some(item), format!("unknown platform pack key {key:?}")));
        }
    }
    Ok(())
}

fn required_string(document: &Table, key: &str, source: ManifestContext<'_>) -> Result<String> {
    optional_string(document, key, source)?
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            source.table_key(
                document,
                key,
                format!("platform pack requires non-empty string {key:?}"),
            )
        })
}

fn optional_string(
    document: &Table,
    key: &str,
    source: ManifestContext<'_>,
) -> Result<Option<String>> {
    document
        .get(key)
        .map(|item| {
            item.as_str().map(str::to_owned).ok_or_else(|| {
                source.item(
                    Some(item),
                    format!("platform pack {key:?} must be a string"),
                )
            })
        })
        .transpose()
}

fn string_array(document: &Table, key: &str, source: ManifestContext<'_>) -> Result<Vec<String>> {
    let Some(item) = document.get(key) else {
        return Ok(Vec::new());
    };
    let values = item.as_array().ok_or_else(|| {
        source.item(
            Some(item),
            format!("platform pack {key:?} must be an array"),
        )
    })?;
    values
        .iter()
        .enumerate()
        .map(|(index, value)| {
            value.as_str().map(str::to_owned).ok_or_else(|| {
                source.error(
                    value.span(),
                    format!("platform pack {key}[{index}] must be a string"),
                )
            })
        })
        .collect()
}

fn validate_id(value: &str, kind: &str) -> std::result::Result<(), String> {
    if value.is_empty()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err(format!("invalid {kind} id {value:?}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn invalid_pack_span(input: &str, name: &str) -> (usize, usize, String) {
        let path = std::env::temp_dir().join(format!(
            "vendor-workbench-platform-diagnostic-{}-{name}.toml",
            std::process::id()
        ));
        fs::write(&path, input).unwrap();
        let error = PlatformPack::load(&path).unwrap_err();
        fs::remove_file(path).unwrap();
        let message = error.to_string();
        let WorkbenchError::ManifestSource {
            kind: "platform pack",
            span,
            ..
        } = error
        else {
            panic!("expected source-aware platform pack error, got {message}")
        };
        (span.offset(), span.len(), message)
    }

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

    #[test]
    fn malformed_pack_reports_its_manifest_path() {
        let path = std::env::temp_dir().join(format!(
            "vendor-workbench-platform-malformed-{}.toml",
            std::process::id()
        ));
        fs::write(&path, "schema = [\n").unwrap();
        let canonical = path.canonicalize().unwrap();
        let error = PlatformPack::load(&path).unwrap_err();
        fs::remove_file(path).unwrap();

        assert!(matches!(
            error,
            WorkbenchError::ManifestSource {
                kind: "platform pack",
                path: reported,
                ..
            } if reported == canonical
        ));
    }

    #[test]
    fn semantic_pack_errors_retain_the_exact_physical_value_span() {
        let cases = [
            (
                "harness-type",
                "schema = 1\nid = \"fixture\"\narchitecture = \"riscv32\"\ncalling-convention = \"riscv-ilp32\"\nharness = [\"bad\"]\n",
                "[\"bad\"]",
                "harness\" must be a string",
            ),
            (
                "absolute-catalog",
                "schema = 1\nid = \"fixture\"\narchitecture = \"riscv32\"\ncalling-convention = \"riscv-ilp32\"\nsemantic-catalogs = [\"/tmp/absolute.toml\"]\n",
                "\"/tmp/absolute.toml\"",
                "must be relative",
            ),
            (
                "unknown-key",
                "schema = 1\nid = \"fixture\"\narchitecture = \"riscv32\"\ncalling-convention = \"riscv-ilp32\"\nsemantic-catalog = []\n",
                "[]",
                "unknown platform pack key",
            ),
        ];

        for (name, input, physical_value, expected_message) in cases {
            let (offset, length, message) = invalid_pack_span(input, name);
            assert_eq!(
                offset,
                input.find(physical_value).unwrap(),
                "{name}: {message}"
            );
            assert_eq!(length, physical_value.len(), "{name}: {message}");
            assert!(message.contains(expected_message), "{name}: {message}");
        }
    }
}
