//! Fast publication readiness; exact comparison belongs to publish/check.

use super::model::{Component, Phase, Readiness};
use crate::{application::ProjectContext, registers::ProjectRegisterWorkspace};

pub(super) fn collect(context: &ProjectContext<'_>) -> Phase {
    let Some(paths) = &context.project.registers else {
        return Phase::collect(
            "publication",
            vec![Component::new("register_outputs", Readiness::NotConfigured)],
        );
    };
    if context.project.review.is_none() {
        return Phase::collect(
            "publication",
            vec![
                Component::new("register_outputs", Readiness::Incomplete)
                    .diagnostic("publication review scopes are not configured")
                    .next_action("configure [review] and its publication-scopes"),
            ],
        );
    }
    let publication_mmio = match crate::review_scopes::load_for_project(context.project) {
        Ok(document) => document.publication_mmio(),
        Err(error) => {
            return Phase::collect(
                "publication",
                vec![
                    Component::new("register_outputs", Readiness::Invalid)
                        .diagnostic(error)
                        .next_action(format!(
                            "refresh review scopes with `blobray project analyze --project {}`",
                            context.project_path.display()
                        )),
                ],
            );
        }
    };
    let unreviewed = match ProjectRegisterWorkspace::load(paths)
        .and_then(|workspace| workspace.unreviewed_mmio_in_scope(&publication_mmio))
    {
        Ok(unreviewed) => unreviewed,
        Err(error) => {
            return Phase::collect(
                "publication",
                vec![Component::new("register_outputs", Readiness::Invalid)
                    .diagnostic(error)
                    .next_action(format!(
                        "resolve register review findings, then run `blobray project publish --check --project {}`",
                        context.project_path.display()
                    ))],
            );
        }
    };
    if !unreviewed.is_empty() {
        let identities = unreviewed
            .iter()
            .map(|(address, width)| format!("{address:#010x}/{width}"))
            .collect::<Vec<_>>()
            .join(", ");
        return Phase::collect(
            "publication",
            vec![Component::new("register_outputs", Readiness::Invalid)
                .detail("unreviewed", unreviewed.len().to_string())
                .detail("addresses", identities.clone())
                .diagnostic(format!(
                    "publication scopes contain {} unreviewed MMIO register(s): {identities}",
                    unreviewed.len()
                ))
                .next_action(format!(
                    "review the registers in {}, then run `blobray project publish --check --project {}`",
                    paths.model.display(),
                    context.project_path.display()
                ))],
        );
    }
    Phase::collect(
        "publication",
        vec![
            output(context, "svd", paths.svd_output.as_deref()),
            output(
                context,
                "pac-raw",
                paths.pac_raw.as_ref().map(|spec| spec.output.as_path()),
            ),
            output(context, "pac-api", paths.api_output.as_deref()),
            output(
                context,
                "bindings",
                paths.bindings.as_ref().map(|spec| spec.output.as_path()),
            ),
        ],
    )
}

fn output(
    context: &ProjectContext<'_>,
    name: &'static str,
    path: Option<&std::path::Path>,
) -> Component {
    let Some(path) = path else {
        return Component::new(name, Readiness::NotConfigured);
    };
    if path.exists() {
        Component::new(name, Readiness::Ready)
            .detail("path", path.display().to_string())
            .detail("file_status", "published")
            .detail("deep_validation", "project publish --check / project check")
    } else {
        Component::new(name, Readiness::Incomplete)
            .detail("path", path.display().to_string())
            .detail("file_status", "missing")
            .next_action(format!(
                "generate the configured outputs with `blobray project publish --project {}`",
                context.project_path.display()
            ))
    }
}
