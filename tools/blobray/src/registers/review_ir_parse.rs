//! Typed projection of schema-v60 linked-IR into register-review evidence.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use super::review_ir::{
    ReviewFieldEvidence, ReviewIrRegister, ReviewPredicateEvidence, ReviewSemanticEvidence,
};
use crate::Result;

#[tracing::instrument(name = "load_register_review_ir", fields(path = %path.display()))]
pub(super) fn parse_report(path: &Path) -> Result<Vec<ReviewIrRegister>> {
    let registers = crate::artifacts::LinkedIrReader::open(path)?.read_registers()?;
    parse_registers(path, registers)
}

fn parse_registers(
    path: &Path,
    registers: Vec<crate::artifacts::StoredMmioRegister>,
) -> Result<Vec<ReviewIrRegister>> {
    let mut seen = BTreeSet::new();
    registers
        .into_iter()
        .map(|register| {
            if !matches!(register.width, 8 | 16 | 32) {
                return Err(crate::Error::invalid(format!(
                    "unsupported register width {} at {:#010x}",
                    register.width, register.address
                )));
            }
            if !seen.insert((register.address, register.width)) {
                return Err(crate::Error::invalid(format!(
                    "duplicate linked-IR register at {:#010x}/{} in {}",
                    register.address,
                    register.width,
                    path.display()
                )));
            }
            let fields = register
                .field_candidates
                .into_iter()
                .map(|field| {
                    let key = (
                        field.least_significant_bit,
                        field.most_significant_bit,
                        field.mask,
                    );
                    if field.least_significant_bit > field.most_significant_bit
                        || field.most_significant_bit >= register.width
                        || contiguous_mask(field.least_significant_bit, field.most_significant_bit)
                            != field.mask
                    {
                        return Err(crate::Error::invalid(format!(
                            "invalid field bit range or mask at {:#010x}/{}",
                            register.address, register.width
                        )));
                    }
                    Ok((
                        key,
                        ReviewFieldEvidence {
                            least_significant_bit: field.least_significant_bit,
                            most_significant_bit: field.most_significant_bit,
                            mask: field.mask,
                            write_shapes: field.write_shapes,
                            predicate_shapes: field.predicate_shapes,
                            poll_shapes: field.poll_shapes,
                            functions: field.functions.into_iter().collect(),
                            access_functions: field.access_functions.into_iter().collect(),
                            predicate_functions: field.predicate_functions.into_iter().collect(),
                            predicate_evidence: field
                                .predicate_evidence
                                .into_iter()
                                .map(|evidence| ReviewPredicateEvidence {
                                    kind: evidence.kind,
                                    function: evidence.function,
                                    producer_path: evidence.producer_path,
                                    condition: evidence.condition,
                                    effective_operation: evidence.effective_operation,
                                    register_comparison_value: evidence.register_comparison_value,
                                })
                                .collect(),
                            semantic_operations: field.semantic_operations.into_iter().collect(),
                            semantic_roots: field.semantic_roots.into_iter().collect(),
                            semantic_evidence: field
                                .semantic_evidence
                                .into_iter()
                                .map(|evidence| ReviewSemanticEvidence {
                                    kind: evidence.kind,
                                    root: evidence.root,
                                    operation: evidence.operation,
                                    action_target: evidence.action_target,
                                    action_origin: evidence.action_origin,
                                    predicate_function: evidence.predicate_function,
                                    path_expression: evidence.path_expression,
                                    residual_path_expression: evidence.residual_path_expression,
                                    condition: evidence.condition,
                                    effective_operation: evidence.effective_operation,
                                })
                                .collect(),
                        },
                    ))
                })
                .collect::<Result<Vec<_>>>()?;
            let mut field_map = BTreeMap::new();
            for (key, field) in fields {
                if field_map.insert(key, field).is_some() {
                    return Err(crate::Error::invalid(format!(
                        "duplicate field candidate at {:#010x}/{}",
                        register.address, register.width
                    )));
                }
            }
            Ok(ReviewIrRegister {
                address: register.address,
                width: register.width,
                names: register.names.into_iter().collect(),
                functions: register.functions.into_iter().collect(),
                fields: field_map,
            })
        })
        .collect()
}

fn contiguous_mask(lsb: u8, msb: u8) -> u32 {
    let width = msb - lsb + 1;
    if width == 32 {
        u32::MAX
    } else {
        ((1_u32 << width) - 1) << lsb
    }
}
