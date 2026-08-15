//! Rendering of symbolic RV32 values into Rust expressions.

use std::collections::BTreeMap;

use crate::{
    ALLOCATED_EXTERNAL_RESULT_TOKEN_FLAG, BitSource, RV32_MODELED_ARGUMENT_COUNT,
    SECONDARY_CALL_RESULT_TOKEN_FLAG, SymbolicValue,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum CallResultAvailability {
    Unmodeled,
    Primary,
    PrimaryAndSecondary,
}

fn call_result_parts(token: u32) -> (u32, bool) {
    (
        token & !SECONDARY_CALL_RESULT_TOKEN_FLAG,
        token & SECONDARY_CALL_RESULT_TOKEN_FLAG != 0,
    )
}

fn call_result_available(token: u32, results: &[CallResultAvailability]) -> bool {
    let (token, secondary) = call_result_parts(token);
    usize::try_from(token)
        .ok()
        .and_then(|token| results.get(token))
        .is_some_and(|availability| match (availability, secondary) {
            (CallResultAvailability::Primary, false)
            | (CallResultAvailability::PrimaryAndSecondary, _) => true,
            (CallResultAvailability::Unmodeled, _) | (CallResultAvailability::Primary, true) => {
                false
            }
        })
}

fn call_result_name(token: u32) -> String {
    let (token, secondary) = call_result_parts(token);
    if secondary {
        format!("call_result{token}_high")
    } else {
        format!("call_result{token}")
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum SourceWord {
    Argument(u8),
    Read(u32),
    MemoryRead(u32),
    CallResult(u32),
    ExternalResult(u32),
    ExternalResultHigh(u32),
    ExternalOutput { call_token: u32, output_index: u8 },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum MmioReadAddress {
    Static(u32),
    Indexed,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct BitGroup {
    source: SourceWord,
    inverted: bool,
    shift: i8,
}

fn source_word(group: BitGroup, arguments: &[String; RV32_MODELED_ARGUMENT_COUNT]) -> String {
    let source = match group.source {
        SourceWord::Argument(index) => arguments[usize::from(index)].clone(),
        SourceWord::Read(token) => format!("read{token}"),
        SourceWord::MemoryRead(token) => format!("memory_read{token}"),
        SourceWord::CallResult(token) => call_result_name(token),
        SourceWord::ExternalResult(token) => format!(
            "external_result{}",
            token & !ALLOCATED_EXTERNAL_RESULT_TOKEN_FLAG
        ),
        SourceWord::ExternalResultHigh(token) => format!("external_result{token}_high"),
        SourceWord::ExternalOutput {
            call_token,
            output_index,
        } => format!("external_output{call_token}_{output_index}"),
    };
    if group.inverted {
        format!("!{source}")
    } else {
        source
    }
}

fn grouped_expression(
    group: BitGroup,
    mask: u32,
    arguments: &[String; RV32_MODELED_ARGUMENT_COUNT],
) -> String {
    let source = source_word(group, arguments);
    let shifted = match group.shift.cmp(&0) {
        std::cmp::Ordering::Less => format!("({source} >> {})", -group.shift),
        std::cmp::Ordering::Equal => source,
        std::cmp::Ordering::Greater => format!("({source} << {})", group.shift),
    };
    format!("{shifted} & {mask:#010x}_u32")
}

fn validate_read(
    reads: &[MmioReadAddress],
    read_token: u32,
    expected_address: u32,
) -> Result<(), String> {
    let actual_address = reads
        .get(read_token as usize)
        .ok_or_else(|| format!("symbolic value refers to missing MMIO read token {read_token}"))?;
    if *actual_address != MmioReadAddress::Static(expected_address) {
        return Err(format!(
            "MMIO read token {read_token} does not refer to static address {expected_address:#010x}"
        ));
    }
    Ok(())
}

fn validate_indexed_read(reads: &[MmioReadAddress], read_token: u32) -> Result<(), String> {
    if usize::try_from(read_token)
        .ok()
        .and_then(|token| reads.get(token))
        == Some(&MmioReadAddress::Indexed)
    {
        Ok(())
    } else {
        Err(format!(
            "symbolic value refers to missing indexed MMIO read token {read_token}"
        ))
    }
}

pub(super) fn render_value_scoped(
    value: &SymbolicValue,
    reads: &[MmioReadAddress],
    memory_read_count: usize,
    call_results: &[CallResultAvailability],
    external_results: usize,
    arguments: &[String; RV32_MODELED_ARGUMENT_COUNT],
) -> Result<String, String> {
    match value {
        SymbolicValue::Unknown => Err("symbolic value is unresolved".to_owned()),
        SymbolicValue::Constant(value) => Ok(format!("{value:#010x}_u32")),
        SymbolicValue::Input { index } => arguments
            .get(usize::from(*index))
            .map(|argument| format!("{argument} & 0xffffffff_u32"))
            .ok_or_else(|| format!("argument index {index} is outside the modeled ABI")),
        SymbolicValue::InputConstant { index, .. } => render_value_scoped(
            &SymbolicValue::input(*index),
            reads,
            memory_read_count,
            call_results,
            external_results,
            arguments,
        ),
        SymbolicValue::StackAddress(offset) => Err(format!(
            "private stack address {offset:+#x} escaped into generated behavior"
        )),
        SymbolicValue::SymbolAddress {
            member,
            symbol,
            hi_addend,
            lo_addend,
            post_offset,
        } => {
            let Some(lo_addend) = lo_addend else {
                return Err(format!(
                    "incomplete HI20 relocation for {member:?}::{symbol} escaped into generated behavior"
                ));
            };
            let member = member
                .as_ref()
                .map_or_else(|| "None".to_owned(), |member| format!("Some({member:?})"));
            let base = format!(
                "riscv_hi20_lo12_address(memory.symbol_address({member}, {symbol:?}), {:#010x}_u32, {:#010x}_u32)",
                *hi_addend as u32, *lo_addend as u32
            );
            Ok(if *post_offset == 0 {
                base
            } else {
                format!("({base}).wrapping_add({:#010x}_u32)", *post_offset as u32)
            })
        }
        SymbolicValue::CallResult(token) => {
            if call_result_available(*token, call_results) {
                Ok(call_result_name(*token))
            } else {
                Err(format!(
                    "unmodeled call result {token} escaped into generated behavior"
                ))
            }
        }
        SymbolicValue::ReviewedExternalTable(contract) => Err(format!(
            "reviewed external ABI table {contract} escaped into generated behavior"
        )),
        SymbolicValue::ReviewedExternalFunction { contract, offset } => Err(format!(
            "reviewed external ABI pointer {contract}+{offset:#x} cannot be emitted as a scalar expression"
        )),
        SymbolicValue::FunctionTable(table) => Err(format!(
            "function table {} escaped into generated behavior",
            table.id()
        )),
        SymbolicValue::FunctionPointer { table, target } => Err(format!(
            "function pointer {}::{target:#010x} escaped into generated behavior",
            table.id()
        )),
        SymbolicValue::ExternalResult(token) => {
            let token = *token & !ALLOCATED_EXTERNAL_RESULT_TOKEN_FLAG;
            if usize::try_from(token).is_ok_and(|token| token < external_results) {
                Ok(format!("external_result{token}"))
            } else {
                Err(format!(
                    "symbolic value refers to missing external-call token {token}"
                ))
            }
        }
        SymbolicValue::ExternalResultHigh(token) => {
            if usize::try_from(*token).is_ok_and(|token| token < external_results) {
                Ok(format!("external_result{token}_high"))
            } else {
                Err(format!(
                    "symbolic value refers to missing external-call token {token}"
                ))
            }
        }
        SymbolicValue::ExternalOutput {
            call_token,
            output_index,
        } => {
            if usize::try_from(*call_token).is_ok_and(|token| token < external_results) {
                Ok(format!("external_output{call_token}_{output_index}"))
            } else {
                Err(format!(
                    "symbolic value refers to missing external-call token {call_token}"
                ))
            }
        }
        SymbolicValue::Expression {
            operation,
            left,
            right,
        } => {
            let left = render_value_scoped(
                left,
                reads,
                memory_read_count,
                call_results,
                external_results,
                arguments,
            )?;
            let right = render_value_scoped(
                right,
                reads,
                memory_read_count,
                call_results,
                external_results,
                arguments,
            )?;
            Ok(match operation {
                crate::ExpressionOperation::Add => {
                    format!("({left}).wrapping_add({right})")
                }
                crate::ExpressionOperation::Subtract => {
                    format!("({left}).wrapping_sub({right})")
                }
                crate::ExpressionOperation::Multiply => {
                    format!("({left}).wrapping_mul({right})")
                }
                crate::ExpressionOperation::DivideSigned => {
                    format!("riscv_div({left}, {right})")
                }
                crate::ExpressionOperation::DivideUnsigned => {
                    format!("riscv_divu({left}, {right})")
                }
                crate::ExpressionOperation::RemainderSigned => {
                    format!("riscv_rem({left}, {right})")
                }
                crate::ExpressionOperation::RemainderUnsigned => {
                    format!("riscv_remu({left}, {right})")
                }
                crate::ExpressionOperation::BitAnd => format!("({left}) & ({right})"),
                crate::ExpressionOperation::BitOr => format!("({left}) | ({right})"),
                crate::ExpressionOperation::BitXor => format!("({left}) ^ ({right})"),
                crate::ExpressionOperation::ShiftLeft => {
                    format!("({left}).wrapping_shl(({right}) & 31)")
                }
                crate::ExpressionOperation::ShiftRight => {
                    format!("({left}).wrapping_shr(({right}) & 31)")
                }
                crate::ExpressionOperation::ShiftRightArithmetic => {
                    format!("(({left}) as i32).wrapping_shr(({right}) & 31) as u32")
                }
                crate::ExpressionOperation::Equal => {
                    format!("u32::from(({left}) == ({right}))")
                }
                crate::ExpressionOperation::LessThanSigned => {
                    format!("u32::from((({left}) as i32) < (({right}) as i32))")
                }
                crate::ExpressionOperation::LessThanUnsigned => {
                    format!("u32::from(({left}) < ({right}))")
                }
                crate::ExpressionOperation::CountLeadingZeros => {
                    format!("({left}).leading_zeros()")
                }
                crate::ExpressionOperation::CountTrailingZeros => {
                    format!("({left}).trailing_zeros()")
                }
                crate::ExpressionOperation::PopulationCount => {
                    format!("({left}).count_ones()")
                }
            })
        }
        SymbolicValue::FloatingPoint {
            operation,
            rounding,
            ..
        } => Err(format!(
            "floating-point value {operation:?} with {rounding:?} has no executable reference model"
        )),
        SymbolicValue::WideSignedDivide {
            dividend_low,
            dividend_high,
            divisor_low,
            divisor_high,
            high_word,
        } => {
            let dividend_low = render_value_scoped(
                dividend_low,
                reads,
                memory_read_count,
                call_results,
                external_results,
                arguments,
            )?;
            let dividend_high = render_value_scoped(
                dividend_high,
                reads,
                memory_read_count,
                call_results,
                external_results,
                arguments,
            )?;
            let divisor_low = render_value_scoped(
                divisor_low,
                reads,
                memory_read_count,
                call_results,
                external_results,
                arguments,
            )?;
            let divisor_high = render_value_scoped(
                divisor_high,
                reads,
                memory_read_count,
                call_results,
                external_results,
                arguments,
            )?;
            Ok(format!(
                "riscv_div_i64_words({dividend_low}, {dividend_high}, {divisor_low}, {divisor_high}).{}",
                usize::from(*high_word)
            ))
        }
        SymbolicValue::RegisterImage {
            read_token,
            address,
            and_mask,
            or_mask,
        } => {
            validate_read(reads, *read_token, *address)?;
            Ok(format!(
                "(read{read_token} & {and_mask:#010x}_u32) | {or_mask:#010x}_u32"
            ))
        }
        SymbolicValue::IndexedRegisterImage {
            read_token,
            and_mask,
            or_mask,
        } => {
            validate_indexed_read(reads, *read_token)?;
            Ok(format!(
                "(read{read_token} & {and_mask:#010x}_u32) | {or_mask:#010x}_u32"
            ))
        }
        SymbolicValue::MemoryImage {
            read_token,
            and_mask,
            or_mask,
        } => {
            if !usize::try_from(*read_token).is_ok_and(|token| token < memory_read_count) {
                return Err(format!(
                    "symbolic value refers to missing memory read token {read_token}"
                ));
            }
            Ok(format!(
                "(memory_read{read_token} & {and_mask:#010x}_u32) | {or_mask:#010x}_u32"
            ))
        }
        SymbolicValue::Bits(bits) => {
            let mut constant = 0_u32;
            let mut groups = BTreeMap::<BitGroup, u32>::new();
            for (destination, source) in bits.iter().copied().enumerate() {
                match source {
                    BitSource::Unknown => {
                        return Err(format!("symbolic bit {destination} is unresolved"));
                    }
                    BitSource::Constant(false) => {}
                    BitSource::Constant(true) => constant |= 1 << destination,
                    BitSource::Input {
                        index,
                        bit,
                        inverted,
                    } => {
                        if usize::from(index) >= RV32_MODELED_ARGUMENT_COUNT {
                            return Err(format!(
                                "argument index {index} is outside the modeled RV32 ABI"
                            ));
                        }
                        let group = BitGroup {
                            source: SourceWord::Argument(index),
                            inverted,
                            shift: destination as i8 - bit as i8,
                        };
                        *groups.entry(group).or_default() |= 1 << destination;
                    }
                    BitSource::Register {
                        read_token,
                        address,
                        bit,
                        inverted,
                    } => {
                        validate_read(reads, read_token, address)?;
                        let group = BitGroup {
                            source: SourceWord::Read(read_token),
                            inverted,
                            shift: destination as i8 - bit as i8,
                        };
                        *groups.entry(group).or_default() |= 1 << destination;
                    }
                    BitSource::IndexedRegister {
                        read_token,
                        bit,
                        inverted,
                    } => {
                        validate_indexed_read(reads, read_token)?;
                        let group = BitGroup {
                            source: SourceWord::Read(read_token),
                            inverted,
                            shift: destination as i8 - bit as i8,
                        };
                        *groups.entry(group).or_default() |= 1 << destination;
                    }
                    BitSource::Memory {
                        read_token,
                        bit,
                        inverted,
                    } => {
                        if !usize::try_from(read_token).is_ok_and(|token| token < memory_read_count)
                        {
                            return Err(format!(
                                "symbolic value refers to missing memory read token {read_token}"
                            ));
                        }
                        let group = BitGroup {
                            source: SourceWord::MemoryRead(read_token),
                            inverted,
                            shift: destination as i8 - bit as i8,
                        };
                        *groups.entry(group).or_default() |= 1 << destination;
                    }
                    BitSource::PrivateStack { read_token, .. } => {
                        return Err(format!(
                            "private-stack read token {read_token} escaped reference composition"
                        ));
                    }
                    BitSource::CallResult {
                        call_token,
                        bit,
                        inverted,
                    } => {
                        if !call_result_available(call_token, call_results) {
                            return Err(format!(
                                "symbolic bit refers to unmodeled call result {call_token}"
                            ));
                        }
                        let group = BitGroup {
                            source: SourceWord::CallResult(call_token),
                            inverted,
                            shift: destination as i8 - bit as i8,
                        };
                        *groups.entry(group).or_default() |= 1 << destination;
                    }
                    BitSource::ExternalResult {
                        call_token,
                        bit,
                        inverted,
                    } => {
                        let call_token = call_token & !ALLOCATED_EXTERNAL_RESULT_TOKEN_FLAG;
                        if usize::try_from(call_token)
                            .ok()
                            .is_none_or(|token| token >= external_results)
                        {
                            return Err(format!(
                                "symbolic bit refers to missing external-call token {call_token}"
                            ));
                        }
                        let group = BitGroup {
                            source: SourceWord::ExternalResult(call_token),
                            inverted,
                            shift: destination as i8 - bit as i8,
                        };
                        *groups.entry(group).or_default() |= 1 << destination;
                    }
                    BitSource::ExternalResultHigh {
                        call_token,
                        bit,
                        inverted,
                    } => {
                        if usize::try_from(call_token)
                            .ok()
                            .is_none_or(|token| token >= external_results)
                        {
                            return Err(format!(
                                "symbolic bit refers to missing external-call token {call_token}"
                            ));
                        }
                        let group = BitGroup {
                            source: SourceWord::ExternalResultHigh(call_token),
                            inverted,
                            shift: destination as i8 - bit as i8,
                        };
                        *groups.entry(group).or_default() |= 1 << destination;
                    }
                    BitSource::ExternalOutput {
                        call_token,
                        output_index,
                        bit,
                        inverted,
                    } => {
                        if usize::try_from(call_token)
                            .ok()
                            .is_none_or(|token| token >= external_results)
                        {
                            return Err(format!(
                                "symbolic bit refers to missing external-call token {call_token}"
                            ));
                        }
                        let group = BitGroup {
                            source: SourceWord::ExternalOutput {
                                call_token,
                                output_index,
                            },
                            inverted,
                            shift: destination as i8 - bit as i8,
                        };
                        *groups.entry(group).or_default() |= 1 << destination;
                    }
                }
            }

            let mut terms = Vec::new();
            if constant != 0 {
                terms.push(format!("{constant:#010x}_u32"));
            }
            terms.extend(
                groups
                    .into_iter()
                    .map(|(group, mask)| grouped_expression(group, mask, arguments)),
            );
            Ok(if terms.is_empty() {
                "0x00000000_u32".to_owned()
            } else {
                terms.join(" | ")
            })
        }
    }
}

#[cfg(test)]
pub(super) fn render_value(
    value: &SymbolicValue,
    reads: &[u32],
    memory_reads: &[u32],
    external_results: usize,
) -> Result<String, String> {
    let arguments = core::array::from_fn(|index| format!("args[{index}]"));
    let reads = reads
        .iter()
        .copied()
        .map(MmioReadAddress::Static)
        .collect::<Vec<_>>();
    render_value_scoped(
        value,
        &reads,
        memory_reads.len(),
        &[],
        external_results,
        &arguments,
    )
}
