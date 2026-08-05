//! Static/indexed MMIO access and simple polling rendering.

use std::fmt::Write as _;

use super::super::*;

pub(super) fn render_event(
    output: &mut String,
    event: &ResolvedReferenceEvent,
    state: &mut RenderState,
    indent: &str,
) -> Result<(), String> {
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
                        "internal IR error: indexed MMIO write has no symbolic value".to_owned(),
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
        _ => unreachable!("event family was checked by the ordered renderer"),
    }
    Ok(())
}
