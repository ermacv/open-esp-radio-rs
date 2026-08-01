//! Fail-closed Rust generation for exact supported symbolic traces.
//!
//! The output is an executable reference model, not a guessed production
//! driver. It deliberately exposes ordered MMIO through a trait and reports an
//! unresolved return value as `None` instead of inventing a C prototype.

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write as _,
};

use crate::{
    Access, BitSource, BranchCondition, BranchOperation, Event, ReferenceEvent, ReferenceFlow,
    ReferenceTerminator, Trace, Value,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct GeneratedReference {
    pub(crate) source: String,
    pub(crate) exit_a0_modeled: bool,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum SourceWord {
    Argument(u8),
    Read(u32),
    MemoryRead(u32),
    CallResult(u32),
    ExternalResult(u32),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MmioReadAddress {
    Static(u32),
    Indexed,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct BitGroup {
    source: SourceWord,
    inverted: bool,
    shift: i8,
}

fn source_word(group: BitGroup, arguments: &[String; 8]) -> String {
    let source = match group.source {
        SourceWord::Argument(index) => arguments[usize::from(index)].clone(),
        SourceWord::Read(token) => format!("read{token}"),
        SourceWord::MemoryRead(token) => format!("memory_read{token}"),
        SourceWord::CallResult(token) => format!("call_result{token}"),
        SourceWord::ExternalResult(token) => format!("external_result{token}"),
    };
    if group.inverted {
        format!("!{source}")
    } else {
        source
    }
}

fn grouped_expression(group: BitGroup, mask: u32, arguments: &[String; 8]) -> String {
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

fn render_value_scoped(
    value: &Value,
    reads: &[MmioReadAddress],
    memory_read_count: usize,
    call_results: &[bool],
    external_results: usize,
    arguments: &[String; 8],
) -> Result<String, String> {
    match value {
        Value::Unknown => Err("symbolic value is unresolved".to_owned()),
        Value::Constant(value) => Ok(format!("{value:#010x}_u32")),
        Value::InputConstant { index, .. } => render_value_scoped(
            &Value::input(*index),
            reads,
            memory_read_count,
            call_results,
            external_results,
            arguments,
        ),
        Value::StackAddress(offset) => Err(format!(
            "private stack address {offset:+#x} escaped into generated behavior"
        )),
        Value::SymbolAddress {
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
        Value::CallResult(token) => {
            if usize::try_from(*token)
                .ok()
                .and_then(|token| call_results.get(token))
                .copied()
                == Some(true)
            {
                Ok(format!("call_result{token}"))
            } else {
                Err(format!(
                    "unmodeled call result {token} escaped into generated behavior"
                ))
            }
        }
        Value::ExternalTable(table) => Err(format!(
            "external ABI table {} escaped into generated behavior",
            crate::external_abi::table_spec(*table).id
        )),
        Value::ExternalFunction { table, function } => Err(format!(
            "external ABI function {}::{function:?} escaped into generated behavior",
            crate::external_abi::table_spec(*table).id
        )),
        Value::ExternalResult(token) => {
            if usize::try_from(*token).is_ok_and(|token| token < external_results) {
                Ok(format!("external_result{token}"))
            } else {
                Err(format!(
                    "symbolic value refers to missing external-call token {token}"
                ))
            }
        }
        Value::Expression {
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
            })
        }
        Value::RegisterImage {
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
        Value::IndexedRegisterImage {
            read_token,
            and_mask,
            or_mask,
        } => {
            validate_indexed_read(reads, *read_token)?;
            Ok(format!(
                "(read{read_token} & {and_mask:#010x}_u32) | {or_mask:#010x}_u32"
            ))
        }
        Value::MemoryImage {
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
        Value::Bits(bits) => {
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
                        if index >= 8 {
                            return Err(format!("argument index {index} is outside the RV32 ABI"));
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
                    BitSource::CallResult {
                        call_token,
                        bit,
                        inverted,
                    } => {
                        if usize::try_from(call_token)
                            .ok()
                            .and_then(|token| call_results.get(token))
                            .copied()
                            != Some(true)
                        {
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
fn render_value(
    value: &Value,
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

fn sanitize_identifier(symbol: &str) -> String {
    let mut output = String::from("open_phy_reference_");
    for character in symbol.chars() {
        if character.is_ascii_alphanumeric() || character == '_' {
            output.push(character);
        } else {
            output.push('_');
        }
    }
    output
}

fn comment_text(value: &str) -> String {
    value.replace(['\r', '\n'], " ")
}

#[derive(Clone, Debug)]
struct RenderState {
    reads: Vec<MmioReadAddress>,
    mmio_access_count: usize,
    memory_read_count: usize,
    memory_access_count: usize,
    call_results: Vec<bool>,
    external_results: Vec<crate::external_abi::Function>,
    validated_external_tables: BTreeSet<crate::external_abi::Table>,
    arguments: [String; 8],
}

impl Default for RenderState {
    fn default() -> Self {
        Self {
            reads: Vec::new(),
            mmio_access_count: 0,
            memory_read_count: 0,
            memory_access_count: 0,
            call_results: Vec::new(),
            external_results: Vec::new(),
            validated_external_tables: BTreeSet::new(),
            arguments: core::array::from_fn(|index| format!("args[{index}]")),
        }
    }
}

fn render_state_value(value: &Value, state: &RenderState) -> Result<String, String> {
    render_value_scoped(
        value,
        &state.reads,
        state.memory_read_count,
        &state.call_results,
        state.external_results.len(),
        &state.arguments,
    )
}

fn render_condition(condition: &BranchCondition, state: &RenderState) -> Result<String, String> {
    let left = render_state_value(&condition.left, state)?;
    let right = render_state_value(&condition.right, state)?;
    Ok(match condition.operation {
        BranchOperation::Equal => format!("({left}) == ({right})"),
        BranchOperation::NotEqual => format!("({left}) != ({right})"),
        BranchOperation::LessSigned => format!("(({left}) as i32) < (({right}) as i32)"),
        BranchOperation::GreaterEqualSigned => {
            format!("(({left}) as i32) >= (({right}) as i32)")
        }
        BranchOperation::LessUnsigned => format!("({left}) < ({right})"),
        BranchOperation::GreaterEqualUnsigned => format!("({left}) >= ({right})"),
    })
}

fn render_events(
    output: &mut String,
    events: &[ReferenceEvent],
    state: &mut RenderState,
    indent: &str,
) -> Result<(), String> {
    for event in events {
        match event {
            ReferenceEvent::Observable(Event::Memory {
                access: Access::Read,
                width,
                address,
                register,
                value: None,
            }) => {
                let token = state.reads.len();
                writeln!(output, "{indent}// Read {}.", comment_text(register)).unwrap();
                writeln!(
                    output,
                    "{indent}let read{token} = io.read({width}, {address:#010x}_u32);"
                )
                .unwrap();
                writeln!(output, "{indent}let _ = read{token};").unwrap();
                state.reads.push(MmioReadAddress::Static(*address));
            }
            ReferenceEvent::Observable(Event::Memory {
                access: Access::Write,
                width,
                address,
                register,
                value: Some(value),
            }) => {
                let value = render_state_value(value, state)?;
                writeln!(output, "{indent}// Write {}.", comment_text(register)).unwrap();
                writeln!(
                    output,
                    "{indent}io.write({width}, {address:#010x}_u32, {value});"
                )
                .unwrap();
            }
            ReferenceEvent::Observable(Event::Memory {
                access: Access::Read,
                value: Some(_),
                ..
            }) => return Err("internal IR error: MMIO read carries a write value".to_owned()),
            ReferenceEvent::Observable(Event::Memory {
                access: Access::Write,
                value: None,
                ..
            }) => return Err("internal IR error: MMIO write has no symbolic value".to_owned()),
            ReferenceEvent::IndexedMmio {
                access,
                width,
                address,
                registers,
                guard,
                value,
            } => {
                let access_token = state.mmio_access_count;
                state.mmio_access_count += 1;
                if registers.is_empty() {
                    return Err("indexed MMIO event has no SVD register domain".to_owned());
                }
                if let Some(guard) = guard {
                    let selector = render_state_value(&guard.selector, state)?;
                    writeln!(
                        output,
                        "{indent}let mmio_selector{access_token} = {selector};"
                    )
                    .unwrap();
                    writeln!(
                        output,
                        "{indent}assert!(mmio_selector{access_token} <= {:#010x}_u32, \"indexed MMIO selector is outside the recovered SVD register bank\");",
                        guard.maximum
                    )
                    .unwrap();
                }
                let address = render_state_value(address, state)?;
                let domain = registers
                    .iter()
                    .map(|register| format!("{:#010x}_u32", register.address))
                    .collect::<Vec<_>>()
                    .join(" | ");
                let names = registers
                    .iter()
                    .map(|register| register.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                writeln!(
                    output,
                    "{indent}// Indexed MMIO SVD bank: {}.",
                    comment_text(&names)
                )
                .unwrap();
                writeln!(
                    output,
                    "{indent}let mmio_address{access_token} = {address};"
                )
                .unwrap();
                writeln!(
                    output,
                    "{indent}assert!(matches!(mmio_address{access_token}, {domain}), \"indexed MMIO address is outside the recovered SVD register bank\");"
                )
                .unwrap();
                match (access, value) {
                    (Access::Read, None) => {
                        let token = state.reads.len();
                        writeln!(
                            output,
                            "{indent}let read{token} = io.read({width}, mmio_address{access_token});"
                        )
                        .unwrap();
                        writeln!(output, "{indent}let _ = read{token};").unwrap();
                        state.reads.push(MmioReadAddress::Indexed);
                    }
                    (Access::Write, Some(value)) => {
                        let value = render_state_value(value, state)?;
                        writeln!(
                            output,
                            "{indent}io.write({width}, mmio_address{access_token}, {value});"
                        )
                        .unwrap();
                    }
                    (Access::Read, Some(_)) => {
                        return Err(
                            "internal IR error: indexed MMIO read carries a write value".to_owned()
                        );
                    }
                    (Access::Write, None) => {
                        return Err(
                            "internal IR error: indexed MMIO write has no symbolic value"
                                .to_owned(),
                        );
                    }
                }
            }
            ReferenceEvent::Observable(Event::Fence {
                fm,
                predecessor,
                successor,
            }) => {
                writeln!(
                    output,
                    "{indent}io.fence({fm:#04x}, {predecessor:#04x}, {successor:#04x});"
                )
                .unwrap();
            }
            ReferenceEvent::DelayMicros { micros } => {
                let micros = render_state_value(micros, state)?;
                writeln!(output, "{indent}io.delay_micros({micros});").unwrap();
            }
            ReferenceEvent::Memory {
                access: Access::Read,
                width,
                address,
                region,
                value: None,
            } => {
                let token = state.memory_read_count;
                let address = render_state_value(address, state)?;
                let access_token = state.memory_access_count;
                state.memory_access_count += 1;
                writeln!(
                    output,
                    "{indent}// Read ELF/RAM region {}.",
                    comment_text(region)
                )
                .unwrap();
                writeln!(
                    output,
                    "{indent}let memory_address{access_token} = {address};"
                )
                .unwrap();
                writeln!(
                    output,
                    "{indent}let memory_read{token} = memory.read({width}, memory_address{access_token});"
                )
                .unwrap();
                writeln!(output, "{indent}let _ = memory_read{token};").unwrap();
                state.memory_read_count += 1;
            }
            ReferenceEvent::Memory {
                access: Access::Write,
                width,
                address,
                region,
                value: Some(value),
            } => {
                let address = render_state_value(address, state)?;
                let value = render_state_value(value, state)?;
                let access_token = state.memory_access_count;
                state.memory_access_count += 1;
                writeln!(
                    output,
                    "{indent}// Write ELF/RAM region {}.",
                    comment_text(region)
                )
                .unwrap();
                writeln!(
                    output,
                    "{indent}let memory_address{access_token} = {address};"
                )
                .unwrap();
                writeln!(output, "{indent}let memory_value{access_token} = {value};").unwrap();
                writeln!(
                    output,
                    "{indent}memory.write({width}, memory_address{access_token}, memory_value{access_token});"
                )
                .unwrap();
            }
            ReferenceEvent::Memory {
                access: Access::Read,
                value: Some(_),
                ..
            } => return Err("internal IR error: memory read carries a write value".to_owned()),
            ReferenceEvent::Memory {
                access: Access::Write,
                value: None,
                ..
            } => return Err("internal IR error: memory write has no symbolic value".to_owned()),
            ReferenceEvent::ExternalCall {
                token,
                table,
                function,
                arguments,
            } => {
                if usize::try_from(*token).ok() != Some(state.external_results.len()) {
                    return Err(format!(
                        "external call token {token} is not ordered in generated behavior"
                    ));
                }
                let slot = crate::external_abi::function(*table, *function);
                let table_spec = crate::external_abi::table_spec(*table);
                if state.validated_external_tables.insert(*table) {
                    writeln!(
                        output,
                        "{indent}assert_eq!(platform.wifi_osi_version(), {:#010x}_u32, \"external ABI version mismatch for {}\");",
                        table_spec.version,
                        table_spec.id
                    )
                    .unwrap();
                    writeln!(
                        output,
                        "{indent}assert_eq!(platform.wifi_osi_magic(), {:#010x}_u32, \"external ABI magic mismatch for {}\");",
                        table_spec.magic,
                        table_spec.id
                    )
                    .unwrap();
                    writeln!(
                        output,
                        "{indent}assert_eq!(platform.wifi_osi_table_size(), {:#010x}_u32, \"external ABI size mismatch for {}\");",
                        table_spec.size,
                        table_spec.id
                    )
                    .unwrap();
                }
                if slot.argument_count != 0
                    && !matches!(function, crate::external_abi::Function::CoexPtiGet)
                {
                    return Err(format!(
                        "external ABI function {}::{} requires unsupported arguments",
                        crate::external_abi::table_spec(*table).id,
                        slot.c_name
                    ));
                }
                writeln!(
                    output,
                    "{indent}// External ABI {}+{:#x}: {}.",
                    table_spec.id, slot.offset, slot.c_name
                )
                .unwrap();
                let call = match function {
                    crate::external_abi::Function::EnvIsChip => {
                        "u32::from(platform.wifi_osi_env_is_chip())".to_owned()
                    }
                    crate::external_abi::Function::Rand => "platform.wifi_osi_rand()".to_owned(),
                    crate::external_abi::Function::Random => {
                        "platform.wifi_osi_random()".to_owned()
                    }
                    crate::external_abi::Function::SlowClockCalibrationGet => {
                        "platform.wifi_osi_slowclk_cal_get()".to_owned()
                    }
                    crate::external_abi::Function::CoexPtiGet => {
                        let event = render_state_value(&arguments[0], state)?;
                        format!("u32::from(platform.wifi_osi_coex_pti_get({event}))")
                    }
                };
                writeln!(output, "{indent}let external_result{token} = {call};").unwrap();
                if let crate::external_abi::ReturnModel::Constant(expected) = slot.return_model {
                    writeln!(
                        output,
                        "{indent}assert_eq!(external_result{token}, {expected:#010x}_u32, \"external ABI profile mismatch for {}\");",
                        slot.c_name
                    )
                    .unwrap();
                }
                writeln!(output, "{indent}let _ = external_result{token};").unwrap();
                state.external_results.push(*function);
            }
            ReferenceEvent::DiagnosticCall {
                function,
                argument_count,
                arguments,
            } => {
                if function != "wifi_log" || *argument_count != 6 {
                    return Err(format!(
                        "unsupported diagnostic call shape: {function} with {argument_count} arguments"
                    ));
                }
                let arguments = arguments
                    .iter()
                    .take(usize::from(*argument_count))
                    .map(|value| render_state_value(value, state))
                    .collect::<Result<Vec<_>, _>>()?;
                writeln!(output, "{indent}// Named diagnostic call: wifi_log.").unwrap();
                writeln!(
                    output,
                    "{indent}platform.wifi_log([{}]);",
                    arguments.join(", ")
                )
                .unwrap();
            }
            ReferenceEvent::ComposedCall {
                token,
                symbol,
                arguments,
                flow,
                result_modeled,
            } => {
                if usize::try_from(*token).ok() != Some(state.call_results.len()) {
                    return Err(format!(
                        "composed call token {token} is not ordered in generated behavior"
                    ));
                }
                let mut child_state = RenderState::default();
                for index in crate::reference_flow_input_indices(flow) {
                    let argument = render_state_value(&arguments[usize::from(index)], state)?;
                    let name = format!("call{token}_arg{index}");
                    writeln!(output, "{indent}let {name} = {argument};").unwrap();
                    child_state.arguments[usize::from(index)] = name;
                }
                writeln!(
                    output,
                    "{indent}// Composed direct call: {}.",
                    comment_text(symbol)
                )
                .unwrap();
                let assignment = if *result_modeled {
                    format!("let call_result{token} = ")
                } else {
                    String::new()
                };
                writeln!(output, "{indent}{assignment}{{").unwrap();
                let child_indent = format!("{indent}    ");
                render_flow(
                    output,
                    flow,
                    child_state,
                    &child_indent,
                    if *result_modeled {
                        FlowReturn::Scalar
                    } else {
                        FlowReturn::Unit
                    },
                )?;
                writeln!(output, "{indent}}};").unwrap();
                if *result_modeled {
                    writeln!(output, "{indent}let _ = call_result{token};").unwrap();
                }
                state.call_results.push(*result_modeled);
            }
            ReferenceEvent::TailCall { site, target, .. } => {
                return Err(format!(
                    "internal IR error: unresolved tail call at {site:#010x} to {target:#010x}"
                ));
            }
            ReferenceEvent::Call {
                token,
                site,
                target,
                ..
            } => {
                return Err(format!(
                    "internal IR error: unresolved call {token} at {site:#010x} to {target:#010x}"
                ));
            }
            ReferenceEvent::BranchDecision { condition, .. } => {
                return Err(format!(
                    "internal IR error: branch decision at {:#010x} escaped structured flow",
                    condition.site
                ));
            }
        }
    }
    Ok(())
}

fn render_outcome(
    output: &mut String,
    value: &Value,
    state: &RenderState,
    indent: &str,
) -> Result<(), String> {
    let available_calls = state
        .call_results
        .iter()
        .copied()
        .enumerate()
        .map(|(token, modeled)| (token as u32, modeled))
        .collect::<BTreeMap<_, _>>();
    let exit_a0 =
        if value.is_resolved() && crate::value_call_results_available(value, &available_calls) {
            format!("Some({})", render_state_value(value, state)?)
        } else {
            "None".to_owned()
        };
    writeln!(output, "{indent}ReferenceOutcome {{ exit_a0: {exit_a0} }}").unwrap();
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FlowReturn {
    Outcome,
    Scalar,
    Unit,
}

fn render_flow(
    output: &mut String,
    flow: &ReferenceFlow,
    mut state: RenderState,
    indent: &str,
    return_kind: FlowReturn,
) -> Result<(), String> {
    render_events(output, &flow.events, &mut state, indent)?;
    match &flow.terminator {
        ReferenceTerminator::Return(value) => match return_kind {
            FlowReturn::Outcome => render_outcome(output, value, &state, indent),
            FlowReturn::Scalar => {
                if !value.is_resolved() {
                    return Err("composed callee has an unresolved `a0` return".to_owned());
                }
                writeln!(output, "{indent}{}", render_state_value(value, &state)?).unwrap();
                Ok(())
            }
            FlowReturn::Unit => {
                writeln!(output, "{indent}()").unwrap();
                Ok(())
            }
        },
        ReferenceTerminator::Branch {
            condition,
            taken,
            not_taken,
        } => {
            let condition_text = render_condition(condition, &state)?;
            writeln!(
                output,
                "{indent}// Symbolic branch from {:#010x}.",
                condition.site
            )
            .unwrap();
            writeln!(output, "{indent}if {condition_text} {{").unwrap();
            let child_indent = format!("{indent}    ");
            render_flow(output, taken, state.clone(), &child_indent, return_kind)?;
            writeln!(output, "{indent}}} else {{").unwrap();
            render_flow(output, not_taken, state, &child_indent, return_kind)?;
            writeln!(output, "{indent}}}").unwrap();
            Ok(())
        }
    }
}

fn collect_external_tables(
    flow: &ReferenceFlow,
    output: &mut BTreeSet<crate::external_abi::Table>,
) {
    for event in &flow.events {
        match event {
            ReferenceEvent::ExternalCall { table, .. } => {
                output.insert(*table);
            }
            ReferenceEvent::ComposedCall { flow, .. } => collect_external_tables(flow, output),
            _ => {}
        }
    }
    if let ReferenceTerminator::Branch {
        taken, not_taken, ..
    } = &flow.terminator
    {
        collect_external_tables(taken, output);
        collect_external_tables(not_taken, output);
    }
}

pub(crate) fn generate(
    trace: &Trace,
    artifact: &str,
    artifact_sha256: &str,
    member: Option<&str>,
    companions: &[(String, String)],
) -> Result<GeneratedReference, String> {
    if !trace.is_reference_eligible() {
        let mut reasons = trace.blockers.clone();
        reasons.extend(trace.reference_blockers.iter().cloned());
        reasons.extend(
            trace
                .events
                .iter()
                .filter_map(Event::unmapped_address)
                .map(|address| format!("unmapped-register {address:#010x}")),
        );
        return Err(format!(
            "{} is not eligible for reference generation: {}",
            trace.symbol,
            reasons.join("; ")
        ));
    }

    let function_name = sanitize_identifier(&trace.symbol);
    let exit_a0_modeled = trace.reference_exit_a0_modeled();
    let mut output = String::new();
    writeln!(
        output,
        "// @generated by open-esp-radio-phy-trace; do not edit."
    )
    .unwrap();
    writeln!(
        output,
        "// Generator version: {}",
        env!("CARGO_PKG_VERSION")
    )
    .unwrap();
    writeln!(output, "// Source artifact: {}", comment_text(artifact)).unwrap();
    writeln!(output, "// Source SHA-256: {artifact_sha256}").unwrap();
    if let Some(member) = member {
        writeln!(output, "// Archive member: {}", comment_text(member)).unwrap();
    }
    for (path, sha256) in companions {
        writeln!(output, "// Companion artifact: {}", comment_text(path)).unwrap();
        writeln!(output, "// Companion SHA-256: {sha256}").unwrap();
    }
    writeln!(output, "// Source symbol: {}", comment_text(&trace.symbol)).unwrap();
    for dependency in &trace.reference_dependencies {
        writeln!(
            output,
            "// Composed direct-call dependency: {}",
            comment_text(dependency)
        )
        .unwrap();
    }
    let mut external_tables = BTreeSet::new();
    for event in &trace.reference_events {
        match event {
            ReferenceEvent::ExternalCall { table, .. } => {
                external_tables.insert(*table);
            }
            ReferenceEvent::ComposedCall { flow, .. } => {
                collect_external_tables(flow, &mut external_tables);
            }
            _ => {}
        }
    }
    if let Some(flow) = &trace.reference_flow {
        collect_external_tables(flow, &mut external_tables);
    }
    for table in external_tables {
        let spec = crate::external_abi::table_spec(table);
        writeln!(output, "// External ABI: {}", spec.id).unwrap();
        writeln!(output, "// External ABI pointer: {}", spec.pointer_symbol).unwrap();
        writeln!(output, "// External ABI backing: {}", spec.backing_symbol).unwrap();
        writeln!(output, "// External ABI version: {:#010x}", spec.version).unwrap();
        writeln!(output, "// External ABI magic: {:#010x}", spec.magic).unwrap();
        writeln!(output, "// External ABI size: {:#x}", spec.size).unwrap();
        writeln!(
            output,
            "// External ABI magic offset: {:#x}",
            spec.magic_offset
        )
        .unwrap();
        writeln!(
            output,
            "// External ABI source commit: {}",
            spec.source_commit
        )
        .unwrap();
        writeln!(output, "// External ABI source: {}", spec.source_header).unwrap();
        writeln!(
            output,
            "// External ABI source SHA-256: {}",
            spec.source_sha256
        )
        .unwrap();
    }
    writeln!(
        output,
        "// Exit a0: {}",
        if exit_a0_modeled {
            "modeled"
        } else {
            "unresolved"
        }
    )
    .unwrap();
    writeln!(output).unwrap();
    writeln!(
        output,
        "/// Ordered MMIO/delay/fence boundary used by the generated reference model."
    )
    .unwrap();
    writeln!(output, "#[allow(dead_code)]").unwrap();
    writeln!(output, "pub trait ReferenceIo {{").unwrap();
    writeln!(
        output,
        "    /// Returns the zero-extended value observed by a read of `width` bits."
    )
    .unwrap();
    writeln!(
        output,
        "    fn read(&mut self, width: u8, address: u32) -> u32;"
    )
    .unwrap();
    writeln!(
        output,
        "    /// Records a write; only the low `width` bits are observable."
    )
    .unwrap();
    writeln!(
        output,
        "    fn write(&mut self, width: u8, address: u32, value: u32);"
    )
    .unwrap();
    writeln!(output, "    fn delay_micros(&mut self, micros: u32);").unwrap();
    writeln!(
        output,
        "    fn fence(&mut self, fm: u8, predecessor: u8, successor: u8);"
    )
    .unwrap();
    writeln!(output, "}}").unwrap();
    writeln!(output).unwrap();
    writeln!(
        output,
        "/// CPU-visible ELF/RAM state used by the generated reference model."
    )
    .unwrap();
    writeln!(
        output,
        "/// Implementations must reject ABI-derived addresses outside declared CPU-owned ranges."
    )
    .unwrap();
    writeln!(
        output,
        "/// MMIO and undeclared, interrupt-owned, DMA-owned or shared memory are not valid here."
    )
    .unwrap();
    writeln!(output, "#[allow(dead_code)]").unwrap();
    writeln!(output, "pub trait ReferenceMemory {{").unwrap();
    writeln!(
        output,
        "    /// Resolves an archive/ELF symbol in the exact linked image used by the scenario."
    )
    .unwrap();
    writeln!(
        output,
        "    fn symbol_address(&mut self, member: Option<&str>, symbol: &str) -> u32;"
    )
    .unwrap();
    writeln!(
        output,
        "    /// Returns the zero-extended value currently stored in `width` bits."
    )
    .unwrap();
    writeln!(
        output,
        "    fn read(&mut self, width: u8, address: u32) -> u32;"
    )
    .unwrap();
    writeln!(
        output,
        "    /// Updates only the low `width` bits at the addressed location."
    )
    .unwrap();
    writeln!(
        output,
        "    fn write(&mut self, width: u8, address: u32, value: u32);"
    )
    .unwrap();
    writeln!(output, "}}").unwrap();
    writeln!(output).unwrap();
    writeln!(
        output,
        "/// Platform callbacks reached through the pinned ESP32-S31 Wi-Fi OSI ABI."
    )
    .unwrap();
    writeln!(output, "#[allow(dead_code)]").unwrap();
    writeln!(output, "pub trait ReferencePlatform {{").unwrap();
    writeln!(output, "    fn wifi_osi_version(&mut self) -> u32;").unwrap();
    writeln!(output, "    fn wifi_osi_magic(&mut self) -> u32;").unwrap();
    writeln!(output, "    fn wifi_osi_table_size(&mut self) -> u32;").unwrap();
    writeln!(output, "    fn wifi_osi_env_is_chip(&mut self) -> bool;").unwrap();
    writeln!(output, "    fn wifi_osi_rand(&mut self) -> u32;").unwrap();
    writeln!(output, "    fn wifi_osi_random(&mut self) -> u32;").unwrap();
    writeln!(output, "    fn wifi_osi_slowclk_cal_get(&mut self) -> u32;").unwrap();
    writeln!(
        output,
        "    /// Returns the byte written through `_coex_pti_get` argument a1; its C status is intentionally not modeled."
    )
    .unwrap();
    writeln!(
        output,
        "    fn wifi_osi_coex_pti_get(&mut self, event: u32) -> u8;"
    )
    .unwrap();
    writeln!(output, "    fn wifi_log(&mut self, arguments: [u32; 6]);").unwrap();
    writeln!(output, "}}").unwrap();
    writeln!(output).unwrap();
    writeln!(output, "#[allow(dead_code)]").unwrap();
    writeln!(
        output,
        "fn riscv_hi20_lo12_address(symbol: u32, hi_addend: u32, lo_addend: u32) -> u32 {{"
    )
    .unwrap();
    writeln!(
        output,
        "    let high = symbol.wrapping_add(hi_addend).wrapping_add(0x00000800) & 0xfffff000;"
    )
    .unwrap();
    writeln!(
        output,
        "    let low = ((symbol.wrapping_add(lo_addend).wrapping_shl(20) as i32) >> 20) as u32;"
    )
    .unwrap();
    writeln!(output, "    high.wrapping_add(low)").unwrap();
    writeln!(output, "}}").unwrap();
    writeln!(output, "#[allow(dead_code)]").unwrap();
    writeln!(output, "fn riscv_div(left: u32, right: u32) -> u32 {{").unwrap();
    writeln!(output, "    if right == 0 {{ u32::MAX }} else if left == i32::MIN as u32 && right == u32::MAX {{ i32::MIN as u32 }} else {{ ((left as i32) / (right as i32)) as u32 }}").unwrap();
    writeln!(output, "}}").unwrap();
    writeln!(output, "#[allow(dead_code)]").unwrap();
    writeln!(output, "fn riscv_divu(left: u32, right: u32) -> u32 {{").unwrap();
    writeln!(output, "    left.checked_div(right).unwrap_or(u32::MAX)").unwrap();
    writeln!(output, "}}").unwrap();
    writeln!(output, "#[allow(dead_code)]").unwrap();
    writeln!(output, "fn riscv_rem(left: u32, right: u32) -> u32 {{").unwrap();
    writeln!(output, "    if right == 0 {{ left }} else if left == i32::MIN as u32 && right == u32::MAX {{ 0 }} else {{ ((left as i32) % (right as i32)) as u32 }}").unwrap();
    writeln!(output, "}}").unwrap();
    writeln!(output, "#[allow(dead_code)]").unwrap();
    writeln!(output, "fn riscv_remu(left: u32, right: u32) -> u32 {{").unwrap();
    writeln!(
        output,
        "    if right == 0 {{ left }} else {{ left % right }}"
    )
    .unwrap();
    writeln!(output, "}}").unwrap();
    writeln!(output).unwrap();
    writeln!(output, "#[derive(Clone, Copy, Debug, Eq, PartialEq)]").unwrap();
    writeln!(output, "pub struct ReferenceOutcome {{").unwrap();
    writeln!(
        output,
        "    /// Value of the ABI `a0` register at exit; this does not infer a C prototype."
    )
    .unwrap();
    writeln!(output, "    pub exit_a0: Option<u32>,").unwrap();
    writeln!(output, "}}").unwrap();
    writeln!(output).unwrap();
    writeln!(output, "#[allow(dead_code)]").unwrap();
    writeln!(output, "pub fn {function_name}(").unwrap();
    writeln!(output, "    io: &mut impl ReferenceIo,").unwrap();
    writeln!(output, "    memory: &mut impl ReferenceMemory,").unwrap();
    writeln!(output, "    platform: &mut impl ReferencePlatform,").unwrap();
    writeln!(output, "    args: [u32; 8],").unwrap();
    writeln!(output, ") -> ReferenceOutcome {{").unwrap();
    writeln!(output, "    let _ = &mut *io;").unwrap();
    writeln!(output, "    let _ = &mut *memory;").unwrap();
    writeln!(output, "    let _ = &mut *platform;").unwrap();
    writeln!(output, "    let _ = &args;").unwrap();

    let mut state = RenderState::default();
    if let Some(flow) = &trace.reference_flow {
        render_flow(&mut output, flow, state, "    ", FlowReturn::Outcome)?;
    } else {
        render_events(&mut output, &trace.reference_events, &mut state, "    ")?;
        render_outcome(&mut output, &trace.return_value, &state, "    ")?;
    }
    writeln!(output, "}}").unwrap();

    Ok(GeneratedReference {
        source: output,
        exit_a0_modeled,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_shifted_argument_bits_into_a_readable_expression() {
        let value = Value::input(0).and(1).shift_left(5);
        assert_eq!(
            render_value(&value, &[], &[], 0).unwrap(),
            "(args[0] << 5) & 0x00000020_u32"
        );
    }

    #[test]
    fn validates_the_address_behind_a_read_token() {
        let value = Value::RegisterImage {
            read_token: 0,
            address: 0x2010_7030,
            and_mask: u32::MAX,
            or_mask: 0,
        };
        assert!(render_value(&value, &[0x2010_7030], &[], 0).is_ok());
        assert!(render_value(&value, &[0x2010_7034], &[], 0).is_err());
    }

    #[test]
    fn distinguishes_static_and_indexed_read_tokens() {
        let value = Value::IndexedRegisterImage {
            read_token: 0,
            and_mask: u32::MAX,
            or_mask: 0,
        };
        let arguments = core::array::from_fn(|index| format!("args[{index}]"));

        assert!(
            render_value_scoped(&value, &[MmioReadAddress::Indexed], 0, &[], 0, &arguments,)
                .is_ok()
        );
        assert!(
            render_value_scoped(
                &value,
                &[MmioReadAddress::Static(0x2010_7030)],
                0,
                &[],
                0,
                &arguments,
            )
            .is_err()
        );
    }

    #[test]
    fn renders_external_results_through_exact_riscv_arithmetic() {
        let value = Value::expression(
            crate::ExpressionOperation::RemainderUnsigned,
            Value::ExternalResult(0),
            Value::Constant(11),
        )
        .add_constant(0xfa)
        .shift_left(21);

        let rendered = render_value(&value, &[], &[], 1).unwrap();
        assert!(rendered.contains("riscv_remu(external_result0, 0x0000000b_u32)"));
        assert!(rendered.contains("wrapping_add(0x000000fa_u32)"));
        assert!(rendered.contains("wrapping_shl"));
        assert!(render_value(&value, &[], &[], 0).is_err());
    }

    #[test]
    fn renders_dynamic_arithmetic_shift_with_rv32_masking() {
        let value = Value::expression(
            crate::ExpressionOperation::ShiftRightArithmetic,
            Value::Constant((-0x81_i32) as u32),
            Value::input(0),
        );

        assert_eq!(
            render_value(&value, &[], &[], 0).unwrap(),
            "((0xffffff7f_u32) as i32).wrapping_shr((args[0] & 0xffffffff_u32) & 31) as u32"
        );
    }

    #[test]
    fn signed_branch_casts_the_complete_rendered_expression() {
        let condition = BranchCondition {
            site: 0,
            operation: BranchOperation::LessSigned,
            left: Value::input(1),
            right: Value::Constant(0),
        };

        assert_eq!(
            render_condition(&condition, &RenderState::default()).unwrap(),
            "((args[1] & 0xffffffff_u32) as i32) < ((0x00000000_u32) as i32)"
        );
    }

    #[test]
    fn generates_a_self_contained_ordered_reference() {
        let trace = Trace {
            symbol: "phy-example".to_owned(),
            events: vec![
                Event::Memory {
                    access: Access::Read,
                    width: 32,
                    address: 0x2010_7030,
                    register: "AGC.CONTROL".to_owned(),
                    value: None,
                },
                Event::Memory {
                    access: Access::Write,
                    width: 32,
                    address: 0x2010_7030,
                    register: "AGC.CONTROL".to_owned(),
                    value: Some(Value::RegisterImage {
                        read_token: 0,
                        address: 0x2010_7030,
                        and_mask: 0xffff_fffe,
                        or_mask: 1,
                    }),
                },
            ],
            reference_events: vec![
                ReferenceEvent::Observable(Event::Memory {
                    access: Access::Read,
                    width: 32,
                    address: 0x2010_7030,
                    register: "AGC.CONTROL".to_owned(),
                    value: None,
                }),
                ReferenceEvent::Observable(Event::Memory {
                    access: Access::Write,
                    width: 32,
                    address: 0x2010_7030,
                    register: "AGC.CONTROL".to_owned(),
                    value: Some(Value::RegisterImage {
                        read_token: 0,
                        address: 0x2010_7030,
                        and_mask: 0xffff_fffe,
                        or_mask: 1,
                    }),
                }),
                ReferenceEvent::DelayMicros {
                    micros: Value::Constant(7),
                },
            ],
            reference_dependencies: vec!["child_leaf".to_owned()],
            blockers: Vec::new(),
            reference_blockers: Vec::new(),
            return_value: Value::input(0),
            reference_flow: None,
            unresolved_branch: None,
        };
        let generated = generate(
            &trace,
            "oracle.elf",
            "abc123",
            None,
            &[("rom.elf".to_owned(), "def456".to_owned())],
        )
        .unwrap();

        assert!(generated.exit_a0_modeled);
        assert!(generated.source.contains("pub trait ReferenceIo"));
        assert!(generated.source.contains("// Companion artifact: rom.elf"));
        assert!(generated.source.contains("// Companion SHA-256: def456"));
        assert!(
            generated
                .source
                .contains("// Composed direct-call dependency: child_leaf")
        );
        assert!(
            generated
                .source
                .contains("pub fn open_phy_reference_phy_example(")
        );
        assert!(
            generated
                .source
                .contains("let read0 = io.read(32, 0x20107030_u32);")
        );
        assert!(
            generated.source.contains(
                "io.write(32, 0x20107030_u32, (read0 & 0xfffffffe_u32) | 0x00000001_u32);"
            )
        );
        assert!(
            generated
                .source
                .contains("io.delay_micros(0x00000007_u32);")
        );
        assert!(
            generated
                .source
                .contains("ReferenceOutcome { exit_a0: Some(args[0] & 0xffffffff_u32) }")
        );
    }

    #[test]
    fn rejects_incomplete_control_flow_instead_of_emitting_a_partial_function() {
        let trace = Trace {
            symbol: "branchy".to_owned(),
            events: Vec::new(),
            reference_events: Vec::new(),
            reference_dependencies: Vec::new(),
            blockers: vec!["control-flow instruction at 0x10".to_owned()],
            reference_blockers: Vec::new(),
            return_value: Value::Unknown,
            reference_flow: None,
            unresolved_branch: None,
        };
        let error = generate(&trace, "oracle.elf", "abc123", None, &[]).unwrap_err();
        assert!(error.contains("not eligible"));
        assert!(error.contains("control-flow"));
    }

    #[test]
    fn preserves_ordered_elf_ram_reads_and_writes() {
        let address = 0x3fcd_0010;
        let trace = Trace {
            symbol: "state_leaf".to_owned(),
            events: Vec::new(),
            reference_events: vec![
                ReferenceEvent::Memory {
                    access: Access::Read,
                    width: 32,
                    address: Value::Constant(address),
                    region: ".data".to_owned(),
                    value: None,
                },
                ReferenceEvent::Memory {
                    access: Access::Write,
                    width: 32,
                    address: Value::Constant(address),
                    region: ".data".to_owned(),
                    value: Some(Value::MemoryImage {
                        read_token: 0,
                        and_mask: 0xffff_ff00,
                        or_mask: 0x55,
                    }),
                },
            ],
            reference_dependencies: Vec::new(),
            blockers: Vec::new(),
            reference_blockers: Vec::new(),
            return_value: Value::MemoryImage {
                read_token: 0,
                and_mask: u32::MAX,
                or_mask: 0,
            },
            reference_flow: None,
            unresolved_branch: None,
        };
        let generated = generate(&trace, "oracle.elf", "abc123", None, &[]).unwrap();

        let read = generated
            .source
            .find("let memory_read0 = memory.read(32, memory_address0);")
            .unwrap();
        let write = generated
            .source
            .find("memory.write(32, memory_address1, memory_value1);")
            .unwrap();
        assert!(
            generated
                .source
                .contains("let memory_address0 = 0x3fcd0010_u32;")
        );
        assert!(
            generated
                .source
                .contains("let memory_address1 = 0x3fcd0010_u32;")
        );
        assert!(read < write);
        assert!(
            generated
                .source
                .contains("(memory_read0 & 0xffffff00_u32) | 0x00000055_u32")
        );
        assert!(
            generated
                .source
                .contains("ReferenceOutcome { exit_a0: Some((memory_read0")
        );
    }
}
