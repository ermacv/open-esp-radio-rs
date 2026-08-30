//! Current linked-IR projection into function-review facts.

use std::collections::BTreeSet;

use crate::{Result, artifacts::StoredMemoryObject};

use super::{
    FunctionCallFact, FunctionContextFieldFact, FunctionDecodeBlockerFact,
    FunctionEventDispatchFact, FunctionFact, FunctionInputFact, FunctionMemoryFieldFact,
    FunctionMemoryObjectFact, FunctionMemoryWriteFact, ScenarioArgumentFact, ScenarioMmioReadFact,
    ScenarioSuggestionFact, ScenarioSuggestionVariantFact,
};

pub(super) fn parse_document(
    profile: &str,
    document: crate::artifacts::LinkedIrStoredDocument,
) -> Result<(Vec<FunctionInputFact>, Vec<FunctionFact>)> {
    let inputs = document
        .artifacts
        .into_iter()
        .map(|artifact| {
            validate_sha256(&artifact.artifact.sha256)?;
            Ok(FunctionInputFact {
                profile: profile.to_owned(),
                source: artifact.source,
                sha256: artifact.artifact.sha256,
            })
        })
        .collect::<Result<_>>()?;
    let functions = document
        .functions
        .into_iter()
        .map(|function| Ok(parse_function(profile, function)))
        .collect::<Result<_>>()?;
    Ok((inputs, functions))
}

pub(super) fn parse_function(
    profile: &str,
    function: crate::artifacts::StoredFunction,
) -> FunctionFact {
    let summary = function.effect_summary;
    FunctionFact {
        profile: profile.to_owned(),
        source: function.source,
        identity: function.identity,
        member: function.member,
        symbol: function.symbol,
        address: function.address,
        selection: function.selection,
        body_complete: function.completeness.body_complete,
        call_targets_complete: function.completeness.call_targets_complete,
        transitive_effects_complete: function.completeness.transitive_effects_complete,
        executable_complete: function.completeness.executable_complete,
        transitive_effects_materialized: summary.transitive_effects_materialized,
        call_graph_closed: summary.call_graph_closed,
        context_projection_materialized: summary.context_projection_materialized,
        context_projection_complete: summary.context_projection_complete,
        context_projection_blockers: summary.context_projection_blockers,
        decode_blockers: function
            .decode_blockers
            .into_iter()
            .map(|blocker| FunctionDecodeBlockerFact {
                address: blocker.address,
                width: blocker.width,
                raw: blocker.raw,
                class: blocker.class,
                linear_control_flow: blocker.linear_control_flow,
            })
            .collect(),
        direct_calls: function.calls.len(),
        calls: function.calls.into_iter().map(call_fact).collect(),
        memory_writes: function
            .instruction_effects
            .into_iter()
            .filter_map(|effect| match effect {
                crate::artifacts::StoredInstructionEffect::Memory {
                    site,
                    access,
                    width,
                    object,
                    offset,
                    value,
                    ..
                } if access == "write" => Some(FunctionMemoryWriteFact {
                    site,
                    object: memory_object_fact(object),
                    offset,
                    width,
                    value,
                }),
                _ => None,
            })
            .collect(),
        mmio_addresses: function
            .mmio_accesses
            .into_iter()
            .map(|access| access.address)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect(),
        context_fields: summary
            .context_fields
            .into_iter()
            .map(|field| FunctionContextFieldFact {
                argument: field.argument,
                offset: field.offset,
                width: field.width,
                reads: field.reads,
                writes: field.writes,
                write_mask: field.write_mask,
            })
            .collect(),
        memory_fields: summary
            .memory_fields
            .into_iter()
            .map(|field| FunctionMemoryFieldFact {
                object: memory_object_fact(field.object),
                offset: field.offset,
                width: field.width,
                reads: field.reads,
                writes: field.writes,
                write_mask: field.write_mask,
                origins: field.origins,
            })
            .collect(),
        semantic_operations: summary
            .semantic_operations
            .into_iter()
            .map(|operation| operation.operation)
            .collect(),
        trampoline_calls: summary.trampoline_calls.len(),
        event_dispatches: summary
            .event_dispatches
            .into_iter()
            .map(|dispatch| FunctionEventDispatchFact {
                mechanism: dispatch.mechanism,
                execution_context: dispatch.execution_context,
                receiver: dispatch.receiver,
                interface_complete: dispatch.interface_complete,
                bindings: dispatch
                    .bindings
                    .into_iter()
                    .map(|binding| (binding.role, binding.argument.value().to_owned()))
                    .collect(),
            })
            .collect(),
        scenario_suggestions: function
            .scenario_suggestions
            .into_iter()
            .map(|suggestion| ScenarioSuggestionFact {
                kind: suggestion.kind,
                site: suggestion.site,
                evidence: suggestion.evidence,
                variants: suggestion
                    .variants
                    .into_iter()
                    .map(|variant| ScenarioSuggestionVariantFact {
                        name: variant.name,
                        arguments: variant
                            .arguments
                            .into_iter()
                            .map(|argument| ScenarioArgumentFact {
                                index: argument.index,
                                value: argument.value,
                            })
                            .collect(),
                        mmio_reads: variant
                            .mmio_reads
                            .into_iter()
                            .map(|read| ScenarioMmioReadFact {
                                address: read.address,
                                mask: read.mask,
                                expected: read.expected,
                                values: read.values,
                            })
                            .collect(),
                    })
                    .collect(),
            })
            .collect(),
        pseudo: function.pseudo,
    }
}

