//! Resolution of reviewed interface slots against compiled execution models.
//!
//! Layout and ABI claims come from the reviewed pack. This module only joins
//! an explicit `execution-model` foreign key to behavior supplied by the
//! configured platform harness and checks that the two sides agree.

use super::{InterfacePack, validation::ValidationResult};
use crate::{ExternalTableRef, HarnessContractSpec};

pub(super) fn resolve(
    pack: &InterfacePack,
    contracts: Option<&HarnessContractSpec>,
) -> ValidationResult<Vec<Option<ExternalTableRef>>> {
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
            let table = contracts
                .external_tables
                .iter()
                .copied()
                .find(|table| table.spec().id == id)
                .ok_or_else(|| {
                    super::validation::ValidationError::anchor(
                        anchor,
                        "execution-contract",
                        format!("compiled platform harness has no execution contract {id:?}"),
                    )
                })?;
            let spec = table.spec();
            if anchor.layout_size != Some(spec.size) {
                return Err(super::validation::ValidationError::anchor(
                    anchor,
                    "layout-size",
                    format!(
                        "reviewed layout size {:#x} does not match execution contract {id:?} size {:#x}",
                        anchor.layout_size.unwrap_or_default(),
                        spec.size
                    ),
                ));
            }
            for slot in &anchor.slots {
                let Some(model_id) = slot.execution_model.as_deref() else {
                    continue;
                };
                let model = spec
                    .functions
                    .iter()
                    .find(|model| model.id == model_id)
                    .ok_or_else(|| {
                        super::validation::ValidationError::slot(
                            anchor,
                            slot,
                            "execution-model",
                            format!(
                                "execution contract {id:?} has no call model {model_id:?}"
                            ),
                        )
                    })?;
                if u32::try_from(slot.offset).ok() != Some(model.offset) {
                    return Err(super::validation::ValidationError::slot(
                        anchor,
                        slot,
                        "execution-model",
                        format!(
                            "call model {id}.{model_id} belongs to offset {:#x}, not {:+#x}",
                            model.offset, slot.offset
                        ),
                    ));
                }
                let argument_count = slot.arguments.as_ref().map_or(0, Vec::len);
                if argument_count != usize::from(model.argument_count) {
                    return Err(super::validation::ValidationError::slot(
                        anchor,
                        slot,
                        "arguments",
                        format!(
                            "reviewed ABI has {argument_count} arguments but call model {id}.{model_id} has {}",
                            model.argument_count
                        ),
                    ));
                }
                if slot.semantic.as_deref() != Some(model.semantic.operation) {
                    return Err(super::validation::ValidationError::slot(
                        anchor,
                        slot,
                        "semantic",
                        format!(
                            "call model {id}.{model_id} requires reviewed semantic {:?}, got {:?}",
                            model.semantic.operation, slot.semantic
                        ),
                    ));
                }
            }
            Ok(Some(table))
        })
        .collect()
}
