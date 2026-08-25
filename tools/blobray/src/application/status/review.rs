//! Reviewed register, interface, function, and context workspace readiness.

use std::collections::BTreeSet;

use super::model::{Component, Phase, Readiness, ReviewScopeDetail};
use crate::application::{ProjectContext, ProjectContextRequirement};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ScopeGate {
    publication_count: usize,
    replacement_coverage_complete: usize,
    replacement_coverage_gaps: usize,
    analysis_inventory_blocked: usize,
}

fn scope_gate(states: impl IntoIterator<Item = (bool, bool, bool)>) -> ScopeGate {
    states.into_iter().fold(
        ScopeGate {
            publication_count: 0,
            replacement_coverage_complete: 0,
            replacement_coverage_gaps: 0,
            analysis_inventory_blocked: 0,
        },
        |mut gate, (publication, coverage_complete, inventory_complete)| {
            if publication {
                gate.publication_count += 1;
                gate.replacement_coverage_complete += usize::from(coverage_complete);
                gate.replacement_coverage_gaps += usize::from(!coverage_complete);
            }
            gate.analysis_inventory_blocked += usize::from(!inventory_complete);
            gate
        },
    )
}

pub(super) fn collect(context: &ProjectContext<'_>) -> Phase {
    fn component(name: &'static str, collect: impl FnOnce() -> Component) -> Component {
        let started = std::time::Instant::now();
        let component = collect();
        tracing::debug!(
            component = name,
            elapsed_ms = started.elapsed().as_millis(),
            "review status component collected"
        );
        component
    }
    Phase::collect(
        "review",
        vec![
            component("code", || code(context)),
            component("registers", || registers(context)),
            component("interfaces", || interfaces(context)),
            component("functions", || functions(context)),
            component("scopes", || scopes(context)),
        ],
    )
}

fn scopes(context: &ProjectContext<'_>) -> Component {
    let Some(workspace) = context.project.review.as_ref() else {
        return Component::new("scopes", Readiness::NotConfigured);
    };
    if !workspace.output.is_file() {
        return Component::new("scopes", Readiness::Incomplete)
            .detail("report", workspace.output.display().to_string())
            .diagnostic("review scope report has not been generated")
            .next_action(
                context.follow_up_command("project analyze", ProjectContextRequirement::Analysis),
            );
    }
    match crate::review_scopes::load_for_project(context.project) {
        Ok(document) => {
            let reports = document.scopes;
            let gate = scope_gate(reports.iter().map(|report| {
                (
                    report.publication,
                    report.replacement_coverage
                        == crate::review_scopes::ReplacementCoverage::Complete,
                    report.analysis_inventory_complete,
                )
            }));
            let research_root_causes = reports
                .iter()
                .flat_map(|report| report.review_queue.iter().map(|item| item.id.as_str()))
                .collect::<BTreeSet<_>>();
            let research_scopes = reports
                .iter()
                .filter(|report| {
                    !report.review_queue.is_empty() || report.has_replacement_coverage_gaps()
                })
                .count();
            if gate.publication_count == 0 {
                return Component::new("scopes", Readiness::Incomplete)
                    .detail("report", workspace.output.display().to_string())
                    .detail("count", reports.len())
                    .diagnostic("no publication review scopes are configured")
                    .next_action(format!(
                        "configure [review].publication-scopes in {}",
                        context.project_path.display()
                    ));
            }
            let details = reports
                .iter()
                .map(|report| ReviewScopeDetail {
                    id: report.id.clone(),
                    protocols: report.protocols.clone(),
                    publication: report.publication,
                    replacement_coverage: match report.replacement_coverage {
                        crate::review_scopes::ReplacementCoverage::Complete => "complete",
                        crate::review_scopes::ReplacementCoverage::Gaps => "gaps",
                    }
                    .to_owned(),
                    analysis_inventory_complete: report.analysis_inventory_complete,
                    profiles: report.profiles.clone(),
                    roots: report.roots,
                    functions: report.functions,
                    replacement_functions: report.replacement_functions,
                    complete_functions: report.complete_functions,
                    mmio_registers: report.mmio_registers,
                    linked_mmio_registers: report.linked_mmio_registers,
                    static_mmio_registers: report.static_mmio_registers,
                    table_calls: report.table_calls,
                    context_fields: report.context_fields,
                    memory_fields: report.memory_fields,
                    decode_blockers: report.decode_blockers,
                    decode_blocker_functions: report.decode_blocker_functions,
                    direct_blockers: report.direct_blockers,
                    call_graph_blockers: report.call_graph_blockers,
                    reference_blockers: report.reference_blockers,
                    unresolved_calls: report.unresolved_calls,
                    replacement_behavioral_matches: report.replacement_behavioral_matches,
                    replacement_production_matches: report.replacement_production_matches,
                    replacement_policy_excluded: report.replacement_policy_excluded,
                    replacement_bounded_matches: report.replacement_bounded_matches,
                    replacement_probe_only_matches: report.replacement_probe_only_matches,
                    replacement_unmapped_matches: report.replacement_unmapped_matches,
                    replacement_mismatches: report.replacement_mismatches,
                    replacement_incomplete: report.replacement_incomplete,
                    replacement_unqualified: report.replacement_unqualified,
                    replacement_uncovered: report.replacement_uncovered,
                })
                .collect::<Vec<_>>();
            let mut component = Component::new("scopes", Readiness::Ready)
                .detail("report", workspace.output.display().to_string())
                .detail("count", reports.len())
                .detail("validation_depth", "shallow")
                .detail("freshness", "unknown")
                .detail("deep_validation", "project doctor / project check")
                .detail("publication_count", gate.publication_count)
                .detail(
                    "replacement_coverage_complete",
                    gate.replacement_coverage_complete,
                )
                .detail("replacement_coverage_gaps", gate.replacement_coverage_gaps)
                .detail(
                    "analysis_inventory_blocked",
                    gate.analysis_inventory_blocked,
                )
                .detail("research_root_causes", research_root_causes.len())
                .detail("research_scopes", research_scopes)
                .detail("scopes", details);
            if recommend_research(research_root_causes.len(), gate.replacement_coverage_gaps) {
                component = component.next_action(format!(
                    "run `{}`",
                    context.follow_up_command(
                        "project research next",
                        ProjectContextRequirement::Target,
                    )
                ));
            }
            component
        }
        Err(error) => Component::new("scopes", Readiness::Invalid).diagnostic(error),
    }
}

