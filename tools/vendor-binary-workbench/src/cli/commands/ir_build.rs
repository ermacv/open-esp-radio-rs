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
    project: &ProjectSpec,
    run_spec: &RunSpec,
    svd: &MmioMap,
    target: &TargetSpec,
) -> Result<bool> {
    if arguments.jobs > 8 {
        return Err(crate::Error::invalid(
            "ir build --jobs accepts 0 (safe automatic mode) or 1..=8",
        ));
    }
    let document = build_project_ir(
        ProjectIrBuildRequest {
            profiles: arguments.profile.into_iter().collect::<BTreeSet<_>>(),
            check: arguments.check,
            jobs: usize::from(arguments.jobs),
        },
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
    outputln!(
        "IR build: {} ({} profile{}, {} document{})",
        document.status,
        document.profiles.len(),
        if document.profiles.len() == 1 {
            ""
        } else {
            "s"
        },
        document.documents,
        if document.documents == 1 { "" } else { "s" }
    );
    for profile in &document.profiles {
        outputln!(
            "  {:<20} functions={:<6} decode-blockers={:<6} registers={:<5} fields={:<5} {}",
            profile.id,
            profile.functions,
            profile.decode_blockers,
            profile.registers,
            profile.field_candidates,
            profile.json.display()
        );
        if let Some(pseudo) = profile.pseudo {
            outputln!("  {:<20} pseudo={}", "", pseudo.display());
        }
    }
}
