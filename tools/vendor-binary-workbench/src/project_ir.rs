//! Reproducible linked-IR generation profiles owned by a project manifest.

use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use toml_edit::{DocumentMut, Item, Table};

use crate::{Result, source_id::validate_source_id};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ProjectIrProfile {
    pub(crate) id: String,
    pub(crate) sources: Vec<String>,
    pub(crate) symbol_prefix: String,
    pub(crate) include_reachable: bool,
    pub(crate) entry_contract: String,
    pub(crate) output: PathBuf,
    pub(crate) pseudo_rust: Option<PathBuf>,
}

pub(crate) fn load_ir_profiles(
    document: &DocumentMut,
    base: &Path,
) -> Result<Vec<ProjectIrProfile>> {
    let Some(analysis) = document.get("analysis") else {
        return Ok(Vec::new());
    };
    let analysis = analysis
        .as_table()
        .ok_or("project manifest analysis must be a table")?;
    let Some(ir) = analysis.get("ir") else {
        return Ok(Vec::new());
    };
    let profiles = ir
        .as_array_of_tables()
        .ok_or("project analysis.ir must be an array of tables")?;
    let mut ids = BTreeSet::new();
    let mut outputs = BTreeSet::new();
    profiles
        .iter()
        .enumerate()
        .map(|(index, table)| {
            let context = format!("project analysis.ir[{index}]");
            let id = required_string(table, "id", &context)?;
            validate_profile_id(&id, &context)?;
            if !ids.insert(id.clone()) {
                return Err(format!("duplicate project IR profile id {id:?}").into());
            }
            let sources = sources(table, &context)?;
            let symbol_prefix =
                optional_string(table, "symbol-prefix", &context)?.unwrap_or_default();
            let include_reachable =
                optional_boolean(table, "include-reachable", &context)?.unwrap_or(true);
            let entry_contract = optional_string(table, "entry-contract", &context)?
                .unwrap_or_else(|| "none".to_owned());
            let output = resolve_path(base, &required_string(table, "output", &context)?);
            reserve_output(&mut outputs, &output, &id)?;
            let pseudo_rust = optional_string(table, "pseudo-rust", &context)?
                .map(|path| resolve_path(base, &path));
            if let Some(path) = &pseudo_rust {
                reserve_output(&mut outputs, path, &id)?;
            }
            Ok(ProjectIrProfile {
                id,
                sources,
                symbol_prefix,
                include_reachable,
                entry_contract,
                output,
                pseudo_rust,
            })
        })
        .collect()
}

fn sources(table: &Table, context: &str) -> Result<Vec<String>> {
    let Some(item) = table.get("sources") else {
        return Ok(Vec::new());
    };
    let values = item
        .as_array()
        .ok_or_else(|| format!("{context}.sources must be an array"))?;
    if values.is_empty() {
        return Err(format!("{context}.sources must not be empty").into());
    }
    let mut seen = BTreeSet::new();
    values
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let source = value
                .as_str()
                .filter(|value| !value.is_empty())
                .ok_or_else(|| format!("{context}.sources[{index}] must be a non-empty string"))?;
            validate_source_id(source).map_err(|_| {
                format!("invalid source id {source:?} in {context}.sources[{index}]")
            })?;
            if !seen.insert(source.to_owned()) {
                return Err(format!("duplicate source {source:?} in {context}").into());
            }
            Ok(source.to_owned())
        })
        .collect()
}

fn reserve_output(outputs: &mut BTreeSet<PathBuf>, path: &Path, id: &str) -> Result<()> {
    if !outputs.insert(path.to_owned()) {
        return Err(format!(
            "project IR profile {id:?} reuses output path {}",
            path.display()
        )
        .into());
    }
    Ok(())
}

fn required_string(table: &Table, key: &str, context: &str) -> Result<String> {
    optional_string(table, key, context)?
        .ok_or_else(|| format!("{context} requires string {key:?}").into())
}

fn optional_string(table: &Table, key: &str, context: &str) -> Result<Option<String>> {
    match table.get(key) {
        None => Ok(None),
        Some(item) => item
            .as_str()
            .filter(|value| !value.is_empty())
            .map(|value| Some(value.to_owned()))
            .ok_or_else(|| format!("{context}.{key} must be a non-empty string").into()),
    }
}

fn optional_boolean(table: &Table, key: &str, context: &str) -> Result<Option<bool>> {
    match table.get(key) {
        None => Ok(None),
        Some(Item::Value(value)) => value
            .as_bool()
            .map(Some)
            .ok_or_else(|| format!("{context}.{key} must be a boolean").into()),
        Some(_) => Err(format!("{context}.{key} must be a boolean").into()),
    }
}

fn validate_profile_id(id: &str, context: &str) -> Result<()> {
    if id
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        Ok(())
    } else {
        Err(format!("invalid IR profile id {id:?} in {context}").into())
    }
}

fn resolve_path(base: &Path, value: &str) -> PathBuf {
    let path = Path::new(value);
    if path.is_absolute() {
        path.to_owned()
    } else {
        base.join(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_profiles_and_resolves_outputs_relative_to_the_project() {
        let document = r#"
[analysis]

[[analysis.ir]]
id = "phy"
sources = ["rom", "archive"]
symbol-prefix = "phy_"
output = "generated/phy.ir.json"
pseudo-rust = "generated/phy.pseudo.rs"

[[analysis.ir]]
id = "all-rom"
sources = ["rom"]
include-reachable = false
entry-contract = "none"
output = "generated/rom.ir.json"
"#
        .parse::<DocumentMut>()
        .unwrap();
        let profiles = load_ir_profiles(&document, Path::new("project")).unwrap();
        assert_eq!(profiles.len(), 2);
        assert_eq!(profiles[0].sources, ["rom", "archive"]);
        assert!(profiles[0].include_reachable);
        assert_eq!(
            profiles[0].pseudo_rust,
            Some(PathBuf::from("project/generated/phy.pseudo.rs"))
        );
        assert_eq!(profiles[1].symbol_prefix, "");
        assert!(!profiles[1].include_reachable);
    }

    #[test]
    fn rejects_duplicate_profile_outputs() {
        let document = r#"
[[analysis.ir]]
id = "one"
output = "generated/shared.json"

[[analysis.ir]]
id = "two"
output = "generated/shared.json"
"#
        .parse::<DocumentMut>()
        .unwrap();
        let error = load_ir_profiles(&document, Path::new("project")).unwrap_err();
        assert!(error.to_string().contains("reuses output path"));
    }
}
