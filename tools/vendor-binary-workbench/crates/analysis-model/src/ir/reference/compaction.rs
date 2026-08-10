//! Proof-driven compaction of completely resolved CPU-RAM loop events.
//!
//! These recognizers preserve the vendor access width and ordering. They
//! deliberately reject MMIO events and any read/call token that escapes the
//! candidate loop.

use super::*;

fn affine_base_and_offset(value: &SymbolicValue) -> (SymbolicValue, u32) {
    match value {
        SymbolicValue::Constant(value) => (SymbolicValue::Constant(0), *value),
        SymbolicValue::Expression {
            operation: ExpressionOperation::Add,
            left,
            right,
        } => {
            if let Some(constant) = right.as_constant() {
                let (base, offset) = affine_base_and_offset(left);
                (base, offset.wrapping_add(constant))
            } else if let Some(constant) = left.as_constant() {
                let (base, offset) = affine_base_and_offset(right);
                (base, offset.wrapping_add(constant))
            } else {
                (value.clone(), 0)
            }
        }
        SymbolicValue::Expression {
            operation: ExpressionOperation::Subtract,
            left,
            right,
        } if right.as_constant().is_some() => {
            let (base, offset) = affine_base_and_offset(left);
            (base, offset.wrapping_sub(right.as_constant().unwrap()))
        }
        _ => (value.clone(), 0),
    }
}

fn affine_value(base: SymbolicValue, offset: u32) -> SymbolicValue {
    if base == SymbolicValue::Constant(0) {
        SymbolicValue::Constant(offset)
    } else if offset == 0 {
        base
    } else {
        SymbolicValue::Expression {
            operation: ExpressionOperation::Add,
            left: std::sync::Arc::new(base),
            right: std::sync::Arc::new(SymbolicValue::Constant(offset)),
        }
    }
}

fn low_byte_is_memory_word_byte(value: &SymbolicValue, read_token: u32, byte: u8) -> bool {
    value.bits()[..8]
        .iter()
        .enumerate()
        .all(|(destination_bit, source)| {
            matches!(
                source,
                super::super::BitSource::Memory {
                    read_token: token,
                    bit,
                    inverted: false,
                } if *token == read_token
                    && *bit == byte * 8 + destination_bit as u8
            )
        })
}

fn four_byte_transfer(
    events: &[ResolvedReferenceEvent],
    first_read_token: u32,
) -> Option<ResolvedReferenceEvent> {
    let group = events.get(..8)?;
    let mut source_base = None;
    let mut source_offset = 0_u32;
    let mut destination_base = None;
    let mut destination_offset = 0_u32;
    let mut source_region = None;
    let mut destination_region = None;
    for byte in 0..4_u8 {
        let ResolvedReferenceEvent::Memory {
            access: MemoryAccess::Read,
            width: 32,
            address: source,
            region: read_region,
            value: None,
        } = &group[usize::from(byte) * 2]
        else {
            return None;
        };
        let ResolvedReferenceEvent::Memory {
            access: MemoryAccess::Write,
            width: 8,
            address: destination,
            region: write_region,
            value: Some(value),
        } = &group[usize::from(byte) * 2 + 1]
        else {
            return None;
        };
        if !low_byte_is_memory_word_byte(value, first_read_token + u32::from(byte), byte) {
            return None;
        }
        let (next_source_base, next_source_offset) = affine_base_and_offset(source);
        let (next_destination_base, next_destination_offset) = affine_base_and_offset(destination);
        match &source_base {
            None => {
                source_base = Some(next_source_base);
                source_offset = next_source_offset;
                source_region = Some(read_region.clone());
            }
            Some(base)
                if *base == next_source_base
                    && source_offset == next_source_offset
                    && source_region.as_deref() == Some(read_region) => {}
            Some(_) => return None,
        }
        match &destination_base {
            None => {
                destination_base = Some(next_destination_base);
                destination_offset = next_destination_offset;
                destination_region = Some(write_region.clone());
            }
            Some(base)
                if *base == next_destination_base
                    && next_destination_offset == destination_offset + u32::from(byte)
                    && destination_region.as_deref() == Some(write_region) => {}
            Some(_) => return None,
        }
    }
    Some(ResolvedReferenceEvent::WordToBytesMemoryLoop {
        source: affine_value(source_base?, source_offset),
        source_region: source_region?,
        destination: affine_value(destination_base?, destination_offset),
        destination_region: destination_region?,
        length: 4,
    })
}

