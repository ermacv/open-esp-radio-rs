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
                "Inspect target",
                "Kinds (N)",
                "Score B/E",
                "G/O/M · Co · Cost",
            ],
            report.actions.iter().map(|candidate| [
                candidate.rank.to_string(),
                compact_middle(&action_label(&candidate.inspect_command), 34),
                format!(
                    "{} ({})",
                    compact_middle(&candidate.kinds.join(","), 14),
                    candidate.findings.len()
                ),
                format!(
                    "{} {}/{}",
                    candidate.score,
                    candidate.score_explanation.benefit_points,
                    candidate.score_explanation.effort_points
                ),
                format!(
                    "{}/{}/{} · {} · {}",
                    candidate.guaranteed_unlock,
                    candidate.optimistic_unlock,
                    candidate.marginal_unlock_after_co_blockers,
                    candidate.co_blockers,
                    candidate.estimated_cost,
                ),
            ]),
        )
    );
    let first = &report.actions[0];
    outputln!(
        "\n{}",
        output::heading(format!("Top {} action", report.strategy.label()))
    );
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
    render_findings(&first.findings);
    outputln!("Next inspection: {}", first.inspect_command);
    for command in revalidation_commands(first) {
        outputln!("Revalidate after human review: {command}");
    }

    if output::details() {
        for candidate in report.actions.iter().skip(1) {
            outputln!(
                "\n#{} {}\n  Kinds: {}\n  Direct functions: {}\n  Co-blockers: {}\n  Next inspection: {}",
                candidate.rank,
                candidate.id,
                candidate.kinds.join(", "),
                candidate.direct_function_ids.join(", "),
                candidate.co_blocker_ids.join(", "),
                candidate.inspect_command
            );
            render_findings(&candidate.findings);
        }
    }
}

fn render_findings(findings: &[research::ResearchFinding]) {
    outputln!("Findings ({}):", findings.len());
    let (visible, hidden) = visible_findings(findings, output::details());
    for (index, finding) in visible.iter().enumerate() {
        for line in finding_lines(finding, index + 1, output::details()) {
            outputln!("{line}");
        }
    }
    if hidden > 0 {
        outputln!(
            "  … {hidden} more finding(s); use --details or --format json for the complete action"
        );
    }
}

fn visible_findings(
    findings: &[research::ResearchFinding],
    details: bool,
) -> (&[research::ResearchFinding], usize) {
    const COMPACT_LIMIT: usize = 8;
    let visible = if details {
        findings
    } else {
        &findings[..findings.len().min(COMPACT_LIMIT)]
    };
    (visible, findings.len() - visible.len())
}

fn finding_lines(finding: &research::ResearchFinding, number: usize, details: bool) -> Vec<String> {
    let summary = if details {
        finding.summary.clone()
    } else {
        table::compact(&finding.summary, 280)
    };
    let mut lines = vec![format!("  {number}. [{}] {summary}", finding.kind)];
    lines.push(format!("     Knowledge: {}", finding.knowledge_required));
    if finding.consumers.is_empty() {
        lines.push(
            "     Consumer: none [unavailable]; inspect evidence before selecting an edit target"
                .to_owned(),
        );
    } else {
        lines.extend(
            finding
                .consumers
                .iter()
                .map(|consumer| format!("     Consumer: {}", consumer_label(consumer))),
        );
    }
    if !finding.inspection_function_ids.is_empty() {
        lines.push(format!(
            "     Inspect in: {}",
            highlight_list(&finding.inspection_function_ids, details)
        ));
    }
    lines.push(format!(
        "     Evidence: sites {}; channels {}",
        evidence_sites_label(&finding.evidence_sites, details),
        highlight_list(&finding.evidence_channels, details)
    ));
    if !finding.evidence_required.is_empty() {
        lines.push(format!(
            "     Needed: {}",
            highlight_list(&finding.evidence_required, details)
        ));
    }
    lines
}

fn evidence_sites_label(sites: &[u32], details: bool) -> String {
    let sites = sites
        .iter()
        .map(|site| format!("{site:#010x}"))
        .collect::<Vec<_>>();
    highlight_list(&sites, details)
}

