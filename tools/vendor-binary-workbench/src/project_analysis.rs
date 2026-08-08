//! Project-owned generic analysis artifacts outside linked-IR profiles.

use std::path::{Path, PathBuf};

use toml_edit::{Item, Table};

use crate::{Result, project::ProjectSource, project_ir::ProjectIrProfile};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SymbolInventorySpec {
    pub(crate) output: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct NavigationIndexSpec {
    pub(crate) output: PathBuf,
}

pub(crate) fn load_symbol_inventory(
    document: &Table,
    base: &Path,
    ir_profiles: &[ProjectIrProfile],
    source: ProjectSource<'_>,
) -> Result<Option<SymbolInventorySpec>> {
    let Some(analysis_item) = document.get("analysis") else {
        return Ok(None);
    };
    let analysis = analysis_item.as_table().ok_or_else(|| {
        source.item(
            Some(analysis_item),
            "project manifest analysis must be a table",
        )
    })?;
    let Some(symbols_item) = analysis.get("symbols") else {
        return Ok(None);
    };
    let symbols = symbols_item.as_table().ok_or_else(|| {
        source.item(
            Some(symbols_item),
            "project analysis.symbols must be a table",
        )
    })?;
    for (key, item) in symbols.iter() {
        if key != "output" {
            return Err(source.item(
                Some(item),
                format!("unknown project analysis.symbols key {key:?}"),
            ));
        }
    }
    let output = symbols
        .get("output")
        .and_then(Item::as_str)
        .filter(|value| !value.is_empty())
        .map(|value| resolve_path(base, value))
        .ok_or_else(|| {
            source.table_key(
                symbols,
                "output",
                "project analysis.symbols requires non-empty string \"output\"",
            )
        })?;
    if ir_profiles
        .iter()
        .any(|profile| profile.output == output || profile.pseudo_rust.as_ref() == Some(&output))
    {
        return Err(source.table_key(
            symbols,
            "output",
            format!(
                "project symbol inventory reuses linked-IR output path {}",
                output.display()
            ),
        ));
    }
    Ok(Some(SymbolInventorySpec { output }))
}

pub(crate) fn load_navigation_index(
    document: &Table,
    base: &Path,
    symbols: Option<&SymbolInventorySpec>,
    ir_profiles: &[ProjectIrProfile],
    source: ProjectSource<'_>,
) -> Result<Option<NavigationIndexSpec>> {
    let Some(analysis_item) = document.get("analysis") else {
        return Ok(None);
    };
    let analysis = analysis_item.as_table().ok_or_else(|| {
        source.item(
            Some(analysis_item),
            "project manifest analysis must be a table",
        )
    })?;
    let Some(navigation_item) = analysis.get("navigation") else {
        return Ok(None);
    };
    let navigation = navigation_item.as_table().ok_or_else(|| {
        source.item(
            Some(navigation_item),
            "project analysis.navigation must be a table",
        )
    })?;
    for (key, item) in navigation.iter() {
        if key != "output" {
            return Err(source.item(
                Some(item),
                format!("unknown project analysis.navigation key {key:?}"),
            ));
        }
    }
    let output = navigation
        .get("output")
        .and_then(Item::as_str)
        .filter(|value| !value.is_empty())
        .map(|value| resolve_path(base, value))
        .ok_or_else(|| {
            source.table_key(
                navigation,
                "output",
                "project analysis.navigation requires non-empty string \"output\"",
            )
        })?;
    let symbols = symbols.ok_or_else(|| {
        source.item(
            Some(navigation_item),
            "project analysis.navigation requires [analysis.symbols]",
        )
    })?;
    if output == symbols.output {
        return Err(source.table_key(
            navigation,
            "output",
            "project navigation index reuses symbol inventory output path",
        ));
    }
    if ir_profiles
        .iter()
        .any(|profile| profile.output == output || profile.pseudo_rust.as_ref() == Some(&output))
    {
        return Err(source.table_key(
            navigation,
            "output",
            format!(
                "project navigation index reuses linked-IR output path {}",
                output.display()
            ),
        ));
    }
    Ok(Some(NavigationIndexSpec { output }))
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
    use toml_edit::DocumentMut;

    #[test]
    fn resolves_inventory_output_and_rejects_ir_collisions() {
        let input = r#"
[analysis.symbols]
output = "generated/symbols.json"
"#;
        let document = input.parse::<DocumentMut>().unwrap();
        let source = ProjectSource::new(Path::new("project.toml"), input);
        let spec = load_symbol_inventory(&document, Path::new("project"), &[], source)
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
            load_symbol_inventory(&document, Path::new("project"), &[profile], source)
                .unwrap_err()
                .to_string()
                .contains("reuses linked-IR output")
        );
    }

    #[test]
    fn navigation_requires_symbols_and_owns_a_distinct_output() {
        let input = r#"
[analysis.symbols]
output = "generated/symbols.json"

[analysis.navigation]
output = "generated/navigation.json"
"#;
        let document = input.parse::<DocumentMut>().unwrap();
        let source = ProjectSource::new(Path::new("project.toml"), input);
        let symbols = load_symbol_inventory(&document, Path::new("project"), &[], source)
            .unwrap()
            .unwrap();
        let navigation =
            load_navigation_index(&document, Path::new("project"), Some(&symbols), &[], source)
                .unwrap()
                .unwrap();
        assert_eq!(
            navigation.output,
            PathBuf::from("project/generated/navigation.json")
        );
        assert!(
            load_navigation_index(&document, Path::new("project"), None, &[], source)
                .unwrap_err()
                .to_string()
                .contains("requires [analysis.symbols]")
        );
    }
}
