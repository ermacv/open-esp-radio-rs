//! Explainable prioritization of the next project research action.

use crate::{
    Result,
    application::{generated_file, research},
    cli::{ResearchNextArgs, ResearchRankingArg, output, table},
};

pub(super) fn run(
    arguments: ResearchNextArgs,
    session: &crate::application::ProjectSession,
) -> Result<bool> {
    let strategy = match arguments.strategy {
        ResearchRankingArg::Impact => research::ResearchRankingStrategy::Impact,
        ResearchRankingArg::QuickWins => research::ResearchRankingStrategy::QuickWins,
        ResearchRankingArg::Frontier => research::ResearchRankingStrategy::Frontier,
    };
    let report = research::next(
        session,
        research::ResearchNextOptions {
            scope: arguments.scope.as_deref(),
            protocol: arguments.protocol.as_deref(),
            strategy,
            budget: arguments.budget,
            limit: usize::from(arguments.limit),
        },
    )?;
    if let Some(path) = arguments.output.as_deref() {
        generated_file::write_or_check_json(path, &report, arguments.check, "research plan", true)?;
    }
    output::render_report(&report, || render(&report));
    Ok(true)
}

fn render(report: &research::ResearchNextReport) {
    outputln!("{}", output::heading("Research next"));
    outputln!(
        "\nReturned {} of {} {} actions ({} total actions from {} findings) across {} review scopes.",
        report.returned_actions,
        report.strategy_actions,
        report.strategy.label(),
        report.total_actions,
        report.total_findings,
        report.analyzed_scopes.len()
    );
    outputln!(
        "Filter: protocol {}; scope {}; budget {}.",
        report.protocol.as_deref().unwrap_or("all"),
        report.scope.as_deref().unwrap_or("all"),
        report.budget.map_or_else(
            || "unbounded".to_owned(),
            |budget| format!("{}/{} cost units", report.consumed_budget, budget)
        )
    );
    if let Some(diagnostic) = &report.verification_diagnostic {
        outputln!(
            "\n{}: verification impact was omitted: {}",
            output::warning("PARTIAL PRIORITIZATION"),
            diagnostic
        );
    }
    if let Some(diagnostic) = &report.capability_diagnostic {
        outputln!(
            "\n{}: capability context was omitted: {}",
            output::warning("PARTIAL PRIORITIZATION"),
            diagnostic
        );
    }
    if let Some(diagnostic) = &report.selection_diagnostic {
        outputln!("\n{}: {diagnostic}", output::warning("NO ACTION FITS"));
    }
    if report.actions.is_empty() {
        if report.total_actions == 0 {
            outputln!(
                "\n{}",
                output::warning("NO CANDIDATES DERIVED FROM CURRENT INPUTS")
            );
        }
        return;
    }
    outputln!(
        "\n{}",
        table::render(
            [
                "#",
                "Kind",
                "Score B/E",
                "Unlock G/O/M",
                "Co",
                "Cost",
                "Findings",
                "Action"
            ],
            report.actions.iter().map(|candidate| [
                candidate.rank.to_string(),
                candidate.kinds.join(","),
                format!(
                    "{} {}/{}",
                    candidate.score,
                    candidate.score_explanation.benefit_points,
                    candidate.score_explanation.effort_points
                ),
                format!(
                    "{}/{}/{}",
                    candidate.guaranteed_unlock,
                    candidate.optimistic_unlock,
                    candidate.marginal_unlock_after_co_blockers
                ),
                candidate.co_blockers.to_string(),
                candidate.estimated_cost.clone(),
                candidate.findings.len().to_string(),
                table::compact(action_label(&candidate.inspect_command), 44),
            ]),
        )
    );
    let first = &report.actions[0];
    let finding = &first.findings[0];
    outputln!(
        "\n{}",
        output::heading(format!("Top {} action", report.strategy.label()))
    );
    outputln!(
        "Why: {}",
        if output::details() {
            finding.summary.clone()
        } else {
            table::compact(&finding.summary, 320)
        }
    );
    outputln!("Knowledge: {}", finding.knowledge_required);
    outputln!("Confidence: {}", first.confidence);
    outputln!(
        "Score: {} = 100 × {} benefit / {} effort ({} cost units)",
        first.score,
        first.score_explanation.benefit_points,
        first.score_explanation.effort_points,
        first.score_explanation.estimated_cost_units,
    );
    outputln!(
        "Impact: {} direct; {} guaranteed / {} optimistic / {} marginal",
        first.direct_functions,
        first.guaranteed_unlock,
        first.optimistic_unlock,
        first.marginal_unlock_after_co_blockers,
    );
    if !first.co_blocker_ids.is_empty() {
        outputln!("Co-blockers: {}", first.co_blocker_ids.join(", "));
    }
    outputln!("Evidence:");
    for evidence in &finding.evidence_required {
        outputln!("  - {evidence}");
    }
    outputln!("Typed consumers:");
    if finding.consumers.is_empty() {
        outputln!("  - unresolved; inspect evidence before selecting an edit target");
    } else {
        for consumer in &finding.consumers {
            outputln!("  - {}", consumer_label(consumer));
        }
    }
    if first.findings.len() > 1 {
        outputln!(
            "Same inspection also exposes {} distinct finding(s)",
            first.findings.len() - 1
        );
    }
    outputln!("Next inspection: {}", first.inspect_command);
    for command in revalidation_commands(first) {
        outputln!("Revalidate after human review: {command}");
    }

    if output::details() {
        for candidate in &report.actions {
            outputln!(
                "\n#{} {}\n  Kinds: {}\n  Direct functions: {}\n  Co-blockers: {}\n  Findings: {}\n  Next inspection: {}",
                candidate.rank,
                candidate.id,
                candidate.kinds.join(", "),
                candidate.direct_function_ids.join(", "),
                candidate.co_blocker_ids.join(", "),
                candidate.findings.len(),
                candidate.inspect_command
            );
            for finding in &candidate.findings {
                outputln!(
                    "    - {} ({}): {}",
                    finding.id,
                    finding.kind,
                    finding.summary
                );
            }
        }
    }
}