pub(super) fn parse_review_projection(
    profile: &str,
    projection: crate::artifacts::LinkedIrReviewProjection,
) -> Result<(Vec<FunctionInputFact>, Vec<FunctionFact>)> {
    let inputs = projection
        .inputs
        .into_iter()
        .map(|(source, sha256)| {
            validate_sha256(&sha256)?;
            Ok(FunctionInputFact {
                profile: profile.to_owned(),
                source,
                sha256,
            })
        })
        .collect::<Result<_>>()?;
    let functions = projection
        .functions
        .into_iter()
        .map(|function| {
            let summary = function.effect_summary;
            Ok(FunctionFact {
                profile: profile.to_owned(),
                source: function.source,
                identity: function.identity,
                member: function.member,
                symbol: function.symbol,
                address: None,
                selection: function.selection,
                body_complete: function.completeness.body_complete,
                call_targets_complete: function.completeness.call_targets_complete,
                transitive_effects_complete: function.completeness.transitive_effects_complete,
                executable_complete: function.completeness.executable_complete,
                transitive_effects_materialized: summary.transitive_effects_materialized,
                call_graph_closed: summary.call_graph_closed,
                context_projection_materialized: summary.context_projection_materialized,
                context_projection_complete: summary.context_projection_complete,
                context_projection_blockers: summary.context_projection_blockers,
                decode_blockers: function
                    .decode_blockers
                    .into_iter()
                    .map(|blocker| FunctionDecodeBlockerFact {
                        address: blocker.address,
                        width: blocker.width,
                        raw: blocker.raw,
                        class: blocker.class,
                        linear_control_flow: blocker.linear_control_flow,
                    })
                    .collect(),
                direct_calls: function.direct_calls,
                calls: function
                    .calls
                    .into_iter()
                    .map(|call| FunctionCallFact {
                        kind: call.kind,
                        target: call.target,
                        direct: call.direct,
                        result_modeled: false,
                        semantic_operation: call.semantic_operation,
                        site: call.site,
                        arguments: Vec::new(),
                        argument_exact: Vec::new(),
                        guard_paths: None,
                    })
                    .collect(),
                memory_writes: Vec::new(),
                mmio_addresses: function.mmio_addresses,
                context_fields: summary
                    .context_fields
                    .into_iter()
                    .map(|field| FunctionContextFieldFact {
                        argument: field.argument,
                        offset: field.offset,
                        width: field.width,
                        reads: field.reads,
                        writes: field.writes,
                        write_mask: field.write_mask,
                    })
                    .collect(),
                memory_fields: summary
                    .memory_fields
                    .into_iter()
                    .map(|field| FunctionMemoryFieldFact {
                        object: memory_object_fact(field.object),
                        offset: field.offset,
                        width: field.width,
                        reads: field.reads,
                        writes: field.writes,
                        write_mask: field.write_mask,
                        origins: field.origins,
                    })
                    .collect(),
                semantic_operations: summary.semantic_operations.into_iter().collect(),
                trampoline_calls: summary.trampoline_calls,
                event_dispatches: summary
                    .event_dispatches
                    .into_iter()
                    .map(|dispatch| FunctionEventDispatchFact {
                        mechanism: dispatch.mechanism,
                        execution_context: dispatch.execution_context,
                        receiver: dispatch.receiver,
                        interface_complete: dispatch.interface_complete,
                        bindings: dispatch
                            .bindings
                            .into_iter()
                            .map(|binding| (binding.role, binding.value))
                            .collect(),
                    })
                    .collect(),
                scenario_suggestions: Vec::new(),
                pseudo: String::new(),
            })
        })
        .collect::<Result<_>>()?;
    Ok((inputs, functions))
}