const fn recommend_research(root_causes: usize, replacement_gaps: usize) -> bool {
    root_causes != 0 || replacement_gaps != 0
}

fn code(context: &ProjectContext<'_>) -> Component {
    let Some(paths) = &context.project.code else {
        return Component::new("code_boundaries", Readiness::NotConfigured);
    };
    let Some(inventory) = &context.project.symbol_inventory else {
        return Component::new("code_boundaries", Readiness::Invalid)
            .diagnostic("[code] requires [analysis.symbols]");
    };
    if !inventory.output.is_file() {
        return Component::new("code_boundaries", Readiness::Incomplete)
            .detail("facts", inventory.output.display().to_string())
            .diagnostic("symbol inventory has not been generated")
            .next_action(
                context.follow_up_command("project analyze", ProjectContextRequirement::Analysis),
            );
    }
    if !paths.pack.is_file() {
        return Component::new("code_boundaries", Readiness::Incomplete)
            .detail("pack", paths.pack.display().to_string())
            .diagnostic("reviewed code-boundary pack has not been initialized")
            .next_action(context.follow_up_command(
                "advanced code init-pack",
                ProjectContextRequirement::ProjectOnly,
            ));
    }
    inventory_ready(
        "code_boundaries",
        [
            ("facts", inventory.output.as_path()),
            ("pack", paths.pack.as_path()),
        ],
    )
}

fn registers(context: &ProjectContext<'_>) -> Component {
    let Some(paths) = &context.project.registers else {
        return Component::new("registers", Readiness::NotConfigured);
    };
    if !paths.model.is_file() {
        return Component::new("registers", Readiness::Incomplete)
            .detail("model", paths.model.display().to_string())
            .diagnostic("register model has not been initialized")
            .next_action(context.follow_up_command(
                "registers init-model",
                ProjectContextRequirement::ProjectOnly,
            ));
    }
    inventory_ready(
        "registers",
        [
            ("facts", paths.facts.as_path()),
            ("model", paths.model.as_path()),
        ],
    )
    .detail("owned_ranges", paths.owned_ranges.join(","))
    .detail("review_ir_reports", paths.review_ir_reports.len())
    .detail("api_pack_configured", paths.api_pack.is_some())
    .detail("lint_pack_configured", paths.lint_pack.is_some())
    .detail("evidence_catalogs", paths.evidence_catalogs.len())
}