fn try_merge_memory_transfers(
    previous: &mut ResolvedReferenceEvent,
    next: &ResolvedReferenceEvent,
) -> bool {
    let ResolvedReferenceEvent::WordToBytesMemoryLoop {
        source: previous_source,
        source_region: previous_source_region,
        destination: previous_destination,
        destination_region: previous_destination_region,
        length: previous_length,
    } = previous
    else {
        return false;
    };
    let ResolvedReferenceEvent::WordToBytesMemoryLoop {
        source: next_source,
        source_region: next_source_region,
        destination: next_destination,
        destination_region: next_destination_region,
        length: next_length,
    } = next
    else {
        return false;
    };
    let (previous_source_base, previous_source_offset) = affine_base_and_offset(previous_source);
    let (next_source_base, next_source_offset) = affine_base_and_offset(next_source);
    let (previous_destination_base, previous_destination_offset) =
        affine_base_and_offset(previous_destination);
    let (next_destination_base, next_destination_offset) = affine_base_and_offset(next_destination);
    if previous_source_region != next_source_region
        || previous_destination_region != next_destination_region
        || previous_source_base != next_source_base
        || previous_destination_base != next_destination_base
        || next_source_offset != previous_source_offset.wrapping_add(*previous_length)
        || next_destination_offset != previous_destination_offset.wrapping_add(*previous_length)
    {
        return false;
    }
    *previous_length = previous_length.wrapping_add(*next_length);
    true
}

pub(super) fn value_uses_memory_tokens(value: &SymbolicValue, start: u32, end: u32) -> bool {
    let token_is_elided = |token: u32| start <= token && token < end;
    match value {
        SymbolicValue::Expression { left, right, .. } => {
            value_uses_memory_tokens(left, start, end)
                || value_uses_memory_tokens(right, start, end)
        }
        SymbolicValue::WideSignedDivide {
            dividend_low,
            dividend_high,
            divisor_low,
            divisor_high,
            ..
        } => {
            value_uses_memory_tokens(dividend_low, start, end)
                || value_uses_memory_tokens(dividend_high, start, end)
                || value_uses_memory_tokens(divisor_low, start, end)
                || value_uses_memory_tokens(divisor_high, start, end)
        }
        SymbolicValue::MemoryImage { read_token, .. } => token_is_elided(*read_token),
        SymbolicValue::Bits(bits) => bits.iter().any(|source| {
            matches!(
                source,
                super::super::BitSource::Memory { read_token, .. }
                    if token_is_elided(*read_token)
            )
        }),
        _ => false,
    }
}

fn event_uses_memory_tokens(event: &ResolvedReferenceEvent, start: u32, end: u32) -> bool {
    let value_uses = |value: &SymbolicValue| value_uses_memory_tokens(value, start, end);
    match event {
        ResolvedReferenceEvent::Observable(ObservableEvent::Memory { value, .. }) => {
            value.as_ref().is_some_and(value_uses)
        }
        ResolvedReferenceEvent::Observable(ObservableEvent::Fence { .. }) => false,
        ResolvedReferenceEvent::IndexedMmio {
            address,
            guard,
            value,
            ..
        } => {
            value_uses(address)
                || guard
                    .as_ref()
                    .is_some_and(|guard| value_uses(&guard.selector))
                || value.as_ref().is_some_and(value_uses)
        }
        ResolvedReferenceEvent::PollMmio { address, guard, .. } => {
            value_uses(address)
                || guard
                    .as_ref()
                    .is_some_and(|guard| value_uses(&guard.selector))
        }
        ResolvedReferenceEvent::BoundedPoll { on_exhausted, .. } => on_exhausted
            .as_deref()
            .is_some_and(|event| event_uses_memory_tokens(event, start, end)),
        // These child flows are rendered with fresh memory-token namespaces.
        ResolvedReferenceEvent::PollFlow { .. }
        | ResolvedReferenceEvent::SymmetricCalibrationSearch { .. } => false,
        ResolvedReferenceEvent::DelayMicros { micros } => value_uses(micros),
        ResolvedReferenceEvent::Memory { address, value, .. } => {
            value_uses(address) || value.as_ref().is_some_and(value_uses)
        }
        ResolvedReferenceEvent::WordToBytesMemoryLoop {
            source,
            destination,
            ..
        }
        | ResolvedReferenceEvent::BytesToWordMemoryLoop {
            source,
            destination,
            ..
        } => value_uses(source) || value_uses(destination),
        ResolvedReferenceEvent::ExternalCall { arguments, .. }
        | ResolvedReferenceEvent::ModeledDirectCall { arguments, .. }
        | ResolvedReferenceEvent::DiagnosticCall { arguments, .. }
        | ResolvedReferenceEvent::ComposedCall { arguments, .. }
        | ResolvedReferenceEvent::ComposedCallWithScratch { arguments, .. } => {
            arguments.iter().any(value_uses)
        }
        ResolvedReferenceEvent::WideSignedDivide {
            dividend_low,
            dividend_high,
            divisor_low,
            divisor_high,
            ..
        } => {
            value_uses(dividend_low)
                || value_uses(dividend_high)
                || value_uses(divisor_low)
                || value_uses(divisor_high)
        }
    }
}

