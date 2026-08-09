//! Stable human summary and schema-2 JSON document.

use std::collections::BTreeMap;

use serde::Serialize;
use tabled::{builder::Builder, settings::Style};

use crate::{
    Result,
    application::status::model::{DetailValue, ProjectStatusReport, Readiness, TargetIdentity},
};

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
    #[serde(skip_serializing_if = "Option::is_none")]
    next_action: Option<&'a str>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    publication: Option<crate::cli::output::Publication>,
}

pub(super) fn print_text(report: &ProjectStatusReport) {
    outputln!(
        "Project status: {} — {}",
        report.project_id,
        report.overall.label()
    );
    outputln!("  manifest: {}", report.manifest);
    outputln!(
        "  target:   {} ({}, {})",
        report.target.id,
        report.target.architecture,
        report.target.calling_convention
    );
    let mut rows = Builder::default();
    rows.push_record(["Phase", "Component", "Status", "Diagnostic"]);
    for phase in &report.phases {
        for component in &phase.components {
            rows.push_record([
                phase.name.to_owned(),
                component.name.to_owned(),
                component.status.label().to_owned(),
                component
                    .diagnostic
                    .as_deref()
                    .map(sanitize)
                    .unwrap_or_default(),
            ]);
        }
    }
    let mut table = rows.build();
    table.with(Style::rounded());
    outputln!("{table}");

    let mut actions = BTreeMap::<&str, Vec<String>>::new();
    for phase in &report.phases {
        for component in &phase.components {
            if let Some(action) = component.next_action.as_deref() {
                actions
                    .entry(action)
                    .or_default()
                    .push(format!("{}/{}", phase.name, component.name));
            }
        }
    }
    if !actions.is_empty() {
        outputln!("\nNext actions:");
        for (action, components) in actions {
            outputln!("  {}: {}", components.join(", "), sanitize(action));
        }
    }
}

pub(super) fn document(
    report: &ProjectStatusReport,
    publication: Option<crate::cli::output::Publication>,
) -> StatusDocument<'_> {
    let phases = report
        .phases
        .iter()
        .map(|phase| {
            let components = phase
                .components
                .iter()
                .map(|component| ComponentDocument {
                    name: &component.name,
                    status: component.status,
                    diagnostic: component.diagnostic.as_deref(),
                    next_action: component.next_action.as_deref(),
                    details: &component.details,
                })
                .collect::<Vec<_>>();
            (
                phase.name.as_str(),
                PhaseDocument {
                    status: phase.status,
                    components,
                },
            )
        })
        .collect();
    StatusDocument {
        schema: 2,
        command: "project status",
        project: ProjectIdentity {
            id: &report.project_id,
            manifest: &report.manifest,
        },
        target: &report.target,
        overall: report.overall,
        phases,
        publication,
    }
}

pub(super) fn json_document(document: &StatusDocument<'_>) -> Result<String> {
    let mut output = serde_json::to_string_pretty(document)?;
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
    use crate::application::status::model::{
        Component, Phase, ProjectStatusReport, Readiness, TargetIdentity,
    };

    #[test]
    fn json_schema_keeps_phase_and_component_states_explicit() {
        let report = ProjectStatusReport::new(
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
                vec![
                    Component::new("linked_ir", Readiness::Incomplete)
                        .detail("profiles", 2usize)
                        .next_action("run ir build"),
                ],
            )],
        );
        let document: serde_json::Value =
            serde_json::from_str(&json_document(&document(&report, None)).unwrap()).unwrap();
        assert_eq!(document["schema"], 2);
        assert_eq!(document["overall"], "incomplete");
        assert_eq!(
            document["phases"]["analysis"]["components"][0]["profiles"],
            2
        );
        assert_eq!(
            document["phases"]["analysis"]["components"][0]["next_action"],
            "run ir build"
        );
    }
}
