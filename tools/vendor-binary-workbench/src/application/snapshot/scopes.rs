//! Release-scope summaries for the project browser.

use super::super::{ProjectSession, model::*};

pub(super) fn collect(
    resolved: &ProjectSession,
    diagnostics: &mut Vec<DiagnosticRecord>,
) -> Vec<ReviewScopeSummary> {
    let Some(review) = resolved.project.review.as_ref() else {
        return Vec::new();
    };
    if !review.output.is_file() {
        return Vec::new();
    }
    let document = match crate::review_scopes::load_for_project(&resolved.project) {
        Ok(document) => document,
        Err(error) => {
            diagnostics.push(DiagnosticRecord {
                severity: DiagnosticSeverity::Warning,
                component: "review.scopes".to_owned(),
                message: error.to_string(),
                path: Some(review.output.clone()),
            });
            return Vec::new();
        }
    };
    document
        .scopes
        .into_iter()
        .map(|scope| ReviewScopeSummary {
            id: scope.id,
            release: scope.release,
            profiles: scope.profiles,
            roots: scope.roots,
            functions: scope.functions,
            replacement_functions: scope.replacement_functions,
            complete_functions: scope.complete_functions,
            mmio_registers: scope.mmio_registers,
            table_calls: scope.table_calls,
            context_fields: scope.context_fields,
            memory_fields: scope.memory_fields,
            blockers: scope.review_queue.len(),
            decode_blockers: scope.decode_blockers,
            unresolved_calls: scope.unresolved_calls,
            replacement_gaps: scope.replacement_mismatches
                + scope.replacement_incomplete
                + scope.replacement_unqualified
                + scope.replacement_uncovered
                + scope.replacement_probe_only_matches
                + scope.replacement_unmapped_matches,
            function_identities: scope.function_identities,
            mmio_addresses: scope.mmio.into_iter().map(|mmio| mmio.address).collect(),
        })
        .collect()
}
