//! Proof and enumeration of symbolic indexed-MMIO register domains.

use std::collections::{BTreeMap, BTreeSet};

use super::value::{BitSource, ExpressionOperation, SymbolicValue};
use crate::MmioMap;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AffineInput {
    index: Option<u8>,
    scale: u32,
    offset: u32,
}

fn merge_affine_input(left: Option<u8>, right: Option<u8>) -> Option<Option<u8>> {
    match (left, right) {
        (Some(left), Some(right)) if left != right => None,
        (Some(index), _) | (_, Some(index)) => Some(Some(index)),
        (None, None) => Some(None),
    }
}

fn affine_input(value: &SymbolicValue) -> Option<AffineInput> {
    match value {
        SymbolicValue::Constant(value) => Some(AffineInput {
            index: None,
            scale: 0,
            offset: *value,
        }),
        SymbolicValue::Input { index } | SymbolicValue::InputConstant { index, .. } => {
            Some(AffineInput {
                index: Some(*index),
                scale: 1,
                offset: 0,
            })
        }
        SymbolicValue::Bits(bits) => {
            let first_input = bits.iter().find_map(|source| match source {
                BitSource::Input { index, .. } => Some(*index),
                _ => None,
            })?;
            for shift in 0..32_usize {
                let matches = bits.iter().enumerate().all(|(destination, source)| {
                    if destination < shift {
                        *source == BitSource::Constant(false)
                    } else {
                        *source
                            == BitSource::Input {
                                index: first_input,
                                bit: (destination - shift) as u8,
                                inverted: false,
                            }
                    }
                });
                if matches {
                    return Some(AffineInput {
                        index: Some(first_input),
                        scale: 1_u32 << shift,
                        offset: 0,
                    });
                }
            }
            None
        }
        SymbolicValue::Expression {
            operation,
            left,
            right,
            ..
        } => {
            let left = affine_input(left)?;
            let right = affine_input(right)?;
            match operation {
                ExpressionOperation::Add | ExpressionOperation::Subtract => {
                    let index = merge_affine_input(left.index, right.index)?;
                    let (scale, offset) = if *operation == ExpressionOperation::Add {
                        (
                            left.scale.wrapping_add(right.scale),
                            left.offset.wrapping_add(right.offset),
                        )
                    } else {
                        (
                            left.scale.wrapping_sub(right.scale),
                            left.offset.wrapping_sub(right.offset),
                        )
                    };
                    Some(AffineInput {
                        index,
                        scale,
                        offset,
                    })
                }
                ExpressionOperation::Multiply if left.index.is_none() => Some(AffineInput {
                    index: right.index,
                    scale: right.scale.wrapping_mul(left.offset),
                    offset: right.offset.wrapping_mul(left.offset),
                }),
                ExpressionOperation::Multiply if right.index.is_none() => Some(AffineInput {
                    index: left.index,
                    scale: left.scale.wrapping_mul(right.offset),
                    offset: left.offset.wrapping_mul(right.offset),
                }),
                ExpressionOperation::ShiftLeft if right.index.is_none() => {
                    let shift = right.offset & 31;
                    Some(AffineInput {
                        index: left.index,
                        scale: left.scale.wrapping_shl(shift),
                        offset: left.offset.wrapping_shl(shift),
                    })
                }
                _ => None,
            }
        }
        _ => None,
    }
}

pub fn collect_evaluable_input_bits(
    value: &SymbolicValue,
    index: &mut Option<u8>,
    bits: &mut BTreeSet<u8>,
) -> bool {
    collect_evaluable_input_bits_masked(value, u32::MAX, index, bits)
}

fn lower_dependency_mask(output_mask: u32) -> u32 {
    if output_mask == 0 {
        0
    } else {
        u32::MAX >> output_mask.leading_zeros()
    }
}

