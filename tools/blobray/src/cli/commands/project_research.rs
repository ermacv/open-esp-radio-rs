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
        "\nRanked {} of {} candidates across {} review scopes.",
        report.returned_candidates,
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
                "Candidate"
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
                table::compact(&candidate.id, 44),
            ]),
        )
    );
    let first = &report.candidates[0];
    outputln!("\n{}", output::heading("Highest-impact action"));
    outputln!("Why: {}", first.summary);
    outputln!("Knowledge: {}", first.knowledge_required);
    outputln!("Confidence: {}", first.confidence);
    outputln!("Next: {}", first.next_command);

    if output::details() {
        for candidate in report.candidates.iter().skip(1) {
            outputln!(
                "\n#{} {} — {}\n  Knowledge: {}\n  Next: {}",
                candidate.rank,
                candidate.id,
                candidate.summary,
                candidate.knowledge_required,
                candidate.next_command
            );
        }
    }
}
