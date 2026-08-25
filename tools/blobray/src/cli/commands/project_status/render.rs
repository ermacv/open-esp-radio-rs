//! Task-first human summary and scope-explicit machine document.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::{
    Result,
    application::{
        FollowUpRequirements, ProjectContext,
        status::model::{
            DetailValue, EvidenceFreshness, ProjectStatusReport, Readiness, ResearchProgress,
            StatusValidation, TargetIdentity, ValidationDepth,
        },
    },
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
struct FreshnessDimension {
    status: EvidenceFreshness,
    validation_depth: ValidationDepth,
}

#[derive(Serialize)]
struct WorkflowDimensions<'a> {
    freshness: FreshnessDimension,
    research: &'a ResearchProgress,
    verification: Readiness,
}

#[derive(Serialize)]
pub(super) struct StatusDocument<'a> {
    schema: u32,
    command: &'static str,
    scope: &'static str,
    project: ProjectIdentity<'a>,
    target: &'a TargetIdentity,
    validation: &'a StatusValidation,
    dimensions: WorkflowDimensions<'a>,
    pipeline_status: Readiness,
    phases: BTreeMap<&'a str, PhaseDocument<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    publication: Option<crate::cli::output::Publication>,
}

pub(super) fn print_text(report: &ProjectStatusReport, context: &ProjectContext<'_>) {
    outputln!("{}", output::heading("Project status"));
    outputln!("Project:  {}", report.project_id);
    outputln!("Manifest: {}", report.manifest);
    outputln!(
        "Target:   {} ({}, {})",
        report.target.id,
        report.target.architecture,
        report.target.calling_convention
    );
    outputln!("Validation: shallow project-status inspection");
    outputln!("Freshness:    {}", freshness_summary(report));
    outputln!("Research:     {}", research_summary(report));
    outputln!("Verification: {}", report.verification.label());
    outputln!("Deep validation:");
    outputln!(
        "  {}",
        context.follow_up_command("project doctor", FollowUpRequirements::ANALYSIS)
    );
    outputln!(
        "  {}",
        context.follow_up_command("project check", FollowUpRequirements::ANALYSIS)
    );
    let outcome = match report.overall {
        Readiness::Ready => output::success(
            "SHALLOW OVERVIEW — configured outputs are present; freshness not validated",
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

    let implemented_unqualified = report
        .phases
        .iter()
        .flat_map(|phase| &phase.components)
        .find(|component| component.name == "last-verification")
        .and_then(|component| component.details.get("implemented_unqualified"))
        .and_then(|value| match value {
            DetailValue::Unsigned(value) => Some(*value),
            _ => None,
        })
        .unwrap_or(0);
    if implemented_unqualified != 0 {
        outputln!(
            "{}",
            output::warning(format!(
                "COVERAGE DEBT — {implemented_unqualified} implemented function(s) have no qualifying production trace"
            ))
        );
    }

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
                let action = sanitize_next_action(action);
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

    outputln!(
        "\n{}",
        output::heading("Artifact readiness (not research completeness)")
    );
    outputln!(
        "{}",
        table::render(
            ["Phase", "Status", "Problems"],
            report.phases.iter().map(|phase| [
                phase.name.clone(),
                phase.status.label().to_owned(),
                phase_problem_count(phase).to_string(),
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
        let component_details = report
            .phases
            .iter()
            .flat_map(|phase| {
                phase.components.iter().flat_map(move |component| {
                    component.details.iter().map(move |(field, value)| {
                        [
                            phase.name.clone(),
                            component.name.clone(),
                            field.clone(),
                            human_detail_value(value),
                        ]
                    })
                })
            })
            .collect::<Vec<_>>();
        if !component_details.is_empty() {
            outputln!("\n{}", output::heading("Component details"));
            outputln!(
                "{}",
                table::render(["Phase", "Component", "Field", "Value"], component_details,)
            );
        }
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
        schema: 9,
        command: "project status",
        scope: "blobray-pipeline",
        project: ProjectIdentity {
            id: &report.project_id,
            manifest: &report.manifest,
        },
        target: &report.target,
        validation: &report.validation,
        dimensions: WorkflowDimensions {
            freshness: FreshnessDimension {
                status: report.validation.freshness,
                validation_depth: report.validation.depth,
            },
            research: &report.research,
            verification: report.verification,
        },
        pipeline_status: report.overall,
        phases,
        publication,
    }
}

fn freshness_summary(report: &ProjectStatusReport) -> &'static str {
    match report.validation.freshness {
        EvidenceFreshness::Unknown => "unknown — run project doctor or project check",
        EvidenceFreshness::Current => "current",
        EvidenceFreshness::Stale => "stale",
    }
}

fn research_summary(report: &ProjectStatusReport) -> String {
    let research = &report.research;
    if research.scopes == 0 {
        return research.status.label().to_owned();
    }
    format!(
        "{} — {}/{} scope inventories open, {} root causes, {} publication coverage gaps",
        research.status.label(),
        research.inventory_open,
        research.scopes,
        research.root_causes,
        research.publication_coverage_gaps,
    )
}

fn human_detail_value(value: &DetailValue) -> String {
    match value {
        DetailValue::String(value) => sanitize(value),
        DetailValue::Unsigned(value) => value.to_string(),
        DetailValue::Bool(value) => value.to_string(),
        DetailValue::Strings(values) => values.join(", "),
        DetailValue::LinkedIrProfiles(values) => {
            structured_detail_summary(values.len(), "linked-IR profile")
        }
        DetailValue::MmioRegions(values) => structured_detail_summary(values.len(), "MMIO region"),
        DetailValue::Artifacts(values) => structured_detail_summary(values.len(), "artifact"),
        DetailValue::ReviewScopes(values) => {
            structured_detail_summary(values.len(), "review scope")
        }
    }
}

fn structured_detail_summary(count: usize, kind: &str) -> String {
    let suffix = if count == 1 { "" } else { "s" };
    format!("{count} {kind}{suffix} (use --format json for structured values)")
}

pub(super) fn json_document(document: &StatusDocument<'_>) -> Result<String> {
    let mut output = serde_json::to_string_pretty(document)?;
    output.push('\n');
    Ok(output)
}

fn sanitize(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn sanitize_next_action(value: &str) -> String {
    value
        .chars()
        .map(|character| match character {
            '\r' | '\n' => ' ',
            character => character,
        })
        .collect()
}

fn phase_problem_count(phase: &crate::application::status::model::Phase) -> usize {
    phase
        .components
        .iter()
        .filter(|component| component.diagnostic.is_some())
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::status::model::{
        Component, LinkedIrProfileDetail, Phase, TargetIdentity,
    };

    #[test]
    fn next_action_sanitizing_preserves_shell_quoted_argument_bytes() {
        let command =
            "blobray project status --project '/tmp/owner'\"'\"'s/project  tree/vendor.toml'";
        assert_eq!(sanitize_next_action(command), command);
        assert_eq!(
            sanitize_next_action("blobray project status\r\n--help"),
            "blobray project status  --help"
        );
    }

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
        assert_eq!(document["schema"], 9);
        assert_eq!(document["scope"], "blobray-pipeline");
        assert_eq!(document["validation"]["depth"], "shallow");
        assert_eq!(document["validation"]["freshness"], "unknown");
        assert_eq!(
            document["dimensions"]["research"]["status"],
            "not-configured"
        );
        assert_eq!(document["dimensions"]["verification"], "not-configured");
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

    #[test]
    fn human_detail_values_preserve_scalar_and_structured_component_evidence() {
        assert_eq!(
            human_detail_value(&DetailValue::String("shallow\ncheck".to_owned())),
            "shallow check"
        );
        assert_eq!(
            human_detail_value(&DetailValue::Strings(vec![
                "first".to_owned(),
                "second".to_owned(),
            ])),
            "first, second"
        );
        assert_eq!(
            human_detail_value(&DetailValue::LinkedIrProfiles(vec![
                LinkedIrProfileDetail {
                    id: "fixture".to_owned(),
                    sources: vec!["vendor".to_owned()],
                    missing_sources: Vec::new(),
                    entry_contract: "neutral".to_owned(),
                    contract_status: "ready",
                    contract_error: None,
                    output: "generated/fixture.ir".to_owned(),
                    output_status: "ready",
                    output_error: None,
                    functions: 1,
                    registers: 2,
                    field_candidates: 3,
                }
            ])),
            "1 linked-IR profile (use --format json for structured values)"
        );
        assert_eq!(
            human_detail_value(&DetailValue::ReviewScopes(Vec::new())),
            "0 review scopes (use --format json for structured values)"
        );
    }

    #[test]
    fn workflow_problem_count_matches_rendered_diagnostics() {
        let phase = Phase::collect(
            "inputs",
            vec![
                Component::new("ready-with-warning", Readiness::Ready)
                    .diagnostic("configured fallback is in use"),
                Component::new("incomplete-without-diagnostic", Readiness::Incomplete),
                Component::new("invalid-with-diagnostic", Readiness::Invalid)
                    .diagnostic("invalid input"),
            ],
        );

        assert_eq!(phase_problem_count(&phase), 2);
    }

    #[test]
    fn diagnostics_are_compacted_to_one_readable_line() {
        assert_eq!(
            sanitize("parse error\n  at line 3\tunknown value\r\n"),
            "parse error at line 3 unknown value"
        );
    }
}
