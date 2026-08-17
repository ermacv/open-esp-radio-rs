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

fn register_family(name: &str) -> String {
    let mut output = String::with_capacity(name.len());
    let mut in_digits = false;
    for character in name.chars() {
        if character.is_ascii_digit() {
            if !in_digits {
                output.push('%');
                in_digits = true;
            }
        } else {
            in_digits = false;
            output.push(character);
        }
    }
    output
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
    const MAX_GUARDED_REGISTERS: u32 = 32;

    let mut input_index = None;
    let mut input_bits = BTreeSet::new();
    if !collect_evaluable_input_bits(address, &mut input_index, &mut input_bits) {
        return None;
    }
    let input_index = input_index?;

    if input_bits.len() <= MAX_EXHAUSTIVE_INPUT_BITS {
        let input_bits = input_bits.into_iter().collect::<Vec<_>>();
        let mut registers = BTreeMap::<u32, String>::new();
        let mut family = None;
        for combination in 0..(1_u32 << input_bits.len()) {
            let input =
                input_bits
                    .iter()
                    .enumerate()
                    .fold(0_u32, |value, (source, destination)| {
                        value | (((combination >> source) & 1) << destination)
                    });
            let address = evaluate_for_input(address, input_index, input)?;
            let register = svd.register(address)?;
            let register_family = register_family(&register.name);
            if family
                .as_ref()
                .is_some_and(|family| family != &register_family)
            {
                return None;
            }
            family = Some(register_family);
            registers.insert(register.address, register.name.clone());
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
    let mut family = None;
    for selector in 0..=MAX_GUARDED_REGISTERS {
        let candidate_address = evaluate_for_input(address, input_index, selector)?;
        let Some(register) = svd.register(candidate_address) else {
            break;
        };
        let register_family = register_family(&register.name);
        if family
            .as_ref()
            .is_some_and(|family| family != &register_family)
        {
            break;
        }
        family = Some(register_family);
        if registers
            .iter()
            .any(|candidate: &IndexedMmioRegister| candidate.address == register.address)
        {
            return None;
        }
        registers.push(IndexedMmioRegister {
            address: register.address,
            name: register.name.clone(),
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
