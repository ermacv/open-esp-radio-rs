//! Stable human summary and schema-1 JSON document.

use serde_json::{Map, Value, json};

use super::model::StatusReport;
use crate::Result;

pub(super) fn print_text(report: &StatusReport) {
    println!(
        "PROJECT-STATUS\tid={}\toverall={}\tmanifest={}",
        report.project_id,
        report.overall.label(),
        report.manifest
    );
    for phase in &report.phases {
        println!(
            "PROJECT-PHASE\tname={}\tstatus={}\tcomponents={}",
            phase.name,
            phase.status.label(),
            phase.components.len()
        );
        for component in &phase.components {
            println!(
                "PROJECT-COMPONENT\tphase={}\tname={}\tstatus={}\tdiagnostic={}",
                phase.name,
                component.name,
                component.status.label(),
                component
                    .diagnostic
                    .as_deref()
                    .map(sanitize)
                    .unwrap_or_else(|| "-".to_owned())
            );
        }
    }
}

pub(super) fn json_document(report: &StatusReport) -> Result<String> {
    let phases = report
        .phases
        .iter()
        .map(|phase| {
            let components = phase
                .components
                .iter()
                .map(|component| {
                    let mut output = component.details.clone();
                    output.insert("name".to_owned(), Value::String(component.name.to_owned()));
                    output.insert(
                        "status".to_owned(),
                        Value::String(component.status.label().to_owned()),
                    );
                    if let Some(diagnostic) = &component.diagnostic {
                        output.insert("diagnostic".to_owned(), Value::String(diagnostic.clone()));
                    }
                    Value::Object(output)
                })
                .collect::<Vec<_>>();
            (
                phase.name.to_owned(),
                json!({
                    "status": phase.status.label(),
                    "components": components,
                }),
            )
        })
        .collect::<Map<_, _>>();
    let document = json!({
        "schema": 1,
        "command": "project status",
        "project": {
            "id": report.project_id,
            "manifest": report.manifest,
        },
        "target": {
            "id": report.target.id,
            "architecture": report.target.architecture,
            "calling_convention": report.target.calling_convention,
            "harness": report.target.harness,
        },
        "overall": report.overall.label(),
        "phases": phases,
    });
    let mut output = serde_json::to_string_pretty(&document)?;
    output.push('\n');
    Ok(output)
}

fn sanitize(value: &str) -> String {
    value
        .chars()
        .map(|character| match character {
            '\t' | '\r' | '\n' => ' ',
            character => character,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::commands::project_status::model::{
        Component, Phase, Readiness, StatusReport, TargetIdentity,
    };

    #[test]
    fn json_schema_keeps_phase_and_component_states_explicit() {
        let report = StatusReport::new(
            "fixture".to_owned(),
            "vendor-project.toml".to_owned(),
            TargetIdentity {
                id: "target".to_owned(),
                architecture: "riscv32".to_owned(),
                calling_convention: "riscv-ilp32".to_owned(),
                harness: None,
            },
            vec![Phase::collect(
                "analysis",
                vec![Component::new("linked_ir", Readiness::Incomplete).detail("profiles", 2usize)],
            )],
        );
        let document: Value = serde_json::from_str(&json_document(&report).unwrap()).unwrap();
        assert_eq!(document["schema"], 1);
        assert_eq!(document["overall"], "incomplete");
        assert_eq!(
            document["phases"]["analysis"]["components"][0]["profiles"],
            2
        );
    }
}
