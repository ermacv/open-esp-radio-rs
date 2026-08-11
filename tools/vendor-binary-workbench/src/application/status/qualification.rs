//! Fail-closed required-feature qualification status.

use super::model::{Component, Phase, Readiness};
use crate::application::ProjectContext;

pub(super) fn collect(context: &ProjectContext<'_>) -> Phase {
    let Some(workspace) = context.project.qualification.as_ref() else {
        return Phase::collect(
            "qualification",
            vec![
                Component::new("required_features", Readiness::Incomplete)
                    .diagnostic("feature qualification is not configured")
                    .next_action(format!(
                        "configure [qualification] in {}",
                        context.project_path.display()
                    )),
            ],
        );
    };
    match crate::qualification::evaluate(context.project) {
        Ok(features) => {
            let required = features.iter().filter(|feature| feature.required).count();
            let blocked = features
                .iter()
                .filter(|feature| {
                    feature.required
                        && feature.status
                            == crate::qualification::FeatureQualificationStatus::Blocked
                })
                .count();
            let blockers = features
                .iter()
                .filter(|feature| feature.required)
                .flat_map(|feature| {
                    feature
                        .blockers
                        .iter()
                        .map(move |blocker| format!("{}: {blocker}", feature.id))
                })
                .collect::<Vec<_>>();
            let scope_effects = features
                .iter()
                .filter(|feature| feature.required)
                .map(|feature| feature.scope_effects)
                .sum::<usize>();
            let covered_effects = features
                .iter()
                .filter(|feature| feature.required)
                .map(|feature| feature.covered_effects)
                .sum::<usize>();
            let mut component = Component::new(
                "required_features",
                if required != 0 && blocked == 0 {
                    Readiness::Ready
                } else {
                    Readiness::Incomplete
                },
            )
            .detail("pack", workspace.pack.display().to_string())
            .detail("required_count", required)
            .detail("blocked", blocked)
            .detail("scope_effects", scope_effects)
            .detail("covered_effects", covered_effects)
            .detail("features", workspace.required_features.clone())
            .detail("blockers", blockers.clone());
            if required == 0 {
                component = component.diagnostic("no required features are configured");
            } else if let Some(first) = blockers.first() {
                component = component
                    .diagnostic(first)
                    .next_action("close the reported feature analysis/proof boundary");
            }
            Phase::collect("qualification", vec![component])
        }
        Err(error) => Phase::collect(
            "qualification",
            vec![
                Component::new("required_features", Readiness::Incomplete)
                    .detail("pack", workspace.pack.display().to_string())
                    .diagnostic(error)
                    .next_action("regenerate review and verification reports, then retry"),
            ],
        ),
    }
}
