//! Task-first human summary and scope-explicit machine document.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::{
    Result,
    application::status::model::{DetailValue, ProjectStatusReport, Readiness, TargetIdentity},
    cli::{output, table},
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
    scope: &'static str,
    project: ProjectIdentity<'a>,
    target: &'a TargetIdentity,
    pipeline_status: Readiness,
    phases: BTreeMap<&'a str, PhaseDocument<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    publication: Option<crate::cli::output::Publication>,
}

pub(super) fn print_text(report: &ProjectStatusReport) {
    outputln!("{}", output::heading("Project status"));
    outputln!("Project:  {}", report.project_id);
    outputln!("Manifest: {}", report.manifest);
    outputln!(
        "Target:   {} ({}, {})",
        report.target.id,
        report.target.architecture,
        report.target.calling_convention
    );
    let outcome = match report.overall {
        Readiness::Ready => output::success(
            "PIPELINE READY — configured Workbench analysis and verification gates pass",
        ),
        Readiness::Inventory => {
            output::success("PIPELINE INVENTORY — observed evidence is available for review")
        }
        Readiness::Incomplete => {
            output::warning("PIPELINE INCOMPLETE — generated or reviewed evidence is missing")
        }
        Readiness::NotConfigured => {
            output::warning("PIPELINE NOT CONFIGURED — initialize the project workflow")
        }
        Readiness::Invalid => output::failure("PIPELINE BLOCKED — project state is invalid"),
    };
    outputln!("\n{outcome}");

    let problems = report
        .phases
        .iter()
        .flat_map(|phase| {
            phase.components.iter().filter_map(move |component| {
                component.diagnostic.as_deref().map(|diagnostic| {
                    (
                        format!("{}/{}", phase.name, component.name),
                        sanitize(diagnostic),
                    )
                })
            })
        })
        .collect::<Vec<_>>();
    if !problems.is_empty() {
        outputln!("\n{}", output::heading("Problems"));
        for (index, (component, diagnostic)) in problems.iter().take(8).enumerate() {
            outputln!("{}. {component}: {diagnostic}", index + 1);
        }
        if problems.len() > 8 {
            outputln!(
                "{} more problem(s); rerun with --details for the complete list.",
                problems.len() - 8
            );
        }
    }

    // Preserve workflow order. Alphabetical sorting made late verification
    // commands appear before the input or analysis repair that unblocks them.
    let mut actions = Vec::<(String, Vec<String>)>::new();
    let mut action_positions = BTreeMap::<String, usize>::new();
    for phase in &report.phases {
        for component in &phase.components {
            if let Some(action) = component.next_action.as_deref() {
                let action = sanitize(action);
                let component = format!("{}/{}", phase.name, component.name);
                if let Some(position) = action_positions.get(&action).copied() {
                    actions[position].1.push(component);
                } else {
                    action_positions.insert(action.clone(), actions.len());
                    actions.push((action, vec![component]));
                }
            }
        }
    }
    if !actions.is_empty() {
        outputln!("\n{}", output::heading("Next"));
        for (index, (action, components)) in actions.iter().enumerate() {
            outputln!("{}. {action}", index + 1);
            if output::details() {
                outputln!("   Resolves: {}", components.join(", "));
            }
        }
    }

    outputln!("\n{}", output::heading("Workflow"));
    outputln!(
        "{}",
        table::render(
            ["Phase", "Status", "Problems"],
            report.phases.iter().map(|phase| [
                phase.name.clone(),
                phase.status.label().to_owned(),
                phase
                    .components
                    .iter()
                    .filter(|component| {
                        matches!(component.status, Readiness::Invalid | Readiness::Incomplete)
                    })
                    .count()
                    .to_string(),
            ]),
        )
    );

    if output::details() {
        outputln!("\n{}", output::heading("Components"));
        outputln!(
            "{}",
            table::render(
                ["Phase", "Component", "Status"],
                report.phases.iter().flat_map(|phase| {
                    phase.components.iter().map(|component| {
                        [
                            phase.name.clone(),
                            component.name.clone(),
                            component.status.label().to_owned(),
                        ]
                    })
                }),
            )
        );
        if problems.len() > 8 {
            outputln!("\n{}", output::heading("All diagnostics"));
            for (index, (component, diagnostic)) in problems.iter().enumerate() {
                outputln!("{}. {component}: {diagnostic}", index + 1);
            }
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
        schema: 6,
        command: "project status",
        scope: "workbench-pipeline",
        project: ProjectIdentity {
            id: &report.project_id,
            manifest: &report.manifest,
        },
        target: &report.target,
        pipeline_status: report.overall,
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
    use crate::application::status::model::{Component, Phase, TargetIdentity};

    #[test]
    fn json_schema_keeps_phase_and_component_states_explicit() {
        let report = ProjectStatusReport::new(
            "fixture".to_owned(),
            "vendor-project.toml".to_owned(),
            TargetIdentity {
                id: "target".to_owned(),
                architecture: "riscv32".to_owned(),
                calling_convention: "riscv-ilp32".to_owned(),
                knowledge_provider: None,
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
        assert_eq!(document["schema"], 6);
        assert_eq!(document["scope"], "workbench-pipeline");
        assert_eq!(document["pipeline_status"], "incomplete");
        assert!(document.get("overall").is_none());
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
