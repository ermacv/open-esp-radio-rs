//! RX descriptor metadata decoding and raw management-frame extraction.

use crate::descriptor::{
    descriptor_address_valid, dma_range_valid, length as descriptor_length, rx_armed_word, rx_done,
    rx_rearm_word, size as descriptor_size, Descriptor, BIT_31, DESCRIPTOR_BYTES,
};
use crate::registers::{
    Mmio, RX_CONTROL, RX_DESCRIPTOR_BASE, RX_DESCRIPTOR_HIGH_WINDOW, RX_DESCRIPTOR_LOW_MASK,
    RX_ENABLE, RX_LAST_DESCRIPTOR_HIGH,
};

pub const INGRESS_STRICT_RXEND: u32 = 0x01;
pub const INGRESS_STRICT_DUMP: u32 = 0x02;
pub const INGRESS_VALID_FLAGS: u32 = 0x03;

pub const FIXED_PREFIX_SIZE: usize = 0x38;
pub const DYNAMIC_TAIL_SIZE: usize = 0x08;
pub const PUBLIC_HEADER_SIZE: usize = 0x40;
pub const FCS_SIZE: usize = 4;
pub const PREFIX_RXEND_STATE_OFFSET: usize = 0x08;
pub const TAIL_STATE_OFFSET: usize = 0x04;
pub const TAIL_INTERNAL_OFFSET: usize = 0x05;

const CSI_LENGTH_LOW_OFFSET: usize = 0x26;
const CSI_LENGTH_FLAGS_OFFSET: usize = 0x27;
const OPTION_LENGTH_HINT_OFFSET: usize = 0x2a;
const OPTION_LENGTH_FLAGS_OFFSET: usize = 0x2b;
const CSI_APPEND_ENABLE: u32 = 0x0080_0000;

const MLME_HEADER_SIZE: usize = 24;
const MLME_AUTH_BODY_SIZE: usize = 6;
const SUBTYPE_ASSOC_RESPONSE: u8 = 0x10;
const SUBTYPE_REASSOC_RESPONSE: u8 = 0x30;
const SUBTYPE_AUTH: u8 = 0xb0;

#[derive(Clone, Copy, Debug)]
pub struct RxSegment<'a> {
    pub descriptor_address: u32,
    pub descriptor_word0: u32,
    pub buffer: &'a [u8],
    pub next_descriptor_address: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RxIngressConfig {
    pub ring_entry_limit: usize,
    pub csi_config: u32,
    pub flags: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RxManagementFrame {
    pub length: usize,
    pub tail_offset: usize,
    pub signal_length: u16,
    pub dump_length: u16,
    pub rxend_state: u8,
    pub rx_state: u8,
    pub internal_state: u8,
    pub dump_length_matches: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RxError {
    Invalid,
    Chain,
    Bounds,
    RxFailure,
    MicFailure,
    Quarantined,
    OutputSmall,
    Metadata,
    Ignored,
    Unsupported,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RxRingError {
    Empty,
    Count,
    Address,
    Size,
    Overflow,
    Busy,
    Corrupt,
}

/// Builds the cold, zero-terminated list used by the recovered S31 RX path.
pub fn build_cold_ring(
    descriptors: &[Descriptor],
    descriptor_dma_base: u32,
    buffer_addresses: &[u32],
    buffer_size: u32,
) -> Result<(), RxRingError> {
    if descriptors.is_empty() {
        return Err(RxRingError::Empty);
    }
    if descriptors.len() != buffer_addresses.len() {
        return Err(RxRingError::Count);
    }
    let word0 = rx_armed_word(buffer_size).ok_or(RxRingError::Size)?;
    let count = u32::try_from(descriptors.len()).map_err(|_| RxRingError::Overflow)?;
    let span = count
        .checked_mul(DESCRIPTOR_BYTES)
        .ok_or(RxRingError::Overflow)?;
    if descriptor_dma_base & 3 != 0 || !dma_range_valid(descriptor_dma_base, span) {
        return Err(RxRingError::Address);
    }
    for &buffer in buffer_addresses {
        if !dma_range_valid(buffer, buffer_size) {
            return Err(RxRingError::Address);
        }
    }
    for (index, descriptor) in descriptors.iter().enumerate() {
        let next = if index + 1 < descriptors.len() {
            descriptor_dma_base + (index as u32 + 1) * DESCRIPTOR_BYTES
        } else {
            0
        };
        descriptor.publish(word0, buffer_addresses[index], next);
    }
    Ok(())
}

/// Publishes a previously built cold ring using the instruction-confirmed
/// fence/high-window/base/enable/fence sequence.
pub fn publish_cold_ring<M: Mmio>(
    mmio: &M,
    descriptor_dma_base: u32,
    enable_rx: bool,
) -> Result<(), RxRingError> {
    if !descriptor_address_valid(descriptor_dma_base) {
        return Err(RxRingError::Address);
    }
    mmio.fence();
    let high = mmio.read32(RX_LAST_DESCRIPTOR_HIGH);
    mmio.write32(
        RX_LAST_DESCRIPTOR_HIGH,
        (high & RX_DESCRIPTOR_LOW_MASK) | RX_DESCRIPTOR_HIGH_WINDOW,
    );
    mmio.write32(RX_DESCRIPTOR_BASE, descriptor_dma_base);
    if enable_rx {
        let control = mmio.read32(RX_CONTROL);
        mmio.write32(RX_CONTROL, control | RX_ENABLE);
    }
    mmio.fence();
    Ok(())
}

/// Returns one CPU-owned completed descriptor to the cold/live ring.
pub fn rearm_descriptor(
    descriptor: &Descriptor,
    expected_buffer_address: u32,
    expected_next_address: u32,
) -> Result<(), RxRingError> {
    let word0 = descriptor.word0();
    let capacity = descriptor_size(word0);
    if descriptor.buffer_address() != expected_buffer_address
        || descriptor.next_address() != expected_next_address
        || !dma_range_valid(expected_buffer_address, capacity)
    {
        return Err(RxRingError::Corrupt);
    }
    if word0 & BIT_31 != 0 {
        return Err(RxRingError::Busy);
    }
    descriptor.write_word0(rx_rearm_word(word0).ok_or(RxRingError::Size)?);
    Ok(())
}

#[inline]
fn read_le32(bytes: &[u8]) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(..4)?.try_into().ok()?))
}

