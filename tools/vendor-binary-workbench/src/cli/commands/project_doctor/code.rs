//! Reviewed executable-code boundary readiness inspection.

use crate::{
    artifacts::symbol_inventory::load_code_boundary_facts, cli::commands::ProjectContext,
    code_workspace::CodeWorkspace,
};

use super::model::{CapabilityReport, DoctorReport};

pub(super) fn collect(context: &ProjectContext<'_>, report: &mut DoctorReport) {
    let Some(paths) = &context.project.code else {
        report.capability(CapabilityReport::new("code-boundaries", "not-configured"));
        return;
    };
    let Some(inventory) = &context.project.symbol_inventory else {
        report.error();
        report.capability(
            CapabilityReport::new("code-boundaries", "invalid")
                .field("error", "[code] requires [analysis.symbols]"),
        );
        return;
    };
    if !inventory.output.is_file() {
        report.absorb(0, 1);
        report.capability(
            CapabilityReport::new("code-boundaries", "facts-not-generated")
                .field("facts", inventory.output.display().to_string())
                .field("pack", paths.pack.display().to_string()),
        );
        return;
    }
    if !paths.pack.is_file() {
        report.absorb(0, 1);
        report.capability(
            CapabilityReport::new("code-boundaries", "pack-not-initialized")
                .field("facts", inventory.output.display().to_string())
                .field("pack", paths.pack.display().to_string()),
        );
        return;
    }
    let workspace = load_code_boundary_facts(&inventory.output)
        .and_then(|facts| CodeWorkspace::load(&facts, &paths.pack, &context.project.id));
    match workspace {
        Ok(workspace) => {
            let summary = workspace.summary();
            report.capability(
                CapabilityReport::new("code-boundaries", "available")
                    .field("candidates", summary.observed_candidates)
                    .field("accepted", summary.accepted)
                    .field("rejected", summary.rejected)
                    .field("unreviewed", summary.unreviewed)
                    .field("facts", inventory.output.display().to_string())
                    .field("pack", paths.pack.display().to_string()),
            );
        }
        Err(error) => {
            report.error();
            report.capability(
                CapabilityReport::new("code-boundaries", "invalid")
                    .field("facts", inventory.output.display().to_string())
                    .field("pack", paths.pack.display().to_string())
                    .field("error", error.to_string())
                    .field("next-action", "code rebase"),
            );
        }
    }
}
