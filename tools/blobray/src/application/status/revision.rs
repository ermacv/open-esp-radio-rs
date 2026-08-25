//! Lightweight durable revision-baseline readiness.

use super::{ProjectContext, model};
use crate::application::{
    ProjectContextRequirement,
    revision::{self, RevisionLedgerHealth},
};

pub(super) fn collect(context: &ProjectContext<'_>) -> model::Phase {
    let inspection = revision::inspect_ledger(context.project_path, &context.project.id, false);
    let status = match inspection.health {
        RevisionLedgerHealth::Ready => model::Readiness::Ready,
        RevisionLedgerHealth::Missing
        | RevisionLedgerHealth::BaselineMissing
        | RevisionLedgerHealth::RevisionReviewPending => model::Readiness::Incomplete,
        RevisionLedgerHealth::Invalid => model::Readiness::Invalid,
    };
    let mut component = model::Component::new("durable-revision-baseline", status)
        .detail("ledger", inspection.path)
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
        let next_action = match inspection.health {
            RevisionLedgerHealth::RevisionReviewPending => format!(
                "review revision diff/rebase; then run {} --accept-current",
                context.follow_up_command(
                    "project revision prepare-update",
                    ProjectContextRequirement::RunSpec,
                )
            ),
            RevisionLedgerHealth::Invalid => context.follow_up_command(
                "project revision snapshot CURRENT",
                ProjectContextRequirement::RunSpec,
            ),
            _ => format!(
                "{}; before replacing bindings run {}",
                context.follow_up_command(
                    "project revision snapshot BASELINE",
                    ProjectContextRequirement::ProjectOnly,
                ),
                context.follow_up_command(
                    "project revision prepare-update",
                    ProjectContextRequirement::RunSpec,
                )
            ),
        };
        component = component.diagnostic(diagnostic).next_action(next_action);
    }
    model::Phase::collect("revision-workflow", vec![component])
}
