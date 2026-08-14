//! Flat verification-policy summaries for the project browser.

use super::super::{ProjectSession, model::*};

pub(super) fn collect(
    resolved: &ProjectSession,
    diagnostics: &mut Vec<DiagnosticRecord>,
) -> Vec<VerificationSurfaceSummary> {
    match crate::verification::policy::evaluate(&resolved.project) {
        Ok(Some(report)) => report
            .surfaces
            .into_iter()
            .map(|surface| VerificationSurfaceSummary {
                id: surface.id,
                description: surface.description,
                kind: surface.kind.as_str().to_owned(),
                scopes: surface.review_scopes,
                requirements: surface.requirements,
                effects: surface.effects,
                closed: surface.closed,
                blockers: surface.blockers,
            })
            .collect(),
        Ok(None) => Vec::new(),
        Err(error) => {
            diagnostics.push(DiagnosticRecord {
                severity: DiagnosticSeverity::Error,
                component: "verification.policy".to_owned(),
                message: error.to_string(),
                path: resolved
                    .project
                    .verification
                    .as_ref()
                    .and_then(|verification| verification.policy.clone()),
            });
            Vec::new()
        }
    }
}
