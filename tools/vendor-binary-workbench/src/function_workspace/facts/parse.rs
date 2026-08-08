//! Schema-v35 linked-IR projection into function-review facts.

use std::collections::BTreeSet;

use serde_json::{Map, Value};

use crate::Result;

use super::{
    FunctionCallFact, FunctionContextFieldFact, FunctionFact, FunctionInputFact,
    FunctionMemoryFieldFact, FunctionMemoryObjectFact, ScenarioArgumentFact, ScenarioMmioReadFact,
    ScenarioSuggestionFact, ScenarioSuggestionVariantFact,
    json::{
        array, boolean, count, hex_u32, integer, object, optional_hex_u32, optional_string, sha256,
        signed, string, strings,
    },
};

pub(super) fn parse_report(
    profile: &str,
    input: &str,
) -> Result<(Vec<FunctionInputFact>, Vec<FunctionFact>)> {
    let root: Value = serde_json::from_str(input)?;
    let root = object(&root, "linked-IR root")?;
    let context = format!("linked-IR profile {profile:?}");
    if integer(root, "schema_version", &context)? != u64::from(crate::artifacts::LINKED_IR.version)
        || string(root, "command", &context)? != crate::artifacts::LINKED_IR.command
    {
        return Err(crate::Error::invalid(format!(
            "function workspace requires a schema-v{} ir export report for profile {profile:?}",
            crate::artifacts::LINKED_IR.version
        )));
    }
    if boolean(root, "completeness_claim", &context)? {
        return Err(crate::Error::invalid(format!(
            "linked-IR profile {profile:?} makes an unsupported completeness claim"
        )));
    }
    Ok((
        parse_inputs(profile, root, &context)?,
        parse_functions(profile, root, &context)?,
    ))
}

fn parse_inputs(
    profile: &str,
    root: &Map<String, Value>,
    context: &str,
) -> Result<Vec<FunctionInputFact>> {
    array(root, "artifacts", context)?
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let context = format!("{context}.artifacts[{index}]");
            let artifact = object(value, &context)?;
            let evidence = object(
                artifact
                    .get("artifact")
                    .ok_or_else(|| format!("{context} requires artifact evidence"))
                    .map_err(crate::Error::invalid)?,
                &format!("{context}.artifact"),
            )?;
            Ok(FunctionInputFact {
                profile: profile.to_owned(),
                source: string(artifact, "source", &context)?.to_owned(),
                sha256: sha256(evidence, "sha256", &context)?.to_owned(),
            })
        })
        .collect()
}

fn parse_functions(
    profile: &str,
    root: &Map<String, Value>,
    context: &str,
) -> Result<Vec<FunctionFact>> {
    array(root, "functions", context)?
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let context = format!("{context}.functions[{index}]");
            let function = object(value, &context)?;
            let summary = object(
                function
                    .get("effect_summary")
                    .ok_or_else(|| format!("{context} requires effect_summary"))
                    .map_err(crate::Error::invalid)?,
                &format!("{context}.effect_summary"),
            )?;
            Ok(FunctionFact {
                profile: profile.to_owned(),
                source: string(function, "source", &context)?.to_owned(),
                identity: string(function, "identity", &context)?.to_owned(),
                member: optional_string(function, "member", &context)?,
                symbol: string(function, "symbol", &context)?.to_owned(),
                selection: string(function, "selection", &context)?.to_owned(),
                direct_complete: boolean(function, "complete", &context)?,
                call_graph_closed: boolean(summary, "call_graph_closed", &context)?,
                context_projection_complete: boolean(
                    summary,
                    "context_projection_complete",
                    &context,
                )?,
                context_projection_blockers: strings(
                    summary,
                    "context_projection_blockers",
                    &context,
                )?,
                reachable_functions: strings(summary, "reachable_functions", &context)?,
                calls: parse_calls(function, &context)?,
                mmio_addresses: array(function, "mmio_accesses", &context)?
                    .iter()
                    .enumerate()
                    .map(|(access_index, value)| {
                        let access_context = format!("{context}.mmio_accesses[{access_index}]");
                        hex_u32(object(value, &access_context)?, "address", &access_context)
                    })
                    .collect::<Result<BTreeSet<_>>>()?
                    .into_iter()
                    .collect(),
                context_fields: parse_fields(summary, &context)?,
                memory_fields: parse_memory_fields(summary, &context)?,
                semantic_operations: array(summary, "semantic_operations", &context)?
                    .iter()
                    .enumerate()
                    .map(|(operation_index, value)| {
                        let operation_context =
                            format!("{context}.semantic_operations[{operation_index}]");
                        Ok(string(
                            object(value, &operation_context)?,
                            "operation",
                            &operation_context,
                        )?
                        .to_owned())
                    })
                    .collect::<Result<Vec<_>>>()?,
                trampoline_calls: array(summary, "trampoline_calls", &context)?.len(),
                event_dispatches: array(summary, "event_dispatches", &context)?.len(),
                scenario_suggestions: parse_scenario_suggestions(function, &context)?,
                pseudo: string(function, "pseudo", &context)?.to_owned(),
            })
        })
        .collect()
}

