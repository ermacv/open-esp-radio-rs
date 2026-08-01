//! Fail-closed Rust generation for exact supported symbolic traces.
//!
//! The output is an executable reference model, not a guessed production
//! driver. It deliberately exposes ordered MMIO through a trait and reports an
//! unresolved return value as `None` instead of inventing a C prototype.

mod value;

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write as _,
};

use crate::{
    BranchCondition, BranchOperation, IndexedMmioGuard, IndexedMmioRegister, MemoryAccess,
    ObservableEvent, RV32_MODELED_ARGUMENT_COUNT, RV32_REGISTER_ARGUMENT_COUNT,
    RV32_STACK_ARGUMENT_COUNT, ResolvedReferenceBody, ResolvedReferenceEvent,
    ResolvedReferenceFlow, ResolvedReferenceProgram, ResolvedReferenceTerminator,
    SECONDARY_CALL_RESULT_TOKEN_FLAG, SymbolicValue,
};
#[cfg(test)]
use value::render_value;
use value::{CallResultAvailability, MmioReadAddress, render_value_scoped};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct GeneratedReference {
    pub(crate) source: String,
    pub(crate) exit_a0_modeled: bool,
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
    bounded_poll_count: usize,
    call_results: Vec<CallResultAvailability>,
    external_results: Vec<crate::external_abi::Function>,
    validated_external_tables: BTreeSet<crate::external_abi::Table>,
    arguments: [String; RV32_MODELED_ARGUMENT_COUNT],
}

impl Default for RenderState {
    fn default() -> Self {
        Self {
            reads: Vec::new(),
            mmio_access_count: 0,
            memory_read_count: 0,
            memory_access_count: 0,
            bounded_poll_count: 0,
            call_results: Vec::new(),
            external_results: Vec::new(),
            validated_external_tables: BTreeSet::new(),
            arguments: core::array::from_fn(|index| format!("args[{index}]")),
        }
    }
}

