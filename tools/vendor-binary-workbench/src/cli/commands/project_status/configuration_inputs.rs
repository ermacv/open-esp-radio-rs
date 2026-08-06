//! Configuration and caller-owned input readiness.

use serde_json::{Value, json};

use super::{
    super::ProjectContext,
    model::{Component, Phase, Readiness},
};
use crate::{artifact, memory_map::MemoryRegionKind};

pub(super) fn configuration(context: &ProjectContext<'_>) -> Phase {
    let backend = match context.target.require_available_backend() {
        Ok(()) => Component::new("backend", Readiness::Ready)
            .detail("architecture", context.target.architecture.label())
            .detail(
                "calling_convention",
                context.target.calling_convention.label(),
            ),
        Err(error) => Component::new("backend", Readiness::Invalid).diagnostic(error),
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
                    Value::Array(
                        pack.semantic_catalogs
                            .iter()
                            .map(|path| Value::String(path.display().to_string()))
                            .collect(),
                    ),
                )
                .detail("semantic_operations", pack.semantic_operations)
        },
    );
    let harness = match &context.target.harness {
        None => Component::new("harness", Readiness::NotConfigured),
        Some(_) => match context.target.require_available_harness() {
            Ok(id) => Component::new("harness", Readiness::Ready).detail("id", id),
            Err(error) => Component::new("harness", Readiness::Invalid).diagnostic(error),
        },
    };
    let memory = match context.memory_map {
        None => Component::new("memory_map", Readiness::Incomplete)
            .diagnostic("project has no memory map"),
        Some(memory) => {
            let mmio = memory
                .regions
                .iter()
                .filter(|region| region.kind == MemoryRegionKind::Mmio)
                .map(|region| {
                    json!({
                        "name": region.name,
                        "address_space": region.address_space,
                        "start": region.start,
                        "end_exclusive": region.end,
                        "permissions": region.permissions,
                    })
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
            .detail("mmio", Value::Array(mmio))
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
        let status = if context.project.ir_profiles.is_empty() {
            Readiness::NotConfigured
        } else {
            Readiness::Incomplete
        };
        let mut component = Component::new("run_spec", status)
            .diagnostic("caller-owned artifact bindings are unavailable");
        if let Some(path) = context.run_spec_path {
            component = component.detail("path", path.display().to_string());
        }
        return Phase::collect("inputs", vec![component]);
    };

    let mut invalid = false;
    let mut incomplete = false;
    let mut ready = 0usize;
    let mut records = Vec::new();
    for (role, path) in run_spec.inputs() {
        let (state, detail) = if !path.is_file() {
            incomplete = true;
            ("missing", json!({"path": path.display().to_string()}))
        } else {
            match artifact::inspect_artifact(path) {
                Ok(inventory) => {
                    let symbols = inventory.symbols().count();
                    if symbols == 0 {
                        incomplete = true;
                    } else {
                        ready += 1;
                    }
                    (
                        if symbols == 0 {
                            "readable-no-symbols"
                        } else {
                            "ready"
                        },
                        json!({
                            "path": path.display().to_string(),
                            "container": inventory.container.label(),
                            "objects": inventory.objects.len(),
                            "skipped_members": inventory.skipped_members,
                            "symbol_facts": symbols,
                        }),
                    )
                }
                Err(error) => {
                    invalid = true;
                    (
                        "invalid",
                        json!({
                            "path": path.display().to_string(),
                            "error": error.to_string(),
                        }),
                    )
                }
            }
        };
        let mut record = detail.as_object().cloned().unwrap_or_default();
        record.insert("role".to_owned(), Value::String(role.to_owned()));
        record.insert("status".to_owned(), Value::String(state.to_owned()));
        records.push(Value::Object(record));
    }
    let status = if invalid {
        Readiness::Invalid
    } else if incomplete || records.is_empty() {
        Readiness::Incomplete
    } else {
        Readiness::Ready
    };
    Phase::collect(
        "inputs",
        vec![
            Component::new("artifacts", status)
                .detail(
                    "run_spec",
                    context
                        .run_spec_path
                        .map(|path| path.display().to_string())
                        .unwrap_or_default(),
                )
                .detail("configured", records.len())
                .detail("ready", ready)
                .detail("items", Value::Array(records)),
        ],
    )
}
