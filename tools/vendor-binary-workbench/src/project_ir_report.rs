//! Generic validation and summary of a generated project linked-IR report.

use std::{fs, path::Path};

use serde_json::{Map, Value};

use crate::Result;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ProjectIrReportSummary {
    pub(crate) functions: usize,
    pub(crate) registers: usize,
    pub(crate) field_candidates: usize,
}

pub(crate) fn inspect_project_ir_report(path: &Path) -> Result<ProjectIrReportSummary> {
    let input = fs::read_to_string(path)?;
    let root: Value = serde_json::from_str(&input)?;
    let root = object(&root, "linked-IR root")?;
    if integer(root, "schema_version", "linked-IR report")? != 35 {
        return Err(crate::Error::invalid(format!(
            "project IR output requires schema 35 in {}",
            path.display()
        )));
    }
    if string(root, "command", "linked-IR report")? != "ir export" {
        return Err(crate::Error::invalid(format!(
            "{} is not an ir export report",
            path.display()
        )));
    }
    if boolean(root, "completeness_claim", "linked-IR report")?
        || boolean(root, "mmio_field_semantics_claim", "linked-IR report")?
    {
        return Err(crate::Error::invalid(format!(
            "project IR output {} makes an unsupported completeness or field-semantics claim",
            path.display()
        )));
    }
    let summary = root
        .get("summary")
        .ok_or("linked-IR report requires summary")
        .map_err(crate::Error::invalid)?;
    let summary = object(summary, "linked-IR summary")?;
    Ok(ProjectIrReportSummary {
        functions: count(summary, "functions", "linked-IR summary")?,
        registers: count(summary, "mmio_registers", "linked-IR summary")?,
        field_candidates: count(summary, "mmio_field_candidates", "linked-IR summary")?,
    })
}

fn object<'a>(value: &'a Value, context: &str) -> Result<&'a Map<String, Value>> {
    value
        .as_object()
        .ok_or_else(|| crate::Error::invalid(format!("{context} must be an object")))
}

fn string<'a>(object: &'a Map<String, Value>, key: &str, context: &str) -> Result<&'a str> {
    object
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            crate::Error::invalid(format!("{context} requires non-empty string {key:?}"))
        })
}

fn integer(object: &Map<String, Value>, key: &str, context: &str) -> Result<u64> {
    object.get(key).and_then(Value::as_u64).ok_or_else(|| {
        crate::Error::invalid(format!("{context} requires non-negative integer {key:?}"))
    })
}

fn count(object: &Map<String, Value>, key: &str, context: &str) -> Result<usize> {
    integer(object, key, context)?
        .try_into()
        .map_err(|_| crate::Error::invalid(format!("invalid count {key:?} in {context}")))
}

fn boolean(object: &Map<String, Value>, key: &str, context: &str) -> Result<bool> {
    object
        .get(key)
        .and_then(Value::as_bool)
        .ok_or_else(|| crate::Error::invalid(format!("{context} requires boolean {key:?}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_schema_and_reads_generic_summary() {
        let path = std::env::temp_dir().join(format!(
            "vendor-workbench-project-ir-report-{}.json",
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
        let summary = inspect_project_ir_report(&path).unwrap();
        std::fs::remove_file(path).unwrap();
        assert_eq!(summary.functions, 3);
        assert_eq!(summary.registers, 2);
        assert_eq!(summary.field_candidates, 4);
    }
}
