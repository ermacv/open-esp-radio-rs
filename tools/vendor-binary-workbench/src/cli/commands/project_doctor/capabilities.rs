//! Generic target and project-analysis capability inspection.

use crate::{application::ProjectContext, memory_map::MemoryRegionKind, registers::RegisterModel};

use super::model::{CapabilityReport, DoctorReport};

pub(super) fn collect(context: &ProjectContext<'_>, report: &mut DoctorReport) {
    collect_knowledge_packs(context, report);
    collect_backend(context, report);
    collect_knowledge_provider(context, report);
    collect_memory_map(context, report);
    collect_register_catalog(context, report);
    collect_symbol_inventory(context, report);
    collect_navigation_index(context, report);
}

fn collect_knowledge_packs(context: &ProjectContext<'_>, report: &mut DoctorReport) {
    report.capability(
        CapabilityReport::new(
            "ecosystem-packs",
            if context.project.ecosystem_packs.is_empty() {
                "not-configured"
            } else {
                "available"
            },
        )
        .field(
            "ids",
            context
                .project
                .ecosystem_packs
                .iter()
                .map(|pack| pack.id.clone())
                .collect::<Vec<_>>(),
        ),
    );
    let chip = match &context.project.chip_pack {
        Some(pack) => CapabilityReport::new("chip-pack", "available")
            .field("id", pack.id.as_str())
            .field(
                "knowledge-provider",
                pack.knowledge_provider.as_deref().unwrap_or("-"),
            )
            .field("knowledge-packs", pack.knowledge_packs.len())
            .field("knowledge-operations", pack.knowledge_operations)
            .field("path", pack.path.display().to_string()),
        None => CapabilityReport::new("chip-pack", "not-configured")
            .field("reason", "architecture-and-ecosystem-only"),
    };
    report.capability(chip);
}

fn collect_backend(context: &ProjectContext<'_>, report: &mut DoctorReport) {
    let capability = match context.target.require_available_backend() {
        Ok(()) => CapabilityReport::new("backend", "available")
            .field("architecture", context.target.architecture.label())
            .field(
                "calling-convention",
                context.target.calling_convention.label(),
            ),
        Err(error) => {
            report.error();
            CapabilityReport::new("backend", "unavailable").field("error", error.to_string())
        }
    };
    report.capability(capability);
}

fn collect_knowledge_provider(context: &ProjectContext<'_>, report: &mut DoctorReport) {
    let capability = match &context.target.knowledge_provider {
        None => CapabilityReport::new("knowledge-provider", "not-configured")
            .field("reason", "generic-analysis-only"),
        Some(_) => match context.target.require_available_knowledge_provider() {
            Ok(provider) => {
                CapabilityReport::new("knowledge-provider", "available").field("id", provider)
            }
            Err(error) => {
                report.absorb(0, 1);
                CapabilityReport::new("knowledge-provider", "unavailable")
                    .field("error", error.to_string())
            }
        },
    };
    report.capability(capability);
}

fn collect_memory_map(context: &ProjectContext<'_>, report: &mut DoctorReport) {
    let capability = if let Some(memory_map) = context.memory_map {
        let mmio_regions = memory_map
            .regions
            .iter()
            .filter(|region| region.kind == MemoryRegionKind::Mmio)
            .count();
        if mmio_regions == 0 {
            report.warning("memory map has no MMIO regions");
        }
        CapabilityReport::new("memory-map", "available")
            .field("spaces", memory_map.address_spaces.len())
            .field("regions", memory_map.regions.len())
            .field("mmio-regions", mmio_regions)
            .field("default-space", memory_map.default_address_space.as_str())
    } else {
        report.absorb(0, 1);
        CapabilityReport::new("memory-map", "not-configured")
    };
    report.capability(capability);
}

fn collect_register_catalog(context: &ProjectContext<'_>, report: &mut DoctorReport) {
    let project_model = context.project.registers.as_ref().is_some_and(|paths| {
        paths.model.is_file() && RegisterModel::is_model_file(&paths.model).unwrap_or(false)
    });
    let capability = if context.svd_paths.is_empty() && !project_model {
        CapabilityReport::new("register-catalog", "not-configured")
            .field("registers", 0usize)
            .field("mmio-windows", context.svd.regions.len())
    } else {
        CapabilityReport::new("register-catalog", "available")
            .field("files", context.svd_paths.len())
            .field("project-model", project_model)
            .field("registers", context.svd.registers.len())
            .field("mmio-windows", context.svd.regions.len())
            .field(
                "paths",
                context
                    .svd_paths
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>(),
            )
    };
    report.capability(capability);
}

fn collect_symbol_inventory(context: &ProjectContext<'_>, report: &mut DoctorReport) {
    let capability = match &context.project.symbol_inventory {
        None => CapabilityReport::new("symbol-inventory", "not-configured"),
        Some(spec) if !spec.output.is_file() => {
            report.absorb(0, 1);
            CapabilityReport::new("symbol-inventory", "not-generated")
                .field("path", spec.output.display().to_string())
        }
        Some(spec) => match crate::artifacts::inspect_symbol_inventory(&spec.output) {
            Ok(summary) => CapabilityReport::new("symbol-inventory", "available")
                .field("artifacts", summary.artifacts)
                .field("symbol-facts", summary.symbol_facts)
                .field("exported-definitions", summary.exported_definitions)
                .field("undefined", summary.undefined)
                .field("unresolved-or-associated", summary.unresolved_or_associated)
                .field("executable-bytes", summary.executable_bytes)
                .field("symbol-covered-bytes", summary.symbol_covered_bytes)
                .field(
                    "uncovered-executable-bytes",
                    summary.uncovered_executable_bytes,
                )
                .field(
                    "named-zero-sized-code-symbols",
                    summary.named_zero_sized_code_symbols,
                )
                .field(
                    "function-boundary-candidates",
                    summary.function_boundary_candidates,
                )
                .field("code-recovery-blockers", summary.code_recovery_blockers)
                .field("path", spec.output.display().to_string()),
            Err(error) => {
                report.error();
                CapabilityReport::new("symbol-inventory", "invalid")
                    .field("path", spec.output.display().to_string())
                    .field("error", error.to_string())
            }
        },
    };
    report.capability(capability);
}

fn collect_navigation_index(context: &ProjectContext<'_>, report: &mut DoctorReport) {
    let capability = match &context.project.navigation_index {
        None => CapabilityReport::new("navigation-index", "not-configured"),
        Some(spec) if !spec.output.is_file() => {
            report.absorb(0, 1);
            CapabilityReport::new("navigation-index", "not-generated")
                .field("path", spec.output.display().to_string())
        }
        Some(spec) => match crate::navigation::inspect_report(&spec.output) {
            Ok(summary) => CapabilityReport::new("navigation-index", "available")
                .field("artifacts", summary.artifacts)
                .field("symbols", summary.symbols)
                .field("linked-ir-functions", summary.linked_ir_functions)
                .field("interface-callers", summary.interface_callers)
                .field("interface-roots", summary.interface_roots)
                .field(
                    "unmatched-interface-roots",
                    summary.unmatched_interface_roots,
                )
                .field("path", spec.output.display().to_string()),
            Err(error) => {
                report.error();
                CapabilityReport::new("navigation-index", "invalid")
                    .field("path", spec.output.display().to_string())
                    .field("error", error.to_string())
            }
        },
    };
    report.capability(capability);
}
