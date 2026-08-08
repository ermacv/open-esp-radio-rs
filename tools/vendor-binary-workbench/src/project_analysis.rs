//! Project-owned generic analysis artifacts outside linked-IR profiles.

use std::path::{Path, PathBuf};

use toml_edit::{DocumentMut, Item};

use crate::{Result, project_ir::ProjectIrProfile};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SymbolInventorySpec {
    pub(crate) output: PathBuf,
}

pub(crate) fn load_symbol_inventory(
    document: &DocumentMut,
    base: &Path,
    ir_profiles: &[ProjectIrProfile],
) -> Result<Option<SymbolInventorySpec>> {
    let Some(analysis) = document.get("analysis") else {
        return Ok(None);
    };
    let analysis = analysis
        .as_table()
        .ok_or("project manifest analysis must be a table")?;
    let Some(symbols) = analysis.get("symbols") else {
        return Ok(None);
    };
    let symbols = symbols
        .as_table()
        .ok_or("project analysis.symbols must be a table")?;
    for (key, _) in symbols.iter() {
        if key != "output" {
            return Err(format!("unknown project analysis.symbols key {key:?}").into());
        }
    }
    let output = symbols
        .get("output")
        .and_then(Item::as_str)
        .filter(|value| !value.is_empty())
        .map(|value| resolve_path(base, value))
        .ok_or("project analysis.symbols requires non-empty string \"output\"")?;
    if ir_profiles
        .iter()
        .any(|profile| profile.output == output || profile.pseudo_rust.as_ref() == Some(&output))
    {
        return Err(format!(
            "project symbol inventory reuses linked-IR output path {}",
            output.display()
        )
        .into());
    }
    Ok(Some(SymbolInventorySpec { output }))
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
    fn resolves_inventory_output_and_rejects_ir_collisions() {
        let document = r#"
[analysis.symbols]
output = "generated/symbols.json"
"#
        .parse::<DocumentMut>()
        .unwrap();
        let spec = load_symbol_inventory(&document, Path::new("project"), &[])
            .unwrap()
            .unwrap();
        assert_eq!(spec.output, PathBuf::from("project/generated/symbols.json"));

        let profile = ProjectIrProfile {
            id: "fixture".to_owned(),
            sources: Vec::new(),
            symbol_prefix: String::new(),
            include_reachable: true,
            entry_contract: "none".to_owned(),
            output: spec.output.clone(),
            pseudo_rust: None,
        };
        assert!(
            load_symbol_inventory(&document, Path::new("project"), &[profile])
                .unwrap_err()
                .to_string()
                .contains("reuses linked-IR output")
        );
    }
}
