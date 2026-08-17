//! CLI adapter for project linked-IR generation.

use std::collections::BTreeSet;

use super::{MmioMap, Result, TargetSpec};
use crate::{
    application::project_ir_build::{BuildDocument, ProjectIrBuildRequest, build_project_ir},
    cli::IrBuildArgs,
    project::ProjectSpec,
    run_spec::RunSpec,
};

pub(super) fn run(
    arguments: IrBuildArgs,
    project_manifest: &std::path::Path,
    project: &ProjectSpec,
    run_spec: &RunSpec,
    svd: &MmioMap,
    target: &TargetSpec,
) -> Result<bool> {
    if !(1..=8).contains(&arguments.jobs) {
        return Err(crate::Error::invalid("ir build --jobs accepts 1..=8"));
    }
    let refresh_review_scopes = arguments.profile.is_empty();
    let document = build_project_ir(
        ProjectIrBuildRequest {
            profiles: arguments.profile.into_iter().collect::<BTreeSet<_>>(),
            check: arguments.check,
            jobs: usize::from(arguments.jobs),
            // A partial profile build must not parse unrelated stale IR
            // documents. The aggregate scope is refreshed by the all-profile
            // build that owns the complete input set.
            refresh_review_scopes,
        },
        project_manifest,
        project,
        run_spec,
        svd,
        target,
    )?;
    if !crate::cli::output::structured(&document) {
        print_human(&document);
    }
    Ok(true)
}

fn print_human(document: &BuildDocument<'_>) {
    use crate::cli::{output, table};

    outputln!("{}", output::heading("Linked IR"));
    outputln!(
        "{}",
        output::success(format!(
            "{} — {} profile(s), {} document(s)",
            document.status,
            document.profiles.len(),
            document.documents
        ))
    );
    outputln!("\n{}", output::heading("Profiles"));
    outputln!(
        "{}",
        table::render(
            [
                "Profile",
                "Functions",
                "Blockers",
                "Registers",
                "Fields",
                "Bundle"
            ],
            document.profiles.iter().map(|profile| [
                profile.id.to_owned(),
                profile.functions.to_string(),
                profile.decode_blockers.to_string(),
                profile.registers.to_string(),
                profile.field_candidates.to_string(),
                profile.bundle.display().to_string(),
            ]),
        )
    );
}
