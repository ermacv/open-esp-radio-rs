//! Focused project review-scope report.

use std::collections::BTreeMap;

use super::super::*;

const DEFAULT_ROOT_CAUSE_LIMIT: usize = 5;
const DEFAULT_MESSAGE_CHAR_LIMIT: usize = 240;
const DEFAULT_INLINE_ID_LIMIT: usize = 3;

#[derive(serde::Serialize)]
struct ScopeInvestigationReport {
    schema_version: u32,
    command: &'static str,
    scope: crate::review_scopes::ReviewScopeReport,
    verification_surfaces: Vec<crate::verification::policy::SurfaceReport>,
}

pub(super) fn run(arguments: InspectScopeArgs, project: &ProjectSpec) -> Result<bool> {
    let document = crate::review_scopes::load_for_project(project)?;
    let scope = document
        .scopes
        .into_iter()
        .find(|scope| scope.id == arguments.scope)
        .ok_or_else(|| {
            crate::Error::invalid(format!("unknown review scope {:?}", arguments.scope))
        })?;
    let verification_surfaces = crate::verification::policy::evaluate(project)?
        .into_iter()
        .flat_map(|report| report.surfaces)
        .filter(|surface| surface.review_scopes.iter().any(|id| id == &scope.id))
        .collect::<Vec<_>>();
    let complete = scope.analysis_inventory_complete;
    let report = ScopeInvestigationReport {
        schema_version: 2,
        command: "inspect scope",
        scope,
        verification_surfaces,
    };
    crate::cli::output::render_report(&report, || {
        let scope = &report.scope;
        outputln!("SCOPE {}", scope.id);
        outputln!(
            "  analysis={} functions={} roots={} complete={} effects={}",
            if scope.analysis_inventory_complete {
                "complete"
            } else {
                "incomplete"
            },
            scope.functions,
            scope.roots,
            scope.complete_functions,
            scope.replacement_function_keys.len(),
        );
        outputln!("  explicit effects:");
        for effect in &scope.replacement_function_keys {
            outputln!("    - {effect}");
        }
        outputln!(
            "  MMIO={} table-calls={} context-fields={} memory-fields={}",
            scope.mmio_registers,
            scope.table_calls,
            scope.context_fields,
            scope.memory_fields,
        );
        if !scope.review_queue.is_empty() {
            let blockers = ranked_blockers(&scope.review_queue);
            let window = diagnostic_window(scope.review_queue.len(), output::details());
            outputln!(
                "  blocker root causes ({} of {}):",
                window.visible,
                scope.review_queue.len(),
            );
            for blocker in blockers.into_iter().take(window.visible) {
                outputln!(
                    "    - {} [{}] {} ({} occurrence(s), up to {} function(s), roots={})",
                    blocker.id,
                    blocker.kind,
                    human_message(&blocker.message, output::details()),
                    blocker.occurrences,
                    blocker.potentially_unblocked_functions,
                    id_summary(&blocker.affected_scope_roots, output::details()),
                );
            }
            render_omitted(window.omitted, "    ", "root cause");
        }
        if !report.verification_surfaces.is_empty() {
            outputln!("  verification policy:");
            let surface_window =
                diagnostic_window(report.verification_surfaces.len(), output::details());
            for surface in report
                .verification_surfaces
                .iter()
                .take(surface_window.visible)
            {
                outputln!(
                    "    - {}: {}, effects={}, proofs={}, blockers={}",
                    surface.id,
                    if surface.closed { "closed" } else { "blocked" },
                    surface.effects,
                    surface.requirements,
                    surface.blockers.len(),
                );
            }
            render_omitted(surface_window.omitted, "    ", "verification surface");
            let policy_causes = grouped_policy_causes(&report.verification_surfaces);
            if !policy_causes.is_empty() {
                let window = diagnostic_window(policy_causes.len(), output::details());
                outputln!(
                    "    policy root causes ({} of {}):",
                    window.visible,
                    policy_causes.len(),
                );
                for cause in policy_causes.iter().take(window.visible) {
                    outputln!(
                        "      - {} (surfaces={})",
                        human_message(&cause.message, output::details()),
                        id_summary(&cause.surfaces, output::details()),
                    );
                }
                render_omitted(window.omitted, "      ", "root cause");
            }
        }
    });
    Ok(complete)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct DiagnosticWindow {
    visible: usize,
    omitted: usize,
}

fn diagnostic_window(total: usize, details: bool) -> DiagnosticWindow {
    let visible = if details {
        total
    } else {
        total.min(DEFAULT_ROOT_CAUSE_LIMIT)
    };
    DiagnosticWindow {
        visible,
        omitted: total - visible,
    }
}

fn ranked_blockers(
    blockers: &[crate::review_scopes::ReviewQueueItem],
) -> Vec<&crate::review_scopes::ReviewQueueItem> {
    let mut ranked = blockers.iter().collect::<Vec<_>>();
    ranked.sort_by(|left, right| {
        left.priority
            .cmp(&right.priority)
            .then_with(|| {
                right
                    .potentially_unblocked_functions
                    .cmp(&left.potentially_unblocked_functions)
            })
            .then_with(|| {
                right
                    .affected_scope_roots
                    .len()
                    .cmp(&left.affected_scope_roots.len())
            })
            .then_with(|| right.occurrences.cmp(&left.occurrences))
            .then_with(|| left.id.cmp(&right.id))
    });
    ranked
}

fn render_omitted(omitted: usize, indent: &str, item: &str) {
    if omitted != 0 {
        outputln!(
            "{indent}{omitted} more {item}(s) omitted; rerun with --details or --format json."
        );
    }
}

fn one_line(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn human_message(value: &str, details: bool) -> String {
    let value = one_line(value);
    if details || value.chars().count() <= DEFAULT_MESSAGE_CHAR_LIMIT {
        return value;
    }
    const HINT: &str = "… (use --details or JSON)";
    let prefix_length = DEFAULT_MESSAGE_CHAR_LIMIT.saturating_sub(HINT.chars().count());
    let prefix = value.chars().take(prefix_length).collect::<String>();
    format!("{prefix}{HINT}")
}

fn id_summary(values: &[String], details: bool) -> String {
    if values.is_empty() {
        "none".to_owned()
    } else {
        let visible = if details {
            values.len()
        } else {
            values.len().min(DEFAULT_INLINE_ID_LIMIT)
        };
        let mut summary = values[..visible].join(", ");
        if values.len() > visible {
            summary.push_str(&format!(", +{} more", values.len() - visible));
        }
        summary
    }
}

#[derive(Debug, Eq, PartialEq)]
struct PolicyCause {
    message: String,
    surfaces: Vec<String>,
}

/// Collapse the same downstream policy failure across surfaces while keeping
/// the affected surface IDs explicit. The typed report remains unchanged.
fn grouped_policy_causes(
    surfaces: &[crate::verification::policy::SurfaceReport],
) -> Vec<PolicyCause> {
    let mut positions = BTreeMap::<String, usize>::new();
    let mut causes = Vec::<PolicyCause>::new();
    for surface in surfaces {
        for blocker in &surface.blockers {
            if let Some(position) = positions.get(blocker).copied() {
                causes[position].surfaces.push(surface.id.clone());
            } else {
                positions.insert(blocker.clone(), causes.len());
                causes.push(PolicyCause {
                    message: blocker.clone(),
                    surfaces: vec![surface.id.clone()],
                });
            }
        }
    }
    causes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::verification::policy::{SurfaceKind, SurfaceReport};

    #[test]
    fn default_diagnostic_window_is_bounded_and_reports_omitted_causes() {
        assert_eq!(
            diagnostic_window(DEFAULT_ROOT_CAUSE_LIMIT + 3, false),
            DiagnosticWindow {
                visible: DEFAULT_ROOT_CAUSE_LIMIT,
                omitted: 3,
            }
        );
        assert_eq!(
            diagnostic_window(DEFAULT_ROOT_CAUSE_LIMIT + 3, true),
            DiagnosticWindow {
                visible: DEFAULT_ROOT_CAUSE_LIMIT + 3,
                omitted: 0,
            }
        );
    }

    #[test]
    fn policy_causes_collapse_repeated_downstream_failures() {
        let surfaces = [
            surface("first", &["shared failure", "first-only failure"]),
            surface("second", &["shared failure"]),
        ];

        assert_eq!(
            grouped_policy_causes(&surfaces),
            [
                PolicyCause {
                    message: "shared failure".to_owned(),
                    surfaces: vec!["first".to_owned(), "second".to_owned()],
                },
                PolicyCause {
                    message: "first-only failure".to_owned(),
                    surfaces: vec!["first".to_owned()],
                },
            ]
        );
    }

    #[test]
    fn diagnostic_fields_are_rendered_as_single_lines() {
        assert_eq!(
            one_line("decode failed\n  at 0x1000\tunknown"),
            "decode failed at 0x1000 unknown"
        );
        assert_eq!(id_summary(&[], false), "none");
    }

    #[test]
    fn default_messages_and_identifier_lists_are_bounded() {
        let message = "diagnostic ".repeat(100);
        let compact = human_message(&message, false);
        assert!(compact.chars().count() <= DEFAULT_MESSAGE_CHAR_LIMIT);
        assert!(compact.ends_with("… (use --details or JSON)"));
        assert_eq!(human_message(&message, true), one_line(&message));

        let ids = (0..5)
            .map(|index| format!("root-{index}"))
            .collect::<Vec<_>>();
        assert_eq!(id_summary(&ids, false), "root-0, root-1, root-2, +2 more");
        assert_eq!(
            id_summary(&ids, true),
            "root-0, root-1, root-2, root-3, root-4"
        );
    }

    #[test]
    fn default_window_prefers_high_impact_causes_within_one_priority() {
        let mut blockers = (1..=6).map(|impact| blocker(1, impact)).collect::<Vec<_>>();
        blockers.push(blocker(0, 1));

        let ranked = ranked_blockers(&blockers);
        assert_eq!(ranked[0].priority, 0);
        assert_eq!(ranked[1].potentially_unblocked_functions, 6);
        assert_eq!(ranked[5].potentially_unblocked_functions, 2);
        assert_eq!(
            ranked
                .iter()
                .take(DEFAULT_ROOT_CAUSE_LIMIT)
                .filter(|blocker| blocker.priority == 1)
                .map(|blocker| blocker.potentially_unblocked_functions)
                .collect::<Vec<_>>(),
            [6, 5, 4, 3]
        );
    }

    fn surface(id: &str, blockers: &[&str]) -> SurfaceReport {
        SurfaceReport {
            id: id.to_owned(),
            description: String::new(),
            kind: SurfaceKind::ReviewScope,
            review_scopes: Vec::new(),
            requirements: 0,
            effects: 0,
            blockers: blockers.iter().map(|value| (*value).to_owned()).collect(),
            closed: blockers.is_empty(),
        }
    }

    fn blocker(priority: u8, impact: usize) -> crate::review_scopes::ReviewQueueItem {
        crate::review_scopes::ReviewQueueItem {
            id: format!("blocker-{priority}-{impact}"),
            kind: "decode".to_owned(),
            priority,
            severity: "blocking".to_owned(),
            occurrences: impact,
            functions: (0..impact).map(|index| format!("fn-{index}")).collect(),
            affected_scope_roots: vec!["root".to_owned()],
            potentially_unblocked_functions: impact,
            sites: Vec::new(),
            channels: Vec::new(),
            message: "decode blocker".to_owned(),
        }
    }
}