fn render_state_value(value: &SymbolicValue, state: &RenderState) -> Result<String, String> {
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

fn render_indexed_mmio_address(
    output: &mut String,
    state: &mut RenderState,
    indent: &str,
    address: &SymbolicValue,
    registers: &[IndexedMmioRegister],
    guard: Option<&IndexedMmioGuard>,
) -> Result<usize, String> {
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
    Ok(access_token)
}

fn render_events(
    output: &mut String,
    events: &[ResolvedReferenceEvent],
    state: &mut RenderState,
    indent: &str,
) -> Result<(), String> {
    for event in events {
        match event {
            ResolvedReferenceEvent::Observable(ObservableEvent::Memory {
                access: MemoryAccess::Read,
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
            ResolvedReferenceEvent::Observable(ObservableEvent::Memory {
                access: MemoryAccess::Write,
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
            ResolvedReferenceEvent::Observable(ObservableEvent::Memory {
                access: MemoryAccess::Read,
                value: Some(_),
                ..
            }) => return Err("internal IR error: MMIO read carries a write value".to_owned()),
            ResolvedReferenceEvent::Observable(ObservableEvent::Memory {
                access: MemoryAccess::Write,
                value: None,
                ..
            }) => return Err("internal IR error: MMIO write has no symbolic value".to_owned()),
            ResolvedReferenceEvent::IndexedMmio {
                access,
                width,
                address,
                registers,
                guard,
                value,
            } => {
                let access_token = render_indexed_mmio_address(
                    output,
                    state,
                    indent,
                    address,
                    registers,
                    guard.as_ref(),
                )?;
                match (access, value) {
                    (MemoryAccess::Read, None) => {
                        let token = state.reads.len();
                        writeln!(
                            output,
                            "{indent}let read{token} = io.read({width}, mmio_address{access_token});"
                        )
                        .unwrap();
                        writeln!(output, "{indent}let _ = read{token};").unwrap();
                        state.reads.push(MmioReadAddress::Indexed);
                    }
                    (MemoryAccess::Write, Some(value)) => {
                        let value = render_state_value(value, state)?;
                        writeln!(
                            output,
                            "{indent}io.write({width}, mmio_address{access_token}, {value});"
                        )
                        .unwrap();
                    }
                    (MemoryAccess::Read, Some(_)) => {
                        return Err(
                            "internal IR error: indexed MMIO read carries a write value".to_owned()
                        );
                    }
                    (MemoryAccess::Write, None) => {
                        return Err(
                            "internal IR error: indexed MMIO write has no symbolic value"
                                .to_owned(),
                        );
                    }
                }
            }
            ResolvedReferenceEvent::PollMmio {
                width,
                address,
                registers,
                guard,
                mask,
                expected,
            } => {
                let access_token = render_indexed_mmio_address(
                    output,
                    state,
                    indent,
                    address,
                    registers,
                    guard.as_ref(),
                )?;
                writeln!(
                    output,
                    "{indent}// Poll until (value & {mask:#010x}) == {expected:#010x}."
                )
                .unwrap();
                writeln!(output, "{indent}loop {{").unwrap();
                writeln!(
                    output,
                    "{indent}    let value = io.read({width}, mmio_address{access_token});"
                )
                .unwrap();
                writeln!(
                    output,
                    "{indent}    if value & {mask:#010x}_u32 == {expected:#010x}_u32 {{ break; }}"
                )
                .unwrap();
                writeln!(output, "{indent}}}").unwrap();
            }
            ResolvedReferenceEvent::BoundedPoll {
                maximum_attempts,
                body,
                repeat_while_mask,
                repeat_while_expected,
                on_exhausted,
            } => {
                let token = state.bounded_poll_count;
                state.bounded_poll_count += 1;
                writeln!(
                    output,
                    "{indent}// Reviewed bounded poll: at most {maximum_attempts} attempts."
                )
                .unwrap();
                writeln!(
                    output,
                    "{indent}for bounded_poll_attempt{token} in 0..{maximum_attempts}_u16 {{"
                )
                .unwrap();
                writeln!(output, "{indent}    let bounded_poll_value{token} = {{").unwrap();
                let body_state = RenderState {
                    arguments: state.arguments.clone(),
                    ..RenderState::default()
                };
                let body_indent = format!("{indent}        ");
                render_flow(output, body, body_state, &body_indent, FlowReturn::Scalar)?;
                writeln!(output, "{indent}    }};").unwrap();
                writeln!(
                    output,
                    "{indent}    if bounded_poll_value{token} & {repeat_while_mask:#010x}_u32 != {repeat_while_expected:#010x}_u32 {{ break; }}"
                )
                .unwrap();
                if let Some(on_exhausted) = on_exhausted {
                    writeln!(
                        output,
                        "{indent}    if bounded_poll_attempt{token} + 1 == {maximum_attempts}_u16 {{"
                    )
                    .unwrap();
                    let mut exhausted_state = state.clone();
                    let exhausted_indent = format!("{indent}        ");
                    render_events(
                        output,
                        std::slice::from_ref(on_exhausted.as_ref()),
                        &mut exhausted_state,
                        &exhausted_indent,
                    )?;
                    writeln!(output, "{indent}    }}").unwrap();
                }
                writeln!(output, "{indent}}}").unwrap();
            }
            ResolvedReferenceEvent::PollFlow {
                body,
                exit_when_mask,
                exit_when_expected,
            } => {
                let token = state.bounded_poll_count;
                state.bounded_poll_count += 1;
                writeln!(
                    output,
                    "{indent}// Poll a complete composed flow until its exit predicate matches."
                )
                .unwrap();
                writeln!(output, "{indent}loop {{").unwrap();
                writeln!(output, "{indent}    let poll_flow_value{token} = {{").unwrap();
                let body_state = RenderState {
                    arguments: state.arguments.clone(),
                    ..RenderState::default()
                };
                let body_indent = format!("{indent}        ");
                render_flow(output, body, body_state, &body_indent, FlowReturn::Scalar)?;
                writeln!(output, "{indent}    }};").unwrap();
                writeln!(
                    output,
                    "{indent}    if poll_flow_value{token} & {exit_when_mask:#010x}_u32 == {exit_when_expected:#010x}_u32 {{ break; }}"
                )
                .unwrap();
                writeln!(output, "{indent}}}").unwrap();
            }
            ResolvedReferenceEvent::SymmetricCalibrationSearch {
                token,
                attempts_per_direction,
                settle_micros,
                sample_shift,
                sample_mask,
                accepted_sample,
                initial_read,
                setup,
                write_candidate,
                sample,
            } => {
                if usize::try_from(*token).ok() != Some(state.call_results.len()) {
                    return Err(format!(
                        "calibration token {token} is not ordered in generated behavior"
                    ));
                }
                writeln!(
                    output,
                    "{indent}// Reviewed symmetric calibration search: two directions, at most {attempts_per_direction} attempts each."
                )
                .unwrap();
                writeln!(output, "{indent}let calibration_initial_word{token} = {{").unwrap();
                let child_indent = format!("{indent}    ");
                render_flow(
                    output,
                    initial_read,
                    RenderState::default(),
                    &child_indent,
                    FlowReturn::Scalar,
                )?;
                writeln!(output, "{indent}}};").unwrap();
                writeln!(
                    output,
                    "{indent}let calibration_initial{token} = (calibration_initial_word{token} & 0x0000ffff_u32) as u16;"
                )
                .unwrap();
                writeln!(output, "{indent}{{").unwrap();
                render_flow(
                    output,
                    setup,
                    RenderState::default(),
                    &child_indent,
                    FlowReturn::Unit,
                )?;
                writeln!(output, "{indent}}};").unwrap();
                writeln!(output, "{indent}let mut calibration_sum{token} = 0_u16;").unwrap();
                writeln!(output, "{indent}let mut calibration_count{token} = 0_u8;").unwrap();
                writeln!(
                    output,
                    "{indent}for calibration_direction{token} in 0..2_u8 {{"
                )
                .unwrap();
                writeln!(
                    output,
                    "{indent}    for calibration_step{token} in 0..{attempts_per_direction}_u16 {{"
                )
                .unwrap();
                writeln!(
                    output,
                    "{indent}        let calibration_candidate{token} = if calibration_direction{token} == 0 {{ calibration_initial{token}.wrapping_sub(calibration_step{token}) }} else {{ calibration_initial{token}.wrapping_add(1).wrapping_add(calibration_step{token}) }};"
                )
                .unwrap();
                writeln!(output, "{indent}        {{").unwrap();
                let writer_state = RenderState {
                    arguments: core::array::from_fn(|index| {
                        if index == 0 {
                            format!("(i32::from(calibration_candidate{token} as i16)) as u32")
                        } else {
                            format!("args[{index}]")
                        }
                    }),
                    ..RenderState::default()
                };
                let loop_child_indent = format!("{indent}            ");
                render_flow(
                    output,
                    write_candidate,
                    writer_state,
                    &loop_child_indent,
                    FlowReturn::Unit,
                )?;
                writeln!(output, "{indent}        }};").unwrap();
                writeln!(
                    output,
                    "{indent}        io.delay_micros({settle_micros:#010x}_u32);"
                )
                .unwrap();
                writeln!(
                    output,
                    "{indent}        let calibration_sample_word{token} = {{"
                )
                .unwrap();
                render_flow(
                    output,
                    sample,
                    RenderState::default(),
                    &loop_child_indent,
                    FlowReturn::Scalar,
                )?;
                writeln!(output, "{indent}        }};").unwrap();
                writeln!(
                    output,
                    "{indent}        let calibration_sample{token} = (calibration_sample_word{token} >> {sample_shift}) & {sample_mask:#010x}_u32;"
                )
                .unwrap();
                writeln!(
                    output,
                    "{indent}        if calibration_sample{token} == {accepted_sample:#010x}_u32 {{"
                )
                .unwrap();
                writeln!(
                    output,
                    "{indent}            calibration_sum{token} = calibration_sum{token}.wrapping_add(calibration_candidate{token});"
                )
                .unwrap();
                writeln!(
                    output,
                    "{indent}            calibration_count{token} = calibration_count{token}.wrapping_add(1);"
                )
                .unwrap();
                writeln!(
                    output,
                    "{indent}        }} else if calibration_count{token} != 0 {{"
                )
                .unwrap();
                writeln!(output, "{indent}            break;").unwrap();
                writeln!(output, "{indent}        }}").unwrap();
                writeln!(output, "{indent}    }}").unwrap();
                writeln!(output, "{indent}}}").unwrap();
                writeln!(
                    output,
                    "{indent}let calibration_selected{token} = if calibration_count{token} == 0 {{ calibration_initial{token} }} else {{ riscv_div(u32::from(calibration_sum{token}), u32::from(calibration_count{token})) as u16 }};"
                )
                .unwrap();
                writeln!(output, "{indent}{{").unwrap();
                let final_writer_state = RenderState {
                    arguments: core::array::from_fn(|index| {
                        if index == 0 {
                            format!("(i32::from(calibration_selected{token} as i16)) as u32")
                        } else {
                            format!("args[{index}]")
                        }
                    }),
                    ..RenderState::default()
                };
                render_flow(
                    output,
                    write_candidate,
                    final_writer_state,
                    &child_indent,
                    FlowReturn::Unit,
                )?;
                writeln!(output, "{indent}}};").unwrap();
                writeln!(
                    output,
                    "{indent}io.delay_micros({settle_micros:#010x}_u32);"
                )
                .unwrap();
                writeln!(
                    output,
                    "{indent}let call_result{token} = (u32::from(calibration_initial{token}) << 16) | u32::from(calibration_selected{token});"
                )
                .unwrap();
                writeln!(output, "{indent}let _ = call_result{token};").unwrap();
                state.call_results.push(CallResultAvailability::Primary);
            }
            ResolvedReferenceEvent::Observable(ObservableEvent::Fence {
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
            ResolvedReferenceEvent::DelayMicros { micros } => {
                let micros = render_state_value(micros, state)?;
                writeln!(output, "{indent}io.delay_micros({micros});").unwrap();
            }
            ResolvedReferenceEvent::Memory {
                access: MemoryAccess::Read,
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
            ResolvedReferenceEvent::Memory {
                access: MemoryAccess::Write,
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
            ResolvedReferenceEvent::Memory {
                access: MemoryAccess::Read,
                value: Some(_),
                ..
            } => return Err("internal IR error: memory read carries a write value".to_owned()),
            ResolvedReferenceEvent::Memory {
                access: MemoryAccess::Write,
                value: None,
                ..
            } => return Err("internal IR error: memory write has no symbolic value".to_owned()),
            ResolvedReferenceEvent::ExternalCall {
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
            ResolvedReferenceEvent::DiagnosticCall {
                function,
                argument_count,
                arguments,
            } => {
                let arguments = arguments
                    .iter()
                    .take(usize::from(*argument_count))
                    .map(|value| render_state_value(value, state))
                    .collect::<Result<Vec<_>, _>>()?;
                match (function.as_str(), *argument_count) {
                    ("wifi_log", 6) => {
                        writeln!(output, "{indent}// Named diagnostic call: wifi_log.").unwrap();
                        writeln!(
                            output,
                            "{indent}platform.wifi_log([{}]);",
                            arguments.join(", ")
                        )
                        .unwrap();
                    }
                    ("ets_printf", 1) => {
                        writeln!(
                            output,
                            "{indent}// Reviewed ROM diagnostic call: ets_printf."
                        )
                        .unwrap();
                        writeln!(output, "{indent}platform.ets_printf({});", arguments[0]).unwrap();
                    }
                    _ => {
                        return Err(format!(
                            "unsupported diagnostic call shape: {function} with {argument_count} arguments"
                        ));
                    }
                }
            }
            ResolvedReferenceEvent::ComposedCall {
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
                for index in crate::resolved_reference_flow_input_indices(flow) {
                    let argument = render_state_value(&arguments[usize::from(index)], state)?;
                    let name = format!("call{token}_arg{index}");
                    writeln!(output, "{indent}let {name} = {argument};").unwrap();
                    // Specialization can turn the last use of a callee input
                    // into a constant while the conservative input inventory
                    // still retains the binding.
                    writeln!(output, "{indent}let _ = &{name};").unwrap();
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
                state.call_results.push(if *result_modeled {
                    CallResultAvailability::Primary
                } else {
                    CallResultAvailability::Unmodeled
                });
            }
            ResolvedReferenceEvent::ComposedCallWithScratch {
                token,
                symbol,
                arguments,
                flow,
                result_modeled,
                scratch_argument,
                scratch_size,
            } => {
                if usize::try_from(*token).ok() != Some(state.call_results.len()) {
                    return Err(format!(
                        "scratch call token {token} is not ordered in generated behavior"
                    ));
                }
                let scratch_index = usize::from(*scratch_argument);
                let scratch_base = 0xfffe_0000_u32.wrapping_add(token.wrapping_mul(0x100));
                let mut child_state = RenderState::default();
                for index in crate::resolved_reference_flow_input_indices(flow) {
                    let name = format!("call{token}_arg{index}");
                    if usize::from(index) == scratch_index {
                        child_state.arguments[usize::from(index)] = name;
                        continue;
                    }
                    let argument = render_state_value(&arguments[usize::from(index)], state)?;
                    writeln!(output, "{indent}let {name} = {argument};").unwrap();
                    writeln!(output, "{indent}let _ = &{name};").unwrap();
                    child_state.arguments[usize::from(index)] = name;
                }
                writeln!(
                    output,
                    "{indent}// Composed direct call with {scratch_size}-byte initialized-on-write scratch: {}.",
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
                writeln!(
                    output,
                    "{child_indent}let call{token}_arg{scratch_argument} = {scratch_base:#010x}_u32;"
                )
                .unwrap();
                writeln!(
                    output,
                    "{child_indent}let mut scratch_memory{token} = ReferenceScratchMemory::new(memory, call{token}_arg{scratch_argument}, {scratch_size});"
                )
                .unwrap();
                writeln!(
                    output,
                    "{child_indent}let memory = &mut scratch_memory{token};"
                )
                .unwrap();
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
                state.call_results.push(if *result_modeled {
                    CallResultAvailability::Primary
                } else {
                    CallResultAvailability::Unmodeled
                });
            }
            ResolvedReferenceEvent::WideSignedDivide {
                token,
                dividend_low,
                dividend_high,
                divisor_low,
                divisor_high,
            } => {
                if usize::try_from(*token).ok() != Some(state.call_results.len()) {
                    return Err(format!(
                        "wide-divide token {token} is not ordered in generated behavior"
                    ));
                }
                let dividend_low = render_state_value(dividend_low, state)?;
                let dividend_high = render_state_value(dividend_high, state)?;
                let divisor_low = render_state_value(divisor_low, state)?;
                let divisor_high = render_state_value(divisor_high, state)?;
                writeln!(
                    output,
                    "{indent}// Reviewed ROM __divdi3: signed a1:a0 / a3:a2."
                )
                .unwrap();
                writeln!(
                    output,
                    "{indent}let (call_result{token}, call_result{token}_high) = riscv_div_i64_words({dividend_low}, {dividend_high}, {divisor_low}, {divisor_high});"
                )
                .unwrap();
                writeln!(output, "{indent}let _ = call_result{token};").unwrap();
                writeln!(output, "{indent}let _ = call_result{token}_high;").unwrap();
                state
                    .call_results
                    .push(CallResultAvailability::PrimaryAndSecondary);
            }
        }
    }
    Ok(())
}

fn render_outcome(
    output: &mut String,
    value: &SymbolicValue,
    state: &RenderState,
    indent: &str,
) -> Result<(), String> {
    let available_calls = state
        .call_results
        .iter()
        .copied()
        .enumerate()
        .flat_map(|(token, availability)| {
            let token = token as u32;
            let primary = !matches!(availability, CallResultAvailability::Unmodeled);
            let secondary = matches!(availability, CallResultAvailability::PrimaryAndSecondary);
            [(token, primary)]
                .into_iter()
                .chain(secondary.then_some((token | SECONDARY_CALL_RESULT_TOKEN_FLAG, true)))
        })
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
    flow: &ResolvedReferenceFlow,
    mut state: RenderState,
    indent: &str,
    return_kind: FlowReturn,
) -> Result<(), String> {
    render_events(output, &flow.events, &mut state, indent)?;
    match &flow.terminator {
        ResolvedReferenceTerminator::Return(value) => match return_kind {
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
        ResolvedReferenceTerminator::Branch {
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
    flow: &ResolvedReferenceFlow,
    output: &mut BTreeSet<crate::external_abi::Table>,
) {
    for event in &flow.events {
        match event {
            ResolvedReferenceEvent::ExternalCall { table, .. } => {
                output.insert(*table);
            }
            ResolvedReferenceEvent::ComposedCall { flow, .. } => {
                collect_external_tables(flow, output)
            }
            ResolvedReferenceEvent::ComposedCallWithScratch { flow, .. } => {
                collect_external_tables(flow, output)
            }
            ResolvedReferenceEvent::BoundedPoll { body, .. } => {
                collect_external_tables(body, output)
            }
            ResolvedReferenceEvent::PollFlow { body, .. } => collect_external_tables(body, output),
            ResolvedReferenceEvent::SymmetricCalibrationSearch {
                initial_read,
                setup,
                write_candidate,
                sample,
                ..
            } => {
                collect_external_tables(initial_read, output);
                collect_external_tables(setup, output);
                collect_external_tables(write_candidate, output);
                collect_external_tables(sample, output);
            }
            _ => {}
        }
    }
    if let ResolvedReferenceTerminator::Branch {
        taken, not_taken, ..
    } = &flow.terminator
    {
        collect_external_tables(taken, output);
        collect_external_tables(not_taken, output);
    }
}

pub(crate) fn generate(
    trace: &ResolvedReferenceProgram,
    artifact: &str,
    artifact_sha256: &str,
    member: Option<&str>,
    companions: &[(String, String)],
) -> Result<GeneratedReference, String> {
    let function_name = sanitize_identifier(&trace.symbol);
    let exit_a0_modeled = trace.exit_a0_modeled;
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
    for dependency in &trace.dependencies {
        writeln!(
            output,
            "// Composed direct-call dependency: {}",
            comment_text(dependency)
        )
        .unwrap();
    }
    let mut external_tables = BTreeSet::new();
    match &trace.body {
        ResolvedReferenceBody::Linear { events, .. } => {
            for event in events {
                match event {
                    ResolvedReferenceEvent::ExternalCall { table, .. } => {
                        external_tables.insert(*table);
                    }
                    ResolvedReferenceEvent::ComposedCall { flow, .. } => {
                        collect_external_tables(flow, &mut external_tables);
                    }
                    _ => {}
                }
            }
        }
        ResolvedReferenceBody::Flow(flow) => {
            collect_external_tables(flow, &mut external_tables);
        }
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
    writeln!(output, "#[allow(dead_code)]").unwrap();
    writeln!(
        output,
        "struct ReferenceScratchMemory<'a, M: ReferenceMemory> {{"
    )
    .unwrap();
    writeln!(output, "    inner: &'a mut M,").unwrap();
    writeln!(output, "    base: u32,").unwrap();
    writeln!(output, "    len: usize,").unwrap();
    writeln!(output, "    bytes: [u8; 256],").unwrap();
    writeln!(output, "    initialized: [bool; 256],").unwrap();
    writeln!(output, "}}").unwrap();
    writeln!(output, "#[allow(dead_code)]").unwrap();
    writeln!(
        output,
        "impl<'a, M: ReferenceMemory> ReferenceScratchMemory<'a, M> {{"
    )
    .unwrap();
    writeln!(
        output,
        "    fn new(inner: &'a mut M, base: u32, len: u16) -> Self {{"
    )
    .unwrap();
    writeln!(
        output,
        "        assert!(len != 0 && len <= 256, \"reference scratch size is outside 1..=256\");"
    )
    .unwrap();
    writeln!(output, "        Self {{ inner, base, len: usize::from(len), bytes: [0; 256], initialized: [false; 256] }}").unwrap();
    writeln!(output, "    }}").unwrap();
    writeln!(
        output,
        "    fn local_range(&self, width: u8, address: u32) -> Option<core::ops::Range<usize>> {{"
    )
    .unwrap();
    writeln!(output, "        let byte_count = match width {{ 8 => 1_u32, 16 => 2, 32 => 4, _ => panic!(\"unsupported reference scratch width {{width}}\") }};").unwrap();
    writeln!(output, "        let end = address.checked_add(byte_count).expect(\"reference scratch address overflow\");").unwrap();
    writeln!(output, "        let limit = self.base.checked_add(self.len as u32).expect(\"reference scratch limit overflow\");").unwrap();
    writeln!(
        output,
        "        let inside = address >= self.base && end <= limit;"
    )
    .unwrap();
    writeln!(
        output,
        "        let disjoint = end <= self.base || address >= limit;"
    )
    .unwrap();
    writeln!(output, "        assert!(inside || disjoint, \"reference memory access partially overlaps private scratch\");").unwrap();
    writeln!(
        output,
        "        inside.then(|| (address - self.base) as usize..(end - self.base) as usize)"
    )
    .unwrap();
    writeln!(output, "    }}").unwrap();
    writeln!(output, "}}").unwrap();
    writeln!(output, "#[allow(dead_code)]").unwrap();
    writeln!(
        output,
        "impl<M: ReferenceMemory> ReferenceMemory for ReferenceScratchMemory<'_, M> {{"
    )
    .unwrap();
    writeln!(output, "    fn symbol_address(&mut self, member: Option<&str>, symbol: &str) -> u32 {{ self.inner.symbol_address(member, symbol) }}").unwrap();
    writeln!(
        output,
        "    fn read(&mut self, width: u8, address: u32) -> u32 {{"
    )
    .unwrap();
    writeln!(output, "        let Some(range) = self.local_range(width, address) else {{ return self.inner.read(width, address); }};").unwrap();
    writeln!(output, "        assert!(range.clone().all(|index| self.initialized[index]), \"read from uninitialized reference scratch\");").unwrap();
    writeln!(output, "        range.enumerate().fold(0_u32, |value, (shift, index)| value | (u32::from(self.bytes[index]) << (shift * 8)))").unwrap();
    writeln!(output, "    }}").unwrap();
    writeln!(
        output,
        "    fn write(&mut self, width: u8, address: u32, value: u32) {{"
    )
    .unwrap();
    writeln!(output, "        let Some(range) = self.local_range(width, address) else {{ self.inner.write(width, address, value); return; }};").unwrap();
    writeln!(output, "        for (shift, index) in range.enumerate() {{ self.bytes[index] = (value >> (shift * 8)) as u8; self.initialized[index] = true; }}").unwrap();
    writeln!(output, "    }}").unwrap();
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
    writeln!(output, "    fn ets_printf(&mut self, format_address: u32);").unwrap();
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
    writeln!(
        output,
        "fn riscv_div_i64_words(dividend_low: u32, dividend_high: u32, divisor_low: u32, divisor_high: u32) -> (u32, u32) {{"
    )
    .unwrap();
    writeln!(
        output,
        "    let dividend = (((dividend_high as u64) << 32) | dividend_low as u64) as i64;"
    )
    .unwrap();
    writeln!(
        output,
        "    let divisor = (((divisor_high as u64) << 32) | divisor_low as u64) as i64;"
    )
    .unwrap();
    writeln!(
        output,
        "    assert!(divisor != 0, \"modeled __divdi3 precondition violated: divisor is zero\");"
    )
    .unwrap();
    writeln!(
        output,
        "    let quotient = dividend.wrapping_div(divisor) as u64;"
    )
    .unwrap();
    writeln!(output, "    (quotient as u32, (quotient >> 32) as u32)").unwrap();
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
        "    /// SymbolicValue of the ABI `a0` register at exit; this does not infer a C prototype."
    )
    .unwrap();
    writeln!(output, "    pub exit_a0: Option<u32>,").unwrap();
    writeln!(output, "}}").unwrap();
    writeln!(output).unwrap();
    writeln!(output, "#[derive(Clone, Copy, Debug, Eq, PartialEq)]").unwrap();
    writeln!(output, "pub struct Rv32ReferenceArguments {{").unwrap();
    writeln!(
        output,
        "    pub registers: [u32; {RV32_REGISTER_ARGUMENT_COUNT}],"
    )
    .unwrap();
    writeln!(output, "    pub stack: [u32; {RV32_STACK_ARGUMENT_COUNT}],").unwrap();
    writeln!(output, "}}").unwrap();
    writeln!(
        output,
        "impl core::ops::Index<usize> for Rv32ReferenceArguments {{"
    )
    .unwrap();
    writeln!(output, "    type Output = u32;").unwrap();
    writeln!(
        output,
        "    fn index(&self, index: usize) -> &Self::Output {{"
    )
    .unwrap();
    writeln!(output, "        if index < {RV32_REGISTER_ARGUMENT_COUNT} {{ &self.registers[index] }} else {{ &self.stack[index - {RV32_REGISTER_ARGUMENT_COUNT}] }}").unwrap();
    writeln!(output, "    }}").unwrap();
    writeln!(output, "}}").unwrap();
    writeln!(output).unwrap();
    writeln!(output, "#[allow(dead_code, non_snake_case)]").unwrap();
    writeln!(output, "pub fn {function_name}(").unwrap();
    writeln!(output, "    io: &mut impl ReferenceIo,").unwrap();
    writeln!(output, "    memory: &mut impl ReferenceMemory,").unwrap();
    writeln!(output, "    platform: &mut impl ReferencePlatform,").unwrap();
    writeln!(output, "    args: Rv32ReferenceArguments,").unwrap();
    writeln!(output, ") -> ReferenceOutcome {{").unwrap();
    writeln!(output, "    let _ = &mut *io;").unwrap();
    writeln!(output, "    let _ = &mut *memory;").unwrap();
    writeln!(output, "    let _ = &mut *platform;").unwrap();
    writeln!(output, "    let _ = &args;").unwrap();

    let mut state = RenderState::default();
    match &trace.body {
        ResolvedReferenceBody::Flow(flow) => {
            render_flow(&mut output, flow, state, "    ", FlowReturn::Outcome)?;
        }
        ResolvedReferenceBody::Linear {
            events,
            return_value,
        } => {
            render_events(&mut output, events, &mut state, "    ")?;
            render_outcome(&mut output, return_value, &state, "    ")?;
        }
    }
    writeln!(output, "}}").unwrap();

    Ok(GeneratedReference {
        source: output,
        exit_a0_modeled,
    })
}

#[cfg(test)]
mod tests;