fn parse_scenario_suggestions(
    function: &Map<String, Value>,
    context: &str,
) -> Result<Vec<ScenarioSuggestionFact>> {
    array(function, "scenario_suggestions", context)?
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let context = format!("{context}.scenario_suggestions[{index}]");
            let suggestion = object(value, &context)?;
            let variants = array(suggestion, "variants", &context)?
                .iter()
                .enumerate()
                .map(|(variant_index, value)| {
                    let context = format!("{context}.variants[{variant_index}]");
                    let variant = object(value, &context)?;
                    let arguments = array(variant, "arguments", &context)?
                        .iter()
                        .enumerate()
                        .map(|(argument_index, value)| {
                            let context = format!("{context}.arguments[{argument_index}]");
                            let argument = object(value, &context)?;
                            Ok(ScenarioArgumentFact {
                                index: integer(argument, "index", &context)?.try_into().map_err(
                                    |_| {
                                        crate::Error::invalid(format!(
                                            "invalid argument index in {context}"
                                        ))
                                    },
                                )?,
                                value: hex_u32(argument, "value", &context)?,
                            })
                        })
                        .collect::<Result<Vec<_>>>()?;
                    let mmio_reads = array(variant, "mmio_reads", &context)?
                        .iter()
                        .enumerate()
                        .map(|(read_index, value)| {
                            let context = format!("{context}.mmio_reads[{read_index}]");
                            let read = object(value, &context)?;
                            let values = array(read, "values", &context)?
                                .iter()
                                .enumerate()
                                .map(|(value_index, value)| {
                                    value
                                        .as_u64()
                                        .and_then(|value| value.try_into().ok())
                                        .ok_or_else(|| {
                                            crate::Error::invalid(format!(
                                                "{context}.values[{value_index}] must be a u32"
                                            ))
                                        })
                                })
                                .collect::<Result<Vec<_>>>()?;
                            Ok(ScenarioMmioReadFact {
                                address: hex_u32(read, "address", &context)?,
                                mask: hex_u32(read, "mask", &context)?,
                                expected: hex_u32(read, "expected", &context)?,
                                values,
                            })
                        })
                        .collect::<Result<Vec<_>>>()?;
                    Ok(ScenarioSuggestionVariantFact {
                        name: string(variant, "name", &context)?.to_owned(),
                        arguments,
                        mmio_reads,
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            Ok(ScenarioSuggestionFact {
                kind: string(suggestion, "kind", &context)?.to_owned(),
                site: optional_hex_u32(suggestion, "site", &context)?,
                evidence: string(suggestion, "evidence", &context)?.to_owned(),
                variants,
            })
        })
        .collect()
}

fn parse_calls(function: &Map<String, Value>, context: &str) -> Result<Vec<FunctionCallFact>> {
    array(function, "calls", context)?
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let context = format!("{context}.calls[{index}]");
            let call = object(value, &context)?;
            Ok(FunctionCallFact {
                kind: string(call, "kind", &context)?.to_owned(),
                target: string(call, "target", &context)?.to_owned(),
                semantic_operation: optional_string(call, "semantic_operation", &context)?,
                site: optional_hex_u32(call, "site", &context)?,
                arguments: strings(call, "arguments", &context)?,
                guard_paths: optional_guard_paths(call, &context)?,
            })
        })
        .collect()
}

