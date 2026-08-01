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
    BranchCondition, BranchOperation, MemoryAccess, ObservableEvent, ResolvedReferenceBody,
    ResolvedReferenceEvent, ResolvedReferenceFlow, ResolvedReferenceProgram,
    ResolvedReferenceTerminator, SymbolicValue,
};
#[cfg(test)]
use value::render_value;
use value::{MmioReadAddress, render_value_scoped};

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
        "    /// SymbolicValue of the ABI `a0` register at exit; this does not infer a C prototype."
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
