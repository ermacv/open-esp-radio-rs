//! Exact static call-site links from validated interface bindings into function review.

use std::collections::{BTreeMap, BTreeSet};

use super::{FunctionFact, FunctionWorkspace};
use crate::{Result, interfaces::ResolvedInterfaceSlot};

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct FunctionInterfaceIrCall {
    pub(crate) kind: String,
    pub(crate) target: String,
    pub(crate) semantic_operation: Option<String>,
    pub(crate) arguments: Vec<String>,
    pub(crate) guard_paths: Option<Vec<String>>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct FunctionInterfaceCall {
    pub(crate) artifact: usize,
    pub(crate) member: Option<String>,
    pub(crate) caller: String,
    pub(crate) function_address: u32,
    pub(crate) site: u32,
    pub(crate) kind: String,
    pub(crate) jalr_offset: i32,
    pub(crate) slot_selector: Option<String>,
    pub(crate) slot_index: Option<u32>,
    pub(crate) slot_index_domain: Option<(u8, u32, u32, String)>,
    pub(crate) arguments: Vec<(usize, String, String)>,
    pub(crate) linked_ir_matches: usize,
    pub(crate) linked_ir: Option<FunctionInterfaceIrCall>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct FunctionInterfaceLink {
    pub(crate) contract: String,
    pub(crate) slot: String,
    pub(crate) profile: String,
    pub(crate) source: String,
    pub(crate) identity: String,
    pub(crate) anchor: String,
    pub(crate) layout_version: String,
    pub(crate) offset: i32,
    pub(crate) width: u8,
    pub(crate) name: String,
    pub(crate) arguments: Vec<String>,
    pub(crate) return_type: String,
    pub(crate) variadic: bool,
    pub(crate) semantic: Option<String>,
    pub(crate) execution_model: Option<String>,
    pub(crate) calls: Vec<FunctionInterfaceCall>,
}

pub(crate) fn link_reviewed_interfaces(
    workspace: &FunctionWorkspace,
    bindings: &[ResolvedInterfaceSlot],
) -> Result<Vec<FunctionInterfaceLink>> {
    // Interface evidence names the concrete caller symbol. Index those facts
    // once instead of scanning every generated function for every
    // root/binding/call combination. On the real ESP32-S31 workspace the old
    // nested scan multiplied 5,132 functions by 54 bindings, 176 calls and a
    // second 5,132-function scan.
    let mut functions_by_caller =
        BTreeMap::<(&str, &str, &str, Option<&str>), Vec<&FunctionFact>>::new();
    for function in &workspace.facts.functions {
        functions_by_caller
            .entry((
                function.profile.as_str(),
                function.source.as_str(),
                function.symbol.as_str(),
                function.member.as_deref(),
            ))
            .or_default()
            .push(function);
    }
    // Keep transitive reachability as an ephemeral in-memory graph of borrowed
    // identities. Persisting every root's closure made artifact-wide profiles
    // quadratic in both RAM and JSON size.
    let identities = workspace
        .facts
        .functions
        .iter()
        .map(|function| {
            (
                (function.profile.as_str(), function.identity.as_str()),
                function.identity.as_str(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let mut symbols = BTreeMap::<(&str, &str, &str), Vec<&str>>::new();
    for function in &workspace.facts.functions {
        symbols
            .entry((
                function.profile.as_str(),
                function.source.as_str(),
                function.symbol.as_str(),
            ))
            .or_default()
            .push(function.identity.as_str());
    }
    let mut adjacency = BTreeMap::<(&str, &str), Vec<&str>>::new();
    for function in &workspace.facts.functions {
        let targets = adjacency
            .entry((function.profile.as_str(), function.identity.as_str()))
            .or_default();
        for call in &function.calls {
            if !matches!(call.kind.as_str(), "internal" | "project-linked") {
                continue;
            }
            let target = identities
                .get(&(function.profile.as_str(), call.target.as_str()))
                .copied()
                .or_else(|| {
                    let candidates = symbols.get(&(
                        function.profile.as_str(),
                        function.source.as_str(),
                        call.target.as_str(),
                    ))?;
                    (candidates.len() == 1).then_some(candidates[0])
                });
            if let Some(target) = target {
                targets.push(target);
            }
        }
        targets.sort_unstable();
        targets.dedup();
    }
    let mut output = Vec::new();
    for reviewed_function in &workspace.facts.functions {
        let mut reachable = BTreeSet::from([reviewed_function.identity.as_str()]);
        if reviewed_function.is_root() {
            let mut pending = vec![reviewed_function.identity.as_str()];
            while let Some(source) = pending.pop() {
                for target in adjacency
                    .get(&(reviewed_function.profile.as_str(), source))
                    .into_iter()
                    .flatten()
                {
                    if reachable.insert(*target) {
                        pending.push(*target);
                    }
                }
            }
        }
        for binding in bindings {
            let mut calls = BTreeSet::new();
            for call in &binding.calls {
                let key = (
                    reviewed_function.profile.as_str(),
                    binding.source.as_str(),
                    call.function.as_str(),
                    call.member.as_deref(),
                );
                let Some(candidates) = functions_by_caller.get(&key) else {
                    continue;
                };
                for function in candidates
                    .iter()
                    .copied()
                    .filter(|function| reachable.contains(function.identity.as_str()))
                {
                    let linked_ir = function
                        .calls
                        .iter()
                        .filter(|candidate| candidate.site == Some(call.site))
                        .collect::<Vec<_>>();
                    let exact_linked_ir = if linked_ir.len() == 1 {
                        let candidate = linked_ir[0];
                        if let (Some(interface_semantic), Some(ir_semantic)) =
                            (&binding.semantic, &candidate.semantic_operation)
                            && interface_semantic != ir_semantic
                        {
                            return Err(crate::Error::invalid(format!(
                                "interface semantic mismatch at {}:{:#010x}: reviewed slot {:?} uses {:?}, linked IR uses {:?}",
                                function.identity,
                                call.site,
                                binding.name,
                                interface_semantic,
                                ir_semantic
                            )));
                        }
                        Some(FunctionInterfaceIrCall {
                            kind: candidate.kind.clone(),
                            target: candidate.target.clone(),
                            semantic_operation: candidate.semantic_operation.clone(),
                            arguments: candidate.arguments.clone(),
                            guard_paths: candidate.guard_paths.clone(),
                        })
                    } else {
                        None
                    };
                    calls.insert(FunctionInterfaceCall {
                        artifact: call.artifact,
                        member: call.member.clone(),
                        caller: function.identity.clone(),
                        function_address: call.function_address,
                        site: call.site,
                        kind: call.kind.clone(),
                        jalr_offset: call.jalr_offset,
                        slot_selector: call.slot_selector.clone(),
                        slot_index: call.slot_index,
                        slot_index_domain: call.slot_index_domain.as_ref().map(|domain| {
                            (
                                domain.argument,
                                domain.min,
                                domain.max,
                                domain.evidence.clone(),
                            )
                        }),
                        arguments: call
                            .arguments
                            .iter()
                            .map(|argument| {
                                (
                                    argument.index,
                                    argument.kind.clone(),
                                    argument.expression.clone(),
                                )
                            })
                            .collect(),
                        linked_ir_matches: linked_ir.len(),
                        linked_ir: exact_linked_ir,
                    });
                }
            }
            if calls.is_empty() {
                continue;
            }
            output.push(FunctionInterfaceLink {
                contract: binding.contract.clone(),
                slot: binding.id.clone(),
                profile: reviewed_function.profile.clone(),
                source: reviewed_function.source.clone(),
                identity: reviewed_function.identity.clone(),
                anchor: binding.anchor.clone(),
                layout_version: binding.layout_version.clone(),
                offset: binding.offset,
                width: binding.width,
                name: binding.name.clone(),
                arguments: binding.arguments.clone(),
                return_type: binding.return_type.clone(),
                variadic: binding.variadic,
                semantic: binding.semantic.clone(),
                execution_model: binding
                    .execution_model
                    .as_ref()
                    .map(|model| model.id.clone()),
                calls: calls.into_iter().collect(),
            });
        }
    }
    output.sort();
    output.dedup();
    Ok(output)
}
