//! CLI adapter for frontend-independent project composition.

use std::path::Path;

use crate::{
    Result,
    application::{ProjectConfigureReport, ProjectConfigureRequest, configure_project},
    cli::ProjectConfigureArgs,
};

pub(super) fn run(arguments: ProjectConfigureArgs, manifest: &Path) -> Result<bool> {
    let ecosystem_packs = if arguments.no_ecosystem_pack {
        Some(Vec::new())
    } else if arguments.ecosystem_pack.is_empty() {
        None
    } else {
        Some(arguments.ecosystem_pack)
    };
    let report = configure_project(
        manifest,
        ProjectConfigureRequest {
            ecosystem_packs,
            check: arguments.check,
        },
    )
    .map_err(|error| error.into_inner())?;
    crate::cli::output::render_report(&report, || print_report(&report));
    Ok(true)
}

fn print_report(report: &ProjectConfigureReport) {
    outputln!("{}", crate::cli::output::heading("Project configuration"));
    outputln!(
        "{}",
        crate::cli::output::success(format!("{} — configuration is valid", report.status))
    );
    outputln!("\nManifest: {}", report.manifest);
    if !report.ecosystem_packs.is_empty() {
        outputln!("Ecosystem packs:     {}", report.ecosystem_packs.join(", "));
        outputln!(
            "Knowledge provider:  {}",
            report.knowledge_provider.as_deref().unwrap_or("none")
        );
        outputln!("Knowledge packs:     {}", report.knowledge_packs);
        outputln!("Knowledge operations:{}", report.knowledge_operations);
    } else {
        outputln!("Ecosystem packs: none");
        outputln!(
            "Knowledge provider: {}",
            report.knowledge_provider.as_deref().unwrap_or("none")
        );
    }
    outputln!("\n{}", crate::cli::output::heading("Next"));
    for (index, step) in report.next_steps.iter().enumerate() {
        outputln!("{}. {}", index + 1, step.instruction);
        for action in &step.commands {
            outputln!("   {}", action.render_posix());
        }
    }
}