fn flow_uses_memory_tokens(flow: &ResolvedReferenceFlow, start: u32, end: u32) -> bool {
    flow.events
        .iter()
        .any(|event| event_uses_memory_tokens(event, start, end))
        || terminator_uses_memory_tokens(&flow.terminator, start, end)
}

pub(super) fn terminator_uses_memory_tokens(
    terminator: &ResolvedReferenceTerminator,
    start: u32,
    end: u32,
) -> bool {
    match terminator {
        ResolvedReferenceTerminator::Return(value) => value_uses_memory_tokens(value, start, end),
        ResolvedReferenceTerminator::Branch {
            condition,
            taken,
            not_taken,
        } => {
            value_uses_memory_tokens(&condition.left, start, end)
                || value_uses_memory_tokens(&condition.right, start, end)
                || flow_uses_memory_tokens(taken, start, end)
                || flow_uses_memory_tokens(not_taken, start, end)
        }
    }
}

pub(super) fn compact_cpu_memory_transfers(
    events: Vec<ResolvedReferenceEvent>,
    continuation_uses_tokens: impl Fn(u32, u32) -> bool,
) -> Vec<ResolvedReferenceEvent> {
    let mut output = Vec::with_capacity(events.len());
    let mut event_index = 0;
    let mut memory_read_token = 0_u32;
    while event_index < events.len() {
        let elided_token_end = memory_read_token + 4;
        let tokens_escape = events[event_index + 8.min(events.len() - event_index)..]
            .iter()
            .any(|event| event_uses_memory_tokens(event, memory_read_token, elided_token_end))
            || continuation_uses_tokens(memory_read_token, elided_token_end);
        if !tokens_escape
            && let Some(transfer) = four_byte_transfer(&events[event_index..], memory_read_token)
        {
            if !output
                .last_mut()
                .is_some_and(|previous| try_merge_memory_transfers(previous, &transfer))
            {
                output.push(transfer);
            }
            event_index += 8;
            memory_read_token += 4;
            continue;
        }
        if matches!(
            events[event_index],
            ResolvedReferenceEvent::Memory {
                access: MemoryAccess::Read,
                ..
            }
        ) {
            memory_read_token += 1;
        }
        output.push(events[event_index].clone());
        event_index += 1;
    }
    output
}

fn value_is_little_endian_memory_word(value: &SymbolicValue) -> bool {
    // `phy_byte_to_word` reads bytes in the optimized instruction order
    // [1, 0, 2, 3], while its result is ordinary little endian.
    let token_for_byte = [1_u32, 0, 2, 3];
    value.bits().iter().enumerate().all(|(bit, source)| {
        let byte = bit / 8;
        matches!(
            source,
            super::super::BitSource::Memory {
                read_token,
                bit: source_bit,
                inverted: false,
            } if *read_token == token_for_byte[byte] && usize::from(*source_bit) == bit % 8
        )
    })
}

fn value_is_call_result(value: &SymbolicValue, token: u32) -> bool {
    match value {
        SymbolicValue::CallResult(value_token) => *value_token == token,
        _ => value.bits().iter().enumerate().all(|(bit, source)| {
            matches!(
                source,
                super::super::BitSource::CallResult {
                    call_token,
                    bit: source_bit,
                    inverted: false,
                } if *call_token == token && usize::from(*source_bit) == bit
            )
        }),
    }
}

