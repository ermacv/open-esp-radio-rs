//! Typed summary projection for stored linked-IR artifacts.

use std::path::Path;

use serde::Deserialize;

use super::{LINKED_IR, read_json};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct LinkedIrSummary {
    pub(crate) functions: usize,
    pub(crate) registers: usize,
    pub(crate) field_candidates: usize,
}

#[derive(Deserialize)]
struct LinkedIrDocument {
    schema_version: u32,
    command: String,
    completeness_claim: bool,
    mmio_field_semantics_claim: bool,
    summary: SummaryDocument,
}

#[derive(Deserialize)]
struct SummaryDocument {
    functions: usize,
    mmio_registers: usize,
    mmio_field_candidates: usize,
}

pub(crate) fn inspect_linked_ir(path: &Path) -> crate::Result<LinkedIrSummary> {
    let document = read_json::<LinkedIrDocument>("linked-IR report", path)?;
    if document.schema_version != LINKED_IR.version || document.command != LINKED_IR.command {
        return Err(crate::Error::invalid(format!(
            "unsupported linked-IR artifact in {}: expected schema_version {} and command {:?}",
            path.display(),
            LINKED_IR.version,
            LINKED_IR.command,
        )));
    }
    if document.completeness_claim || document.mmio_field_semantics_claim {
        return Err(crate::Error::invalid(format!(
            "linked-IR artifact {} makes an unsupported completeness or field-semantics claim",
            path.display()
        )));
    }
    Ok(LinkedIrSummary {
        functions: document.summary.functions,
        registers: document.summary.mmio_registers,
        field_candidates: document.summary.mmio_field_candidates,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_identity_claims_and_reads_summary() {
        let path = std::env::temp_dir().join(format!(
            "vendor-workbench-linked-ir-artifact-{}.json",
            std::process::id()
        ));
        std::fs::write(
            &path,
            r#"{
  "schema_version": 35,
  "command": "ir export",
  "completeness_claim": false,
  "mmio_field_semantics_claim": false,
  "summary": {
    "functions": 3,
    "mmio_registers": 2,
    "mmio_field_candidates": 4
  }
}"#,
        )
        .unwrap();
        let summary = inspect_linked_ir(&path).unwrap();
        assert_eq!(summary.functions, 3);
        assert_eq!(summary.registers, 2);
        assert_eq!(summary.field_candidates, 4);

        std::fs::write(
            &path,
            r#"{"schema_version":34,"command":"ir export","completeness_claim":false,"mmio_field_semantics_claim":false,"summary":{"functions":0,"mmio_registers":0,"mmio_field_candidates":0}}"#,
        )
        .unwrap();
        assert!(
            inspect_linked_ir(&path)
                .unwrap_err()
                .to_string()
                .contains("expected schema_version 35")
        );
        std::fs::remove_file(path).unwrap();
    }
}
