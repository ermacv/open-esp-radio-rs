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
        ResolvedReferenceEvent::ReviewedExternalCall {
            token,
            call,
            arguments,
            ..
        } => {
            if usize::try_from(*token).ok() != Some(state.external_results.len()) {
                return Err(format!(
                    "external call token {token} is not ordered in generated behavior"
                ));
            }
            let model = call
                .execution_model
                .as_ref()
                .expect("resolved reviewed external call has an execution model");
            writeln!(
                output,
                "{indent}// Reviewed external ABI {}: {} (model {}).",
                comment_text(&call.contract),
                comment_text(&call.name),
                comment_text(&model.id),
            )
            .unwrap();
            let call_arguments = arguments
                .iter()
                .take(call.argument_types.len())
                .map(|value| render_state_value(value, state))
                .collect::<Result<Vec<_>, _>>()?;
            let invocation = format!(
                "platform.external_call({:?}, {:?}, &[{}])",
                call.contract,
                model.id,
                call_arguments.join(", ")
            );
            writeln!(
                output,
                "{indent}let external_outcome{token} = {invocation};"
            )
            .unwrap();
            match model.return_model {
                ExternalReturnModel::Void | ExternalReturnModel::Unmodeled => {}
                ExternalReturnModel::Constant(_)
                | ExternalReturnModel::SymbolicU32
                | ExternalReturnModel::AllocatedZeroed { .. }
                | ExternalReturnModel::OpaquePointer => {
                    writeln!(
                        output,
                        "{indent}let external_result{token} = external_outcome{token}.return_words[0];"
                    )
                    .unwrap();
                }
                ExternalReturnModel::SymbolicU64 => {
                    writeln!(
                        output,
                        "{indent}let external_result{token} = external_outcome{token}.return_words[0];"
                    )
                    .unwrap();
                    writeln!(
                        output,
                        "{indent}let external_result{token}_high = external_outcome{token}.return_words[1];"
                    )
                    .unwrap();
                }
            }
            if let ExternalReturnModel::Constant(expected) = model.return_model {
                writeln!(
                        output,
                        "{indent}assert_eq!(external_result{token}, {expected:#010x}_u32, \"external ABI profile mismatch for {}\");",
                        comment_text(&call.name),
                    )
                    .unwrap();
            }
            for (output_index, output_model) in model.outputs.iter().enumerate() {
                match output_model {
                    ExternalOutputModel::PrivateStack { width, .. } => {
                        let mask = match width {
                            8 => 0xff,
                            16 => 0xffff,
                            _ => u32::MAX,
                        };
                        writeln!(
                            output,
                            "{indent}let external_output{token}_{output_index} = external_outcome{token}.outputs[{output_index}] & {mask:#010x}_u32;"
                        )
                        .unwrap();
                        writeln!(
                            output,
                            "{indent}let _ = external_output{token}_{output_index};"
                        )
                        .unwrap();
                    }
                }
            }
            writeln!(output, "{indent}let _ = external_outcome{token};").unwrap();
            state.external_results.push(());
        }
        ResolvedReferenceEvent::ModeledDirectCall {
            token,
            function,
            arguments,
            ..
        } => {
            if usize::try_from(*token).ok() != Some(state.external_results.len()) {
                return Err(format!(
                    "direct external call token {token} is not ordered in generated behavior"
                ));
            }
            let arguments = arguments
                .iter()
                .take(usize::from(function.argument_count))
                .map(|value| render_state_value(value, state))
                .collect::<Result<Vec<_>, _>>()?;
            writeln!(
                output,
                "{indent}// Modeled direct platform call: {} ({}).",
                comment_text(&function.name),
                comment_text(&function.operation),
            )
            .unwrap();
            writeln!(
                output,
                "{indent}let external_result{token} = platform.direct_external_call({:?}, &[{}]);",
                function.id,
                arguments.join(", "),
            )
            .unwrap();
            if let ExternalReturnModel::Constant(expected) = function.return_model {
                writeln!(
                    output,
                    "{indent}assert_eq!(external_result{token}, {expected:#010x}_u32, \"direct external profile mismatch for {}\");",
                    comment_text(&function.name),
                )
                .unwrap();
            }
            writeln!(output, "{indent}let _ = external_result{token};").unwrap();
            state.external_results.push(());
        }
        ResolvedReferenceEvent::DiagnosticCall {
            site,
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
                "{indent}// Harness-reviewed diagnostic call at {site:#010x}: {}.",
                comment_text(function),
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
