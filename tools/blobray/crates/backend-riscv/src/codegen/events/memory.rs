//! ELF/RAM accesses and proven memory-transfer loop rendering.

use std::fmt::Write as _;

use super::super::*;

pub(super) fn render_event(
    output: &mut String,
    event: &ResolvedReferenceEvent,
    state: &mut RenderState,
    indent: &str,
) -> Result<(), String> {
    match event {
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
        _ => unreachable!("event family was checked by the ordered renderer"),
    }
    Ok(())
}
