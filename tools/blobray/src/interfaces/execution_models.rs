//! Resolution of reviewed interface slots against compiled execution models.
//!
//! Layout and ABI claims come from the reviewed pack. This module only joins
//! an explicit `execution-model` foreign key to behavior supplied by the
//! configured knowledge provider and checks that the two sides agree.

use super::{InterfacePack, validation::ValidationResult};
use crate::{
    ExternalCallModelSetRef, ExternalOutputModel, ExternalReturnModel, KnowledgeContractSpec,
};

pub(super) fn resolve(
    pack: &InterfacePack,
    contracts: Option<&KnowledgeContractSpec>,
) -> ValidationResult<Vec<Option<ExternalCallModelSetRef>>> {
    pack.anchors
        .iter()
        .map(|anchor| {
            let Some(id) = anchor.execution_contract.as_deref() else {
                return Ok(None);
            };
            let contracts = contracts.ok_or_else(|| {
                super::validation::ValidationError::anchor(
                    anchor,
                    "execution-contract",
                    format!(
                        "execution contract {id:?} requires a configured compiled knowledge provider"
                    ),
                )
            })?;
            let model_set = contracts
                .external_call_model_sets
                .iter()
                .copied()
                .find(|model_set| model_set.spec().id == id)
                .ok_or_else(|| {
                    super::validation::ValidationError::anchor(
                        anchor,
                        "execution-contract",
                        format!("compiled knowledge provider has no execution contract {id:?}"),
                    )
                })?;
            for slot in &anchor.slots {
                let Some(model_id) = slot.execution_model.as_deref() else {
                    continue;
                };
                let model = model_set.model(model_id).ok_or_else(|| {
                    super::validation::ValidationError::slot(
                        anchor,
                        slot,
                        "execution-model",
                        format!("execution contract {id:?} has no call model {model_id:?}"),
                    )
                })?;
                let model = model.spec();
                if model.outputs.len() > usize::from(u8::MAX) + 1 {
                    return Err(super::validation::ValidationError::slot(
                        anchor,
                        slot,
                        "execution-model",
                        format!(
                            "call model {model_id:?} declares {} outputs; one call model supports at most 256 independently identified writes",
                            model.outputs.len()
                        ),
                    ));
                }
                if model.return_model == ExternalReturnModel::Unmodeled
                    && !model.outputs.is_empty()
                {
                    return Err(super::validation::ValidationError::slot(
                        anchor,
                        slot,
                        "execution-model",
                        format!(
                            "call model {model_id:?} cannot attach executable outputs to an unmodeled call"
                        ),
                    ));
                }
                let arguments = slot.arguments.as_deref().unwrap_or_default();
                if let ExternalReturnModel::Allocated { size_argument }
                | ExternalReturnModel::AllocatedZeroed { size_argument } = model.return_model
                {
                    let Some(argument_type) = arguments.get(usize::from(size_argument)) else {
                        return Err(super::validation::ValidationError::slot(
                            anchor,
                            slot,
                            "execution-model",
                            format!(
                                "call model {model_id:?} allocation size refers to missing argument a{size_argument}"
                            ),
                        ));
                    };
                    if !matches!(argument_type.as_str(), "usize" | "u32") {
                        return Err(super::validation::ValidationError::slot(
                            anchor,
                            slot,
                            "execution-model",
                            format!(
                                "call model {model_id:?} allocation size argument a{size_argument} has unsupported size ABI type {argument_type:?}"
                            ),
                        ));
                    }
                }
                let mut output_ranges = std::collections::BTreeMap::<u8, Vec<(u16, u16)>>::new();
                for output in model.outputs {
                    let (pointer_argument, byte_offset, width, private_stack_only) = match output {
                        ExternalOutputModel::PrivateStack {
                            pointer_argument,
                            width,
                        } => (*pointer_argument, 0, *width, true),
                        ExternalOutputModel::Memory {
                            pointer_argument,
                            byte_offset,
                            width,
                        } => (*pointer_argument, *byte_offset, *width, false),
                    };
                    if !matches!(width, 8 | 16 | 32) {
                        return Err(super::validation::ValidationError::slot(
                            anchor,
                            slot,
                            "execution-model",
                            format!(
                                "call model {model_id:?} output width must be 8, 16, or 32 bits"
                            ),
                        ));
                    }
                    let Some(argument_type) = arguments.get(usize::from(pointer_argument)) else {
                        return Err(super::validation::ValidationError::slot(
                            anchor,
                            slot,
                            "execution-model",
                            format!(
                                "call model {model_id:?} output refers to missing argument a{pointer_argument}"
                            ),
                        ));
                    };
                    if !matches!(argument_type.as_str(), "out-ptr" | "mut-ptr") {
                        return Err(super::validation::ValidationError::slot(
                            anchor,
                            slot,
                            "execution-model",
                            format!(
                                "call model {model_id:?} output argument a{pointer_argument} has non-output ABI type {argument_type:?}"
                            ),
                        ));
                    }
                    let byte_width = u16::from(width / 8);
                    let Some(end) = byte_offset.checked_add(byte_width) else {
                        return Err(super::validation::ValidationError::slot(
                            anchor,
                            slot,
                            "execution-model",
                            format!(
                                "call model {model_id:?} output range for argument a{pointer_argument} overflows"
                            ),
                        ));
                    };
                    let ranges = output_ranges.entry(pointer_argument).or_default();
                    if private_stack_only && !ranges.is_empty()
                        || ranges
                            .iter()
                            .any(|(existing_start, existing_end)| {
                                byte_offset < *existing_end && *existing_start < end
                            })
                    {
                        return Err(super::validation::ValidationError::slot(
                            anchor,
                            slot,
                            "execution-model",
                            format!(
                                "call model {model_id:?} declares overlapping outputs for argument a{pointer_argument}"
                            ),
                        ));
                    }
                    ranges.push((byte_offset, end));
                }
            }
            Ok(Some(model_set))
        })
        .collect()
}
