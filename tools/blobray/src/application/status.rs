//! Read-only project lifecycle inventory for every frontend.

use super::{ProjectContext, ProjectContextRequirement};
use model::{FollowUpStep, ProjectStatusReport, TargetIdentity};

mod analysis;
mod configuration_inputs;
pub(crate) mod model;
mod policy;
mod publication;
mod review;
mod revision;
mod verification;

pub(super) fn executable_step<I, S>(
    context: &ProjectContext<'_>,
    instruction: impl Into<String>,
    command: I,
    requirement: ProjectContextRequirement,
) -> FollowUpStep
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let instruction = instruction.into();
    match context.follow_up_action(command, requirement) {
        Ok(command) => FollowUpStep::command(instruction, command),
        Err(error) => FollowUpStep::manual(format!(
            "{instruction}; an executable command could not be represented: {error}"
        )),
    }
}

pub(super) fn executable_steps(
    context: &ProjectContext<'_>,
    instruction: impl Into<String>,
    commands: Vec<(Vec<String>, ProjectContextRequirement)>,
) -> FollowUpStep {
    let instruction = instruction.into();
    let commands = commands
        .into_iter()
        .map(|(command, requirement)| context.follow_up_action(command, requirement))
        .collect::<crate::Result<Vec<_>>>();
    match commands {
        Ok(commands) => FollowUpStep::commands(instruction, commands),
        Err(error) => FollowUpStep::manual(format!(
            "{instruction}; executable commands could not be represented: {error}"
        )),
    }
}

pub(super) fn inputs_init_step(
    context: &ProjectContext<'_>,
    instruction: impl Into<String>,
) -> FollowUpStep {
    let instruction = instruction.into();
    match context.inputs_init_help_action() {
        Ok(command) => FollowUpStep::command(instruction, command),
        Err(error) => FollowUpStep::manual(format!(
            "{instruction}; an executable command could not be represented: {error}"
        )),
    }
}

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