fn collect_evaluable_input_bits_masked(
    value: &SymbolicValue,
    output_mask: u32,
    index: &mut Option<u8>,
    bits: &mut BTreeSet<u8>,
) -> bool {
    match value {
        SymbolicValue::Constant(_) => true,
        SymbolicValue::Input {
            index: source_index,
        }
        | SymbolicValue::InputConstant {
            index: source_index,
            ..
        } => {
            if index.is_some_and(|index| index != *source_index) {
                return false;
            }
            *index = Some(*source_index);
            bits.extend((0..32).filter(|bit| output_mask & (1 << bit) != 0));
            true
        }
        SymbolicValue::Expression {
            operation,
            left,
            right,
            ..
        } => {
            let (left_mask, right_mask) = match operation {
                ExpressionOperation::BitAnd => match (left.as_constant(), right.as_constant()) {
                    (_, Some(mask)) => (output_mask & mask, 0),
                    (Some(mask), _) => (0, output_mask & mask),
                    _ => (output_mask, output_mask),
                },
                ExpressionOperation::BitOr => match (left.as_constant(), right.as_constant()) {
                    (_, Some(mask)) => (output_mask & !mask, 0),
                    (Some(mask), _) => (0, output_mask & !mask),
                    _ => (output_mask, output_mask),
                },
                ExpressionOperation::BitXor => (output_mask, output_mask),
                ExpressionOperation::Add
                | ExpressionOperation::Subtract
                | ExpressionOperation::Multiply => {
                    let mask = lower_dependency_mask(output_mask);
                    (mask, mask)
                }
                ExpressionOperation::ShiftLeft => {
                    let Some(shift) = right.as_constant() else {
                        return false;
                    };
                    (output_mask.wrapping_shr(shift & 31), 0)
                }
                ExpressionOperation::ShiftRight => {
                    let Some(shift) = right.as_constant() else {
                        return false;
                    };
                    (output_mask.wrapping_shl(shift & 31), 0)
                }
                ExpressionOperation::ShiftRightArithmetic
                | ExpressionOperation::DivideSigned
                | ExpressionOperation::DivideUnsigned
                | ExpressionOperation::RemainderSigned
                | ExpressionOperation::RemainderUnsigned
                | ExpressionOperation::Equal
                | ExpressionOperation::LessThanSigned
                | ExpressionOperation::LessThanUnsigned => (u32::MAX, u32::MAX),
                ExpressionOperation::CountLeadingZeros
                | ExpressionOperation::CountTrailingZeros
                | ExpressionOperation::PopulationCount => (u32::MAX, 0),
            };
            collect_evaluable_input_bits_masked(left, left_mask, index, bits)
                && collect_evaluable_input_bits_masked(right, right_mask, index, bits)
        }
        SymbolicValue::Bits(sources) => sources.iter().enumerate().all(|(destination, source)| {
            if output_mask & (1 << destination) == 0 {
                return true;
            }
            match source {
                BitSource::Constant(_) => true,
                BitSource::Input {
                    index: source_index,
                    bit,
                    ..
                } => {
                    if index.is_some_and(|index| index != *source_index) {
                        return false;
                    }
                    *index = Some(*source_index);
                    bits.insert(*bit);
                    true
                }
                _ => false,
            }
        }),
        _ => false,
    }
}

