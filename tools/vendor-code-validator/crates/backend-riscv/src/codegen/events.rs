//! Rendering of ordered observable events inside one resolved flow.

use std::fmt::Write as _;

use super::*;

pub(super) fn render_events(
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
            ResolvedReferenceEvent::WordToBytesMemoryLoop {
                source,
                source_region,
                destination,
                destination_region,
                length,
            } => {
                if *length == 0 || length % 4 != 0 {
                    return Err(format!(
                        "internal IR error: word-to-bytes loop length {length} is not a positive multiple of four"
                    ));
                }
                let token = state.memory_access_count;
                state.memory_access_count += 1;
                let source = render_state_value(source, state)?;
                let destination = render_state_value(destination, state)?;
                writeln!(
                    output,
                    "{indent}// Proven {length}-byte CPU-RAM word-to-bytes loop: {} -> {}.",
                    comment_text(source_region),
                    comment_text(destination_region),
                )
                .unwrap();
                writeln!(
                    output,
                    "{indent}let memory_transfer_source{token} = {source};"
                )
                .unwrap();
                writeln!(
                    output,
                    "{indent}let memory_transfer_destination{token} = {destination};"
                )
                .unwrap();
                writeln!(
                    output,
                    "{indent}for memory_transfer_word_offset{token} in (0..{length}_u32).step_by(4) {{"
                )
                .unwrap();
                writeln!(
                    output,
                    "{indent}    for memory_transfer_byte_offset{token} in 0..4_u32 {{"
                )
                .unwrap();
                writeln!(
                    output,
                    "{indent}        let memory_transfer_word{token} = memory.read(32, memory_transfer_source{token}.wrapping_add(memory_transfer_word_offset{token}));"
                )
                .unwrap();
                writeln!(
                    output,
                    "{indent}        let memory_transfer_byte{token} = memory_transfer_word{token}.wrapping_shr(memory_transfer_byte_offset{token}.wrapping_mul(8));"
                )
                .unwrap();
                writeln!(
                    output,
                    "{indent}        memory.write(8, memory_transfer_destination{token}.wrapping_add(memory_transfer_word_offset{token}).wrapping_add(memory_transfer_byte_offset{token}), memory_transfer_byte{token});"
                )
                .unwrap();
                writeln!(output, "{indent}    }}").unwrap();
                writeln!(output, "{indent}}}").unwrap();
                // The proven source pattern performed one 32-bit read per
                // destination byte. Preserve the outer token namespace even
                // though the compact semantic transfer no longer materializes
                // those dead intermediate values.
                state.memory_read_count = state.memory_read_count.wrapping_add(*length as usize);
            }
            ResolvedReferenceEvent::BytesToWordMemoryLoop {
                first_call_token,
                source,
                source_region,
                destination,
                destination_region,
                length,
            } => {
                if *length == 0 || length % 4 != 0 {
                    return Err(format!(
                        "internal IR error: bytes-to-word loop length {length} is not a positive multiple of four"
                    ));
                }
                if usize::try_from(*first_call_token).ok() != Some(state.call_results.len()) {
                    return Err(format!(
                        "compacted call token {first_call_token} is not ordered in generated behavior"
                    ));
                }
                let token = state.memory_access_count;
                state.memory_access_count += 1;
                let source = render_state_value(source, state)?;
                let destination = render_state_value(destination, state)?;
                writeln!(
                    output,
                    "{indent}// Proven {length}-byte CPU-RAM bytes-to-word loop: {} -> {}.",
                    comment_text(source_region),
                    comment_text(destination_region),
                )
                .unwrap();
                writeln!(
                    output,
                    "{indent}let memory_transfer_source{token} = {source};"
                )
                .unwrap();
                writeln!(
                    output,
                    "{indent}let memory_transfer_destination{token} = {destination};"
                )
                .unwrap();
                writeln!(
                    output,
                    "{indent}for memory_transfer_word_offset{token} in (0..{length}_u32).step_by(4) {{"
                )
                .unwrap();
                for byte in [1_u32, 0, 2, 3] {
                    writeln!(
                        output,
                        "{indent}    let memory_transfer_byte{byte}_{token} = memory.read(8, memory_transfer_source{token}.wrapping_add(memory_transfer_word_offset{token}).wrapping_add({byte}));"
                    )
                    .unwrap();
                }
                writeln!(
                    output,
                    "{indent}    let memory_transfer_word{token} = (memory_transfer_byte0_{token} & 0xff) | ((memory_transfer_byte1_{token} & 0xff) << 8) | ((memory_transfer_byte2_{token} & 0xff) << 16) | ((memory_transfer_byte3_{token} & 0xff) << 24);"
                )
                .unwrap();
                writeln!(
                    output,
                    "{indent}    memory.write(32, memory_transfer_destination{token}.wrapping_add(memory_transfer_word_offset{token}), memory_transfer_word{token});"
                )
                .unwrap();
                writeln!(output, "{indent}}}").unwrap();
                let call_count = usize::try_from(*length / 4)
                    .map_err(|_| "bytes-to-word call count does not fit usize".to_owned())?;
                state.call_results.extend(core::iter::repeat_n(
                    CallResultAvailability::Primary,
                    call_count,
                ));
            }
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
                let slot = function.spec();
                let table_spec = table.spec();
                if state.validated_external_tables.insert(*table) {
                    writeln!(
                        output,
                        "{indent}assert_eq!(platform.external_table_version({:?}), {:#010x}_u32, \"external ABI version mismatch for {}\");",
                        table_spec.id,
                        table_spec.version,
                        table_spec.id
                    )
                    .unwrap();
                    writeln!(
                        output,
                        "{indent}assert_eq!(platform.external_table_magic({:?}), {:#010x}_u32, \"external ABI magic mismatch for {}\");",
                        table_spec.id,
                        table_spec.magic,
                        table_spec.id
                    )
                    .unwrap();
                    writeln!(
                        output,
                        "{indent}assert_eq!(platform.external_table_size({:?}), {:#010x}_u32, \"external ABI size mismatch for {}\");",
                        table_spec.id,
                        table_spec.size,
                        table_spec.id
                    )
                    .unwrap();
                }
                writeln!(
                    output,
                    "{indent}// External ABI {}+{:#x}: {}.",
                    table_spec.id, slot.offset, slot.c_name
                )
                .unwrap();
                let call_arguments = arguments
                    .iter()
                    .take(usize::from(slot.argument_count))
                    .map(|value| render_state_value(value, state))
                    .collect::<Result<Vec<_>, _>>()?;
                let call = format!(
                    "platform.external_call({:?}, {:?}, &[{}])",
                    table_spec.id,
                    slot.id,
                    call_arguments.join(", ")
                );
                writeln!(output, "{indent}let external_result{token} = {call};").unwrap();
                if let ExternalReturnModel::Constant(expected) = slot.return_model {
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
                writeln!(
                    output,
                    "{indent}// Harness-reviewed diagnostic call: {}.",
                    comment_text(function)
                )
                .unwrap();
                writeln!(
                    output,
                    "{indent}platform.diagnostic_call({function:?}, &[{}]);",
                    arguments.join(", ")
                )
                .unwrap();
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
                let child_prefix = format!("{}call{token}_", state.composed_argument_prefix);
                let mut child_state = RenderState {
                    composed_argument_prefix: child_prefix.clone(),
                    ..RenderState::default()
                };
                for index in crate::resolved_reference_flow_input_indices(flow) {
                    let argument = render_state_value(&arguments[usize::from(index)], state)?;
                    let name = format!("{child_prefix}arg{index}");
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
                let child_prefix = format!("{}call{token}_", state.composed_argument_prefix);
                let mut child_state = RenderState {
                    composed_argument_prefix: child_prefix.clone(),
                    ..RenderState::default()
                };
                for index in crate::resolved_reference_flow_input_indices(flow) {
                    let name = format!("{child_prefix}arg{index}");
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
                    "{child_indent}let {child_prefix}arg{scratch_argument} = {scratch_base:#010x}_u32;"
                )
                .unwrap();
                writeln!(
                    output,
                    "{child_indent}let mut scratch_memory{token} = ReferenceScratchMemory::new(memory, {child_prefix}arg{scratch_argument}, {scratch_size});"
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