fn pure_little_endian_loader(flow: &ResolvedReferenceFlow) -> Option<String> {
    if flow.events.len() != 4 {
        return None;
    }
    let expected_offsets = [1_u32, 0, 2, 3];
    let mut region = None;
    for (event, expected_offset) in flow.events.iter().zip(expected_offsets) {
        let ResolvedReferenceEvent::Memory {
            access: MemoryAccess::Read,
            width: 8,
            address,
            region: read_region,
            value: None,
        } = event
        else {
            return None;
        };
        let (base, offset) = affine_base_and_offset(address);
        if base != SymbolicValue::input(0) || offset != expected_offset {
            return None;
        }
        match &region {
            None => region = Some(read_region.clone()),
            Some(region) if region == read_region => {}
            Some(_) => return None,
        }
    }
    let ResolvedReferenceTerminator::Return(value) = &flow.terminator else {
        return None;
    };
    value_is_little_endian_memory_word(value).then_some(region?)
}

fn bytes_to_word_transfer(events: &[ResolvedReferenceEvent]) -> Option<ResolvedReferenceEvent> {
    let [
        ResolvedReferenceEvent::ComposedCall {
            token,
            arguments,
            flow,
            result_modeled: true,
            ..
        },
        ResolvedReferenceEvent::Memory {
            access: MemoryAccess::Write,
            width: 32,
            address: destination,
            region: destination_region,
            value: Some(value),
        },
        ..,
    ] = events
    else {
        return None;
    };
    if !value_is_call_result(value, *token) {
        return None;
    }
    let source_region = pure_little_endian_loader(flow)?;
    let source = arguments.first()?.clone();
    Some(ResolvedReferenceEvent::BytesToWordMemoryLoop {
        first_call_token: *token,
        source,
        source_region,
        destination: destination.clone(),
        destination_region: destination_region.clone(),
        length: 4,
    })
}

fn try_merge_bytes_to_word_loops(
    previous: &mut ResolvedReferenceEvent,
    next: &ResolvedReferenceEvent,
) -> bool {
    let ResolvedReferenceEvent::BytesToWordMemoryLoop {
        first_call_token: previous_token,
        source: previous_source,
        source_region: previous_source_region,
        destination: previous_destination,
        destination_region: previous_destination_region,
        length: previous_length,
    } = previous
    else {
        return false;
    };
    let ResolvedReferenceEvent::BytesToWordMemoryLoop {
        first_call_token: next_token,
        source: next_source,
        source_region: next_source_region,
        destination: next_destination,
        destination_region: next_destination_region,
        length: next_length,
    } = next
    else {
        return false;
    };
    let (previous_source_base, previous_source_offset) = affine_base_and_offset(previous_source);
    let (next_source_base, next_source_offset) = affine_base_and_offset(next_source);
    let (previous_destination_base, previous_destination_offset) =
        affine_base_and_offset(previous_destination);
    let (next_destination_base, next_destination_offset) = affine_base_and_offset(next_destination);
    if previous_source_region != next_source_region
        || previous_destination_region != next_destination_region
        || previous_source_base != next_source_base
        || previous_destination_base != next_destination_base
        || *next_token != previous_token.wrapping_add(*previous_length / 4)
        || next_source_offset != previous_source_offset.wrapping_add(*previous_length)
        || next_destination_offset != previous_destination_offset.wrapping_add(*previous_length)
    {
        return false;
    }
    *previous_length = previous_length.wrapping_add(*next_length);
    true
}

pub(super) fn value_uses_call_tokens(value: &SymbolicValue, start: u32, end: u32) -> bool {
    let token_is_elided = |token: u32| start <= token && token < end;
    match value {
        SymbolicValue::CallResult(token) => token_is_elided(*token),
        SymbolicValue::Expression { left, right, .. } => {
            value_uses_call_tokens(left, start, end) || value_uses_call_tokens(right, start, end)
        }
        SymbolicValue::WideSignedDivide {
            dividend_low,
            dividend_high,
            divisor_low,
            divisor_high,
            ..
        } => {
            value_uses_call_tokens(dividend_low, start, end)
                || value_uses_call_tokens(dividend_high, start, end)
                || value_uses_call_tokens(divisor_low, start, end)
                || value_uses_call_tokens(divisor_high, start, end)
        }
        SymbolicValue::Bits(bits) => bits.iter().any(|source| {
            matches!(
                source,
                super::super::BitSource::CallResult { call_token, .. }
                    if token_is_elided(*call_token)
            )
        }),
        _ => false,
    }
}

