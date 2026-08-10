//! Typed summary projection for stored linked-IR artifacts.

use std::path::Path;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct LinkedIrSummary {
    pub(crate) functions: usize,
    pub(crate) decode_blockers: usize,
    pub(crate) registers: usize,
    pub(crate) field_candidates: usize,
}

pub(crate) fn inspect_linked_ir(path: &Path) -> crate::Result<LinkedIrSummary> {
    let input = std::fs::read_to_string(path)?;
    let document = super::parse_linked_ir(&input).map_err(|error| {
        crate::Error::invalid(format!(
            "unsupported linked-IR artifact in {}: {error}",
            path.display()
        ))
    })?;
    Ok(LinkedIrSummary {
        functions: document.summary.functions,
        decode_blockers: document.summary.decode_blockers,
        registers: document.summary.mmio_registers,
        field_candidates: document.summary.mmio_field_candidates,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn document() -> serde_json::Value {
        let mut document: serde_json::Value = serde_json::from_str(
            &crate::artifacts::render_linked_ir_fixture(Vec::new(), Vec::new()),
        )
        .unwrap();
        document["summary"]["functions"] = serde_json::json!(3);
        document["summary"]["decode_blockers"] = serde_json::json!(5);
        document["summary"]["mmio_registers"] = serde_json::json!(2);
        document["summary"]["mmio_field_candidates"] = serde_json::json!(4);
        document
    }

    #[test]
    fn validates_identity_claims_and_reads_summary() {
        let path = std::env::temp_dir().join(format!(
            "vendor-workbench-linked-ir-artifact-{}.json",
            std::process::id()
        ));
        std::fs::write(&path, serde_json::to_string_pretty(&document()).unwrap()).unwrap();
        let summary = inspect_linked_ir(&path).unwrap();
        assert_eq!(summary.functions, 3);
        assert_eq!(summary.decode_blockers, 5);
        assert_eq!(summary.registers, 2);
        assert_eq!(summary.field_candidates, 4);

        let mut stale = document();
        stale["schema_version"] = serde_json::json!(34);
        std::fs::write(&path, stale.to_string()).unwrap();
        assert!(
            inspect_linked_ir(&path)
                .unwrap_err()
                .to_string()
                .contains("expected schema_version 38")
        );
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn rejects_unknown_and_missing_fields_at_every_projection_boundary() {
        let mut unknown = document();
        unknown["summary"]["legacy_field"] = serde_json::json!(true);
        let error = super::super::parse_linked_ir(&unknown.to_string()).unwrap_err();
        assert!(error.to_string().contains("unknown field `legacy_field`"));

        let mut missing = document();
        missing
            .as_object_mut()
            .unwrap()
            .remove("scenario_suggestion_mode");
        let error = super::super::parse_linked_ir(&missing.to_string()).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("missing field `scenario_suggestion_mode`")
        );
    }
}
