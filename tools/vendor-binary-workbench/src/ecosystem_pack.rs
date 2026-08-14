//! Reusable vendor/ecosystem semantics kept above chip-specific knowledge.

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
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct EcosystemPack {
    pub(crate) path: PathBuf,
    pub(crate) id: String,
    pub(crate) knowledge_packs: Vec<PathBuf>,
    pub(crate) knowledge_operations: usize,
}

impl EcosystemPack {
    #[tracing::instrument(name = "load_ecosystem_pack", fields(path = %path.display()))]
    pub(crate) fn load(path: &Path) -> Result<Self> {
        let path = path
            .canonicalize()
            .map_err(|error| WorkbenchError::manifest("ecosystem pack", path, error))?;
        let input = fs::read_to_string(&path)?;
        let document = toml_edit::Document::parse(input.as_str()).map_err(|error| {
            WorkbenchError::manifest_source("ecosystem pack", &path, &input, &error, error.span())
        })?;
        let source = ManifestContext::new("ecosystem pack", &path, &input);
        Self::parse(&path, &document, source)
    }

    fn parse(path: &Path, document: &Table, source: ManifestContext<'_>) -> Result<Self> {
        reject_unknown_keys(document, source)?;
        if document.get("schema").and_then(Item::as_integer) != Some(3) {
            return Err(source.item(document.get("schema"), "ecosystem pack requires schema = 3"));
        }
        let id = required_string(document, "id", source)?;
        validate_id(&id, "ecosystem pack")
            .map_err(|message| source.table_key(document, "id", message))?;
        let base = path.parent().unwrap_or_else(|| Path::new("."));
        let catalogs = string_array(document, "knowledge-packs", source)?;
        let values = document
            .get("knowledge-packs")
            .and_then(Item::as_array)
            .expect("string_array accepted knowledge-packs");
        let mut unique = BTreeSet::new();
        let mut knowledge_packs = Vec::with_capacity(catalogs.len());
        for (index, value) in catalogs.into_iter().enumerate() {
            let relative = Path::new(&value);
            let span = values.get(index).and_then(toml_edit::Value::span);
            if relative.is_absolute() {
                return Err(source.error(
                    span,
                    "knowledge pack paths must be relative to the ecosystem pack",
                ));
            }
            let resolved = base.join(relative);
            let resolved = resolved.canonicalize().map_err(|error| {
                source.error(
                    span.clone(),
                    format!(
                        "cannot resolve knowledge pack {}: {error}",
                        relative.display()
                    ),
                )
            })?;
            if !unique.insert(resolved.clone()) {
                return Err(source.error(span, "duplicate knowledge pack path"));
            }
            knowledge_packs.push(resolved);
        }
        let knowledge_operations = SemanticCatalogs::load(&knowledge_packs)?.len();
        Ok(Self {
            path: path.to_owned(),
            id,
            knowledge_packs,
            knowledge_operations,
        })
    }
}

fn reject_unknown_keys(document: &Table, source: ManifestContext<'_>) -> Result<()> {
    for (key, item) in document.iter() {
        if !matches!(key, "schema" | "id" | "knowledge-packs") {
            return Err(source.item(Some(item), format!("unknown ecosystem pack key {key:?}")));
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
                format!("ecosystem pack requires non-empty string {key:?}"),
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
                    format!("ecosystem pack {key:?} must be a string"),
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
            format!("ecosystem pack {key:?} must be an array"),
        )
    })?;
    values
        .iter()
        .enumerate()
        .map(|(index, value)| {
            value.as_str().map(str::to_owned).ok_or_else(|| {
                source.error(
                    value.span(),
                    format!("ecosystem pack {key}[{index}] must be a string"),
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
        let error = EcosystemPack::load(&path).unwrap_err();
        fs::remove_file(path).unwrap();
        let message = error.to_string();
        let WorkbenchError::ManifestSource {
            kind: "ecosystem pack",
            span,
            ..
        } = error
        else {
            panic!("expected source-aware ecosystem pack error, got {message}")
        };
        (span.offset(), span.len(), message)
    }

    #[test]
    fn pack_loads_relative_ecosystem_knowledge_without_chip_authority() {
        let root = std::env::temp_dir().join(format!(
            "vendor-workbench-ecosystem-pack-{}",
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
            root.join("ecosystem.toml"),
            "schema = 3\nid = \"fixture-ecosystem\"\nknowledge-packs = [\"semantics.toml\"]\n",
        )
        .unwrap();
        let pack = EcosystemPack::load(&root.join("ecosystem.toml")).unwrap();
        assert_eq!(pack.knowledge_operations, 1);

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
        let error = EcosystemPack::load(&path).unwrap_err();
        fs::remove_file(path).unwrap();

        assert!(matches!(
            error,
            WorkbenchError::ManifestSource {
                kind: "ecosystem pack",
                path: reported,
                ..
            } if reported == canonical
        ));
    }

    #[test]
    fn semantic_pack_errors_retain_the_exact_physical_value_span() {
        let cases = [
            (
                "absolute-catalog",
                "schema = 3\nid = \"fixture\"\nknowledge-packs = [\"/tmp/absolute.toml\"]\n",
                "\"/tmp/absolute.toml\"",
                "must be relative",
            ),
            (
                "unknown-key",
                "schema = 3\nid = \"fixture\"\nsemantic-catalog = []\n",
                "[]",
                "unknown ecosystem pack key",
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
