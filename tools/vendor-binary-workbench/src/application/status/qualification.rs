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
            let surface_effects = features
                .iter()
                .filter(|feature| feature.required)
                .map(|feature| feature.surface_effects)
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
            .detail("surface_effects", surface_effects)
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
            let optional = features
                .iter()
                .filter(|feature| !feature.required)
                .collect::<Vec<_>>();
            let blocked_optional = optional
                .iter()
                .filter(|feature| {
                    feature.status == crate::qualification::FeatureQualificationStatus::Blocked
                })
                .map(|feature| feature.id.clone())
                .collect::<Vec<_>>();
            let remaining_transactions = optional
                .iter()
                .map(|feature| {
                    feature
                        .surface_effects
                        .saturating_sub(feature.covered_effects)
                })
                .sum::<usize>();
            let mut components = vec![component];
            if !optional.is_empty() {
                components.push(
                    Component::new("feature_backlog", Readiness::Inventory)
                        .detail("features", blocked_optional)
                        .detail("remaining_transactions", remaining_transactions),
                );
            }
            Phase::collect("qualification", components)
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
