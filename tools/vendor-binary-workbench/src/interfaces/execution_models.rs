//! Resolution of reviewed interface slots against compiled execution models.
//!
//! Layout and ABI claims come from the reviewed pack. This module only joins
//! an explicit `execution-model` foreign key to behavior supplied by the
//! configured platform harness and checks that the two sides agree.

use super::{InterfacePack, validation::ValidationResult};
use crate::{
    ExternalCallModelSetRef, ExternalOutputModel, ExternalReturnModel, HarnessContractSpec,
};

pub(super) fn resolve(
    pack: &InterfacePack,
    contracts: Option<&HarnessContractSpec>,
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
                        "execution contract {id:?} requires a configured compiled platform harness"
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
                        format!("compiled platform harness has no execution contract {id:?}"),
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
                if model.outputs.len() > 8 {
                    return Err(super::validation::ValidationError::slot(
                        anchor,
                        slot,
                        "execution-model",
                        format!(
                            "call model {model_id:?} declares {} outputs; RV32 supports at most eight argument-bound outputs",
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
                if let ExternalReturnModel::AllocatedZeroed { size_argument } = model.return_model
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
                    if argument_type != "usize" {
                        return Err(super::validation::ValidationError::slot(
                            anchor,
                            slot,
                            "execution-model",
                            format!(
                                "call model {model_id:?} allocation size argument a{size_argument} has non-size ABI type {argument_type:?}"
                            ),
                        ));
                    }
                }
                let mut output_arguments = std::collections::BTreeSet::new();
                for output in model.outputs {
                    let ExternalOutputModel::PrivateStackU8 { pointer_argument } = output;
                    let Some(argument_type) = arguments.get(usize::from(*pointer_argument)) else {
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
                    if !output_arguments.insert(*pointer_argument) {
                        return Err(super::validation::ValidationError::slot(
                            anchor,
                            slot,
                            "execution-model",
                            format!(
                                "call model {model_id:?} declares more than one output for argument a{pointer_argument}"
                            ),
                        ));
                    }
                }
            }
            Ok(Some(model_set))
        })
        .collect()
}
