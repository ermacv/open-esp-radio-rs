//! Strict summary projection for stored symbol-inventory reports.

use std::{fs, path::Path};

use serde::Deserialize;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct StoredInventorySummary {
    pub(crate) artifacts: usize,
    pub(crate) symbol_facts: usize,
    pub(crate) exported_definitions: usize,
    pub(crate) undefined: usize,
    pub(crate) unresolved_or_associated: usize,
}

#[derive(Deserialize)]
struct StoredInventoryDocument {
    schema_version: u32,
    command: String,
    summary: StoredSummaryDocument,
}

#[derive(Deserialize)]
struct StoredSummaryDocument {
    artifacts: usize,
    symbol_facts: usize,
    exported_definitions: usize,
    undefined: usize,
    unresolved_or_associated: usize,
}

pub(crate) fn inspect(path: &Path) -> crate::Result<StoredInventorySummary> {
    let input = fs::read_to_string(path)?;
    let document = serde_json::from_str::<StoredInventoryDocument>(&input)?;
    if document.schema_version != 2 || document.command != "symbols inventory" {
        return Err(crate::Error::invalid(format!(
            "unsupported symbol inventory in {}: expected schema_version 2 and command \"symbols inventory\"",
            path.display()
        )));
    }
    Ok(StoredInventorySummary {
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
        fs::write(
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
        let summary = inspect(&path).unwrap();
        assert_eq!(summary.artifacts, 3);
        assert_eq!(summary.symbol_facts, 40);
        assert_eq!(summary.exported_definitions, 12);

        fs::write(
            &path,
            r#"{"schema_version":1,"command":"symbols inventory","summary":{"artifacts":0,"symbol_facts":0,"exported_definitions":0,"undefined":0,"unresolved_or_associated":0}}"#,
        )
        .unwrap();
        assert!(
            inspect(&path)
                .unwrap_err()
                .to_string()
                .contains("expected schema_version 2")
        );
        fs::remove_file(path).unwrap();
    }
}
