//! Complete lowering for finite, required-MMIO leaves.

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write as _,
};

use crate::{
    BitSource, BranchCondition, BranchOperation, DriverAction, DriverFlow, DriverPlan,
    DriverTerminator, EffectDisposition, ExpressionOperation, MemoryAccess, Result, SymbolicValue,
    collect_value_inputs,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PacLeafOutput {
    pub function_name: String,
    pub source: String,
}

#[derive(Clone, Debug, Default)]
struct RenderState {
    reads: Vec<PacRead>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PacRead {
    Static { address: u32, width: u8 },
    Indexed { width: u8 },
}

fn identifier(value: &str) -> String {
    let mut output = String::new();
    for character in value.chars() {
        output.push(if character.is_ascii_alphanumeric() || character == '_' {
            character.to_ascii_lowercase()
        } else {
            '_'
        });
    }
    if output.as_bytes().first().is_some_and(u8::is_ascii_digit) {
        output.insert(0, '_');
    }
    output
}

fn validate_read(state: &RenderState, token: u32, address: u32) -> Result<()> {
    let actual = usize::try_from(token)
        .ok()
        .and_then(|token| state.reads.get(token));
    if actual.is_some_and(
        |actual| matches!(actual, PacRead::Static { address: actual, .. } if *actual == address),
    ) {
        Ok(())
    } else {
        Err(format!("symbolic PAC value refers to missing read{token} at {address:#010x}").into())
    }
}

fn validate_indexed_read(state: &RenderState, token: u32) -> Result<()> {
    let actual = usize::try_from(token)
        .ok()
        .and_then(|token| state.reads.get(token));
    if matches!(actual, Some(PacRead::Indexed { .. })) {
        Ok(())
    } else {
        Err(format!("symbolic PAC value refers to missing indexed read{token}").into())
    }
}

fn render_bit_source(source: BitSource, state: &RenderState) -> Result<String> {
    match source {
        BitSource::Constant(false) => Ok("0_u32".to_owned()),
        BitSource::Constant(true) => Ok("1_u32".to_owned()),
        BitSource::Input {
            index,
            bit,
            inverted,
        } => {
            let source = if inverted {
                format!("!arg{index}")
            } else {
                format!("arg{index}")
            };
            Ok(format!("({source} >> {bit}) & 1_u32"))
        }
        BitSource::Register {
            read_token,
            address,
            bit,
            inverted,
        } => {
            validate_read(state, read_token, address)?;
            let source = if inverted {
                format!("!read{read_token}")
            } else {
                format!("read{read_token}")
            };
            Ok(format!("({source} >> {bit}) & 1_u32"))
        }
        BitSource::IndexedRegister {
            read_token,
            bit,
            inverted,
        } => {
            validate_indexed_read(state, read_token)?;
            let source = if inverted {
                format!("!read{read_token}")
            } else {
                format!("read{read_token}")
            };
            Ok(format!("({source} >> {bit}) & 1_u32"))
        }
        BitSource::Unknown
        | BitSource::Memory { .. }
        | BitSource::PrivateStack { .. }
        | BitSource::CallResult { .. }
        | BitSource::ExternalResult { .. }
        | BitSource::ExternalResultHigh { .. }
        | BitSource::ExternalOutput { .. } => {
            Err(format!("unsupported bit source in PAC leaf: {source:?}").into())
        }
    }
}

fn render_value(value: &SymbolicValue, state: &RenderState) -> Result<String> {
    if let Some(index) = value.direct_input_index() {
        return Ok(format!("arg{index}"));
    }
    match value {
        SymbolicValue::Constant(value) => Ok(format!("{value:#010x}_u32")),
        SymbolicValue::Input { index } | SymbolicValue::InputConstant { index, .. } => {
            Ok(format!("arg{index}"))
        }
        SymbolicValue::Expression {
            operation,
            left,
            right,
        } => {
            let left = render_value(left, state)?;
            let right = render_value(right, state)?;
            Ok(match operation {
                ExpressionOperation::Add => format!("({left}).wrapping_add({right})"),
                ExpressionOperation::Subtract => format!("({left}).wrapping_sub({right})"),
                ExpressionOperation::Multiply => format!("({left}).wrapping_mul({right})"),
                ExpressionOperation::BitAnd => format!("({left}) & ({right})"),
                ExpressionOperation::BitOr => format!("({left}) | ({right})"),
                ExpressionOperation::BitXor => format!("({left}) ^ ({right})"),
                ExpressionOperation::ShiftLeft => {
                    format!("({left}).wrapping_shl(({right}) & 31)")
                }
                ExpressionOperation::ShiftRight => {
                    format!("({left}).wrapping_shr(({right}) & 31)")
                }
                ExpressionOperation::ShiftRightArithmetic => {
                    format!("(({left}) as i32).wrapping_shr(({right}) & 31) as u32")
                }
                ExpressionOperation::Equal => {
                    format!("u32::from(({left}) == ({right}))")
                }
                ExpressionOperation::LessThanSigned => {
                    format!("u32::from((({left}) as i32) < (({right}) as i32))")
                }
                ExpressionOperation::LessThanUnsigned => {
                    format!("u32::from(({left}) < ({right}))")
                }
                ExpressionOperation::DivideSigned
                | ExpressionOperation::DivideUnsigned
                | ExpressionOperation::RemainderSigned
                | ExpressionOperation::RemainderUnsigned => {
                    return Err(format!(
                        "division/remainder has no architecture-neutral PAC leaf lowering: {operation:?}"
                    )
                    .into());
                }
            })
        }
        SymbolicValue::RegisterImage {
            read_token,
            address,
            and_mask,
            or_mask,
        } => {
            validate_read(state, *read_token, *address)?;
            Ok(format!(
                "(read{read_token} & {and_mask:#010x}_u32) | {or_mask:#010x}_u32"
            ))
        }
        SymbolicValue::IndexedRegisterImage {
            read_token,
            and_mask,
            or_mask,
        } => {
            validate_indexed_read(state, *read_token)?;
            Ok(format!(
                "(read{read_token} & {and_mask:#010x}_u32) | {or_mask:#010x}_u32"
            ))
        }
        SymbolicValue::Bits(bits) => {
            let mut terms = Vec::new();
            for (destination, source) in bits.iter().copied().enumerate() {
                if source == BitSource::Constant(false) {
                    continue;
                }
                let source = render_bit_source(source, state)?;
                terms.push(if destination == 0 {
                    source
                } else {
                    format!("(({source}) << {destination})")
                });
            }
            Ok(if terms.is_empty() {
                "0_u32".to_owned()
            } else {
                terms.join(" | ")
            })
        }
        SymbolicValue::Unknown
        | SymbolicValue::StackAddress(_)
        | SymbolicValue::SymbolAddress { .. }
        | SymbolicValue::CallResult(_)
        | SymbolicValue::ReviewedExternalTable(_)
        | SymbolicValue::ReviewedExternalFunction { .. }
        | SymbolicValue::FunctionTable(_)
        | SymbolicValue::FunctionPointer { .. }
        | SymbolicValue::ExternalResult(_)
        | SymbolicValue::ExternalResultHigh(_)
        | SymbolicValue::ExternalOutput { .. }
        | SymbolicValue::WideSignedDivide { .. }
        | SymbolicValue::MemoryImage { .. } => {
            Err(format!("symbolic value has no PAC leaf lowering: {value:?}").into())
        }
    }
}

fn render_condition(condition: &BranchCondition, state: &RenderState) -> Result<String> {
    let left = render_value(&condition.left, state)?;
    let right = render_value(&condition.right, state)?;
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

fn render_flow(
    output: &mut String,
    flow: &DriverFlow,
    mut state: RenderState,
    indent: &str,
    exit_return_modeled: bool,
) -> Result<()> {
    for action in &flow.actions {
        match action {
            DriverAction::Mmio {
                access,
                binding,
                value,
                disposition: EffectDisposition::Required,
            } => {
                let root = format!("{}_registers", binding.peripheral_module);
                let path = binding.method_path(&root);
                match (access, value) {
                    (MemoryAccess::Read, None) => {
                        let token = state.reads.len();
                        writeln!(
                            output,
                            "{indent}let read{token} = {path}.read().bits() as u32;"
                        )?;
                        state.reads.push(PacRead::Static {
                            address: binding.address,
                            width: binding.width,
                        });
                    }
                    (MemoryAccess::Write, Some(value)) => {
                        let value = render_value(value, &state)?;
                        let raw_type = format!("u{}", binding.width);
                        writeln!(
                            output,
                            "{indent}// SAFETY: the Effect Contract requires the complete evidenced register image."
                        )?;
                        writeln!(output, "{indent}unsafe {{")?;
                        writeln!(
                            output,
                            "{indent}    {path}.write_with_zero(|writer| writer.bits(({value}) as {raw_type}));"
                        )?;
                        writeln!(output, "{indent}}}")?;
                    }
                    (MemoryAccess::Read, Some(_)) => {
                        return Err("PAC read unexpectedly carries a write value".into());
                    }
                    (MemoryAccess::Write, None) => {
                        return Err("PAC write has no symbolic value".into());
                    }
                }
            }
            DriverAction::Mmio { disposition, .. } => {
                return Err(format!(
                    "PAC leaf lowering requires `required`, received {}",
                    disposition.canonical()
                )
                .into());
            }
            DriverAction::IndexedMmio {
                access,
                width,
                input_index,
                bindings,
                value,
                disposition: EffectDisposition::Required,
            } => match (access, value) {
                (MemoryAccess::Read, None) => {
                    let token = state.reads.len();
                    writeln!(
                        output,
                        "{indent}let read{token} = match arg{input_index} {{"
                    )?;
                    for candidate in bindings {
                        let root = format!("{}_registers", candidate.binding.peripheral_module);
                        let path = candidate.binding.method_path(&root);
                        writeln!(
                            output,
                            "{indent}    {} => {path}.read().bits() as u32,",
                            candidate.selector
                        )?;
                    }
                    writeln!(
                        output,
                        "{indent}    _ => panic!(\"indexed PAC selector is outside the evidenced register bank\"),"
                    )?;
                    writeln!(output, "{indent}}};")?;
                    state.reads.push(PacRead::Indexed { width: *width });
                }
                (MemoryAccess::Write, Some(value)) => {
                    let value = render_value(value, &state)?;
                    let raw_type = format!("u{width}");
                    writeln!(output, "{indent}match arg{input_index} {{")?;
                    for candidate in bindings {
                        let root = format!("{}_registers", candidate.binding.peripheral_module);
                        let path = candidate.binding.method_path(&root);
                        writeln!(output, "{indent}    {} => {{", candidate.selector)?;
                        writeln!(
                            output,
                            "{indent}        // SAFETY: the Effect Contract requires the complete evidenced register image."
                        )?;
                        writeln!(output, "{indent}        unsafe {{")?;
                        writeln!(
                            output,
                            "{indent}            {path}.write_with_zero(|writer| writer.bits(({value}) as {raw_type}));"
                        )?;
                        writeln!(output, "{indent}        }}")?;
                        writeln!(output, "{indent}    }}")?;
                    }
                    writeln!(
                        output,
                        "{indent}    _ => panic!(\"indexed PAC selector is outside the evidenced register bank\"),"
                    )?;
                    writeln!(output, "{indent}}}")?;
                }
                (MemoryAccess::Read, Some(_)) => {
                    return Err("indexed PAC read unexpectedly carries a write value".into());
                }
                (MemoryAccess::Write, None) => {
                    return Err("indexed PAC write has no symbolic value".into());
                }
            },
            DriverAction::IndexedMmio { disposition, .. } => {
                return Err(format!(
                    "PAC leaf lowering requires `required`, received {}",
                    disposition.canonical()
                )
                .into());
            }
            DriverAction::Delay { .. } => {
                return Err("PAC leaf lowering does not admit delays".into());
            }
        }
    }
    match &flow.terminator {
        DriverTerminator::Return(value) if exit_return_modeled => {
            writeln!(output, "{indent}{}", render_value(value, &state)?)?;
        }
        DriverTerminator::Return(_) => {}
        DriverTerminator::Branch {
            condition,
            taken,
            not_taken,
        } => {
            writeln!(
                output,
                "{indent}if {} {{",
                render_condition(condition, &state)?
            )?;
            render_flow(
                output,
                taken,
                state.clone(),
                &format!("{indent}    "),
                exit_return_modeled,
            )?;
            writeln!(output, "{indent}}} else {{")?;
            render_flow(
                output,
                not_taken,
                state,
                &format!("{indent}    "),
                exit_return_modeled,
            )?;
            writeln!(output, "{indent}}}")?;
        }
    }
    Ok(())
}

fn collect_inputs_from_flow(flow: &DriverFlow, output: &mut BTreeSet<u8>) {
    for action in &flow.actions {
        match action {
            DriverAction::Mmio {
                value: Some(value), ..
            }
            | DriverAction::Delay { micros: value, .. } => collect_value_inputs(value, output),
            DriverAction::Mmio { value: None, .. } => {}
            DriverAction::IndexedMmio {
                input_index, value, ..
            } => {
                output.insert(*input_index);
                if let Some(value) = value {
                    collect_value_inputs(value, output);
                }
            }
        }
    }
    match &flow.terminator {
        DriverTerminator::Return(value) => collect_value_inputs(value, output),
        DriverTerminator::Branch {
            condition,
            taken,
            not_taken,
        } => {
            collect_value_inputs(&condition.left, output);
            collect_value_inputs(&condition.right, output);
            collect_inputs_from_flow(taken, output);
            collect_inputs_from_flow(not_taken, output);
        }
    }
}

fn collect_peripherals<'a>(
    flow: &'a DriverFlow,
    output: &mut BTreeMap<String, &'a crate::PacRegisterBinding>,
) -> Result<()> {
    for action in &flow.actions {
        match action {
            DriverAction::Mmio { binding, .. } => {
                if let Some(existing) = output.insert(binding.peripheral_module.clone(), binding)
                    && existing.peripheral_type != binding.peripheral_type
                {
                    return Err(format!(
                        "PAC module {} maps to conflicting peripheral types",
                        binding.peripheral_module
                    )
                    .into());
                }
            }
            DriverAction::IndexedMmio { bindings, .. } => {
                for candidate in bindings {
                    let binding = &candidate.binding;
                    if let Some(existing) =
                        output.insert(binding.peripheral_module.clone(), binding)
                        && existing.peripheral_type != binding.peripheral_type
                    {
                        return Err(format!(
                            "PAC module {} maps to conflicting peripheral types",
                            binding.peripheral_module
                        )
                        .into());
                    }
                }
            }
            DriverAction::Delay { .. } => {}
        }
    }
    if let DriverTerminator::Branch {
        taken, not_taken, ..
    } = &flow.terminator
    {
        collect_peripherals(taken, output)?;
        collect_peripherals(not_taken, output)?;
    }
    Ok(())
}

pub fn lower_pac_leaf(plan: &DriverPlan, pac_crate: &str) -> Result<PacLeafOutput> {
    let function_name = format!("generated_{}", identifier(&plan.symbol));
    let mut inputs = BTreeSet::new();
    collect_inputs_from_flow(&plan.flow, &mut inputs);
    let mut peripherals = BTreeMap::new();
    collect_peripherals(&plan.flow, &mut peripherals)?;
    if peripherals.is_empty() {
        return Err("PAC leaf plan has no peripheral effects".into());
    }

    let mut source = String::new();
    writeln!(
        source,
        "// @generated production candidate; review ownership and public types before integration."
    )?;
    writeln!(source, "// Source vendor symbol: {}", plan.symbol)?;
    writeln!(source, "use {pac_crate} as pac;")?;
    writeln!(source)?;
    writeln!(source, "#[inline(always)]")?;
    writeln!(source, "pub fn {function_name}(")?;
    for binding in peripherals.values() {
        writeln!(
            source,
            "    {}_registers: &pac::{},",
            binding.peripheral_module, binding.peripheral_type
        )?;
    }
    for input in inputs {
        writeln!(source, "    arg{input}: u32,")?;
    }
    let return_type = if plan.exit_return_modeled {
        " -> u32"
    } else {
        ""
    };
    writeln!(source, "){return_type} {{")?;
    render_flow(
        &mut source,
        &plan.flow,
        RenderState::default(),
        "    ",
        plan.exit_return_modeled,
    )?;
    writeln!(source, "}}")?;
    Ok(PacLeafOutput {
        function_name,
        source,
    })
}
