//! Exact read-only comparison of configured derived register outputs.

use super::model::{Component, Phase, Readiness};
use crate::{
    application::ProjectContext,
    registers::{self, ProjectRegisterWorkspace},
};

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
                vec![Component::new("register_outputs", Readiness::Invalid)
                    .diagnostic(error)
                    .next_action(format!(
                        "refresh review scopes with `vendor-binary-workbench project analyze --project {}`",
                        context.project_path.display()
                    ))],
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
                        "resolve register review findings, then run `vendor-binary-workbench project publish --check --project {}`",
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
                    "review the registers in {}, then run `vendor-binary-workbench project publish --check --project {}`",
                    paths.model.display(),
                    context.project_path.display()
                ))],
        );
    }
    Phase::collect(
        "publication",
        vec![
            output(context, "svd", paths.svd_output.is_some(), || {
                registers::prepare_project_svd(paths, &publication_mmio)
            }),
            output(context, "pac-raw", paths.pac_raw.is_some(), || {
                registers::prepare_project_pac_raw(paths, &publication_mmio)
            }),
            output(context, "pac-api", paths.api_output.is_some(), || {
                registers::prepare_project_pac_api(paths)
            }),
            output(context, "bindings", paths.bindings.is_some(), || {
                registers::prepare_project_bindings(paths, &publication_mmio)
            }),
        ],
    )
}

fn output(
    context: &ProjectContext<'_>,
    name: &'static str,
    configured: bool,
    prepare: impl FnOnce() -> crate::Result<registers::PreparedPublication>,
) -> Component {
    if !configured {
        return Component::new(name, Readiness::NotConfigured);
    }
    let publication = match prepare() {
        Ok(publication) => publication,
        Err(error) => {
            return Component::new(name, Readiness::Invalid)
                .diagnostic(error)
                .next_action(format!(
                    "resolve register review findings, then run `vendor-binary-workbench project publish --check --project {}`",
                    context.project_path.display()
                ));
        }
    };
    match publication.readiness() {
        Ok(readiness) => {
            let mut component = Component::new(
                name,
                if readiness == registers::PublicationReadiness::Current {
                    Readiness::Ready
                } else {
                    Readiness::Incomplete
                },
            )
            .detail("path", publication.output().display().to_string())
            .detail("file_status", readiness.label());
            if readiness != registers::PublicationReadiness::Current {
                component = component.next_action(format!(
                    "refresh and verify the configured outputs with `vendor-binary-workbench project publish --project {}`",
                    context.project_path.display()
                ));
            }
            component
        }
        Err(error) => Component::new(name, Readiness::Invalid)
            .detail("path", publication.output().display().to_string())
            .diagnostic(error)
            .next_action(format!(
                "repair the output or regenerate it with `vendor-binary-workbench project publish --project {}`",
                context.project_path.display()
            )),
    }
}
