//! Read-only project lifecycle inventory for every frontend.

use super::ProjectContext;
use model::{ProjectStatusReport, TargetIdentity};

mod analysis;
mod configuration_inputs;
pub(crate) mod model;
mod policy;
mod publication;
mod review;
mod revision;
mod verification;

pub(crate) fn collect(context: &ProjectContext<'_>) -> ProjectStatusReport {
    fn phase(name: &'static str, collect: impl FnOnce() -> model::Phase) -> model::Phase {
        let started = std::time::Instant::now();
        let phase = collect();
        tracing::debug!(
            phase = name,
            elapsed_ms = started.elapsed().as_millis(),
            "project status phase collected"
        );
        phase
    }
    ProjectStatusReport::new(
        context.project.id.clone(),
        context.project_path.display().to_string(),
        TargetIdentity {
            id: context.target.id.clone(),
            architecture: context.target.architecture.label().to_owned(),
            calling_convention: context.target.calling_convention.label().to_owned(),
            knowledge_provider: context.target.knowledge_provider.clone(),
        },
        vec![
            phase("configuration", || {
                configuration_inputs::configuration(context)
            }),
            phase("inputs", || configuration_inputs::inputs(context)),
            phase("analysis", || analysis::collect(context)),
            phase("review", || review::collect(context)),
            phase("revision-workflow", || revision::collect(context)),
            phase("verification", || verification::collect(context)),
            phase("verification-policy", || policy::collect(context)),
            phase("publication", || publication::collect(context)),
        ],
    )
}

pub(crate) fn verification_report_status(context: &ProjectContext<'_>) -> model::Component {
    verification::last_report_component(context)
}
