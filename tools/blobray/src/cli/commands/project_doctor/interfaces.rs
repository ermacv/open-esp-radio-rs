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
    match InterfaceWorkspace::load_with_templates(
        &paths.facts,
        pack,
        &paths.semantic_catalogs,
        &paths.interface_template_packs,
        context.target.calling_convention.label(),
        context
            .target
            .knowledge_provider
            .as_deref()
            .and_then(|harness| crate::providers::contracts(harness).ok()),
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
                    .field("execution-contracts", summary.execution_contracts)
                    .field("execution-models", summary.execution_models)
                    .field(
                        "interface-template-packs",
                        paths.interface_template_packs.len(),
                    )
                    .field("interface-templates", summary.interface_templates)
                    .field("templated-anchors", summary.templated_anchors)
                    .field("facts", paths.facts.display().to_string())
                    .field("pack", pack.display().to_string()),
            );
            if paths.capability_packs.is_empty() {
                report.capability(CapabilityReport::new(
                    "reusable-capabilities",
                    "not-configured",
                ));
            } else {
                match workspace.evaluate_capabilities(&paths.capability_packs) {
                    Ok(capabilities) => {
                        if capabilities.status != crate::interfaces::CapabilityMatchStatus::Matched
                        {
                            report.absorb(0, 1);
                        }
                        report.capability(
                            CapabilityReport::new(
                                "reusable-capabilities",
                                capabilities.status.label(),
                            )
                            .field("packs", capabilities.packs)
                            .field("rules", capabilities.rules.len())
                            .field("matched", capabilities.matched)
                            .field("incomplete", capabilities.incomplete)
                            .field("unknown", capabilities.unknown),
                        );
                    }
                    Err(error) => {
                        report.error();
                        report.capability(
                            CapabilityReport::new("reusable-capabilities", "invalid")
                                .field("error", error.to_string()),
                        );
                    }
                }
            }
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
