//! Scope-driven projection of structured analysis blockers.

use super::super::{ProjectSession, model::*};

pub(super) fn collect(
    resolved: &ProjectSession,
    diagnostics: &mut Vec<DiagnosticRecord>,
) -> Vec<ReviewQueueSummary> {
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
                component: "review.queue".to_owned(),
                message: error.to_string(),
                path: Some(review.output.clone()),
            });
            return Vec::new();
        }
    };
    let has_publication_scopes = document.scopes.iter().any(|scope| scope.publication);
    let mut queue = document
        .scopes
        .into_iter()
        .filter(|scope| !has_publication_scopes || scope.publication)
        .flat_map(|scope| {
            let scope_id = scope.id;
            let publication = scope.publication;
            scope
                .review_queue
                .into_iter()
                .map(move |item| ReviewQueueSummary {
                    scope: scope_id.clone(),
                    publication,
                    id: item.id,
                    kind: item.kind,
                    priority: item.priority,
                    severity: item.severity,
                    occurrences: item.occurrences,
                    functions: item.functions,
                    affected_scope_roots: item.affected_scope_roots,
                    potentially_unblocked_functions: item.potentially_unblocked_functions,
                    sites: item.sites,
                    channels: item.channels,
                    message: item.message,
                })
        })
        .collect::<Vec<_>>();
    queue.sort_by(|left, right| {
        left.priority
            .cmp(&right.priority)
            .then_with(|| left.scope.cmp(&right.scope))
            .then_with(|| left.id.cmp(&right.id))
    });
    queue
}
