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
                    crate::qualification::FeatureQualificationStatus::HardwareQualified => {
                        "hardware-qualified"
                    }
                    crate::qualification::FeatureQualificationStatus::Blocked => "blocked",
                }
                .to_owned(),
                coverage: feature.coverage.as_str().to_owned(),
                scopes: feature.scopes,
                requirements: feature.requirements,
                surface_effects: feature.surface_effects,
                covered_effects: feature.covered_effects,
                dependencies: feature
                    .dependencies
                    .into_iter()
                    .map(|dependency| FeatureDependencySummary {
                        feature: dependency.feature,
                        phase: dependency.phase,
                        status: dependency.status.as_str().to_owned(),
                        blockers: dependency.blockers.len(),
                    })
                    .collect(),
                phases: feature
                    .phases
                    .into_iter()
                    .map(|phase| FeaturePhaseSummary {
                        id: phase.id,
                        transactions: phase.transactions,
                        covered_transactions: phase.covered_transactions,
                        requirements: phase.requirements,
                        blockers: phase.blockers.len(),
                    })
                    .collect(),
                hardware: feature.hardware.map(|hardware| FeatureHardwareSummary {
                    status: hardware.status,
                    successful_runs: hardware.successful_runs,
                    minimum_successful_runs: hardware.minimum_successful_runs,
                    blockers: hardware.blockers,
                }),
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
