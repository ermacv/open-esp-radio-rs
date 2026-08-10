//! Typed ABI projection for reviewed direct and trampoline calls.

use super::super::*;

pub(in crate::analysis::linked_ir) fn direct_semantic_typed_arguments(
    function: &crate::DirectSemanticFunctionSpec,
    arguments: &[String],
) -> Vec<LinkedCallArgument> {
    arguments
        .iter()
        .take(usize::from(function.argument_count))
        .enumerate()
        .map(|(position, value)| {
            let semantic = function.semantic.arguments.get(position);
            LinkedCallArgument {
                position,
                name: semantic.map_or_else(
                    || format!("arg{position}"),
                    |argument| argument.name.to_owned(),
                ),
                c_type: semantic
                    .map_or_else(|| "u32".to_owned(), |argument| argument.c_type.to_owned()),
                direction: semantic
                    .map_or("unknown", |argument| external_direction(argument.direction)),
                value: value.clone(),
            }
        })
        .collect()
}

pub(super) fn reviewed_external_typed_arguments(
    candidates: &[ReviewedExternalCall],
    arguments: &[SymbolicValue],
) -> Vec<LinkedCallArgument> {
    arguments
        .iter()
        .enumerate()
        .map(|(position, value)| {
            let types = candidates
                .iter()
                .filter_map(|candidate| candidate.argument_types.get(position))
                .collect::<BTreeSet<_>>();
            LinkedCallArgument {
                position,
                name: format!("arg{position}"),
                c_type: if types.len() == 1 {
                    (*types.first().expect("one reviewed argument type")).clone()
                } else {
                    types
                        .into_iter()
                        .map(String::as_str)
                        .collect::<Vec<_>>()
                        .join(" | ")
                },
                direction: "unknown",
                value: value.canonical(),
            }
        })
        .collect()
}

pub(in crate::analysis::linked_ir) fn external_return_model(model: ExternalReturnModel) -> String {
    match model {
        ExternalReturnModel::Void => "void".to_owned(),
        ExternalReturnModel::Constant(value) => format!("constant:{value:#010x}"),
        ExternalReturnModel::SymbolicU32 => "symbolic-u32".to_owned(),
        ExternalReturnModel::SymbolicU64 => "symbolic-u64".to_owned(),
        ExternalReturnModel::Unmodeled => "unmodeled".to_owned(),
    }
}

pub(super) fn external_return_is_modeled(model: ExternalReturnModel) -> bool {
    matches!(
        model,
        ExternalReturnModel::Constant(_)
            | ExternalReturnModel::SymbolicU32
            | ExternalReturnModel::SymbolicU64
    )
}

pub(super) fn linked_external_execution_model(
    model: &ReviewedExternalCallExecutionModel,
) -> LinkedExternalExecutionModel {
    LinkedExternalExecutionModel {
        id: model.id.clone(),
        return_model: external_return_model(model.return_model),
        outputs: model
            .outputs
            .iter()
            .map(|output| match output {
                ExternalOutputModel::PrivateStackU8 { pointer_argument } => {
                    LinkedExternalOutputModel {
                        kind: "private-stack-u8",
                        pointer_argument: *pointer_argument,
                        width: 8,
                    }
                }
            })
            .collect(),
    }
}

pub(in crate::analysis::linked_ir) fn linked_event_dispatch_contract(
    semantic: crate::ExternalSemanticSpec,
) -> Option<LinkedEventDispatchContract> {
    let dispatch = semantic.event_dispatch?;
    Some(LinkedEventDispatchContract {
        mechanism: dispatch.mechanism,
        execution_context: dispatch.execution_context,
        receiver: dispatch.receiver,
        argument_roles: dispatch
            .argument_roles
            .iter()
            .map(|binding| LinkedEventDispatchArgumentRole {
                role: binding.role,
                argument: binding.argument,
            })
            .collect(),
    })
}

fn external_direction(direction: crate::ExternalArgumentDirection) -> &'static str {
    match direction {
        crate::ExternalArgumentDirection::Input => "input",
        crate::ExternalArgumentDirection::Output => "output",
        crate::ExternalArgumentDirection::InputOutput => "input-output",
    }
}
