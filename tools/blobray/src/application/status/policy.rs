//! Fail-closed status for the flat verification policy.

use super::model::{Component, Phase, Readiness};
use crate::application::{FollowUpRequirements, ProjectContext};

pub(super) fn collect(context: &ProjectContext<'_>) -> Phase {
    let policy_path = context
        .project
        .verification
        .as_ref()
        .and_then(|verification| verification.policy.as_ref());
    let Some(policy_path) = policy_path else {
        return Phase::collect(
            "verification-policy",
            vec![
                Component::new("surfaces", Readiness::Incomplete)
                    .diagnostic("verification policy is not configured")
                    .next_action(format!(
                        "configure verification.policy in {}",
                        context.project_path.display()
                    )),
            ],
        );
    };
    match crate::verification::policy::evaluate(context.project) {
        Ok(Some(report)) => {
            let blockers = report
                .surfaces
                .iter()
                .flat_map(|surface| {
                    surface
                        .blockers
                        .iter()
                        .map(move |blocker| format!("{}: {blocker}", surface.id))
                })
                .collect::<Vec<_>>();
            let mut component = Component::new(
                "surfaces",
                if report.passed {
                    Readiness::Ready
                } else {
                    Readiness::Incomplete
                },
            )
            .detail("policy", policy_path.display().to_string())
            .detail("closed", report.closed)
            .detail("blocked", report.blocked)
            .detail("blockers", blockers.clone());
            if let Some(first) = blockers.first() {
                component = component
                    .diagnostic(first)
                    .next_action("close the reported verification surface");
            }
            Phase::collect("verification-policy", vec![component])
        }
        Ok(None) => unreachable!("policy path was present"),
        Err(error) => Phase::collect(
            "verification-policy",
            vec![
                Component::new("surfaces", Readiness::Incomplete)
                    .detail("policy", policy_path.display().to_string())
                    .diagnostic(error)
                    .next_action(format!(
                        "regenerate review with `{}`, then verification with `{}`",
                        context
                            .follow_up_command("project analyze", FollowUpRequirements::ANALYSIS,),
                        context
                            .follow_up_command("project verify", FollowUpRequirements::ANALYSIS,)
                    )),
            ],
        ),
    }
}
