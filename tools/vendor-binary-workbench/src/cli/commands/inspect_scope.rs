//! Focused project review-scope report.

use super::super::*;

#[derive(serde::Serialize)]
struct ScopeInvestigationReport {
    schema_version: u32,
    command: &'static str,
    scope: crate::review_scopes::ReviewScopeReport,
    features: Vec<crate::qualification::FeatureQualificationReport>,
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
    let features = crate::qualification::evaluate(project)?
        .into_iter()
        .filter(|feature| feature.scopes.iter().any(|id| id == &scope.id))
        .collect::<Vec<_>>();
    let complete = scope.analysis_inventory_complete;
    let report = ScopeInvestigationReport {
        schema_version: 1,
        command: "inspect scope",
        scope,
        features,
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
            outputln!("  blockers:");
            for blocker in &scope.review_queue {
                outputln!(
                    "    - [{}] {} ({} occurrence(s))",
                    blocker.kind,
                    blocker.message,
                    blocker.occurrences
                );
            }
        }
        if !report.features.is_empty() {
            outputln!("  feature qualifications:");
            for feature in &report.features {
                outputln!(
                    "    - {}: {:?}, effects={}/{}, proofs={}, blockers={}",
                    feature.id,
                    feature.status,
                    feature.covered_effects,
                    feature.surface_effects,
                    feature.requirements,
                    feature.blockers.len(),
                );
                for blocker in &feature.blockers {
                    outputln!("        ! {blocker}");
                }
            }
        }
    });
    Ok(complete)
}