pub fn evaluate_for_input(value: &SymbolicValue, input_index: u8, input: u32) -> Option<u32> {
    match value {
        SymbolicValue::Constant(value) => Some(*value),
        SymbolicValue::Input { index } | SymbolicValue::InputConstant { index, .. }
            if *index == input_index =>
        {
            Some(input)
        }
        SymbolicValue::Expression {
            operation,
            left,
            right,
            ..
        } => {
            let left = evaluate_for_input(left, input_index, input)?;
            let right = evaluate_for_input(right, input_index, input)?;
            Some(match operation {
                ExpressionOperation::Add => left.wrapping_add(right),
                ExpressionOperation::Subtract => left.wrapping_sub(right),
                ExpressionOperation::Multiply => left.wrapping_mul(right),
                ExpressionOperation::DivideSigned => {
                    let (left, right) = (left as i32, right as i32);
                    if right == 0 {
                        u32::MAX
                    } else if left == i32::MIN && right == -1 {
                        i32::MIN as u32
                    } else {
                        left.wrapping_div(right) as u32
                    }
                }
                ExpressionOperation::DivideUnsigned => left.checked_div(right).unwrap_or(u32::MAX),
                ExpressionOperation::RemainderSigned => {
                    let (left, right) = (left as i32, right as i32);
                    if right == 0 {
                        left as u32
                    } else if left == i32::MIN && right == -1 {
                        0
                    } else {
                        left.wrapping_rem(right) as u32
                    }
                }
                ExpressionOperation::RemainderUnsigned => left.checked_rem(right).unwrap_or(left),
                ExpressionOperation::BitAnd => left & right,
                ExpressionOperation::BitOr => left | right,
                ExpressionOperation::BitXor => left ^ right,
                ExpressionOperation::ShiftLeft => left.wrapping_shl(right & 31),
                ExpressionOperation::ShiftRight => left.wrapping_shr(right & 31),
                ExpressionOperation::ShiftRightArithmetic => {
                    (left as i32).wrapping_shr(right & 31) as u32
                }
                ExpressionOperation::Equal => u32::from(left == right),
                ExpressionOperation::LessThanSigned => u32::from((left as i32) < (right as i32)),
                ExpressionOperation::LessThanUnsigned => u32::from(left < right),
                ExpressionOperation::CountLeadingZeros => left.leading_zeros(),
                ExpressionOperation::CountTrailingZeros => left.trailing_zeros(),
                ExpressionOperation::PopulationCount => left.count_ones(),
            })
        }
        SymbolicValue::Bits(sources) => {
            let mut output = 0_u32;
            for (destination, source) in sources.iter().enumerate() {
                let bit = match source {
                    BitSource::Constant(value) => *value,
                    BitSource::Input {
                        index,
                        bit,
                        inverted,
                    } if *index == input_index => ((input >> bit) & 1 != 0) ^ *inverted,
                    _ => return None,
                };
                output |= u32::from(bit) << destination;
            }
            Some(output)
        }
        _ => None,
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IndexedMmioRegister {
    pub address: u32,
    pub name: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IndexedMmioGuard {
    pub selector: SymbolicValue,
    pub maximum: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IndexedMmioDomain {
    pub registers: Vec<IndexedMmioRegister>,
    pub guard: Option<IndexedMmioGuard>,
}

pub fn indexed_mmio_domain(address: &SymbolicValue, svd: &MmioMap) -> Option<IndexedMmioDomain> {
    const MAX_EXHAUSTIVE_INPUT_BITS: usize = 8;
    const MAX_GUARDED_REGISTERS: u32 = 4096;

    let mut input_index = None;
    let mut input_bits = BTreeSet::new();
    if !collect_evaluable_input_bits(address, &mut input_index, &mut input_bits) {
        return None;
    }
    let input_index = input_index?;

    if input_bits.len() <= MAX_EXHAUSTIVE_INPUT_BITS {
        let input_bits = input_bits.into_iter().collect::<Vec<_>>();
        let mut registers = BTreeMap::<u32, String>::new();

        for combination in 0..(1_u32 << input_bits.len()) {
            let input =
                input_bits
                    .iter()
                    .enumerate()
                    .fold(0_u32, |value, (source, destination)| {
                        value | (((combination >> source) & 1) << destination)
                    });
            let address = evaluate_for_input(address, input_index, input)?;
            if !svd.contains_mmio(address) && svd.register(address).is_none() {
                return None;
            }
            let name = svd.register(address).map_or_else(
                || format!("UNKNOWN@{address:#010x}"),
                |register| register.name.clone(),
            );
            registers.insert(address, name);
        }
        if registers.len() >= 2 {
            return Some(IndexedMmioDomain {
                registers: registers
                    .into_iter()
                    .map(|(address, name)| IndexedMmioRegister { address, name })
                    .collect(),
                guard: None,
            });
        }
    }

    let affine = affine_input(address)?;
    if affine.index != Some(input_index) || affine.scale == 0 {
        return None;
    }
    let mut registers = Vec::new();

    for selector in 0..=MAX_GUARDED_REGISTERS {
        let candidate_address = evaluate_for_input(address, input_index, selector)?;
        if !svd.contains_mmio(candidate_address) && svd.register(candidate_address).is_none() {
            break;
        }
        let name = svd.register(candidate_address).map_or_else(
            || format!("UNKNOWN@{candidate_address:#010x}"),
            |register| register.name.clone(),
        );
        if registers
            .iter()
            .any(|candidate: &IndexedMmioRegister| candidate.address == candidate_address)
        {
            return None;
        }
        registers.push(IndexedMmioRegister {
            address: candidate_address,
            name,
        });
    }
    if !(2..=MAX_GUARDED_REGISTERS as usize).contains(&registers.len()) {
        return None;
    }
    Some(IndexedMmioDomain {
        guard: Some(IndexedMmioGuard {
            selector: SymbolicValue::input(input_index),
            maximum: registers.len() as u32 - 1,
        }),
        registers,
    })
}

/// Compressed intersection of an affine address expression with a declared
/// MMIO region. Selector bounds are conditional on region membership; they
/// are not a claim that every execution satisfies that condition.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub struct ConditionalMmioDomain {
    pub region: String,
    pub input: u8,
    pub first_selector: u32,
    pub last_selector: u32,
    pub first_address: u32,
    pub last_address: u32,
    pub stride: u32,
    pub conditional_on_region: bool,
}

pub fn conditional_mmio_domains(
    address: &SymbolicValue,
    map: &MmioMap,
) -> Vec<ConditionalMmioDomain> {
    let Some(affine) = affine_input(address) else {
        return Vec::new();
    };
    let Some(input) = affine.index else {
        return Vec::new();
    };
    if affine.scale == 0 {
        return Vec::new();
    }
    // This projection covers the non-wrapping segment. The original
    // expression remains evidence for modular wrap and other regions.
    map.regions
        .iter()
        .filter_map(|region| {
            let lower = u64::from(region.start).saturating_sub(u64::from(affine.offset));
            let upper = u64::from(region.end)
                .checked_sub(1)?
                .checked_sub(u64::from(affine.offset))?;
            let scale = u64::from(affine.scale);
            let first = lower.div_ceil(scale);
            let last = upper / scale;
            if first > last {
                return None;
            }
            Some(ConditionalMmioDomain {
                region: region.name.clone(),
                input,
                first_selector: first as u32,
                last_selector: last as u32,
                first_address: (u64::from(affine.offset) + first * scale) as u32,
                last_address: (u64::from(affine.offset) + last * scale) as u32,
                stride: affine.scale,
                conditional_on_region: true,
            })
        })
        .collect()
}

#[cfg(test)]
mod inventory_tests {
    use super::*;
    use crate::MmioRegion;
    #[test]
    fn unknown_indexed_banks_do_not_require_names_and_large_domains_stay_compressed() {
        let map = MmioMap {
            registers: Vec::new(),
            regions: vec![MmioRegion {
                name: "bank".into(),
                start: 0x1000,
                end: 0x101000,
                readable: true,
                writable: true,
            }],
        };
        let expression = SymbolicValue::expression(
            ExpressionOperation::Add,
            SymbolicValue::Constant(0x1000),
            SymbolicValue::expression(
                ExpressionOperation::Multiply,
                SymbolicValue::input(0),
                SymbolicValue::Constant(4),
            ),
        );
        let spans = conditional_mmio_domains(&expression, &map);
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].last_selector, 262143);
        assert_eq!(spans[0].stride, 4);
        let bounded = SymbolicValue::expression(
            ExpressionOperation::Add,
            SymbolicValue::Constant(0x1000),
            SymbolicValue::expression(
                ExpressionOperation::Multiply,
                SymbolicValue::expression(
                    ExpressionOperation::BitAnd,
                    SymbolicValue::input(0),
                    SymbolicValue::Constant(63),
                ),
                SymbolicValue::Constant(4),
            ),
        );
        let domain = indexed_mmio_domain(&bounded, &map).unwrap();
        assert_eq!(domain.registers.len(), 64);
        assert!(domain.guard.is_none());
    }
}
