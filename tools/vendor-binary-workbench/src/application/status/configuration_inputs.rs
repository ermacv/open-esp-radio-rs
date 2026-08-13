//! Configuration and caller-owned input readiness.

use std::{collections::BTreeMap, path::PathBuf};

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
    // One binary is commonly bound under several roles (for example a linked
    // ELF is both a source artifact and a companion). Probe each physical path
    // once; role readiness still remains explicit in the report.
    let mut probes = BTreeMap::<PathBuf, std::result::Result<&'static str, String>>::new();
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
            let probe = probes.entry(input.path.clone()).or_insert_with(|| {
                artifact::inspect_artifact_container(&input.path)
                    .map(|container| container.label())
                    .map_err(|error| error.to_string())
            });
            match probe {
                Ok(container) => {
                    ready += 1;
                    ArtifactDetail {
                        role: input.role.to_string(),
                        status: "ready",
                        path: input.path.display().to_string(),
                        container: Some(*container),
                        objects: None,
                        skipped_members: None,
                        symbol_facts: None,
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
    let next_action = (status != Readiness::Ready)
        .then(|| input_repair_action(context.run_spec_path, &records, context.project_path));
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
    if let Some(next_action) = next_action {
        component = component.next_action(next_action);
    }
    Phase::collect("inputs", vec![component])
}

fn input_repair_action(
    run_spec_path: Option<&std::path::Path>,
    records: &[ArtifactDetail],
    project_path: &std::path::Path,
) -> String {
    let binding = run_spec_path
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "the local run spec".to_owned());
    if let Some(first) = records.iter().find(|record| record.status == "missing") {
        let missing = records
            .iter()
            .filter(|record| record.status == "missing")
            .count();
        return format!(
            "rebuild or restore {missing} artifact(s) already bound in {binding}; first: {} -> {}",
            first.role, first.path
        );
    }
    if let Some(first) = records.iter().find(|record| record.status == "invalid") {
        return format!(
            "replace the invalid artifact already bound in {binding}: {} -> {}",
            first.role, first.path
        );
    }
    if records.is_empty() {
        return format!(
            "bind the project inputs with `vendor-binary-workbench project inputs init --project {}`",
            project_path.display()
        );
    }
    format!("restore usable symbols in the artifacts bound by {binding}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn missing_bound_artifact_requests_rebuild_instead_of_binding_check() {
        let records = vec![ArtifactDetail {
            role: "source-artifact:libpp".to_owned(),
            status: "missing",
            path: "/tmp/vendor-linked-libpp".to_owned(),
            container: None,
            objects: None,
            skipped_members: None,
            symbol_facts: None,
            error: None,
        }];
        let action = input_repair_action(
            Some(Path::new("targets/chip/local.toml")),
            &records,
            Path::new("targets/chip/vendor-project.toml"),
        );
        assert!(action.contains("rebuild or restore"));
        assert!(action.contains("source-artifact:libpp -> /tmp/vendor-linked-libpp"));
        assert!(action.contains("targets/chip/local.toml"));
        assert!(!action.contains("inputs init --check"));
    }
}