fn interfaces(context: &ProjectContext<'_>) -> Component {
    let Some(paths) = &context.project.interfaces else {
        return Component::new("interfaces", Readiness::NotConfigured);
    };
    if !paths.facts.is_file() {
        return Component::new("interfaces", Readiness::Incomplete)
            .detail("facts", paths.facts.display().to_string())
            .diagnostic("interface facts have not been generated")
            .next_action(
                context.follow_up_command("project analyze", ProjectContextRequirement::Analysis),
            );
    }
    let Some(pack) = paths.pack.as_deref() else {
        return Component::new("interfaces", Readiness::Incomplete)
            .diagnostic("interface pack is not configured")
            .next_action(format!(
                "configure [interfaces].pack in {}",
                context.project_path.display()
            ));
    };
    if !pack.is_file() {
        return Component::new("interfaces", Readiness::Incomplete)
            .detail("pack", pack.display().to_string())
            .diagnostic("interface pack has not been initialized")
            .next_action(context.follow_up_command(
                "advanced interfaces init-pack",
                ProjectContextRequirement::Target,
            ));
    }
    inventory_ready(
        "interfaces",
        [("facts", paths.facts.as_path()), ("pack", pack)],
    )
}

fn functions(context: &ProjectContext<'_>) -> Component {
    let Some(paths) = &context.project.functions else {
        return Component::new("functions", Readiness::NotConfigured);
    };
    let reports = match context.project.function_ir_reports() {
        Ok(reports) => reports,
        Err(error) => {
            return Component::new("functions", Readiness::Invalid).diagnostic(error);
        }
    };
    let missing = reports
        .iter()
        .filter(|(_, report)| !report.is_dir())
        .count();
    if missing != 0 {
        return Component::new("functions", Readiness::Incomplete)
            .detail("profiles", reports.len())
            .detail("missing_reports", missing)
            .diagnostic("linked-IR function facts have not been generated")
            .next_action(
                context.follow_up_command("advanced ir build", ProjectContextRequirement::Analysis),
            );
    }
    if !paths.pack.is_file() {
        return Component::new("functions", Readiness::Incomplete)
            .detail("pack", paths.pack.display().to_string())
            .diagnostic("function pack has not been initialized")
            .next_action(context.follow_up_command(
                "advanced functions init-pack",
                ProjectContextRequirement::ProjectOnly,
            ));
    }
    inventory_ready("functions", [("pack", paths.pack.as_path())]).detail("profiles", reports.len())
}

fn inventory_ready<'a>(
    name: &'static str,
    paths: impl IntoIterator<Item = (&'static str, &'a std::path::Path)>,
) -> Component {
    let mut component = Component::new(name, Readiness::Inventory)
        .detail("gating", false)
        .detail("validation_depth", "shallow")
        .detail("freshness", "unknown")
        .detail("deep_validation", "project doctor / project check");
    for (key, path) in paths {
        component = component.detail(key, path.display().to_string());
    }
    component
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn artifact_inventory_blockers_remain_visible_without_changing_scope_selection() {
        let gate = scope_gate([
            (false, false, false),
            (true, true, false),
            (false, false, false),
        ]);
        assert_eq!(gate.publication_count, 1);
        assert_eq!(gate.replacement_coverage_complete, 1);
        assert_eq!(gate.replacement_coverage_gaps, 0);
        assert_eq!(gate.analysis_inventory_blocked, 3);
    }

    #[test]
    fn replacement_coverage_gaps_remain_visible_as_research_debt() {
        let gate = scope_gate([
            (true, true, true),
            (true, false, false),
            (false, false, false),
        ]);
        assert_eq!(gate.publication_count, 2);
        assert_eq!(gate.replacement_coverage_complete, 1);
        assert_eq!(gate.replacement_coverage_gaps, 1);
        assert_eq!(gate.analysis_inventory_blocked, 2);
    }

    #[test]
    fn research_is_recommended_for_analysis_or_replacement_debt() {
        assert!(recommend_research(1, 0));
        assert!(recommend_research(0, 1));
        assert!(!recommend_research(0, 0));
    }
}