fn call_fact(call: crate::artifacts::StoredCall) -> FunctionCallFact {
    let direct = call.direct();
    let result_modeled = call.result_modeled();
    let argument_exact = (0..call.arguments.len())
        .map(|position| call.argument_is_exact(position))
        .collect();
    FunctionCallFact {
        kind: call.kind,
        target: call.target,
        direct,
        result_modeled,
        semantic_operation: call.semantic_operation,
        site: call.site,
        arguments: call.arguments,
        argument_exact,
        guard_paths: call.guard_paths.map(|paths| {
            paths
                .into_iter()
                .map(|path| {
                    let literals = path
                        .guards
                        .into_iter()
                        .map(|guard| {
                            if guard.taken {
                                format!("({})", guard.condition)
                            } else {
                                format!("!({})", guard.condition)
                            }
                        })
                        .collect::<Vec<_>>();
                    if literals.is_empty() {
                        "true".to_owned()
                    } else {
                        literals.join(" && ")
                    }
                })
                .collect()
        }),
    }
}

fn memory_object_fact(object: StoredMemoryObject) -> FunctionMemoryObjectFact {
    match object {
        StoredMemoryObject::Argument { index } => FunctionMemoryObjectFact::Argument { index },
        StoredMemoryObject::Global { member, symbol } => {
            FunctionMemoryObjectFact::Global { member, symbol }
        }
        StoredMemoryObject::Dereferenced {
            pointer,
            pointer_offset,
        } => FunctionMemoryObjectFact::Dereferenced {
            pointer: Box::new(memory_object_fact(*pointer)),
            pointer_offset,
        },
        StoredMemoryObject::Absolute {
            address_space,
            address,
        } => FunctionMemoryObjectFact::Absolute {
            address_space,
            address,
        },
        StoredMemoryObject::Indexed {
            object,
            argument,
            stride,
        } => FunctionMemoryObjectFact::Indexed {
            object: Box::new(memory_object_fact(*object)),
            argument,
            stride,
        },
        StoredMemoryObject::Allocation { call_token } => {
            FunctionMemoryObjectFact::Allocation { call_token }
        }
        StoredMemoryObject::ZeroedAllocation { call_token } => {
            FunctionMemoryObjectFact::ZeroedAllocation { call_token }
        }
        StoredMemoryObject::OpaqueExternalObject { call_token } => {
            FunctionMemoryObjectFact::OpaqueExternalObject { call_token }
        }
    }
}

fn validate_sha256(value: &str) -> Result<()> {
    if value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        Ok(())
    } else {
        Err(crate::Error::invalid(
            "invalid lowercase SHA-256 in linked-IR artifact",
        ))
    }
}
