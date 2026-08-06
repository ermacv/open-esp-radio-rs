//! Ordered rendering of resolved reference events.

mod calls;
mod memory;
mod mmio;
mod polls;

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
            ResolvedReferenceEvent::Observable(ObservableEvent::Memory { .. })
            | ResolvedReferenceEvent::IndexedMmio { .. }
            | ResolvedReferenceEvent::PollMmio { .. } => {
                mmio::render_event(output, event, state, indent)?;
            }
            ResolvedReferenceEvent::BoundedPoll { .. }
            | ResolvedReferenceEvent::PollFlow { .. }
            | ResolvedReferenceEvent::SymmetricCalibrationSearch { .. } => {
                polls::render_event(output, event, state, indent)?;
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
            ResolvedReferenceEvent::Memory { .. }
            | ResolvedReferenceEvent::WordToBytesMemoryLoop { .. }
            | ResolvedReferenceEvent::BytesToWordMemoryLoop { .. } => {
                memory::render_event(output, event, state, indent)?;
            }
            ResolvedReferenceEvent::ExternalCall { .. }
            | ResolvedReferenceEvent::DiagnosticCall { .. }
            | ResolvedReferenceEvent::ComposedCall { .. }
            | ResolvedReferenceEvent::ComposedCallWithScratch { .. }
            | ResolvedReferenceEvent::WideSignedDivide { .. } => {
                calls::render_event(output, event, state, indent)?;
            }
        }
    }
    Ok(())
}