fn dynamic_tail_offset(prefix: &[u8], csi_config: u32) -> Option<usize> {
    if prefix.len() < FIXED_PREFIX_SIZE {
        return None;
    }
    let mut offset = FIXED_PREFIX_SIZE;
    let option_flags = *prefix.get(OPTION_LENGTH_FLAGS_OFFSET)?;
    if option_flags & 0x80 != 0 {
        let hint = *prefix.get(OPTION_LENGTH_HINT_OFFSET)?;
        let option_length = usize::from(option_flags & 0x7f) + usize::from(hint >> 5 != 0);
        offset = offset.checked_add((option_length + 3) & !3)?;
    }
    if csi_config & CSI_APPEND_ENABLE != 0 {
        let low = *prefix.get(CSI_LENGTH_LOW_OFFSET)?;
        let flags = *prefix.get(CSI_LENGTH_FLAGS_OFFSET)?;
        let csi_length = usize::from(low) | (usize::from(flags & 0x03) << 8);
        if flags & 0x04 != 0 || csi_length != 0 {
            offset = offset.checked_add((csi_length + 3) & 0x7fc)?;
        }
    }
    Some(offset)
}

fn segment_valid(segment: &RxSegment<'_>) -> bool {
    let capacity = descriptor_size(segment.descriptor_word0) as usize;
    let length = descriptor_length(segment.descriptor_word0) as usize;
    descriptor_address_valid(segment.descriptor_address)
        && segment.descriptor_word0 & BIT_31 == 0
        && capacity != 0
        && length != 0
        && length <= capacity
        && length <= segment.buffer.len()
}

