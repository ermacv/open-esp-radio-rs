//! External, diagnostic, composed and reviewed arithmetic call rendering.

use std::fmt::Write as _;

use super::super::*;

pub(super) fn render_event(
    output: &mut String,
    event: &ResolvedReferenceEvent,
    state: &mut RenderState,
    indent: &str,
) -> Result<(), String> {
    match event {
        ResolvedReferenceEvent::ExternalCall {
            token,
            table,
            function,
            arguments,
            ..
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
        _ => unreachable!("event family was checked by the ordered renderer"),
    }
    Ok(())
}
