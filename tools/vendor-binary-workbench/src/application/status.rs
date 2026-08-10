//! Read-only project lifecycle inventory for every frontend.

use super::ProjectContext;
use model::{ProjectStatusReport, TargetIdentity};

mod analysis;
mod configuration_inputs;
pub(crate) mod model;
mod publication;
mod review;
mod verification;

pub(crate) fn collect(context: &ProjectContext<'_>) -> ProjectStatusReport {
    ProjectStatusReport::new(
        context.project.id.clone(),
        context.project_path.display().to_string(),
        TargetIdentity {
            id: context.target.id.clone(),
            architecture: context.target.architecture.label().to_owned(),
            calling_convention: context.target.calling_convention.label().to_owned(),
            harness: context.target.harness.clone(),
        },
        vec![
            configuration_inputs::configuration(context),
            configuration_inputs::inputs(context),
            analysis::collect(context),
            review::collect(context),
            verification::collect(context),
            publication::collect(context),
        ],
    )
}

pub(crate) fn verification_report_status(context: &ProjectContext<'_>) -> model::Component {
    verification::last_report_component(context)
}
