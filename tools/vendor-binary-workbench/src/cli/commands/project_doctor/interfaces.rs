//! Interface-fact and reviewed-interface workspace readiness inspection.

use crate::{
    cli::commands::ProjectContext,
    interfaces::{InterfaceFacts, InterfaceWorkspace},
};

use super::model::{CapabilityReport, DoctorReport};

pub(super) fn collect(context: &ProjectContext<'_>, report: &mut DoctorReport) {
    let Some(paths) = &context.project.interfaces else {
        report.capability(CapabilityReport::new("interface-facts", "not-configured"));
        return;
    };
    if !paths.facts.is_file() {
        report.absorb(0, 1);
        report.capability(
            CapabilityReport::new("interface-facts", "not-generated")
                .field("facts", paths.facts.display().to_string()),
        );
        return;
    }
    let facts = match InterfaceFacts::load(&paths.facts) {
        Ok(facts) => facts,
        Err(error) => {
            report.error();
            report.capability(
                CapabilityReport::new("interface-workspace", "invalid-facts")
                    .field("facts", paths.facts.display().to_string())
                    .field("error", error.to_string()),
            );
            return;
        }
    };
    let Some(pack) = paths.pack.as_deref() else {
        report.capability(
            CapabilityReport::new("interface-facts", "available")
                .field("tables", facts.tables.len())
                .field("observed-slots", facts.observed_slots())
                .field("observed-calls", facts.observed_calls())
                .field("facts", paths.facts.display().to_string()),
        );
        return;
    };
    if !pack.is_file() {
        report.absorb(0, 1);
        report.capability(
            CapabilityReport::new("interface-workspace", "pack-not-initialized")
                .field("tables", facts.tables.len())
                .field("observed-slots", facts.observed_slots())
                .field("observed-calls", facts.observed_calls())
                .field("facts", paths.facts.display().to_string())
                .field("pack", pack.display().to_string()),
        );
        return;
    }
    match InterfaceWorkspace::load(
        &paths.facts,
        pack,
        &paths.semantic_catalogs,
        context.target.calling_convention.label(),
    ) {
        Ok(workspace) => {
            let summary = workspace.summary();
            report.capability(
                CapabilityReport::new("interface-workspace", "available")
                    .field("fact-tables", summary.fact_tables)
                    .field("observed-slots", summary.observed_slots)
                    .field("observed-calls", summary.observed_calls)
                    .field("resolved-calls", summary.resolved_calls)
                    .field("reviewed-anchors", summary.reviewed_anchors)
                    .field("ignored-anchors", summary.ignored_anchors)
                    .field("unreviewed-anchors", summary.unreviewed_anchors)
                    .field("reviewed-slots", summary.reviewed_slots)
                    .field("ignored-slots", summary.ignored_slots)
                    .field("unreviewed-slots", summary.unreviewed_slots)
                    .field("semantic-links", summary.semantic_links)
                    .field("semantic-operations", summary.semantic_operations)
                    .field("facts", paths.facts.display().to_string())
                    .field("pack", pack.display().to_string()),
            );
        }
        Err(error) => {
            report.error();
            report.capability(
                CapabilityReport::new("interface-workspace", "invalid")
                    .field("facts", paths.facts.display().to_string())
                    .field("pack", pack.display().to_string())
                    .field("error", error.to_string()),
            );
        }
    }
}
