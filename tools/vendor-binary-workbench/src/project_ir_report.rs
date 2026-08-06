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
    if integer(root, "schema_version", "linked-IR report")? != 31 {
        return Err(format!("project IR output requires schema 31 in {}", path.display()).into());
    }
    if string(root, "command", "linked-IR report")? != "ir export" {
        return Err(format!("{} is not an ir export report", path.display()).into());
    }
    if boolean(root, "completeness_claim", "linked-IR report")?
        || boolean(root, "mmio_field_semantics_claim", "linked-IR report")?
    {
        return Err(format!(
            "project IR output {} makes an unsupported completeness or field-semantics claim",
            path.display()
        )
        .into());
    }
    let summary = root
        .get("summary")
        .ok_or("linked-IR report requires summary")?;
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
        .ok_or_else(|| format!("{context} must be an object").into())
}

fn string<'a>(object: &'a Map<String, Value>, key: &str, context: &str) -> Result<&'a str> {
    object
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("{context} requires non-empty string {key:?}").into())
}

fn integer(object: &Map<String, Value>, key: &str, context: &str) -> Result<u64> {
    object
        .get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("{context} requires non-negative integer {key:?}").into())
}

fn count(object: &Map<String, Value>, key: &str, context: &str) -> Result<usize> {
    integer(object, key, context)?
        .try_into()
        .map_err(|_| format!("invalid count {key:?} in {context}").into())
}

fn boolean(object: &Map<String, Value>, key: &str, context: &str) -> Result<bool> {
    object
        .get(key)
        .and_then(Value::as_bool)
        .ok_or_else(|| format!("{context} requires boolean {key:?}").into())
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
  "schema_version": 31,
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
