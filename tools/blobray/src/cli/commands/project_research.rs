//! Explainable prioritization of the next project research action.

use crate::{
    Result,
    application::{generated_file, research},
    cli::{ResearchNextArgs, output, table},
};

pub(super) fn run(
    arguments: ResearchNextArgs,
    session: &crate::application::ProjectSession,
) -> Result<bool> {
    let report = research::next(
        session,
        arguments.scope.as_deref(),
        usize::from(arguments.limit),
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
        "\nRanked {} of {} actions from {} findings across {} review scopes.",
        report.returned_candidates,
        report.total_actions,
        report.total_candidates,
        report.analyzed_scopes.len()
    );
    if let Some(diagnostic) = &report.verification_diagnostic {
        outputln!(
            "\n{}: verification impact was omitted: {}",
            output::warning("PARTIAL PRIORITIZATION"),
            diagnostic
        );
    }
    if report.candidates.is_empty() {
        outputln!("\n{}", output::success("NO OPEN RESEARCH CANDIDATES"));
        return;
    }
    outputln!(
        "\n{}",
        table::render(
            [
                "#",
                "Kind",
                "Score",
                "Unlock G/O/M",
                "Co",
                "Cost",
                "Findings",
                "Action"
            ],
            report.candidates.iter().map(|candidate| [
                candidate.rank.to_string(),
                candidate.kind.clone(),
                candidate.score.to_string(),
                format!(
                    "{}/{}/{}",
                    candidate.guaranteed_unlock,
                    candidate.optimistic_unlock,
                    candidate.marginal_unlock_after_co_blockers
                ),
                candidate.co_blockers.to_string(),
                candidate.estimated_cost.clone(),
                (candidate.related_findings.len() + 1).to_string(),
                table::compact(action_label(&candidate.next_command), 44),
            ]),
        )
    );
    let first = &report.candidates[0];
    outputln!("\n{}", output::heading("Highest-impact action"));
    outputln!(
        "Why: {}",
        if output::details() {
            first.summary.clone()
        } else {
            table::compact(&first.summary, 320)
        }
    );
    outputln!("Knowledge: {}", first.knowledge_required);
    outputln!("Confidence: {}", first.confidence);
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
    for evidence in &first.evidence_required {
        outputln!("  - {evidence}");
    }
    outputln!("Review into: {}", first.review_destinations.join("; "));
    outputln!("Done when: {}", first.completion_conditions.join("; "));
    if !first.related_findings.is_empty() {
        outputln!(
            "Also covers: {} related finding(s)",
            first.related_findings.len()
        );
    }
    outputln!("Next: {}", first.next_command);

    if output::details() {
        for candidate in report.candidates.iter().skip(1) {
            outputln!(
                "\n#{} {} — {}\n  Knowledge: {}\n  Direct functions: {}\n  Co-blockers: {}\n  Review into: {}\n  Done when: {}\n  Related findings: {}\n  Next: {}",
                candidate.rank,
                candidate.id,
                candidate.summary,
                candidate.knowledge_required,
                candidate.direct_function_ids.join(", "),
                candidate.co_blocker_ids.join(", "),
                candidate.review_destinations.join("; "),
                candidate.completion_conditions.join("; "),
                candidate.related_findings.len(),
                candidate.next_command
            );
            for finding in &candidate.related_findings {
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
