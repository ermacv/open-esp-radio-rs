//! Explainable prioritization of the next project research action.

use std::collections::BTreeSet;

use crate::{
    Result,
    application::{generated_file, research},
    cli::{ResearchFocusArg, ResearchNextArgs, ResearchRankingArg, output, table},
};

const IMPACT_LEGEND: &str = "G = functions for which this is the sole observed blocker and analyzed dependencies are complete; O = optimistic reverse-call reachability within the current analyzed graph; M = directly affected functions counted after co-blockers close; Co = other observed blocker roots. Sets overlap; these estimates rank research and do not prove runtime behavior or completion.";

pub(super) fn run(
    arguments: ResearchNextArgs,
    session: &crate::application::ProjectSession,
) -> Result<bool> {
    let strategy = match arguments.strategy {
        ResearchRankingArg::Impact => research::ResearchRankingStrategy::Impact,
        ResearchRankingArg::QuickWins => research::ResearchRankingStrategy::QuickWins,
        ResearchRankingArg::Frontier => research::ResearchRankingStrategy::Frontier,
    };
    let focus = match arguments.focus {
        ResearchFocusArg::All => research::ResearchFocus::All,
        ResearchFocusArg::HardwareAccess => research::ResearchFocus::HardwareAccess,
    };
    let report = research::next(
        session,
        research::ResearchNextOptions {
            scope: arguments.scope.as_deref(),
            protocol: arguments.protocol.as_deref(),
            finding: arguments.finding.as_deref(),
            strategy,
            focus,
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
    let prerequisites = selected_prerequisites(report);
    let actions = selected_actions(report);
    outputln!("{}", output::heading("Research next"));
    outputln!(
        "\nSelected {} prerequisites and {} actions within limit {}. Complete inventory: {} prerequisites, {} actions and {} findings; {} prerequisites and {} {} actions are strategy-eligible across {} review scopes.",
        prerequisites.len(),
        actions.len(),
        report.selection.limit,
        report.inventory.prerequisites.len(),
        report.inventory.actions.len(),
        report.inventory.findings.len(),
        report.selection.eligible_prerequisites,
        report.selection.eligible_actions,
        report.selection.strategy.label(),
        report.analyzed_scopes.len()
    );
    outputln!("Inventory: {}", report.inventory.sha256);
    outputln!(
        "Finding query: {}{} (completion claim: false)",
        match report.finding_query.state {
            research::ResearchFindingQueryState::All => "all",
            research::ResearchFindingQueryState::Open => "open",
            research::ResearchFindingQueryState::ConditionSatisfied => "condition-satisfied",
            research::ResearchFindingQueryState::InputNotObserved => "input-not-observed",
            research::ResearchFindingQueryState::FilteredOut => "filtered-out",
            research::ResearchFindingQueryState::NotPresent => "not-present",
        },
        report
            .finding_query
            .finding_id
            .as_deref()
            .map_or_else(String::new, |id| format!(" — {id}"))
    );
    outputln!("Meaning: {}", report.finding_query.interpretation);
    if let Some(evidence) = &report.finding_query.resolution_evidence {
        render_resolution_evidence(evidence);
    }
    outputln!(
        "Filter: focus {}; protocol {}; scope {}; budget {}.",
        report.focus.label(),
        report.protocol.as_deref().unwrap_or("all"),
        report.scope.as_deref().unwrap_or("all"),
        report.selection.budget.map_or_else(
            || "unbounded".to_owned(),
            |budget| format!("{}/{} cost units", report.selection.consumed_budget, budget)
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
    if let Some(diagnostic) = &report.selection.diagnostic {
        outputln!("\n{}: {diagnostic}", output::warning("NO STEP FITS"));
    }
    render_prerequisites(&prerequisites);
    if actions.is_empty() {
        if matches!(
            report.finding_query.state,
            research::ResearchFindingQueryState::ConditionSatisfied
                | research::ResearchFindingQueryState::InputNotObserved
                | research::ResearchFindingQueryState::FilteredOut
                | research::ResearchFindingQueryState::NotPresent
        ) {
            outputln!(
                "\n{}",
                output::warning(
                    "EXACT FINDING IS NOT OPEN — this state is not proof of correctness or completion"
                )
            );
            return;
        }
        if report.inventory.actions.is_empty() {
            outputln!(
                "\n{}",
                output::warning("NO CANDIDATES DERIVED FROM CURRENT INPUTS")
            );
        } else {
            outputln!(
                "\n{}",
                output::warning(format!(
                    "NO ACTION SELECTED — {} eligible action(s) remain but none fit the shared limit/budget; increase --limit or --budget",
                    report.selection.eligible_actions
                ))
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
                "State / kinds (N)",
                "Score B/E",
                "G/O/M · Co · Cost",
            ],
            actions.iter().map(|candidate| [
                candidate.rank.to_string(),
                compact_middle(&action_inspect_label(report, candidate.action), 34),
                format!(
                    "{} · {} ({})",
                    action_lane_label(&candidate.findings),
                    compact_middle(&candidate.action.kinds.join(","), 12),
                    candidate.findings.len()
                ),
                format!(
                    "{} {}/{}",
                    candidate.action.score,
                    candidate.action.score_explanation.benefit_points,
                    candidate.action.score_explanation.effort_points
                ),
                format!(
                    "{}/{}/{} · {} · {}",
                    union_finding_ids(&candidate.findings, |finding| &finding
                        .guaranteed_function_ids)
                    .len(),
                    union_finding_ids(&candidate.findings, |finding| &finding
                        .optimistic_function_ids)
                    .len(),
                    union_finding_ids(&candidate.findings, |finding| &finding
                        .marginal_function_ids)
                    .len(),
                    co_blocker_ids(candidate).len(),
                    candidate.action.estimated_cost,
                ),
            ]),
        )
    );
    outputln!("Impact legend: {IMPACT_LEGEND}");
    let first = &actions[0];
    outputln!(
        "\n{}",
        output::heading(format!("Top {} action", report.selection.strategy.label()))
    );
    outputln!("Confidence: {}", first.action.confidence);
    outputln!(
        "Score: {} = 100 × {} benefit / {} effort ({} cost units)",
        first.action.score,
        first.action.score_explanation.benefit_points,
        first.action.score_explanation.effort_points,
        first.action.score_explanation.estimated_cost_units,
    );
    let direct = union_finding_ids(&first.findings, |finding| &finding.direct_function_ids);
    let guaranteed = union_finding_ids(&first.findings, |finding| &finding.guaranteed_function_ids);
    let optimistic = union_finding_ids(&first.findings, |finding| &finding.optimistic_function_ids);
    let marginal = union_finding_ids(&first.findings, |finding| &finding.marginal_function_ids);
    outputln!(
        "Impact estimates (overlapping): {} direct; {} guaranteed / {} optimistic / {} marginal",
        direct.len(),
        guaranteed.len(),
        optimistic.len(),
        marginal.len(),
    );
    let co_blockers = co_blocker_ids(first);
    if !co_blockers.is_empty() {
        outputln!("Co-blockers: {}", co_blockers.join(", "));
    }
    outputln!(
        "Actionability: {}",
        actionability_summary_label(&first.findings)
    );
    let prerequisite_ids = union_finding_ids(&first.findings, |finding| &finding.prerequisite_ids);
    if !prerequisite_ids.is_empty() {
        outputln!("Blocked by prerequisites: {}", prerequisite_ids.join(", "));
    }
    render_findings(&first.findings);
    outputln!("Next action: {}", first.action.next_action.render_posix());
    for action in revalidation_actions(&first.findings) {
        outputln!("Revalidate after human review: {}", action.render_posix());
    }

    if output::details() {
        for candidate in actions.iter().skip(1) {
            let direct =
                union_finding_ids(&candidate.findings, |finding| &finding.direct_function_ids);
            let co_blockers = co_blocker_ids(candidate);
            outputln!(
                "\n#{} {}\n  Kinds: {}\n  Direct functions: {}\n  Co-blockers: {}\n  Next action: {}",
                candidate.rank,
                candidate.action.id,
                candidate.action.kinds.join(", "),
                direct.join(", "),
                co_blockers.join(", "),
                candidate.action.next_action.render_posix()
            );
            render_findings(&candidate.findings);
        }
    }
}

fn render_resolution_evidence(evidence: &research::ResearchFindingResolutionEvidence) {
    match evidence {
        research::ResearchFindingResolutionEvidence::RegisterWorkspaceAbsent { address, width } => {
            outputln!(
                "Evidence: register workspace absent; parsed subject {address:#010x}/{width}."
            )
        }
        research::ResearchFindingResolutionEvidence::AbsentRegisterModel {
            current_observation,
            current_identity,
            matching_scopes,
            applied_assertions,
            ..
        } => render_register_resolution_summary(
            current_observation.as_ref(),
            current_identity.as_deref(),
            matching_scopes,
            applied_assertions,
            None,
        ),
        research::ResearchFindingResolutionEvidence::UnknownHardwareWriteSemantics {
            effective_write_semantics,
            current_observation,
            current_identity,
            matching_scopes,
            applied_assertions,
            ..
        } => render_register_resolution_summary(
            current_observation.as_ref(),
            current_identity.as_deref(),
            matching_scopes,
            applied_assertions,
            Some(effective_write_semantics),
        ),
    }
}

fn render_register_resolution_summary(
    observation: Option<&research::ResearchRegisterObservationEvidence>,
    identity: Option<&str>,
    matching_scopes: &[String],
    applied_assertions: &[open_radio_vendor_review::EffectiveAssertion],
    write_semantics: Option<&str>,
) {
    let observation = observation.map_or_else(
        || "not observed".to_owned(),
        |observation| {
            let ownership = match observation.publication_ownership {
                research::ResearchRegisterPublicationOwnership::Owned => "owned",
                research::ResearchRegisterPublicationOwnership::External => "external",
            };
            format!(
                "observed ({ownership}, range {}, {} functions, {} sites, {} analysis artifacts)",
                observation.range,
                observation.read_functions.len() + observation.write_functions.len(),
                observation.read_sites.len() + observation.write_sites.len(),
                observation.analysis_artifacts.len()
            )
        },
    );
    let assertion_ids = applied_assertions
        .iter()
        .map(|assertion| assertion.id.clone())
        .collect::<Vec<_>>();
    outputln!(
        "Evidence: input {observation}; scopes {}; identity {}; assertions {}{}.",
        list_or_none(matching_scopes),
        identity.unwrap_or("none"),
        list_or_none(&assertion_ids),
        write_semantics.map_or_else(String::new, |value| format!("; write semantics {value}"))
    );
}

fn render_prerequisites(prerequisites: &[SelectedPrerequisite<'_>]) {
    if prerequisites.is_empty() {
        return;
    }
    let visible = if output::details() {
        prerequisites
    } else {
        &prerequisites[..prerequisites.len().min(12)]
    };
    outputln!(
        "\n{}",
        output::heading(format!("Prerequisite actions ({})", prerequisites.len()))
    );
    outputln!(
        "{}",
        table::render(
            ["#", "Kind", "Benefit/cost", "Findings", "Required action"],
            visible.iter().map(|prerequisite| [
                prerequisite.rank.to_string(),
                prerequisite_kind_label(prerequisite.prerequisite.kind).to_owned(),
                format!(
                    "{}/{}",
                    prerequisite.prerequisite.benefit_points,
                    prerequisite.prerequisite.estimated_cost_units
                ),
                prerequisite
                    .prerequisite
                    .satisfies_finding_ids
                    .len()
                    .to_string(),
                table::compact(&prerequisite.prerequisite.manual_action, 72),
            ]),
        )
    );
    if visible.len() != prerequisites.len() {
        outputln!(
            "Showing {} of {}; use --details for every prerequisite.",
            visible.len(),
            prerequisites.len()
        );
    }
    outputln!("\n{}", output::heading("Top prerequisite"));
    for line in prerequisite_lines(&prerequisites[0], output::details()) {
        outputln!("{line}");
    }
    if output::details() {
        for prerequisite in prerequisites.iter().skip(1) {
            outputln!(
                "\n{}",
                output::heading(format!(
                    "Prerequisite #{} — {}",
                    prerequisite.rank, prerequisite.prerequisite.id
                ))
            );
            for line in prerequisite_lines(prerequisite, true) {
                outputln!("{line}");
            }
        }
    }
}

fn prerequisite_lines(prerequisite: &SelectedPrerequisite<'_>, details: bool) -> Vec<String> {
    let entry = prerequisite.prerequisite;
    let affected_functions = prerequisite_affected_functions(prerequisite);
    let publication_scopes = prerequisite_publication_scopes(prerequisite);
    let evidence_sites = prerequisite_evidence_sites(prerequisite);
    let evidence_channels = prerequisite_evidence_channels(prerequisite);
    let mut lines = Vec::new();
    if details {
        lines.push(format!("ID: {}", entry.id));
        lines.push(format!("Kind: {}", prerequisite_kind_label(entry.kind)));
        lines.push(format!("Subject: {}", entry.subject));
        lines.push(format!(
            "Destination: {}",
            entry
                .path
                .as_ref()
                .map_or("not configured".to_owned(), |path| path
                    .display()
                    .to_string())
        ));
        lines.push(format!("Reason: {}", entry.reason));
    }
    lines.push(format!("Required action: {}", entry.manual_action));
    if entry.kind == research::ResearchPrerequisiteKind::AcquireRequiredAnalysisSurface {
        lines.push(format!(
            "Why first: a required declared analysis surface is absent or invalid; this coverage gate blocks {} finding(s) and {} typed analysis action(s), cost {} units.",
            entry.satisfies_finding_ids.len(),
            entry.blocked_action_ids.len(),
            entry.estimated_cost_units,
        ));
    } else {
        lines.push(format!(
            "Why high-profit: {} benefit points = {} guaranteed functions × 20 + {} optimistic functions × 3 + {} scope roots × 10 + {} publication scopes × 20; {} cost units. Completing this setup may unblock {} findings across {} research actions.",
            entry.benefit_points,
            entry.guaranteed_unlock,
            entry.optimistic_unlock,
            entry.affected_scope_roots.len(),
            publication_scopes.len(),
            entry.estimated_cost_units,
            entry.satisfies_finding_ids.len(),
            entry.blocked_action_ids.len(),
        ));
    }
    if details {
        lines.push(format!(
            "Satisfied finding IDs: {}",
            list_or_none(&entry.satisfies_finding_ids)
        ));
        lines.push(format!(
            "Blocked action IDs: {}",
            list_or_none(&entry.blocked_action_ids)
        ));
        lines.push(format!(
            "Affected functions: {}",
            list_or_none(&affected_functions)
        ));
        lines.push(format!("Scopes: {}", list_or_none(&entry.scopes)));
        lines.push(format!(
            "Affected scope roots: {}",
            list_or_none(&entry.affected_scope_roots)
        ));
        lines.push(format!(
            "Publication scopes: {}",
            list_or_none(&publication_scopes)
        ));
        lines.push(format!(
            "Evidence sites: {}",
            evidence_sites_label(&evidence_sites, true)
        ));
        lines.push(format!(
            "Evidence channels: {}",
            list_or_none(&evidence_channels)
        ));
    } else {
        lines.push(format!(
            "Affected: {} functions; scopes {}.",
            affected_functions.len(),
            highlight_list(&entry.scopes, false)
        ));
    }
    if prerequisite.actions.is_empty() {
        lines.push("Action path: no blocked next action is linked; use --format json to audit the complete prerequisite record".to_owned());
    } else if details {
        lines.extend(
            prerequisite
                .actions
                .iter()
                .map(|action| format!("Action path: {}", action.next_action.render_posix())),
        );
    } else {
        lines.push(format!(
            "Next action: {}",
            prerequisite.actions[0].next_action.render_posix()
        ));
    }
    if let Some(finding) = prerequisite.findings.first() {
        lines.push(format!(
            "Re-query exact finding: {}",
            finding.requery_action.render_posix()
        ));
    }
    lines.push(
        "Boundary: this prerequisite is a manual setup step, not evidence and not a completion claim; inspect the linked evidence, reanalyze, and re-query the finding."
            .to_owned(),
    );
    lines
}

fn prerequisite_affected_functions(prerequisite: &SelectedPrerequisite<'_>) -> Vec<String> {
    prerequisite
        .findings
        .iter()
        .flat_map(|finding| {
            finding
                .guaranteed_function_ids
                .iter()
                .chain(&finding.optimistic_function_ids)
        })
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn prerequisite_publication_scopes(prerequisite: &SelectedPrerequisite<'_>) -> Vec<String> {
    prerequisite
        .findings
        .iter()
        .flat_map(|finding| finding.publication_scopes.iter().cloned())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn prerequisite_evidence_sites(prerequisite: &SelectedPrerequisite<'_>) -> Vec<u32> {
    prerequisite
        .findings
        .iter()
        .flat_map(|finding| finding.evidence_sites.iter().copied())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn prerequisite_evidence_channels(prerequisite: &SelectedPrerequisite<'_>) -> Vec<String> {
    prerequisite
        .findings
        .iter()
        .flat_map(|finding| finding.evidence_channels.iter().cloned())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn list_or_none(values: &[String]) -> String {
    if values.is_empty() {
        "none".to_owned()
    } else {
        values.join(", ")
    }
}

fn prerequisite_kind_label(kind: research::ResearchPrerequisiteKind) -> &'static str {
    match kind {
        research::ResearchPrerequisiteKind::AcquireRequiredAnalysisSurface => "analysis-surface",
        research::ResearchPrerequisiteKind::ConfigureInterfaceDestination => {
            "interface-destination"
        }
        research::ResearchPrerequisiteKind::CreateInterfaceAnchor => "interface-anchor",
        research::ResearchPrerequisiteKind::SelectReviewedKnowledgeDestination => {
            "knowledge-destination"
        }
    }
}

struct SelectedPrerequisite<'a> {
    rank: usize,
    prerequisite: &'a research::ResearchPrerequisiteCatalogEntry,
    findings: Vec<&'a research::ResearchFinding>,
    actions: Vec<&'a research::ResearchActionCatalogEntry>,
}

struct SelectedAction<'a> {
    rank: usize,
    action: &'a research::ResearchActionCatalogEntry,
    findings: Vec<&'a research::ResearchFinding>,
}

fn selected_prerequisites(report: &research::ResearchNextReport) -> Vec<SelectedPrerequisite<'_>> {
    report
        .selection
        .steps
        .iter()
        .enumerate()
        .filter(|(_, step)| step.kind == research::ResearchStepKind::Prerequisite)
        .map(|(index, step)| {
            let prerequisite = report
                .inventory
                .prerequisites
                .binary_search_by(|candidate| candidate.id.cmp(&step.id))
                .map(|index| &report.inventory.prerequisites[index])
                .expect("validated prerequisite selection reference");
            let findings = prerequisite
                .satisfies_finding_ids
                .iter()
                .map(|id| {
                    report
                        .inventory
                        .findings
                        .binary_search_by(|candidate| candidate.id.cmp(id))
                        .map(|index| &report.inventory.findings[index])
                        .expect("validated prerequisite finding reference")
                })
                .collect();
            let actions = prerequisite
                .blocked_action_ids
                .iter()
                .map(|id| {
                    report
                        .inventory
                        .actions
                        .binary_search_by(|candidate| candidate.id.cmp(id))
                        .map(|index| &report.inventory.actions[index])
                        .expect("validated prerequisite action reference")
                })
                .collect();
            SelectedPrerequisite {
                rank: index + 1,
                prerequisite,
                findings,
                actions,
            }
        })
        .collect()
}

fn selected_actions(report: &research::ResearchNextReport) -> Vec<SelectedAction<'_>> {
    report
        .selection
        .steps
        .iter()
        .enumerate()
        .filter(|(_, step)| step.kind == research::ResearchStepKind::Action)
        .map(|(index, step)| {
            let action = report
                .inventory
                .actions
                .binary_search_by(|candidate| candidate.id.cmp(&step.id))
                .map(|index| &report.inventory.actions[index])
                .expect("validated action selection reference");
            let findings = action
                .finding_ids
                .iter()
                .map(|id| {
                    report
                        .inventory
                        .findings
                        .binary_search_by(|candidate| candidate.id.cmp(id))
                        .map(|index| &report.inventory.findings[index])
                        .expect("validated action finding reference")
                })
                .collect();
            SelectedAction {
                rank: index + 1,
                action,
                findings,
            }
        })
        .collect()
}

fn union_finding_ids<'a>(
    findings: &[&'a research::ResearchFinding],
    values: impl Fn(&'a research::ResearchFinding) -> &'a [String],
) -> Vec<&'a str> {
    findings
        .iter()
        .flat_map(|finding| values(finding).iter().map(String::as_str))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn co_blocker_ids<'a>(action: &'a SelectedAction<'a>) -> Vec<&'a str> {
    let internal = action
        .action
        .finding_ids
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    union_finding_ids(&action.findings, |finding| &finding.co_blocker_ids)
        .into_iter()
        .filter(|id| !internal.contains(id))
        .collect()
}

fn action_lane_label(findings: &[&research::ResearchFinding]) -> &'static str {
    if findings
        .iter()
        .any(|finding| finding.actionability == research::ResearchActionability::Ready)
    {
        "ready"
    } else if findings
        .iter()
        .any(|finding| finding.actionability == research::ResearchActionability::InspectionOnly)
    {
        "inspect"
    } else {
        "blocked"
    }
}

fn actionability_summary_label(findings: &[&research::ResearchFinding]) -> String {
    let count = |expected| {
        findings
            .iter()
            .filter(|finding| finding.actionability == expected)
            .count()
    };
    [
        ("ready", count(research::ResearchActionability::Ready)),
        (
            "needs-anchor",
            count(research::ResearchActionability::NeedsAnchor),
        ),
        (
            "needs-destination",
            count(research::ResearchActionability::NeedsDestination),
        ),
        (
            "coverage-blocked",
            count(research::ResearchActionability::CoverageBlocked),
        ),
        (
            "inspection-only",
            count(research::ResearchActionability::InspectionOnly),
        ),
    ]
    .into_iter()
    .filter(|(_, count)| *count != 0)
    .map(|(label, count)| format!("{label}={count}"))
    .collect::<Vec<_>>()
    .join(", ")
}

fn render_findings(findings: &[&research::ResearchFinding]) {
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

fn visible_findings<'a, 'finding>(
    findings: &'a [&'finding research::ResearchFinding],
    details: bool,
) -> (&'a [&'finding research::ResearchFinding], usize) {
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
    let mut lines = vec![format!(
        "  {number}. [{}] {} — {summary}",
        finding.kind, finding.id
    )];
    lines.push(format!(
        "     Actionability: {}",
        finding_actionability_label(finding.actionability)
    ));
    if !finding.prerequisite_ids.is_empty() {
        lines.push(format!(
            "     Prerequisites: {}",
            finding.prerequisite_ids.join(", ")
        ));
    }
    lines.push(format!("     Knowledge: {}", finding.knowledge_required));
    lines.push(format!(
        "     Resolution owner: {}",
        finding.resolution_owner.label()
    ));
    if let Some(route) = &finding.blocker_resolution_route {
        lines.push(format!(
            "     Resolution: owner {}, effect {}",
            route.owner.label(),
            route.producer_effect.label()
        ));
        if let Some(path) = &route.destination {
            lines.push(format!(
                "     Record: {} — {}",
                path.display(),
                route.record_action.as_deref().unwrap_or("typed consumer")
            ));
        } else {
            lines.push("     Record: none — no writable project consumer".to_owned());
        }
        lines.push(format!(
            "     Completion predicate: {} / {} / {}",
            route.completion_predicate.producer,
            route.completion_predicate.kind.label(),
            route.completion_predicate.root_id
        ));
        if details {
            lines.push(format!("     Route rationale: {}", route.rationale));
        }
    } else if finding.consumers.is_empty() {
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

fn finding_actionability_label(actionability: research::ResearchActionability) -> &'static str {
    match actionability {
        research::ResearchActionability::Ready => "ready",
        research::ResearchActionability::NeedsAnchor => "needs-anchor",
        research::ResearchActionability::NeedsDestination => "needs-destination",
        research::ResearchActionability::CoverageBlocked => "coverage-blocked",
        research::ResearchActionability::InspectionOnly => "inspection-only",
    }
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
        research::ResearchConsumerResolution::NeedsAnchor => "needs-anchor",
        research::ResearchConsumerResolution::NeedsDestination => "needs-destination",
    }
}

fn analysis_surface_state_label(state: research::ResearchAnalysisSurfaceState) -> &'static str {
    match state {
        research::ResearchAnalysisSurfaceState::MissingVendorArtifact => "missing-vendor-artifact",
        research::ResearchAnalysisSurfaceState::MissingSymbolInventory => {
            "missing-symbol-inventory"
        }
        research::ResearchAnalysisSurfaceState::StaleSymbolFamily => "stale-symbol-family",
        research::ResearchAnalysisSurfaceState::MissingProfileDefinition => {
            "missing-profile-definition"
        }
        research::ResearchAnalysisSurfaceState::MissingProfileOutput => "missing-profile-output",
        research::ResearchAnalysisSurfaceState::InvalidProfileOutput => "invalid-profile-output",
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
        research::ResearchConsumer::RequiredAnalysisSurface {
            state,
            source,
            profile,
            diagnostic,
            ..
        } => format!(
            "required analysis surface [{}], source {source}, profile {} ({diagnostic})",
            analysis_surface_state_label(*state),
            profile.as_deref().unwrap_or("not configured")
        ),
    }
}

fn revalidation_actions<'a>(
    findings: &[&'a research::ResearchFinding],
) -> Vec<&'a crate::application::ExecutableAction> {
    let mut actions = findings
        .iter()
        .flat_map(|finding| finding.revalidation_actions.iter())
        .collect::<Vec<_>>();
    actions.sort_unstable();
    actions.dedup();
    actions
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

fn action_inspect_label(
    report: &research::ResearchNextReport,
    action: &research::ResearchActionCatalogEntry,
) -> String {
    let fallback = action.next_action.inspect_label();
    let Some(selector) = action
        .next_action
        .argv
        .windows(3)
        .find(|arguments| arguments[0] == "inspect" && arguments[1] == "function")
        .map(|arguments| arguments[2].as_str())
    else {
        return fallback;
    };
    reviewed_function_name(&report.reviewed_functions, selector)
        .map_or(fallback, |name| format!("function {name}"))
}

fn reviewed_function_name<'a>(
    functions: &'a [research::ResearchReviewedFunction],
    selector: &str,
) -> Option<&'a str> {
    let (source, symbol) = selector.split_once(':')?;
    let identity = format!("{source}::{symbol}");
    functions
        .iter()
        .find(|function| function.identity == identity)
        .map(|function| function.name.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn action(argv: &[&str]) -> crate::application::ExecutableAction {
        crate::application::ExecutableAction::new(
            argv.iter().map(|value| (*value).to_owned()).collect(),
            "/workspace".into(),
            crate::application::ProjectContextRequirement::Analysis,
        )
        .unwrap()
    }

    fn finding(id: &str, kind: &str, summary: &str) -> research::ResearchFinding {
        research::ResearchFinding {
            id: id.to_owned(),
            kind: kind.to_owned(),
            severity: "warning".to_owned(),
            subject: research::ResearchSubject::AnalysisRoot {
                root_id: id.to_owned(),
            },
            consumers: Vec::new(),
            blocker_resolution_route: None,
            resolution_owner: crate::BlockerResolutionOwner::Unsupported,
            actionability: research::ResearchActionability::InspectionOnly,
            prerequisite_ids: Vec::new(),
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
            revalidation_actions: Vec::new(),
            requery_action: action(&[
                "blobray",
                "project",
                "research",
                "next",
                "--finding",
                id,
                "--project",
                "project.toml",
            ]),
            summary: summary.to_owned(),
        }
    }

    #[test]
    fn typed_action_label_removes_copyable_command_boilerplate() {
        assert_eq!(
            action(&[
                "blobray",
                "inspect",
                "function",
                "ble:controller_init",
                "--project",
                "project.toml",
            ])
            .inspect_label(),
            "function ble:controller_init"
        );
        assert_eq!(
            action(&["blobray", "project", "status", "--project", "project.toml",]).inspect_label(),
            "blobray project status --project project.toml"
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
    fn reviewed_function_names_replace_obfuscated_research_labels() {
        let reviewed = [research::ResearchReviewedFunction {
            profile: "ble-controller-all".to_owned(),
            source: "ble-controller".to_owned(),
            identity: "ble-controller::r_sym_bt_obfuscated".to_owned(),
            name: "btdm_broker_detach".to_owned(),
            role: Some("btdm.broker.detach".to_owned()),
            summary: None,
        }];

        assert_eq!(
            reviewed_function_name(&reviewed, "ble-controller:r_sym_bt_obfuscated"),
            Some("btdm_broker_detach")
        );
        assert_eq!(
            reviewed_function_name(&reviewed, "ble-controller:other"),
            None
        );
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
        first.actionability = research::ResearchActionability::Ready;
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

        assert!(lines.contains("1. [interface-layout] slot-a — review producer slot"));
        assert!(lines.contains("interface pack [ready]"));
        assert!(lines.contains("Inspect in: btbb::producer, btbb::consumer"));
        assert!(lines.contains("sites 0x40001000; channels linked-ir"));
        assert!(lines.contains("Needed: producer and consumer access sites"));
        assert!(lines.contains("2. [register-model] register-a — name the observed register"));
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
        let finding_refs = findings.iter().collect::<Vec<_>>();

        let (compact, hidden) = visible_findings(&finding_refs, false);
        assert_eq!(compact.len(), 8);
        assert_eq!(hidden, 4);

        let (detailed, hidden) = visible_findings(&finding_refs, true);
        assert_eq!(detailed.len(), 12);
        assert_eq!(hidden, 0);
    }

    #[test]
    fn compact_prerequisite_is_actionable_without_a_completion_claim() {
        let mut first = finding("finding-a", "interface-layout", "bind the first slot");
        first.guaranteed_function_ids = vec!["btbb::guaranteed".to_owned()];
        first.optimistic_function_ids =
            vec!["btbb::guaranteed".to_owned(), "btbb::optimistic".to_owned()];
        first.publication_scopes = vec!["ieee-publication".to_owned()];
        first.requery_action = action(&[
            "blobray",
            "project",
            "research",
            "next",
            "--finding",
            "finding-a",
            "--project",
            "project.toml",
            "--run-spec",
            "local.toml",
        ]);
        let findings = [first];
        let action = research::ResearchActionCatalogEntry {
            id: "action-a".to_owned(),
            kinds: vec!["interface-layout".to_owned()],
            score: 10,
            next_action: action(&[
                "blobray",
                "inspect",
                "function",
                "btbb:worker",
                "--project",
                "project.toml",
                "--run-spec",
                "local.toml",
            ]),
            estimated_cost: "low".to_owned(),
            confidence: "medium".to_owned(),
            resolution_owner: crate::BlockerResolutionOwner::Unsupported,
            required_model: "reviewed semantic model".to_owned(),
            score_breakdown: research::ResearchScoreBreakdown {
                guaranteed_weight: 0,
                optimistic_weight: 0,
                marginal_weight: 0,
                root_weight: 0,
                capability_weight: 0,
                verification_weight: 0,
                publication_weight: 0,
                cost_penalty: 0,
                co_blocker_penalty: 0,
            },
            score_explanation: research::ResearchScoreExplanation {
                benefit_points: 0,
                effort_points: 1,
                estimated_cost_units: 1,
            },
            finding_ids: vec!["finding-a".to_owned()],
        };
        let prerequisite = research::ResearchPrerequisiteCatalogEntry {
            id: "prerequisite-a".to_owned(),
            kind: research::ResearchPrerequisiteKind::CreateInterfaceAnchor,
            reason: "the observation has no reviewed anchor".to_owned(),
            path: Some("review/interfaces.toml".into()),
            subject: "unmatched:btbb:callbacks::fact-7".to_owned(),
            manual_action: "Create the reviewed anchor in review/interfaces.toml".to_owned(),
            satisfies_finding_ids: vec!["finding-a".to_owned()],
            blocked_action_ids: vec!["action-a".to_owned()],
            guaranteed_unlock: 1,
            optimistic_unlock: 2,
            affected_scope_roots: vec!["btbb:root".to_owned()],
            scopes: vec!["ieee-runtime".to_owned()],
            benefit_points: 56,
            estimated_cost_units: 3,
        };
        let selected = SelectedPrerequisite {
            rank: 1,
            prerequisite: &prerequisite,
            findings: findings.iter().collect(),
            actions: vec![&action],
        };

        let rendered = prerequisite_lines(&selected, false).join("\n");

        assert!(
            rendered
                .contains("Required action: Create the reviewed anchor in review/interfaces.toml"),
            "{rendered}"
        );
        assert!(
            rendered.contains("Why high-profit: 56 benefit points"),
            "{rendered}"
        );
        assert!(
            rendered.contains("Affected: 2 functions; scopes ieee-runtime"),
            "{rendered}"
        );
        assert!(
            rendered.contains(
                "Next action: blobray inspect function btbb:worker --project project.toml --run-spec local.toml"
            ),
            "{rendered}"
        );
        assert!(
            rendered.contains(
                "Re-query exact finding: blobray project research next --finding finding-a --project project.toml --run-spec local.toml"
            ),
            "{rendered}"
        );
        assert!(rendered.contains("not a completion claim"), "{rendered}");
        assert!(!rendered.contains("Subject:"), "{rendered}");
    }

    #[test]
    fn detailed_prerequisite_exposes_full_ownership_and_impact() {
        let mut finding = finding("finding-a", "interface-layout", "bind the slot");
        finding.guaranteed_function_ids = vec!["btbb::producer".to_owned()];
        finding.optimistic_function_ids = vec!["btbb::consumer".to_owned()];
        finding.publication_scopes = vec!["ieee-publication".to_owned()];
        finding.evidence_sites = vec![0x1000_0020, 0x1000_0010];
        finding.evidence_channels = vec!["linked-ir".to_owned(), "interface".to_owned()];
        let findings = [finding];
        let action = research::ResearchActionCatalogEntry {
            id: "action-a".to_owned(),
            kinds: vec!["interface-layout".to_owned()],
            score: 1,
            next_action: action(&[
                "blobray",
                "inspect",
                "function",
                "btbb:consumer",
                "--project",
                "projects/radio.toml",
            ]),
            estimated_cost: "low".to_owned(),
            confidence: "medium".to_owned(),
            resolution_owner: crate::BlockerResolutionOwner::Unsupported,
            required_model: "reviewed semantic model".to_owned(),
            score_breakdown: research::ResearchScoreBreakdown {
                guaranteed_weight: 0,
                optimistic_weight: 0,
                marginal_weight: 0,
                root_weight: 0,
                capability_weight: 0,
                verification_weight: 0,
                publication_weight: 0,
                cost_penalty: 0,
                co_blocker_penalty: 0,
            },
            score_explanation: research::ResearchScoreExplanation {
                benefit_points: 0,
                effort_points: 1,
                estimated_cost_units: 1,
            },
            finding_ids: vec!["finding-a".to_owned()],
        };
        let prerequisite = research::ResearchPrerequisiteCatalogEntry {
            id: "prerequisite-a".to_owned(),
            kind: research::ResearchPrerequisiteKind::CreateInterfaceAnchor,
            reason: "the exact producer is not bound".to_owned(),
            path: Some("review/interfaces.toml".into()),
            subject: "unmatched:btbb:bounded-data-address::fact-303".to_owned(),
            manual_action: "Create an exact reviewed anchor".to_owned(),
            satisfies_finding_ids: vec!["finding-a".to_owned()],
            blocked_action_ids: vec!["action-a".to_owned()],
            guaranteed_unlock: 1,
            optimistic_unlock: 1,
            affected_scope_roots: vec!["btbb:root".to_owned()],
            scopes: vec!["ieee-runtime".to_owned()],
            benefit_points: 53,
            estimated_cost_units: 3,
        };
        let selected = SelectedPrerequisite {
            rank: 1,
            prerequisite: &prerequisite,
            findings: findings.iter().collect(),
            actions: vec![&action],
        };

        let rendered = prerequisite_lines(&selected, true).join("\n");

        for expected in [
            "ID: prerequisite-a",
            "Kind: interface-anchor",
            "Subject: unmatched:btbb:bounded-data-address::fact-303",
            "Destination: review/interfaces.toml",
            "Reason: the exact producer is not bound",
            "Satisfied finding IDs: finding-a",
            "Blocked action IDs: action-a",
            "Affected functions: btbb::consumer, btbb::producer",
            "Scopes: ieee-runtime",
            "Affected scope roots: btbb:root",
            "Publication scopes: ieee-publication",
            "Evidence sites: 0x10000010, 0x10000020",
            "Evidence channels: interface, linked-ir",
            "Action path: blobray inspect function btbb:consumer --project projects/radio.toml",
        ] {
            assert!(
                rendered.contains(expected),
                "missing {expected:?}:\n{rendered}"
            );
        }
    }

    #[test]
    fn ranking_table_keeps_the_inspect_target_readable() {
        let rendered = table::render(
            [
                "#",
                "Inspect target",
                "State / kinds (N)",
                "Score B/E",
                "G/O/M · Co · Cost",
            ],
            [[
                "1".to_owned(),
                compact_middle("function ble-controller:r_ble_hci_trans_env_deinit", 34),
                format!(
                    "inspect · {} (2)",
                    compact_middle("control-flow,memory-store", 12)
                ),
                "1369 972/71".to_owned(),
                "33/34/34 · 2 · high".to_owned(),
            ]],
        );

        assert!(
            rendered.contains("function ble-cont…trans_env_deinit"),
            "inspect target was wrapped:\n{rendered}"
        );
        assert!(
            rendered.contains("inspect · contro…store (2)"),
            "{rendered}"
        );
        assert!(rendered.contains("33/34/34"), "{rendered}");
        assert!(rendered.contains("high"), "{rendered}");
    }

    #[test]
    fn impact_legend_fails_closed_about_unlock_estimates() {
        assert!(IMPACT_LEGEND.contains("within the current analyzed graph"));
        assert!(!IMPACT_LEGEND.contains("upper bound"));
        assert!(IMPACT_LEGEND.contains("Sets overlap"));
        assert!(IMPACT_LEGEND.contains("do not prove runtime behavior or completion"));
    }
}
