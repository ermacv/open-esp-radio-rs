//! Project-status projection for the read-only workspace snapshot.

use crate::application::{model::*, status::model::StatusReport};

pub(super) fn collect(report: &StatusReport) -> ProjectStatusSnapshot {
    ProjectStatusSnapshot {
        project_id: report.project_id.clone(),
        manifest: report.manifest.clone(),
        target_id: report.target.id.clone(),
        architecture: report.target.architecture.clone(),
        calling_convention: report.target.calling_convention.clone(),
        harness: report.target.harness.clone(),
        overall: readiness(report.overall),
        phases: report
            .phases
            .iter()
            .map(|phase| WorkspacePhaseSnapshot {
                name: phase.name.to_owned(),
                status: readiness(phase.status),
                components: phase
                    .components
                    .iter()
                    .map(|component| WorkspaceComponentSnapshot {
                        name: component.name.to_owned(),
                        status: readiness(component.status),
                        details: component
                            .details
                            .iter()
                            .map(|(key, value)| {
                                (
                                    key.clone(),
                                    serde_json::to_value(value)
                                        .expect("status detail values are serializable"),
                                )
                            })
                            .collect(),
                        diagnostic: component.diagnostic.clone(),
                    })
                    .collect(),
            })
            .collect(),
    }
}

fn readiness(value: crate::application::status::model::Readiness) -> WorkspaceReadiness {
    use crate::application::status::model::Readiness;
    match value {
        Readiness::Ready => WorkspaceReadiness::Ready,
        Readiness::Incomplete => WorkspaceReadiness::Incomplete,
        Readiness::NotConfigured => WorkspaceReadiness::NotConfigured,
        Readiness::Invalid => WorkspaceReadiness::Invalid,
    }
}
