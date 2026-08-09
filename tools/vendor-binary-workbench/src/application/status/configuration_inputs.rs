//! Configuration and caller-owned input readiness.

use super::model::{ArtifactDetail, Component, MmioRegionDetail, Phase, Readiness};
use crate::application::ProjectContext;
use crate::{artifact, memory_map::MemoryRegionKind};

pub(super) fn configuration(context: &ProjectContext<'_>) -> Phase {
    let backend = match context.target.require_available_backend() {
        Ok(()) => Component::new("backend", Readiness::Ready)
            .detail("architecture", context.target.architecture.label())
            .detail(
                "calling_convention",
                context.target.calling_convention.label(),
            ),
        Err(error) => Component::new("backend", Readiness::Invalid)
            .diagnostic(error)
            .next_action(format!(
                "select a compiled backend in {}",
                context.project_path.display()
            )),
    };
    let platform = context.project.platform_pack.as_ref().map_or_else(
        || Component::new("platform_pack", Readiness::NotConfigured),
        |pack| {
            Component::new("platform_pack", Readiness::Ready)
                .detail("id", pack.id.clone())
                .detail("path", pack.path.display().to_string())
                .detail("semantic_catalogs", pack.semantic_catalogs.len())
                .detail(
                    "semantic_catalog_paths",
                    pack.semantic_catalogs
                        .iter()
                        .map(|path| path.display().to_string())
                        .collect::<Vec<_>>(),
                )
                .detail("semantic_operations", pack.semantic_operations)
        },
    );
    let harness = match &context.target.harness {
        None => Component::new("harness", Readiness::NotConfigured),
        Some(_) => match context.target.require_available_harness() {
            Ok(id) => Component::new("harness", Readiness::Ready).detail("id", id),
            Err(error) => Component::new("harness", Readiness::Invalid)
                .diagnostic(error)
                .next_action(format!(
                    "rebuild the workbench with the feature that registers this target harness; the target is selected by {}",
                    context.project_path.display()
                )),
        },
    };
    let memory = match context.memory_map {
        None => Component::new("memory_map", Readiness::Incomplete)
            .diagnostic("project has no memory map")
            .next_action(format!(
                "configure memory-map in {}",
                context.project_path.display()
            )),
        Some(memory) => {
            let mmio = memory
                .regions
                .iter()
                .filter(|region| region.kind == MemoryRegionKind::Mmio)
                .map(|region| MmioRegionDetail {
                    name: region.name.clone(),
                    address_space: region.address_space.clone(),
                    start: region.start,
                    end_exclusive: region.end,
                    permissions: region.permissions.clone(),
                })
                .collect::<Vec<_>>();
            Component::new(
                "memory_map",
                if mmio.is_empty() {
                    Readiness::Incomplete
                } else {
                    Readiness::Ready
                },
            )
            .detail("address_spaces", memory.address_spaces.len())
            .detail("regions", memory.regions.len())
            .detail("mmio_regions", mmio.len())
            .detail("mmio", mmio)
            .detail(
                "default_address_space",
                memory.default_address_space.clone(),
            )
        }
    };
    Phase::collect("configuration", vec![backend, platform, harness, memory])
}

pub(super) fn inputs(context: &ProjectContext<'_>) -> Phase {
    let Some(run_spec) = context.run_spec else {
        let has_artifact_analysis = context.project.symbol_inventory.is_some()
            || !context.project.ir_profiles.is_empty()
            || context.project.registers.is_some()
            || context.project.interfaces.is_some();
        let status = if has_artifact_analysis {
            Readiness::Incomplete
        } else {
            Readiness::NotConfigured
        };
        let mut component = Component::new("run_spec", status)
            .diagnostic("caller-owned artifact bindings are unavailable")
            .next_action(format!(
                "vendor-binary-workbench project inputs init --project {}",
                context.project_path.display()
            ));
        if let Some(path) = context.run_spec_path {
            component = component.detail("path", path.display().to_string());
        }
        return Phase::collect("inputs", vec![component]);
    };

    let mut invalid = false;
    let mut incomplete = false;
    let mut ready = 0usize;
    let mut records = Vec::new();
    for input in run_spec.inputs() {
        let record = if !input.path.is_file() {
            incomplete = true;
            ArtifactDetail {
                role: input.role.to_string(),
                status: "missing",
                path: input.path.display().to_string(),
                container: None,
                objects: None,
                skipped_members: None,
                symbol_facts: None,
                error: None,
            }
        } else {
            match artifact::inspect_artifact(&input.path) {
                Ok(inventory) => {
                    let symbols = inventory.symbols().count();
                    if symbols == 0 {
                        incomplete = true;
                    } else {
                        ready += 1;
                    }
                    ArtifactDetail {
                        role: input.role.to_string(),
                        status: if symbols == 0 {
                            "readable-no-symbols"
                        } else {
                            "ready"
                        },
                        path: input.path.display().to_string(),
                        container: Some(inventory.container.label()),
                        objects: Some(inventory.objects.len()),
                        skipped_members: Some(inventory.skipped_members),
                        symbol_facts: Some(symbols),
                        error: None,
                    }
                }
                Err(error) => {
                    invalid = true;
                    ArtifactDetail {
                        role: input.role.to_string(),
                        status: "invalid",
                        path: input.path.display().to_string(),
                        container: None,
                        objects: None,
                        skipped_members: None,
                        symbol_facts: None,
                        error: Some(error.to_string()),
                    }
                }
            }
        };
        records.push(record);
    }
    let status = if invalid {
        Readiness::Invalid
    } else if incomplete || records.is_empty() {
        Readiness::Incomplete
    } else {
        Readiness::Ready
    };
    let mut component = Component::new("artifacts", status)
        .detail(
            "run_spec",
            context
                .run_spec_path
                .map(|path| path.display().to_string())
                .unwrap_or_default(),
        )
        .detail("configured", records.len())
        .detail("ready", ready)
        .detail("items", records);
    if status != Readiness::Ready {
        component = component.next_action(format!(
            "repair local artifact bindings with `vendor-binary-workbench project inputs init --check --project {}`",
            context.project_path.display()
        ));
    }
    Phase::collect("inputs", vec![component])
}