fn event_uses_call_tokens(event: &ResolvedReferenceEvent, start: u32, end: u32) -> bool {
    let value_uses = |value: &SymbolicValue| value_uses_call_tokens(value, start, end);
    match event {
        ResolvedReferenceEvent::Observable(ObservableEvent::Memory { value, .. }) => {
            value.as_ref().is_some_and(value_uses)
        }
        ResolvedReferenceEvent::Observable(ObservableEvent::Fence { .. }) => false,
        ResolvedReferenceEvent::IndexedMmio {
            address,
            guard,
            value,
            ..
        } => {
            value_uses(address)
                || guard
                    .as_ref()
                    .is_some_and(|guard| value_uses(&guard.selector))
                || value.as_ref().is_some_and(value_uses)
        }
        ResolvedReferenceEvent::PollMmio { address, guard, .. } => {
            value_uses(address)
                || guard
                    .as_ref()
                    .is_some_and(|guard| value_uses(&guard.selector))
        }
        ResolvedReferenceEvent::BoundedPoll { on_exhausted, .. } => on_exhausted
            .as_deref()
            .is_some_and(|event| event_uses_call_tokens(event, start, end)),
        ResolvedReferenceEvent::PollFlow { .. }
        | ResolvedReferenceEvent::SymmetricCalibrationSearch { .. } => false,
        ResolvedReferenceEvent::DelayMicros { micros } => value_uses(micros),
        ResolvedReferenceEvent::Memory { address, value, .. } => {
            value_uses(address) || value.as_ref().is_some_and(value_uses)
        }
        ResolvedReferenceEvent::WordToBytesMemoryLoop {
            source,
            destination,
            ..
        }
        | ResolvedReferenceEvent::BytesToWordMemoryLoop {
            source,
            destination,
            ..
        } => value_uses(source) || value_uses(destination),
        ResolvedReferenceEvent::ExternalCall { arguments, .. }
        | ResolvedReferenceEvent::ModeledDirectCall { arguments, .. }
        | ResolvedReferenceEvent::DiagnosticCall { arguments, .. }
        | ResolvedReferenceEvent::ComposedCall { arguments, .. }
        | ResolvedReferenceEvent::ComposedCallWithScratch { arguments, .. } => {
            arguments.iter().any(value_uses)
        }
        ResolvedReferenceEvent::WideSignedDivide {
            dividend_low,
            dividend_high,
            divisor_low,
            divisor_high,
            ..
        } => {
            value_uses(dividend_low)
                || value_uses(dividend_high)
                || value_uses(divisor_low)
                || value_uses(divisor_high)
        }
    }
}

fn flow_uses_call_tokens(flow: &ResolvedReferenceFlow, start: u32, end: u32) -> bool {
    flow.events
        .iter()
        .any(|event| event_uses_call_tokens(event, start, end))
        || terminator_uses_call_tokens(&flow.terminator, start, end)
}

pub(super) fn terminator_uses_call_tokens(
    terminator: &ResolvedReferenceTerminator,
    start: u32,
    end: u32,
) -> bool {
    match terminator {
        ResolvedReferenceTerminator::Return(value) => value_uses_call_tokens(value, start, end),
        ResolvedReferenceTerminator::Branch {
            condition,
            taken,
            not_taken,
        } => {
            value_uses_call_tokens(&condition.left, start, end)
                || value_uses_call_tokens(&condition.right, start, end)
                || flow_uses_call_tokens(taken, start, end)
                || flow_uses_call_tokens(not_taken, start, end)
        }
    }
}

pub(super) fn compact_bytes_to_word_memory_loops(
    events: Vec<ResolvedReferenceEvent>,
    continuation_uses_tokens: impl Fn(u32, u32) -> bool,
) -> Vec<ResolvedReferenceEvent> {
    let mut output = Vec::with_capacity(events.len());
    let mut event_index = 0;
    while event_index < events.len() {
        let transfer = bytes_to_word_transfer(&events[event_index..]);
        let tokens_escape = transfer.as_ref().is_some_and(|transfer| {
            let ResolvedReferenceEvent::BytesToWordMemoryLoop {
                first_call_token, ..
            } = transfer
            else {
                unreachable!()
            };
            events[event_index + 2.min(events.len() - event_index)..]
                .iter()
                .any(|event| {
                    event_uses_call_tokens(
                        event,
                        *first_call_token,
                        first_call_token.wrapping_add(1),
                    )
                })
                || continuation_uses_tokens(*first_call_token, first_call_token.wrapping_add(1))
        });
        if !tokens_escape && let Some(transfer) = transfer {
            if !output
                .last_mut()
                .is_some_and(|previous| try_merge_bytes_to_word_loops(previous, &transfer))
            {
                output.push(transfer);
            }
            event_index += 2;
            continue;
        }
        output.push(events[event_index].clone());
        event_index += 1;
    }
    output
}
