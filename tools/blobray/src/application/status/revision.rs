//! Lightweight durable revision-baseline readiness.

use super::{ProjectContext, executable_step, executable_steps, model};
use crate::application::{
    ProjectContextRequirement,
    revision::{self, RevisionStateHealth},
};

pub(super) fn collect(context: &ProjectContext<'_>) -> model::Phase {
    let inspection = revision::inspect_state(context.project_path, &context.project.id, false);
    let status = match inspection.health {
        RevisionStateHealth::Ready => model::Readiness::Ready,
        RevisionStateHealth::Missing
        | RevisionStateHealth::BaselineMissing
        | RevisionStateHealth::RevisionReviewPending => model::Readiness::Incomplete,
        RevisionStateHealth::Invalid => model::Readiness::Invalid,
    };
    let mut component = model::Component::new("durable-revision-baseline", status)
        .detail("state", inspection.path)
        .detail("revisions", inspection.revisions)
        .detail(
            "baseline",
            inspection.baseline.unwrap_or_else(|| "-".to_owned()),
        )
        .detail(
            "current",
            inspection.current.unwrap_or_else(|| "-".to_owned()),
        )
        .detail("update-prepared", inspection.update_prepared);
    if let Some(diagnostic) = inspection.diagnostic {
        let next_step = match inspection.health {
            RevisionStateHealth::RevisionReviewPending => executable_step(
                context,
                "review the revision diff/rebase, then accept the current revision",
                ["project", "revision", "prepare-update", "--accept-current"],
                ProjectContextRequirement::RunSpec,
            ),
            RevisionStateHealth::Invalid => executable_step(
                context,
                "preserve historical state and snapshots, move the invalid active state aside, then capture authenticated current findings",
                ["project", "revision", "snapshot", "CURRENT"],
                ProjectContextRequirement::RunSpec,
            ),
            _ => executable_steps(
                context,
                "snapshot the baseline, then prepare the update before replacing bindings",
                vec![
                    (
                        vec![
                            "project".into(),
                            "revision".into(),
                            "snapshot".into(),
                            "BASELINE".into(),
                        ],
                        ProjectContextRequirement::ProjectOnly,
                    ),
                    (
                        vec!["project".into(), "revision".into(), "prepare-update".into()],
                        ProjectContextRequirement::RunSpec,
                    ),
                ],
            ),
        };
        component = component.diagnostic(diagnostic).next_step(next_step);
    }
    model::Phase::collect("revision-workflow", vec![component])
}