fn resolution_label(resolution: research::ResearchConsumerResolution) -> &'static str {
    match resolution {
        research::ResearchConsumerResolution::Ready => "ready",
        research::ResearchConsumerResolution::NeedsDestination => "needs-destination",
        research::ResearchConsumerResolution::Unavailable => "unavailable",
        research::ResearchConsumerResolution::UnsupportedTarget => "unsupported-target",
    }
}

fn consumer_label(consumer: &research::ResearchConsumer) -> String {
    match consumer {
        research::ResearchConsumer::ReviewedKnowledgeAssertions {
            resolution,
            selected_path,
            assertion_kinds,
            diagnostic,
            ..
        } => format!(
            "reviewed knowledge [{}], assertions {}, target {}{}",
            resolution_label(*resolution),
            assertion_kinds.join(","),
            selected_path.as_ref().map_or_else(
                || "not selected".to_owned(),
                |path| path.display().to_string()
            ),
            diagnostic
                .as_deref()
                .map_or_else(String::new, |value| format!(" ({value})"))
        ),
        research::ResearchConsumer::InterfacePackSlot {
            resolution,
            path,
            contract,
            offset,
            diagnostic,
            ..
        } => format!(
            "interface pack [{}], contract {contract} offset {offset:+#x}, target {}{}",
            resolution_label(*resolution),
            path.as_ref().map_or_else(
                || "not configured".to_owned(),
                |path| path.display().to_string()
            ),
            diagnostic
                .as_deref()
                .map_or_else(String::new, |value| format!(" ({value})"))
        ),
    }
}

fn revalidation_commands(action: &research::ResearchAction) -> Vec<&str> {
    let mut commands = action
        .findings
        .iter()
        .flat_map(|finding| finding.revalidation_commands.iter().map(String::as_str))
        .collect::<Vec<_>>();
    commands.sort_unstable();
    commands.dedup();
    commands
}

fn action_label(command: &str) -> String {
    command
        .strip_prefix("blobray inspect ")
        .and_then(|value| value.split_once(" --project").map(|(target, _)| target))
        .unwrap_or(command)
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_label_removes_copyable_command_boilerplate() {
        assert_eq!(
            action_label("blobray inspect function ble:controller_init --project <project>"),
            "function ble:controller_init"
        );
        assert_eq!(
            action_label("blobray project status --project <project>"),
            "blobray project status --project <project>"
        );
    }
}