fn highlight_list(values: &[String], details: bool) -> String {
    if values.is_empty() {
        return "none linked".to_owned();
    }
    const COMPACT_LIMIT: usize = 4;
    if details || values.len() <= COMPACT_LIMIT {
        return values.join(", ");
    }
    format!(
        "{} (+{} more; use --details)",
        values[..COMPACT_LIMIT].join(", "),
        values.len() - COMPACT_LIMIT
    )
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

fn compact_middle(value: &str, max_chars: usize) -> String {
    let characters = value.chars().collect::<Vec<_>>();
    if characters.len() <= max_chars {
        return value.to_owned();
    }
    if max_chars == 0 {
        return String::new();
    }
    if max_chars == 1 {
        return "…".to_owned();
    }
    let retained = max_chars - 1;
    let leading = retained.div_ceil(2);
    let trailing = retained - leading;
    characters[..leading]
        .iter()
        .copied()
        .chain(std::iter::once('…'))
        .chain(characters[characters.len() - trailing..].iter().copied())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn finding(id: &str, kind: &str, summary: &str) -> research::ResearchFinding {
        research::ResearchFinding {
            id: id.to_owned(),
            kind: kind.to_owned(),
            severity: "warning".to_owned(),
            subject: research::ResearchSubject::AnalysisRoot {
                root_id: id.to_owned(),
            },
            consumers: Vec::new(),
            evidence_sites: Vec::new(),
            evidence_channels: Vec::new(),
            inspection_function_ids: Vec::new(),
            direct_function_ids: Vec::new(),
            guaranteed_function_ids: Vec::new(),
            optimistic_function_ids: Vec::new(),
            marginal_function_ids: Vec::new(),
            co_blocker_ids: Vec::new(),
            affected_scope_roots: Vec::new(),
            scopes: Vec::new(),
            capability_links: Vec::new(),
            verification_links: Vec::new(),
            publication_scopes: Vec::new(),
            knowledge_required: "reviewed semantic model".to_owned(),
            evidence_required: Vec::new(),
            revalidation_commands: Vec::new(),
            summary: summary.to_owned(),
        }
    }

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

    #[test]
    fn inspect_targets_preserve_the_identity_prefix_and_suffix() {
        assert_eq!(
            compact_middle("function ble-controller:r_ble_hci_trans_env_deinit", 34),
            "function ble-cont…trans_env_deinit"
        );
        assert_eq!(compact_middle("radio", 1), "…");
        assert_eq!(compact_middle("radio", 0), "");
    }

    #[test]
    fn coalesced_findings_each_keep_summary_consumer_and_evidence_highlights() {
        let mut first = finding("slot-a", "interface-layout", "review producer slot");
        first.consumers = vec![research::ResearchConsumer::InterfacePackSlot {
            resolution: research::ResearchConsumerResolution::Ready,
            path: Some("review/interfaces.toml".into()),
            contract: "btbb:callbacks".to_owned(),
            anchor: Some("controller".to_owned()),
            template: None,
            offset: 0x10,
            width: 32,
            diagnostic: None,
        }];
        first.evidence_sites = vec![0x4000_1000];
        first.evidence_channels = vec!["linked-ir".to_owned()];
        first.inspection_function_ids =
            vec!["btbb::producer".to_owned(), "btbb::consumer".to_owned()];
        first.evidence_required = vec!["producer and consumer access sites".to_owned()];

        let second = finding("register-a", "register-model", "name the observed register");
        let lines = [first, second]
            .iter()
            .enumerate()
            .flat_map(|(index, finding)| finding_lines(finding, index + 1, false))
            .collect::<Vec<_>>()
            .join("\n");

        assert!(lines.contains("1. [interface-layout] review producer slot"));
        assert!(lines.contains("interface pack [ready]"));
        assert!(lines.contains("Inspect in: btbb::producer, btbb::consumer"));
        assert!(lines.contains("sites 0x40001000; channels linked-ir"));
        assert!(lines.contains("Needed: producer and consumer access sites"));
        assert!(lines.contains("2. [register-model] name the observed register"));
        assert!(lines.contains("Consumer: none [unavailable]"));
    }

    #[test]
    fn compact_highlights_disclose_truncation_without_affecting_details() {
        let values = ["one", "two", "three", "four", "five"]
            .map(str::to_owned)
            .to_vec();

        assert_eq!(
            highlight_list(&values, false),
            "one, two, three, four (+1 more; use --details)"
        );
        assert_eq!(highlight_list(&values, true), "one, two, three, four, five");
    }

    #[test]
    fn compact_summary_bounds_large_coalesced_actions_without_losing_detail_mode() {
        let findings = (0..12)
            .map(|index| finding(&format!("finding-{index}"), "analysis", "review"))
            .collect::<Vec<_>>();

        let (compact, hidden) = visible_findings(&findings, false);
        assert_eq!(compact.len(), 8);
        assert_eq!(hidden, 4);

        let (detailed, hidden) = visible_findings(&findings, true);
        assert_eq!(detailed.len(), 12);
        assert_eq!(hidden, 0);
    }

    #[test]
    fn ranking_table_keeps_the_inspect_target_readable() {
        let rendered = table::render(
            [
                "#",
                "Inspect target",
                "Kinds (N)",
                "Score B/E",
                "G/O/M · Co · Cost",
            ],
            [[
                "1".to_owned(),
                compact_middle("function ble-controller:r_ble_hci_trans_env_deinit", 34),
                format!("{} (2)", compact_middle("control-flow,memory-store", 14)),
                "1369 972/71".to_owned(),
                "33/34/34 · 2 · high".to_owned(),
            ]],
        );

        assert!(
            rendered.contains("function ble-cont…trans_env_deinit"),
            "inspect target was wrapped:\n{rendered}"
        );
        assert!(rendered.contains("control…-store (2)"), "{rendered}");
        assert!(rendered.contains("33/34/34 · 2 · high"), "{rendered}");
    }
}
