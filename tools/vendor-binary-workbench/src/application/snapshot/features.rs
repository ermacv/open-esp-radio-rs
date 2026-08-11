//! Release-feature qualification summaries for the project browser.

use super::super::{ProjectSession, model::*};

pub(super) fn collect(
    resolved: &ProjectSession,
    diagnostics: &mut Vec<DiagnosticRecord>,
) -> Vec<FeatureQualificationSummary> {
    match crate::qualification::evaluate(&resolved.project) {
        Ok(features) => features
            .into_iter()
            .map(|feature| FeatureQualificationSummary {
                id: feature.id,
                description: feature.description,
                required: feature.required,
                status: match feature.status {
                    crate::qualification::FeatureQualificationStatus::Qualified => "qualified",
                    crate::qualification::FeatureQualificationStatus::Blocked => "blocked",
                }
                .to_owned(),
                scopes: feature.scopes,
                requirements: feature.requirements,
                scope_effects: feature.scope_effects,
                covered_effects: feature.covered_effects,
                blockers: feature.blockers,
            })
            .collect(),
        Err(error) => {
            diagnostics.push(DiagnosticRecord {
                severity: DiagnosticSeverity::Warning,
                component: "qualification.features".to_owned(),
                message: error.to_string(),
                path: resolved
                    .project
                    .qualification
                    .as_ref()
                    .map(|workspace| workspace.pack.clone()),
            });
            Vec::new()
        }
    }
}
