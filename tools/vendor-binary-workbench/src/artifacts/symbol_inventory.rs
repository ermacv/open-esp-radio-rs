//! Typed summary projection for stored symbol-inventory artifacts.

use std::path::Path;

use serde::Deserialize;

use super::{SYMBOL_INVENTORY, read_json};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SymbolInventorySummary {
    pub(crate) artifacts: usize,
    pub(crate) symbol_facts: usize,
    pub(crate) exported_definitions: usize,
    pub(crate) undefined: usize,
    pub(crate) unresolved_or_associated: usize,
}

#[derive(Deserialize)]
struct SymbolInventoryDocument {
    schema_version: u32,
    command: String,
    summary: SummaryDocument,
}

#[derive(Deserialize)]
struct SummaryDocument {
    artifacts: usize,
    symbol_facts: usize,
    exported_definitions: usize,
    undefined: usize,
    unresolved_or_associated: usize,
}

pub(crate) fn inspect_symbol_inventory(path: &Path) -> crate::Result<SymbolInventorySummary> {
    let document = read_json::<SymbolInventoryDocument>("symbol inventory", path)?;
    if document.schema_version != SYMBOL_INVENTORY.version
        || document.command != SYMBOL_INVENTORY.command
    {
        return Err(crate::Error::invalid(format!(
            "unsupported symbol inventory in {}: expected schema_version {} and command {:?}",
            path.display(),
            SYMBOL_INVENTORY.version,
            SYMBOL_INVENTORY.command,
        )));
    }
    Ok(SymbolInventorySummary {
        artifacts: document.summary.artifacts,
        symbol_facts: document.summary.symbol_facts,
        exported_definitions: document.summary.exported_definitions,
        undefined: document.summary.undefined,
        unresolved_or_associated: document.summary.unresolved_or_associated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stored_inventory_summary_is_strictly_versioned() {
        let path = std::env::temp_dir().join(format!(
            "vendor-workbench-symbol-inventory-{}.json",
            std::process::id()
        ));
        std::fs::write(
            &path,
            r#"{
  "schema_version": 2,
  "command": "symbols inventory",
  "summary": {
    "artifacts": 3,
    "symbol_facts": 40,
    "emitted": 40,
    "exported_definitions": 12,
    "undefined": 7,
    "unresolved_or_associated": 5
  }
}
"#,
        )
        .unwrap();
        let summary = inspect_symbol_inventory(&path).unwrap();
        assert_eq!(summary.artifacts, 3);
        assert_eq!(summary.symbol_facts, 40);
        assert_eq!(summary.exported_definitions, 12);

        std::fs::write(
            &path,
            r#"{"schema_version":1,"command":"symbols inventory","summary":{"artifacts":0,"symbol_facts":0,"exported_definitions":0,"undefined":0,"unresolved_or_associated":0}}"#,
        )
        .unwrap();
        assert!(
            inspect_symbol_inventory(&path)
                .unwrap_err()
                .to_string()
                .contains("expected schema_version 2")
        );
        std::fs::remove_file(path).unwrap();
    }
}
