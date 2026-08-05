//! Structured bounded polls, composed polling flows and calibration searches.

use std::fmt::Write as _;

use super::super::*;
use super::render_events;

pub(super) fn render_event(
    output: &mut String,
    event: &ResolvedReferenceEvent,
    state: &mut RenderState,
    indent: &str,
) -> Result<(), String> {
    match event {
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
        _ => unreachable!("event family was checked by the ordered renderer"),
    }
    Ok(())
}
