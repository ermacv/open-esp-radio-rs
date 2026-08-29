//! Control-flow rendering and external-table discovery.

use std::fmt::Write as _;

use super::{events::render_events, *};

pub(super) fn render_outcome(
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
pub(super) enum FlowReturn {
    Outcome,
    Scalar,
    Unit,
}

pub(super) fn render_flow(
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
        ResolvedReferenceTerminator::FailStop {
            site,
            function,
            argument_count,
            arguments,
        } => {
            let arguments = arguments
                .iter()
                .take(usize::from(*argument_count))
                .map(|value| render_state_value(value, &state))
                .collect::<Result<Vec<_>, _>>()?;
            writeln!(
                output,
                "{indent}// Reviewed non-returning fail-stop at {site:#010x}."
            )
            .unwrap();
            writeln!(
                output,
                "{indent}platform.fail_stop({function:?}, &[{}])",
                arguments.join(", ")
            )
            .unwrap();
            Ok(())
        }
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