fn optional_guard_paths(call: &Map<String, Value>, context: &str) -> Result<Option<Vec<String>>> {
    let Some(paths) = call.get("cfg_guard_paths") else {
        return Err(crate::Error::invalid(format!(
            "{context} requires cfg_guard_paths"
        )));
    };
    if paths.is_null() {
        return Ok(None);
    }
    let paths = paths
        .as_array()
        .ok_or_else(|| format!("{context}.cfg_guard_paths must be an array or null"))
        .map_err(crate::Error::invalid)?;
    paths
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let context = format!("{context}.cfg_guard_paths[{index}]");
            let path = object(value, &context)?;
            let guards = array(path, "guards", &context)?;
            let literals = guards
                .iter()
                .enumerate()
                .map(|(guard_index, guard)| {
                    let guard_context = format!("{context}.guards[{guard_index}]");
                    let guard = object(guard, &guard_context)?;
                    let condition = string(guard, "condition", &guard_context)?;
                    Ok(if boolean(guard, "taken", &guard_context)? {
                        format!("({condition})")
                    } else {
                        format!("!({condition})")
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            Ok(if literals.is_empty() {
                "true".to_owned()
            } else {
                literals.join(" && ")
            })
        })
        .collect::<Result<Vec<_>>>()
        .map(Some)
}

fn parse_fields(
    summary: &Map<String, Value>,
    context: &str,
) -> Result<Vec<FunctionContextFieldFact>> {
    array(summary, "context_fields", context)?
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let context = format!("{context}.context_fields[{index}]");
            let field = object(value, &context)?;
            Ok(FunctionContextFieldFact {
                argument: integer(field, "argument", &context)?
                    .try_into()
                    .map_err(|_| format!("invalid argument in {context}"))
                    .map_err(crate::Error::invalid)?,
                offset: signed(field, "offset", &context)?
                    .try_into()
                    .map_err(|_| format!("invalid offset in {context}"))
                    .map_err(crate::Error::invalid)?,
                width: integer(field, "width", &context)?
                    .try_into()
                    .map_err(|_| format!("invalid width in {context}"))
                    .map_err(crate::Error::invalid)?,
                reads: count(field, "reads", &context)?,
                writes: count(field, "writes", &context)?,
                write_mask: hex_u32(field, "write_mask", &context)?,
            })
        })
        .collect()
}

fn parse_memory_fields(
    summary: &Map<String, Value>,
    context: &str,
) -> Result<Vec<FunctionMemoryFieldFact>> {
    array(summary, "memory_fields", context)?
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let context = format!("{context}.memory_fields[{index}]");
            let field = object(value, &context)?;
            let object_context = format!("{context}.object");
            let object = object(
                field
                    .get("object")
                    .ok_or_else(|| format!("{context} requires object"))
                    .map_err(crate::Error::invalid)?,
                &object_context,
            )?;
            let object = match string(object, "kind", &object_context)? {
                "argument" => FunctionMemoryObjectFact::Argument {
                    index: integer(object, "index", &object_context)?
                        .try_into()
                        .map_err(|_| format!("invalid argument index in {object_context}"))
                        .map_err(crate::Error::invalid)?,
                },
                "global" => FunctionMemoryObjectFact::Global {
                    member: optional_string(object, "member", &object_context)?,
                    symbol: string(object, "symbol", &object_context)?.to_owned(),
                },
                "dereferenced-global" => FunctionMemoryObjectFact::DereferencedGlobal {
                    member: optional_string(object, "member", &object_context)?,
                    symbol: string(object, "symbol", &object_context)?.to_owned(),
                    pointer_offset: signed(object, "pointer_offset", &object_context)?,
                },
                "absolute" => FunctionMemoryObjectFact::Absolute {
                    address_space: string(object, "address_space", &object_context)?.to_owned(),
                    address: hex_u32(object, "address", &object_context)?,
                },
                kind => {
                    return Err(crate::Error::invalid(format!(
                        "unsupported memory object kind {kind:?} in {object_context}"
                    )));
                }
            };
            Ok(FunctionMemoryFieldFact {
                object,
                offset: signed(field, "offset", &context)?,
                width: integer(field, "width", &context)?
                    .try_into()
                    .map_err(|_| format!("invalid width in {context}"))
                    .map_err(crate::Error::invalid)?,
                reads: count(field, "reads", &context)?,
                writes: count(field, "writes", &context)?,
                write_mask: hex_u32(field, "write_mask", &context)?,
            })
        })
        .collect()
}
