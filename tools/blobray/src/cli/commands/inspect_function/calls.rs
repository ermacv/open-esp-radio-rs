//! Focused callsite report and human rendering.

use super::{CallsiteInvestigationReport, ProfiledCallsite};
use crate::{
    cli::output,
    function_investigation::{CallKnowledgeEvidence, FunctionInvestigationReport},
};

fn call_matches(call: &CallKnowledgeEvidence, filter: Option<&str>) -> bool {
    filter.is_none_or(|filter| {
        call.target.contains(filter)
            || call.kind.contains(filter)
            || call
                .semantic_operation
                .as_deref()
                .is_some_and(|operation| operation.contains(filter))
            || call
                .provenance
                .iter()
                .any(|evidence| evidence.contains(filter))
    })
}

pub(super) fn report<'a>(
    report: &'a FunctionInvestigationReport,
    filter: Option<&'a str>,
) -> CallsiteInvestigationReport<'a> {
    CallsiteInvestigationReport {
        schema_version: 1,
        command: "inspect function calls",
        source: &report.source,
        symbol: &report.symbol,
        filter,
        calls: report
            .semantics
            .iter()
            .flat_map(|semantic| {
                semantic
                    .calls
                    .iter()
                    .filter(move |call| call_matches(call, filter))
                    .map(move |call| ProfiledCallsite {
                        profile: &semantic.profile,
                        call,
                    })
            })
            .collect(),
    }
}

pub(super) fn render(report: &FunctionInvestigationReport, filter: Option<&str>) {
    outputln!("{}", output::heading("Callsite investigation"));
    outputln!("Function: {}:{}", report.source, report.symbol);
    if let Some(filter) = filter {
        outputln!("Filter:   {filter}");
    }

    let calls = report
        .semantics
        .iter()
        .flat_map(|semantic| {
            semantic.calls.iter().filter_map(move |call| {
                call_matches(call, filter).then_some((semantic.profile.as_str(), call))
            })
        })
        .collect::<Vec<_>>();
    if calls.is_empty() {
        outputln!(
            "\n{}",
            output::warning("No matching callsites were recovered.")
        );
        outputln!(
            "Use --full to inspect the lossless body when semantic execution stops before a call."
        );
        return;
    }

    if filter.is_none() || calls.len() > 1 {
        if output::human_width() < 120 {
            outputln!("\nCalls:");
            for (_, call) in &calls {
                outputln!(
                    "  {}  target={}  arguments={}",
                    call.site
                        .map(|site| format!("{site:#010x}"))
                        .unwrap_or_else(|| "composed".to_owned()),
                    call.target_status,
                    argument_summary(call).replace('\n', ", ")
                );
                outputln!("    {}", call.target);
                if let Some(operation) = &call.semantic_operation {
                    outputln!("    → {operation}");
                }
            }
        } else {
            outputln!(
                "\n{}",
                crate::cli::table::render(
                    ["Site", "Target", "Target proof", "Arguments"],
                    calls.iter().map(|(_, call)| [
                        call.site
                            .map(|site| format!("{site:#010x}"))
                            .unwrap_or_else(|| "composed".to_owned()),
                        call.semantic_operation.as_ref().map_or_else(
                            || call.target.clone(),
                            |operation| { format!("{}\n→ {operation}", call.target) }
                        ),
                        call.target_status.to_owned(),
                        argument_summary(call),
                    ]),
                )
            );
        }
    }

    if filter.is_none() && !output::details() {
        outputln!(
            "\nUse --call TARGET to inspect one boundary, or --details to expand every callsite."
        );
        return;
    }

    for (profile, call) in calls {
        outputln!(
            "\n{}",
            output::heading(format!(
                "{} @ {}",
                call.target,
                call.site
                    .map(|site| format!("{site:#010x}"))
                    .unwrap_or_else(|| "composed call".to_owned())
            ))
        );
        outputln!("Profile: {profile}");
        outputln!("Target:  {} ({})", call.target, call.target_status);
        if !call.target_candidates.is_empty() {
            outputln!("Candidates: {}", call.target_candidates.join(", "));
        }
        if let Some(blocker) = &call.target_blocker {
            outputln!("Target blocker: {blocker}");
        }
        if call.argument_evidence.is_empty() {
            outputln!(
                "Arguments: {}",
                if call.argument_shapes == 0 {
                    "not recovered"
                } else {
                    "ABI has no arguments"
                }
            );
        } else {
            outputln!("Arguments:");
            for argument in &call.argument_evidence {
                outputln!(
                    "  a{} = {}  [{}]",
                    argument.position,
                    argument.value,
                    argument.status
                );
                if output::details() {
                    outputln!("       {}", argument.provenance);
                }
            }
        }
        if !call.guards.is_empty() {
            outputln!("Paths: {} guarded expression(s)", call.guards.len());
            if output::details() {
                for (index, guard) in call.guards.iter().enumerate() {
                    let abbreviated = abbreviate(guard, 240);
                    outputln!("  {}. {abbreviated}", index + 1);
                }
                if call.guards.iter().any(|guard| guard.chars().count() > 240) {
                    outputln!("  Human view abbreviated; --format json preserves exact guards.");
                }
            }
        }
        if !call.provenance.is_empty() {
            outputln!("Evidence:");
            for evidence in &call.provenance {
                outputln!("  - {evidence}");
            }
        }
    }
}

fn argument_summary(call: &crate::function_investigation::CallKnowledgeEvidence) -> String {
    if call.argument_evidence.is_empty() {
        return if call.argument_shapes == 0 {
            "not recovered".to_owned()
        } else {
            "none".to_owned()
        };
    }
    let exact = call
        .argument_evidence
        .iter()
        .filter(|argument| argument.status == "exact")
        .count();
    let partial = call
        .argument_evidence
        .iter()
        .filter(|argument| argument.status == "partial")
        .count();
    let unresolved = call.argument_evidence.len() - exact - partial;
    if call.semantic_operation.is_none() && call.execution_model.is_none() {
        let mut parts = Vec::new();
        if exact != 0 {
            parts.push(format!("{exact} exact"));
        }
        if partial != 0 {
            parts.push(format!("{partial} partial"));
        }
        parts.push("arity unknown".to_owned());
        if call.argument_shapes > 1 {
            parts.push(format!("{} paths", call.argument_shapes));
        }
        return parts.join("\n");
    }
    let mut parts = vec![format!("{exact}/{} exact", call.argument_evidence.len())];
    if partial != 0 {
        parts.push(format!("{partial} partial"));
    }
    if unresolved != 0 {
        parts.push(format!("{unresolved} unresolved"));
    }
    if call.argument_shapes > 1 {
        parts.push(format!("{} paths", call.argument_shapes));
    }
    parts.join("\n")
}

fn abbreviate(value: &str, maximum_chars: usize) -> String {
    let mut chars = value.chars();
    let prefix = chars.by_ref().take(maximum_chars).collect::<String>();
    if chars.next().is_some() {
        format!("{prefix}…")
    } else {
        prefix
    }
}
