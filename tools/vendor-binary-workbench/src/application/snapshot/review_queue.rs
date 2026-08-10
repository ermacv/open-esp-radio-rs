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
    let has_release_scopes = document.scopes.iter().any(|scope| scope.release);
    let mut queue = document
        .scopes
        .into_iter()
        .filter(|scope| !has_release_scopes || scope.release)
        .flat_map(|scope| {
            let scope_id = scope.id;
            let release = scope.release;
            scope
                .review_queue
                .into_iter()
                .map(move |item| ReviewQueueSummary {
                    scope: scope_id.clone(),
                    release,
                    id: item.id,
                    kind: item.kind,
                    priority: item.priority,
                    severity: item.severity,
                    occurrences: item.occurrences,
                    functions: item.functions,
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
