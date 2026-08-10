//! Resolution of reviewed interface slots against compiled execution models.
//!
//! Layout and ABI claims come from the reviewed pack. This module only joins
//! an explicit `execution-model` foreign key to behavior supplied by the
//! configured platform harness and checks that the two sides agree.

use super::{InterfacePack, validation::ValidationResult};
use crate::{ExternalCallModelSetRef, HarnessContractSpec};

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
                model_set.model(model_id).ok_or_else(|| {
                    super::validation::ValidationError::slot(
                        anchor,
                        slot,
                        "execution-model",
                        format!("execution contract {id:?} has no call model {model_id:?}"),
                    )
                })?;
            }
            Ok(Some(model_set))
        })
        .collect()
}
