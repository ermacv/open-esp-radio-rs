//! Stable human summary and schema-1 JSON document.

use std::collections::BTreeMap;

use serde::Serialize;

use super::model::{DetailValue, Readiness, StatusReport, TargetIdentity};
use crate::Result;

#[derive(Serialize)]
struct ProjectIdentity<'a> {
    id: &'a str,
    manifest: &'a str,
}

#[derive(Serialize)]
struct ComponentDocument<'a> {
    name: &'a str,
    status: Readiness,
    #[serde(skip_serializing_if = "Option::is_none")]
    diagnostic: Option<&'a str>,
    #[serde(flatten)]
    details: &'a BTreeMap<String, DetailValue>,
}

#[derive(Serialize)]
struct PhaseDocument<'a> {
    status: Readiness,
    components: Vec<ComponentDocument<'a>>,
}

#[derive(Serialize)]
pub(super) struct StatusDocument<'a> {
    schema: u32,
    command: &'static str,
    project: ProjectIdentity<'a>,
    target: &'a TargetIdentity,
    overall: Readiness,
    phases: BTreeMap<&'a str, PhaseDocument<'a>>,
}

pub(super) fn print_text(report: &StatusReport) {
    outputln!(
        "PROJECT-STATUS\tid={}\toverall={}\tmanifest={}",
        report.project_id,
        report.overall.label(),
        report.manifest
    );
    for phase in &report.phases {
        outputln!(
            "PROJECT-PHASE\tname={}\tstatus={}\tcomponents={}",
            phase.name,
            phase.status.label(),
            phase.components.len()
        );
        for component in &phase.components {
            outputln!(
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

pub(super) fn document(report: &StatusReport) -> StatusDocument<'_> {
    let phases = report
        .phases
        .iter()
        .map(|phase| {
            let components = phase
                .components
                .iter()
                .map(|component| ComponentDocument {
                    name: component.name,
                    status: component.status,
                    diagnostic: component.diagnostic.as_deref(),
                    details: &component.details,
                })
                .collect::<Vec<_>>();
            (
                phase.name,
                PhaseDocument {
                    status: phase.status,
                    components,
                },
            )
        })
        .collect();
    StatusDocument {
        schema: 1,
        command: "project status",
        project: ProjectIdentity {
            id: &report.project_id,
            manifest: &report.manifest,
        },
        target: &report.target,
        overall: report.overall,
        phases,
    }
}

pub(super) fn json_document(report: &StatusReport) -> Result<String> {
    let mut output = serde_json::to_string_pretty(&document(report))?;
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
        let document: serde_json::Value =
            serde_json::from_str(&json_document(&report).unwrap()).unwrap();
        assert_eq!(document["schema"], 1);
        assert_eq!(document["overall"], "incomplete");
        assert_eq!(
            document["phases"]["analysis"]["components"][0]["profiles"],
            2
        );
    }
}
