//! Exact read-only comparison of configured derived register outputs.

use super::model::{Component, Phase, Readiness};
use crate::{application::ProjectContext, registers};

pub(super) fn collect(context: &ProjectContext<'_>) -> Phase {
    let Some(paths) = &context.project.registers else {
        return Phase::collect(
            "publication",
            vec![Component::new("register_outputs", Readiness::NotConfigured)],
        );
    };
    Phase::collect(
        "publication",
        vec![
            output("svd", paths.svd_output.is_some(), || {
                registers::prepare_project_svd(paths)
            }),
            output("pac", paths.pac.is_some(), || {
                registers::prepare_project_pac(paths)
            }),
            output("bindings", paths.bindings.is_some(), || {
                registers::prepare_project_bindings(paths)
            }),
        ],
    )
}

fn output(
    name: &'static str,
    configured: bool,
    prepare: impl FnOnce() -> crate::Result<registers::PreparedPublication>,
) -> Component {
    if !configured {
        return Component::new(name, Readiness::NotConfigured);
    }
    let publication = match prepare() {
        Ok(publication) => publication,
        Err(error) => return Component::new(name, Readiness::Invalid).diagnostic(error),
    };
    match publication.readiness() {
        Ok(readiness) => Component::new(
            name,
            if readiness == registers::PublicationReadiness::Current {
                Readiness::Ready
            } else {
                Readiness::Incomplete
            },
        )
        .detail("path", publication.output().display().to_string())
        .detail("file_status", readiness.label()),
        Err(error) => Component::new(name, Readiness::Invalid)
            .detail("path", publication.output().display().to_string())
            .diagnostic(error),
    }
}
