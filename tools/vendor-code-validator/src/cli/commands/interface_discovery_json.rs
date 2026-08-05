//! Stable JSON projection of generic indirect-call evidence.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

use serde_json::{Value, json};

use super::{
    super::*,
    interface_discovery::{DiscoveredCall, Discovery, root_linkage},
};
use crate::{
    analysis::LinkageSymbolLocation,
    interface_discovery::{
        InterfaceArgumentValue, InterfaceCallCandidate, InterfaceCallKind, InterfaceRoot,
    },
};

fn root_json(root: &InterfaceRoot) -> Value {
    match root {
        InterfaceRoot::RelocatedSymbol {
            member,
            symbol,
            addend,
            addressing,
        } => json!({
            "kind": root.kind(),
            "canonical": root.canonical(),
            "member": member,
            "symbol": symbol,
            "addend": addend,
            "addressing": addressing.label(),
        }),
        InterfaceRoot::FunctionArgument { index } => json!({
            "kind": root.kind(),
            "canonical": root.canonical(),
            "argument": index,
        }),
        InterfaceRoot::AbsoluteAddress { address } => json!({
            "kind": root.kind(),
            "canonical": root.canonical(),
            "address": format!("{address:#010x}"),
        }),
    }
}

fn location_json(location: &LinkageSymbolLocation) -> Value {
    json!({
        "artifact": location.artifact,
        "member": location.member,
        "address": format!("{:#x}", location.address),
        "kind": location.kind.label(),
    })
}

fn argument_json(index: usize, value: &InterfaceArgumentValue) -> Value {
    match value {
        InterfaceArgumentValue::Unknown => json!({"index": index, "kind": "unknown"}),
        InterfaceArgumentValue::Constant(value) => json!({
            "index": index,
            "kind": "constant",
            "value": format!("{value:#010x}"),
        }),
        InterfaceArgumentValue::Pointer(pointer) => json!({
            "index": index,
            "kind": "pointer-provenance",
            "canonical": pointer.canonical(),
            "root": root_json(&pointer.root),
            "loads": pointer.loads.iter().map(|load| json!({
                "site": format!("{:#x}", load.site),
                "offset": load.offset,
                "width": load.width,
            })).collect::<Vec<_>>(),
            "post_offset": pointer.post_offset,
        }),
    }
}

fn call_json(discovery: &Discovery, discovered: &DiscoveredCall) -> Value {
    let call = &discovered.call;
    let linkage = root_linkage(discovery, discovered.artifact, &call.target.root);
    json!({
        "artifact": discovered.artifact,
        "member": call.member,
        "function": call.function,
        "function_address": format!("{:#x}", call.function_address),
        "site": format!("{:#x}", call.site),
        "kind": call.kind.label(),
        "link_register": match call.kind {
            InterfaceCallKind::Call => Some(1),
            InterfaceCallKind::TailJump => Some(0),
            InterfaceCallKind::LinkedJump(register) => Some(register),
        },
        "target": {
            "canonical": call.target.canonical(),
            "root": root_json(&call.target.root),
            "loads": call.target.loads.iter().map(|load| json!({
                "site": format!("{:#x}", load.site),
                "offset": load.offset,
                "width": load.width,
            })).collect::<Vec<_>>(),
            "container_depth": call.target.container_loads().len(),
            "slot_offset": call.target.slot().map(|load| load.offset),
            "jalr_offset": call.jalr_offset,
        },
        "root_linkage": {
            "mode": "association-only",
            "symbols": linkage.symbols,
            "resolutions": linkage.resolutions,
            "candidates": linkage.candidates.iter().map(location_json).collect::<Vec<_>>(),
        },
        "arguments": call.arguments.iter().enumerate().map(|(index, value)| argument_json(index, value)).collect::<Vec<_>>(),
    })
}

fn table_groups_json(discovery: &Discovery) -> Vec<Value> {
    type GroupKey = (usize, InterfaceRoot, Vec<(i32, u8)>);
    let mut groups = BTreeMap::<GroupKey, Vec<&InterfaceCallCandidate>>::new();
    for discovered in &discovery.calls {
        if discovered.call.target.loads.is_empty() {
            continue;
        }
        groups
            .entry((
                discovered.artifact,
                discovered.call.target.root.clone(),
                discovered
                    .call
                    .target
                    .container_loads()
                    .iter()
                    .map(|load| (load.offset, load.width))
                    .collect(),
            ))
            .or_default()
            .push(&discovered.call);
    }
    groups
        .into_iter()
        .map(|((artifact, root, container), calls)| {
            let slots = calls
                .iter()
                .filter_map(|call| call.target.slot())
                .map(|slot| (slot.offset, slot.width))
                .collect::<BTreeSet<_>>();
            let functions = calls
                .iter()
                .map(|call| call.function.clone())
                .collect::<BTreeSet<_>>();
            json!({
                "artifact": artifact,
                "root": root_json(&root),
                "container_path": container.iter().map(|(offset, width)| json!({"offset": offset, "width": width})).collect::<Vec<_>>(),
                "slots": slots.iter().map(|(offset, width)| json!({"offset": offset, "width": width})).collect::<Vec<_>>(),
                "functions": functions,
                "call_sites": calls.len(),
            })
        })
        .collect()
}

pub(super) fn write_json_report(path: &Path, discovery: &Discovery) -> Result<()> {
    let document = json!({
        "schema_version": 1,
        "command": "interfaces-discover",
        "analysis_scope": {
            "architecture": "riscv32",
            "calling_convention": "riscv-ilp32",
            "evidence": "control-flow-merged register provenance",
            "relocation_evidence": ["absolute", "pc-relative", "got"],
            "semantic_claim": false,
            "table_layout_claim": false,
            "linker_resolution_claim": false,
            "completeness_claim": false,
        },
        "artifacts": discovery.linkage.artifacts.iter().enumerate().map(|(index, artifact)| json!({
            "index": index,
            "path": artifact.path,
            "roles": artifact.roles,
            "sources": artifact.sources,
            "container": artifact.container.label(),
            "functions": discovery.functions[index],
        })).collect::<Vec<_>>(),
        "calls": discovery.calls.iter().map(|call| call_json(discovery, call)).collect::<Vec<_>>(),
        "table_candidates": table_groups_json(discovery),
        "decode_failures": discovery.failures.iter().map(|failure| json!({
            "artifact": failure.artifact,
            "member": failure.member,
            "function": failure.function,
            "error": failure.error,
        })).collect::<Vec<_>>(),
    });
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, serde_json::to_string_pretty(&document)? + "\n")?;
    Ok(())
}