fn copy_frame(
    segments: &[RxSegment<'_>],
    first_offset: usize,
    length: usize,
    output: &mut [u8],
) -> bool {
    let mut copied = 0;
    for (index, segment) in segments.iter().enumerate() {
        if copied == length {
            break;
        }
        let segment_length = descriptor_length(segment.descriptor_word0) as usize;
        let offset = if index == 0 { first_offset } else { 0 };
        let Some(available) = segment_length.checked_sub(offset) else {
            return false;
        };
        let take = available.min(length - copied);
        let Some(source) = segment.buffer.get(offset..offset + take) else {
            return false;
        };
        output[copied..copied + take].copy_from_slice(source);
        copied += take;
    }
    copied == length
}

/// Validates one completed chain, strips the four-byte FCS and copies one
/// unfragmented, unprotected management MPDU into caller-owned storage.
pub fn extract_management(
    segments: &[RxSegment<'_>],
    config: RxIngressConfig,
    output: &mut [u8],
) -> Result<RxManagementFrame, RxError> {
    if segments.is_empty()
        || config.ring_entry_limit == 0
        || segments.len() > config.ring_entry_limit
        || config.flags & !INGRESS_VALID_FLAGS != 0
    {
        return Err(RxError::Invalid);
    }

    for (index, segment) in segments.iter().enumerate() {
        if !segment_valid(segment)
            || segments[..index]
                .iter()
                .any(|old| old.descriptor_address == segment.descriptor_address)
        {
            return Err(RxError::Chain);
        }
        if let Some(next) = segments.get(index + 1) {
            if rx_done(segment.descriptor_word0)
                || segment.next_descriptor_address != next.descriptor_address
            {
                return Err(RxError::Chain);
            }
        }
    }
    if !rx_done(segments.last().unwrap().descriptor_word0) {
        return Err(RxError::Chain);
    }

    let first_length = descriptor_length(segments[0].descriptor_word0) as usize;
    let tail_offset = dynamic_tail_offset(&segments[0].buffer[..first_length], config.csi_config)
        .ok_or(RxError::Bounds)?;
    if tail_offset < FIXED_PREFIX_SIZE
        || tail_offset > first_length
        || first_length - tail_offset < DYNAMIC_TAIL_SIZE
    {
        return Err(RxError::Bounds);
    }

    let tail_word = read_le32(&segments[0].buffer[tail_offset..]).ok_or(RxError::Bounds)?;
    let signal_length = (tail_word & 0x3fff) as usize;
    let dump_length = ((tail_word & 0x3fff_0000) >> 16) as usize;
    let rxend_state = segments[0].buffer[PREFIX_RXEND_STATE_OFFSET];
    let rx_state = segments[0].buffer[tail_offset + TAIL_STATE_OFFSET];
    match rx_state {
        0xf5 => return Err(RxError::MicFailure),
        0xc6 => return Err(RxError::Quarantined),
        0 => {}
        _ => return Err(RxError::RxFailure),
    }
    if config.flags & INGRESS_STRICT_RXEND != 0 && rxend_state != 0 {
        return Err(RxError::RxFailure);
    }
    let dump_matches = signal_length >= FCS_SIZE && dump_length == signal_length - FCS_SIZE;
    if config.flags & INGRESS_STRICT_DUMP != 0 && !dump_matches {
        return Err(RxError::Metadata);
    }
    if signal_length < MLME_HEADER_SIZE + FCS_SIZE {
        return Err(RxError::Bounds);
    }

    let frame_offset = tail_offset + DYNAMIC_TAIL_SIZE;
    let mut available = first_length - frame_offset;
    for segment in &segments[1..] {
        available = available
            .checked_add(descriptor_length(segment.descriptor_word0) as usize)
            .ok_or(RxError::Bounds)?;
    }
    if signal_length > available {
        return Err(RxError::Bounds);
    }
    let frame_length = signal_length - FCS_SIZE;
    if frame_length > output.len() {
        return Err(RxError::OutputSmall);
    }
    if !copy_frame(segments, frame_offset, frame_length, output) {
        return Err(RxError::Bounds);
    }

    if output[0] & 0x0f != 0 {
        return Err(RxError::Ignored);
    }
    if output[1] & (0x04 | 0x40) != 0 || output[22] & 0x0f != 0 {
        return Err(RxError::Unsupported);
    }
    let subtype = output[0] & 0xf0;
    if matches!(
        subtype,
        SUBTYPE_AUTH | SUBTYPE_ASSOC_RESPONSE | SUBTYPE_REASSOC_RESPONSE
    ) && frame_length < MLME_HEADER_SIZE + MLME_AUTH_BODY_SIZE
    {
        return Err(RxError::Bounds);
    }

    Ok(RxManagementFrame {
        length: frame_length,
        tail_offset,
        signal_length: signal_length as u16,
        dump_length: dump_length as u16,
        rxend_state,
        rx_state,
        internal_state: segments[0].buffer[tail_offset + TAIL_INTERNAL_OFFSET],
        dump_length_matches: dump_matches,
    })
}
