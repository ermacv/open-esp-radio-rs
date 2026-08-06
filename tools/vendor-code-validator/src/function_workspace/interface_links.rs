//! Exact static call-site links from validated interface bindings into function review.

use std::collections::BTreeSet;

use super::FunctionWorkspace;
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
    pub(crate) arguments: Vec<(usize, String, String)>,
    pub(crate) linked_ir_matches: usize,
    pub(crate) linked_ir: Option<FunctionInterfaceIrCall>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct FunctionInterfaceLink {
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
    pub(crate) calls: Vec<FunctionInterfaceCall>,
}

pub(crate) fn link_reviewed_interfaces(
    workspace: &FunctionWorkspace,
    bindings: &[ResolvedInterfaceSlot],
) -> Result<Vec<FunctionInterfaceLink>> {
    let mut output = Vec::new();
    for reviewed_function in &workspace.facts.functions {
        let reachable = std::iter::once(reviewed_function.identity.as_str())
            .chain(
                reviewed_function
                    .is_root()
                    .then_some(&reviewed_function.reachable_functions)
                    .into_iter()
                    .flatten()
                    .map(String::as_str),
            )
            .collect::<BTreeSet<_>>();
        for binding in bindings {
            let mut calls = BTreeSet::new();
            for call in &binding.calls {
                for function in workspace
                    .facts
                    .functions
                    .iter()
                    .filter(|function| function.profile == reviewed_function.profile)
                    .filter(|function| reachable.contains(function.identity.as_str()))
                    .filter(|function| function.source == binding.source)
                    .filter(|function| function.symbol == call.function)
                    .filter(|function| function.member == call.member)
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
                            return Err(format!(
                                "interface semantic mismatch at {}:{:#010x}: reviewed slot {:?} uses {:?}, linked IR uses {:?}",
                                function.identity,
                                call.site,
                                binding.name,
                                interface_semantic,
                                ir_semantic
                            )
                            .into());
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
                calls: calls.into_iter().collect(),
            });
        }
    }
    output.sort();
    output.dedup();
    Ok(output)
}
