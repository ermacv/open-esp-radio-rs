//! Allocation-free TX BlockAck negotiation state in the live MAC crate.
//!
//! The stock `ieee80211_ampdu_request` allocates one vendor agreement object
//! per TID and arms an OS timer. This module owns the protocol state and its
//! deadline instead. A caller sends the returned action body through the
//! fixed management-frame pool and programs `TxBlockAckAlarm::deadline_us`
//! into a Rust async timer.

use open_esp_radio_pac_esp32s31::RadioRegisters;

pub const BLOCK_ACK_CATEGORY: u8 = 3;
pub const ADDBA_REQUEST_ACTION: u8 = 0;
pub const ADDBA_RESPONSE_ACTION: u8 = 1;
pub const DELBA_ACTION: u8 = 2;
pub const ADDBA_ACTION_BODY_LEN: usize = 9;
pub const TX_BLOCK_ACK_MAX_WINDOW: u16 = 32;
pub const TX_AMPDU_SLOT_CAPACITY: usize = TX_BLOCK_ACK_MAX_WINDOW as usize;
const BASIC_HT_RATE_MIN: u8 = 16;
const BASIC_HT_RATE_MAX: u8 = 35;
const TX_DESCRIPTOR_HE_BIT: u32 = 0x8000_0000;
const TX_DESCRIPTOR_BAR_BIT: u32 = 0x0020_0000;
const TX_DESCRIPTOR_AMPDU_BIT: u32 = 0x0040_0000;
const TX_DESCRIPTOR_AMPDU_FIRST_BITS: u32 = 0x0048_0000;
const TX_BUFFER_END_BIT: u32 = 0x4000_0000;
const FIRST_MPDU_RETRY_HEADER_BIT: u32 = 0x0100_0000;
const HT_MPDU_LENGTH_MASK: u32 = 0x3fff;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BlockAckAction {
    AddbaRequest {
        dialog_token: u8,
        tid: u8,
        immediate: bool,
        amsdu: bool,
        window: u16,
        timeout_tu: u16,
        starting_sequence: u16,
    },
    AddbaResponse {
        dialog_token: u8,
        status: u16,
        tid: u8,
        immediate: bool,
        amsdu: bool,
        window: u16,
        timeout_tu: u16,
    },
    Delba {
        tid: u8,
        initiator: bool,
        reason: u16,
    },
}

/// Parse the body of one IEEE 802.11 Block Ack Action frame.
///
/// This is a stateless leaf: it only reads the supplied bytes and does not
/// allocate, wait, access global state or call into the vendor library.
pub fn parse_block_ack_action(body: &[u8]) -> Option<BlockAckAction> {
    if body.len() < 2 || body[0] != BLOCK_ACK_CATEGORY {
        return None;
    }
    match body[1] {
        ADDBA_REQUEST_ACTION if body.len() >= ADDBA_ACTION_BODY_LEN => {
            let parameters = u16::from_le_bytes([body[3], body[4]]);
            let starting_sequence = u16::from_le_bytes([body[7], body[8]]) >> 4;
            Some(BlockAckAction::AddbaRequest {
                dialog_token: body[2],
                tid: ((parameters >> 2) & 0x0f) as u8,
                immediate: parameters & 0x0002 != 0,
                amsdu: parameters & 0x0001 != 0,
                window: (parameters >> 6) & 0x03ff,
                timeout_tu: u16::from_le_bytes([body[5], body[6]]),
                starting_sequence,
            })
        }
        ADDBA_RESPONSE_ACTION if body.len() >= ADDBA_ACTION_BODY_LEN => {
            let parameters = u16::from_le_bytes([body[5], body[6]]);
            Some(BlockAckAction::AddbaResponse {
                dialog_token: body[2],
                status: u16::from_le_bytes([body[3], body[4]]),
                tid: ((parameters >> 2) & 0x0f) as u8,
                immediate: parameters & 0x0002 != 0,
                amsdu: parameters & 0x0001 != 0,
                window: (parameters >> 6) & 0x03ff,
                timeout_tu: u16::from_le_bytes([body[7], body[8]]),
            })
        }
        DELBA_ACTION if body.len() >= 6 => {
            let parameters = u16::from_le_bytes([body[2], body[3]]);
            Some(BlockAckAction::Delba {
                tid: ((parameters >> 12) & 0x0f) as u8,
                initiator: parameters & 0x0800 != 0,
                reason: u16::from_le_bytes([body[4], body[5]]),
            })
        }
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HtAmpduLengthError {
    InvalidLimits,
    Empty,
    ZeroMpduLength,
    WindowFull,
    AggregateTooLong(u32),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HtAmpduLength {
    pub bytes: u16,
    pub subframes: u8,
}

/// Exact basic-HT A-MPDU byte accounting recovered from the pinned PP blob.
///
/// Bits 0..13 of `payload_word` carry the MPDU length. `empty_delimiters` is
/// the byte immediately following that word in the PP metadata prefix. Every
/// non-final MPDU contributes its four-byte delimiter, its length rounded to
/// four bytes, and the requested empty delimiters. `finish` removes the final
/// padding and empty delimiters, leaving the last MPDU's mandatory delimiter.
///
/// The accumulator has a caller-selected fixed window and byte ceiling. It
/// never allocates, accesses pointers, reads time, retries, waits or invokes a
/// callback.
pub struct HtAmpduLengthAccumulator {
    bytes_with_tail: u32,
    tail_bytes: u16,
    count: u8,
    max_subframes: u8,
    max_bytes: u16,
}

impl HtAmpduLengthAccumulator {
    pub const fn new(max_subframes: u8, max_bytes: u16) -> Result<Self, HtAmpduLengthError> {
        if max_subframes == 0 || max_subframes as usize > TX_AMPDU_SLOT_CAPACITY || max_bytes == 0 {
            return Err(HtAmpduLengthError::InvalidLimits);
        }
        Ok(Self {
            bytes_with_tail: 0,
            tail_bytes: 0,
            count: 0,
            max_subframes,
            max_bytes,
        })
    }

    pub const fn push(
        &mut self,
        payload_word: u32,
        empty_delimiters: u8,
    ) -> Result<(), HtAmpduLengthError> {
        if self.count >= self.max_subframes {
            return Err(HtAmpduLengthError::WindowFull);
        }
        let mpdu_bytes = payload_word & HT_MPDU_LENGTH_MASK;
        if mpdu_bytes == 0 {
            return Err(HtAmpduLengthError::ZeroMpduLength);
        }
        let padding = (4 - (mpdu_bytes & 3)) & 3;
        let empty_bytes = (empty_delimiters as u32) * 4;
        let contribution = mpdu_bytes + padding + empty_bytes + 4;
        let next = self.bytes_with_tail + contribution;
        let final_bytes = next - padding - empty_bytes;
        if final_bytes > self.max_bytes as u32 {
            return Err(HtAmpduLengthError::AggregateTooLong(final_bytes));
        }
        self.bytes_with_tail = next;
        self.tail_bytes = (padding + empty_bytes) as u16;
        self.count += 1;
        Ok(())
    }

    pub const fn finish(&self) -> Result<HtAmpduLength, HtAmpduLengthError> {
        if self.count == 0 {
            return Err(HtAmpduLengthError::Empty);
        }
        let bytes = self.bytes_with_tail - self.tail_bytes as u32;
        if bytes > self.max_bytes as u32 {
            return Err(HtAmpduLengthError::AggregateTooLong(bytes));
        }
        Ok(HtAmpduLength {
            bytes: bytes as u16,
            subframes: self.count,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BasicHtAmpduAssemblyError {
    AggregateShorterThanHeader,
    UnsupportedRate(u8),
    UnsupportedDescriptor(u32),
    TailAlreadyTerminated(u32),
    NullFrame,
    NullDescriptor,
    NullBufferDescriptor,
    NullPayload,
    LastFrameNotReachable,
    FrameAfterLast,
    BufferChainMismatch,
    WindowExceeded,
}

/// Inputs consumed by the finite HT `ppAssembleAMPDU` leaf.
///
/// This value form keeps the recovered bit transition host-testable. The
/// target-only pointer wrapper below validates the complete ESF/buffer chain
/// before applying the same mutation to SRAM.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BasicHtAmpduAssemblyInput {
    pub aggregate_length: u16,
    pub first_header_length: u16,
    pub first_payload_word: u32,
    pub first_descriptor_flags: u32,
    pub first_descriptor_word1: u32,
    pub first_rate: u8,
    pub tail_buffer_flags: u32,
    pub tail_timestamp: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BasicHtAmpduAssemblyOutput {
    pub first_remaining_length: u16,
    pub first_payload_word: u32,
    pub first_descriptor_flags: u32,
    pub first_descriptor_word1: u32,
    pub tail_buffer_flags: u32,
    pub first_timestamp: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BasicHtAmpduCompletionInput {
    pub descriptor_flags: u32,
    pub descriptor_queue_word: u32,
    pub frame_control: u16,
    pub acknowledged: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BasicHtAmpduCompletionOutput {
    pub descriptor_flags: u32,
    pub descriptor_queue_word: u32,
    pub frame_control: u16,
}

/// Reproduce the per-MPDU markers observed around `ppResortTxAMPDU` after the
/// aggregate topology has been detached.
///
/// Acknowledged MPDUs retain the aggregate marker so the later TX-done stage
/// skips duplicate per-frame rate control and gain bit 24 in the descriptor
/// queue word. A missing MPDU remains a normal detached descriptor and gains
/// only the IEEE 802.11 Retry bit; its CCMP-ready payload is reused unchanged.
#[inline(always)]
pub const fn basic_ht_ampdu_completion(
    input: BasicHtAmpduCompletionInput,
) -> BasicHtAmpduCompletionOutput {
    if input.acknowledged {
        BasicHtAmpduCompletionOutput {
            descriptor_flags: input.descriptor_flags | TX_DESCRIPTOR_AMPDU_BIT,
            descriptor_queue_word: input.descriptor_queue_word | 0x0100_0000,
            frame_control: input.frame_control,
        }
    } else {
        BasicHtAmpduCompletionOutput {
            descriptor_flags: input.descriptor_flags,
            descriptor_queue_word: input.descriptor_queue_word,
            frame_control: input.frame_control | 0x0800,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BasicHtAmpduChainError {
    Empty,
    TooManyFrames(usize),
    NullFrame(u8),
    DuplicateFrame(u8),
    NullDescriptor(u8),
    NullBufferDescriptor(u8),
    NullPayload(u8),
    ExistingFrameLink(u8),
    ExistingBufferLink(u8),
    UnsupportedDescriptor { index: u8, flags: u32 },
    UnsupportedRate { index: u8, rate: u8 },
    RateMismatch { index: u8, first: u8, rate: u8 },
    Length(HtAmpduLengthError),
    Assembly(BasicHtAmpduAssemblyError),
}

#[derive(Debug, Eq, PartialEq)]
pub struct BasicHtAmpduChain {
    pub first: *mut u8,
    pub last: *mut u8,
    pub aggregate_length: u16,
    pub subframes: u8,
    frames: [*mut u8; TX_AMPDU_SLOT_CAPACITY],
    sequences: [u16; TX_AMPDU_SLOT_CAPACITY],
    original_first_remaining_length: u16,
    original_first_payload_word: u32,
    original_first_descriptor_flags: u32,
    original_first_descriptor_word1: u32,
    original_first_timestamp: u32,
    original_first_frame_count: u8,
    original_first_spatial_count: u8,
    original_first_coding_count: u8,
    original_tail_buffer_flags: [u32; TX_AMPDU_SLOT_CAPACITY],
}

impl BasicHtAmpduChain {
    /// Return one frame owned by this prepared aggregate.
    ///
    /// The fixed array is intentionally private so no caller can substitute a
    /// pointer after validation and before hardware completion.
    pub const fn frame(&self, index: u8) -> Option<*mut u8> {
        if index < self.subframes {
            Some(self.frames[index as usize])
        } else {
            None
        }
    }

    /// Return the QoS sequence number captured from the validated ESF.
    ///
    /// The ESP32-S31 PP metadata stores this 12-bit value in `frame + 0x24`.
    /// Keeping every value avoids assuming that a retry aggregate is
    /// necessarily consecutive when interpreting the BlockAck bitmap.
    pub const fn sequence(&self, index: u8) -> Option<u16> {
        if index < self.subframes {
            Some(self.sequences[index as usize])
        } else {
            None
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BasicHtAmpduRestoreError {
    InvalidCount(u8),
    NullFrame(u8),
    FrameLinkMismatch(u8),
    NullBufferDescriptor(u8),
    BufferLinkMismatch(u8),
    NullDescriptor,
    NullPayload,
    AggregateStateMissing(u32),
    FrameCountMismatch(u8),
    DescriptorCountMismatch { spatial: u8, coding: u8 },
    TailStateMissing(u32),
}

/// Reproduce the `ni + 0x82` protection-spacing value written by the pinned
/// `rcUpdateAMPDUParam` body from the peer's HT A-MPDU Parameters byte.
///
/// Bits 2..=4 encode the IEEE 802.11 minimum MPDU start spacing. The hardware
/// consumes the recovered finite value in all three 10-bit protection fields.
pub(crate) const fn basic_ht_ampdu_protection_spacing(parameters: u8) -> u16 {
    match (parameters >> 2) & 0x07 {
        0..=4 => 20,
        5 => 40,
        6 => 76,
        _ => 148,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BasicHtAmpduFrameCompletionError {
    NullFrame,
    FrameStillLinked,
    NullDescriptor,
    UnsupportedDescriptor(u32),
    NullBufferDescriptor,
    BufferStillLinked,
    NullPayload,
    UnsupportedFrameControl(u16),
}

/// Reproduce the mutation made by the pinned non-HE `ppAssembleAMPDU` body.
///
/// There is no allocation, callback, lock, timer, retry or pointer access in
/// this value-level operation.
#[inline(always)]
pub const fn basic_ht_ampdu_assembly(
    input: BasicHtAmpduAssemblyInput,
) -> Result<BasicHtAmpduAssemblyOutput, BasicHtAmpduAssemblyError> {
    if input.aggregate_length < input.first_header_length {
        return Err(BasicHtAmpduAssemblyError::AggregateShorterThanHeader);
    }
    if input.first_rate < BASIC_HT_RATE_MIN || input.first_rate > BASIC_HT_RATE_MAX {
        return Err(BasicHtAmpduAssemblyError::UnsupportedRate(input.first_rate));
    }
    if input.first_descriptor_flags
        & (TX_DESCRIPTOR_HE_BIT | TX_DESCRIPTOR_BAR_BIT | TX_DESCRIPTOR_AMPDU_BIT)
        != 0
    {
        return Err(BasicHtAmpduAssemblyError::UnsupportedDescriptor(
            input.first_descriptor_flags,
        ));
    }
    if input.tail_buffer_flags & TX_BUFFER_END_BIT != 0 {
        return Err(BasicHtAmpduAssemblyError::TailAlreadyTerminated(
            input.tail_buffer_flags,
        ));
    }
    Ok(BasicHtAmpduAssemblyOutput {
        first_remaining_length: input
            .aggregate_length
            .wrapping_sub(input.first_header_length),
        first_payload_word: input.first_payload_word & !FIRST_MPDU_RETRY_HEADER_BIT,
        first_descriptor_flags: (input.first_descriptor_flags & !TX_BUFFER_END_BIT)
            | TX_DESCRIPTOR_AMPDU_FIRST_BITS,
        // The ROM leaf performs one byte load followed by a word store.
        first_descriptor_word1: input.first_descriptor_word1 & 0xff,
        tail_buffer_flags: input.tail_buffer_flags | TX_BUFFER_END_BIT,
        first_timestamp: input.tail_timestamp,
    })
}

/// Validate and assemble one statically owned basic-HT ESF chain.
///
/// # Safety
///
/// `first` through `last`, their `+0x04/+0x08` buffer descriptors, `+0x34`
/// TX descriptors and the first payload pointer must remain valid writable
/// SRAM for this call. Each `+0x30` frame link and buffer-descriptor `+0x08`
/// link must describe the same finite chain. The function validates every
/// pointer/flag/length invariant before performing any write.
#[cfg(target_arch = "riscv32")]
#[link_section = ".rwtext.wifi_strict.tx_ampdu_assemble"]
pub unsafe fn assemble_basic_ht_ampdu(
    first: *mut u8,
    last: *mut u8,
    aggregate_length: u16,
) -> Result<u8, BasicHtAmpduAssemblyError> {
    const FRAME_PAYLOAD_BUFFER_OFFSET: usize = 0x04;
    const FRAME_CHAIN_BUFFER_OFFSET: usize = 0x08;
    const FRAME_HEADER_LENGTH_OFFSET: usize = 0x14;
    const FRAME_REMAINING_LENGTH_OFFSET: usize = 0x16;
    const FRAME_NEXT_OFFSET: usize = 0x30;
    const FRAME_DESCRIPTOR_OFFSET: usize = 0x34;
    const BUFFER_DATA_OFFSET: usize = 0x04;
    const BUFFER_NEXT_OFFSET: usize = 0x08;
    const DESCRIPTOR_RATE_OFFSET: usize = 0x0c;
    const DESCRIPTOR_TIMESTAMP_OFFSET: usize = 0x18;

    if first.is_null() || last.is_null() {
        return Err(BasicHtAmpduAssemblyError::NullFrame);
    }

    // Prove both linked representations before mutating either one. This is a
    // bounded validation pass, not a wait/retry loop.
    let mut frame = first;
    let mut count = 0_u8;
    let mut found_last = false;
    while usize::from(count) < TX_AMPDU_SLOT_CAPACITY {
        if frame.is_null() {
            return Err(BasicHtAmpduAssemblyError::LastFrameNotReachable);
        }
        count += 1;
        let descriptor = frame.add(FRAME_DESCRIPTOR_OFFSET).cast::<*mut u8>().read();
        if descriptor.is_null() {
            return Err(BasicHtAmpduAssemblyError::NullDescriptor);
        }
        let descriptor_flags = descriptor.cast::<u32>().read();
        if descriptor_flags
            & (TX_DESCRIPTOR_HE_BIT | TX_DESCRIPTOR_BAR_BIT | TX_DESCRIPTOR_AMPDU_BIT)
            != 0
        {
            return Err(BasicHtAmpduAssemblyError::UnsupportedDescriptor(
                descriptor_flags,
            ));
        }
        let buffer = frame
            .add(FRAME_CHAIN_BUFFER_OFFSET)
            .cast::<*mut u8>()
            .read();
        if buffer.is_null() {
            return Err(BasicHtAmpduAssemblyError::NullBufferDescriptor);
        }
        let next = frame.add(FRAME_NEXT_OFFSET).cast::<*mut u8>().read();
        let next_buffer = buffer.add(BUFFER_NEXT_OFFSET).cast::<*mut u8>().read();
        if frame == last {
            if !next.is_null() || !next_buffer.is_null() {
                return Err(BasicHtAmpduAssemblyError::FrameAfterLast);
            }
            found_last = true;
            break;
        }
        if next.is_null() {
            return Err(BasicHtAmpduAssemblyError::LastFrameNotReachable);
        }
        let expected_buffer = next
            .add(FRAME_PAYLOAD_BUFFER_OFFSET)
            .cast::<*mut u8>()
            .read();
        if expected_buffer.is_null() || next_buffer != expected_buffer {
            return Err(BasicHtAmpduAssemblyError::BufferChainMismatch);
        }
        frame = next;
    }
    if !found_last {
        return Err(BasicHtAmpduAssemblyError::WindowExceeded);
    }

    let first_descriptor = first.add(FRAME_DESCRIPTOR_OFFSET).cast::<*mut u8>().read();
    let first_buffer = first
        .add(FRAME_PAYLOAD_BUFFER_OFFSET)
        .cast::<*mut u8>()
        .read();
    let tail_buffer = last.add(FRAME_CHAIN_BUFFER_OFFSET).cast::<*mut u8>().read();
    let tail_descriptor = last.add(FRAME_DESCRIPTOR_OFFSET).cast::<*mut u8>().read();
    if first_buffer.is_null() || tail_buffer.is_null() {
        return Err(BasicHtAmpduAssemblyError::NullBufferDescriptor);
    }
    if first_descriptor.is_null() || tail_descriptor.is_null() {
        return Err(BasicHtAmpduAssemblyError::NullDescriptor);
    }
    let first_payload = first_buffer
        .add(BUFFER_DATA_OFFSET)
        .cast::<*mut u8>()
        .read();
    if first_payload.is_null() {
        return Err(BasicHtAmpduAssemblyError::NullPayload);
    }
    let output = basic_ht_ampdu_assembly(BasicHtAmpduAssemblyInput {
        aggregate_length,
        first_header_length: first.add(FRAME_HEADER_LENGTH_OFFSET).cast::<u16>().read(),
        first_payload_word: first_payload.cast::<u32>().read(),
        first_descriptor_flags: first_descriptor.cast::<u32>().read(),
        first_descriptor_word1: first_descriptor.add(4).cast::<u32>().read(),
        first_rate: first_descriptor.add(DESCRIPTOR_RATE_OFFSET).read(),
        tail_buffer_flags: tail_buffer.cast::<u32>().read(),
        tail_timestamp: tail_descriptor
            .add(DESCRIPTOR_TIMESTAMP_OFFSET)
            .cast::<u32>()
            .read(),
    })?;

    first_payload.cast::<u32>().write(output.first_payload_word);
    first_descriptor
        .cast::<u32>()
        .write(output.first_descriptor_flags);
    first_descriptor
        .add(4)
        .cast::<u32>()
        .write(output.first_descriptor_word1);
    first
        .add(FRAME_REMAINING_LENGTH_OFFSET)
        .cast::<u16>()
        .write(output.first_remaining_length);
    tail_buffer.cast::<u32>().write(output.tail_buffer_flags);
    first_descriptor
        .add(DESCRIPTOR_TIMESTAMP_OFFSET)
        .cast::<u32>()
        .write(output.first_timestamp);
    Ok(count)
}

/// Build and assemble a basic-HT chain from independently owned ESF frames.
///
/// The function performs a complete bounded validation pass before its first
/// write. It then links each frame's `+0x30` field and each frame-tail buffer's
/// `+0x08` field, and applies the recovered first/tail A-MPDU mutation. Both
/// passes are capped at the 32-frame static BlockAck window.
///
/// # Safety
///
/// Every entry and the pointers reachable through its `+0x04`, `+0x08`, and
/// `+0x34` fields must remain valid writable SRAM under the single radio owner
/// for this call. Each input frame must still be independent: its frame link
/// and tail-buffer link must be null. Its existing internal buffer chain is
/// owned by the caller and must connect the first buffer to the stated tail.
#[cfg(target_arch = "riscv32")]
#[link_section = ".rwtext.wifi_strict.tx_ampdu_chain"]
pub unsafe fn prepare_basic_ht_ampdu_chain(
    frames: &[*mut u8],
    max_aggregate_length: u16,
) -> Result<BasicHtAmpduChain, BasicHtAmpduChainError> {
    const FRAME_FIRST_BUFFER_OFFSET: usize = 0x04;
    const FRAME_TAIL_BUFFER_OFFSET: usize = 0x08;
    const FRAME_HEADER_LENGTH_OFFSET: usize = 0x14;
    const FRAME_REMAINING_LENGTH_OFFSET: usize = 0x16;
    const FRAME_SEQUENCE_OFFSET: usize = 0x24;
    const FRAME_AGGREGATE_COUNT_OFFSET: usize = 0x26;
    const FRAME_NEXT_OFFSET: usize = 0x30;
    const FRAME_DESCRIPTOR_OFFSET: usize = 0x34;
    const BUFFER_DATA_OFFSET: usize = 0x04;
    const BUFFER_NEXT_OFFSET: usize = 0x08;
    const DESCRIPTOR_RATE_OFFSET: usize = 0x0c;
    const DESCRIPTOR_TIMESTAMP_OFFSET: usize = 0x18;
    const DESCRIPTOR_SPATIAL_COUNT_OFFSET: usize = 0x2a;
    const DESCRIPTOR_CODING_COUNT_OFFSET: usize = 0x2e;

    if frames.is_empty() {
        return Err(BasicHtAmpduChainError::Empty);
    }
    if frames.len() > TX_AMPDU_SLOT_CAPACITY {
        return Err(BasicHtAmpduChainError::TooManyFrames(frames.len()));
    }

    let mut length = HtAmpduLengthAccumulator::new(frames.len() as u8, max_aggregate_length)
        .map_err(BasicHtAmpduChainError::Length)?;
    let mut first_rate = 0_u8;
    let mut sequences = [0_u16; TX_AMPDU_SLOT_CAPACITY];
    let mut original_tail_buffer_flags = [0_u32; TX_AMPDU_SLOT_CAPACITY];
    let mut index = 0_usize;
    while index < frames.len() {
        let frame = frames[index];
        let index_u8 = index as u8;
        if frame.is_null() {
            return Err(BasicHtAmpduChainError::NullFrame(index_u8));
        }
        let mut prior = 0_usize;
        while prior < index {
            if frames[prior] == frame {
                return Err(BasicHtAmpduChainError::DuplicateFrame(index_u8));
            }
            prior += 1;
        }
        if !frame
            .add(FRAME_NEXT_OFFSET)
            .cast::<*mut u8>()
            .read()
            .is_null()
        {
            return Err(BasicHtAmpduChainError::ExistingFrameLink(index_u8));
        }

        let descriptor = frame.add(FRAME_DESCRIPTOR_OFFSET).cast::<*mut u8>().read();
        if descriptor.is_null() {
            return Err(BasicHtAmpduChainError::NullDescriptor(index_u8));
        }
        let flags = descriptor.cast::<u32>().read();
        if flags & (TX_DESCRIPTOR_HE_BIT | TX_DESCRIPTOR_BAR_BIT | TX_DESCRIPTOR_AMPDU_BIT) != 0 {
            return Err(BasicHtAmpduChainError::UnsupportedDescriptor {
                index: index_u8,
                flags,
            });
        }
        let rate = descriptor.add(DESCRIPTOR_RATE_OFFSET).read();
        if rate < BASIC_HT_RATE_MIN || rate > BASIC_HT_RATE_MAX {
            return Err(BasicHtAmpduChainError::UnsupportedRate {
                index: index_u8,
                rate,
            });
        }
        if index == 0 {
            first_rate = rate;
        } else if rate != first_rate {
            return Err(BasicHtAmpduChainError::RateMismatch {
                index: index_u8,
                first: first_rate,
                rate,
            });
        }
        sequences[index] = (frame.add(FRAME_SEQUENCE_OFFSET).cast::<u32>().read() & 0x0fff) as u16;

        let first_buffer = frame
            .add(FRAME_FIRST_BUFFER_OFFSET)
            .cast::<*mut u8>()
            .read();
        let tail_buffer = frame.add(FRAME_TAIL_BUFFER_OFFSET).cast::<*mut u8>().read();
        if first_buffer.is_null() || tail_buffer.is_null() {
            return Err(BasicHtAmpduChainError::NullBufferDescriptor(index_u8));
        }
        if !tail_buffer
            .add(BUFFER_NEXT_OFFSET)
            .cast::<*mut u8>()
            .read()
            .is_null()
        {
            return Err(BasicHtAmpduChainError::ExistingBufferLink(index_u8));
        }
        original_tail_buffer_flags[index] = tail_buffer.cast::<u32>().read();
        let payload = first_buffer
            .add(BUFFER_DATA_OFFSET)
            .cast::<*mut u8>()
            .read();
        if payload.is_null() {
            return Err(BasicHtAmpduChainError::NullPayload(index_u8));
        }
        length
            .push(payload.cast::<u32>().read(), payload.add(4).read())
            .map_err(BasicHtAmpduChainError::Length)?;
        index += 1;
    }

    let aggregate = length.finish().map_err(BasicHtAmpduChainError::Length)?;
    let first = frames[0];
    let last = frames[frames.len() - 1];
    let first_descriptor = first.add(FRAME_DESCRIPTOR_OFFSET).cast::<*mut u8>().read();
    let first_buffer = first
        .add(FRAME_FIRST_BUFFER_OFFSET)
        .cast::<*mut u8>()
        .read();
    let first_payload = first_buffer
        .add(BUFFER_DATA_OFFSET)
        .cast::<*mut u8>()
        .read();
    let tail_buffer = last.add(FRAME_TAIL_BUFFER_OFFSET).cast::<*mut u8>().read();
    let tail_descriptor = last.add(FRAME_DESCRIPTOR_OFFSET).cast::<*mut u8>().read();
    let output = basic_ht_ampdu_assembly(BasicHtAmpduAssemblyInput {
        aggregate_length: aggregate.bytes,
        first_header_length: first.add(FRAME_HEADER_LENGTH_OFFSET).cast::<u16>().read(),
        first_payload_word: first_payload.cast::<u32>().read(),
        first_descriptor_flags: first_descriptor.cast::<u32>().read(),
        first_descriptor_word1: first_descriptor.add(4).cast::<u32>().read(),
        first_rate,
        // `ppMapTxQueue` presents every independently mapped MPDU as a
        // complete one-frame DMA chain, so its tail already carries END.
        // The recovered `ppAssembleAMPDU` leaf instead receives a chain whose
        // intermediate END markers have been cleared by the scheduler.
        tail_buffer_flags: tail_buffer.cast::<u32>().read() & !TX_BUFFER_END_BIT,
        tail_timestamp: tail_descriptor
            .add(DESCRIPTOR_TIMESTAMP_OFFSET)
            .cast::<u32>()
            .read(),
    })
    .map_err(BasicHtAmpduChainError::Assembly)?;

    let original_first_remaining_length = first
        .add(FRAME_REMAINING_LENGTH_OFFSET)
        .cast::<u16>()
        .read();
    let original_first_payload_word = first_payload.cast::<u32>().read();
    let original_first_descriptor_flags = first_descriptor.cast::<u32>().read();
    let original_first_descriptor_word1 = first_descriptor.add(4).cast::<u32>().read();
    let original_first_timestamp = first_descriptor
        .add(DESCRIPTOR_TIMESTAMP_OFFSET)
        .cast::<u32>()
        .read();
    let original_first_frame_count = first.add(FRAME_AGGREGATE_COUNT_OFFSET).read();
    let original_first_spatial_count = first_descriptor.add(DESCRIPTOR_SPATIAL_COUNT_OFFSET).read();
    let original_first_coding_count = first_descriptor.add(DESCRIPTOR_CODING_COUNT_OFFSET).read();
    let mut owned_frames = [core::ptr::null_mut(); TX_AMPDU_SLOT_CAPACITY];
    owned_frames[..frames.len()].copy_from_slice(frames);

    index = 0;
    while index < frames.len() {
        let frame = frames[index];
        let tail_buffer = frame.add(FRAME_TAIL_BUFFER_OFFSET).cast::<*mut u8>().read();
        let next = if index + 1 < frames.len() {
            frames[index + 1]
        } else {
            core::ptr::null_mut()
        };
        let next_buffer = if next.is_null() {
            core::ptr::null_mut()
        } else {
            next.add(FRAME_FIRST_BUFFER_OFFSET).cast::<*mut u8>().read()
        };
        frame.add(FRAME_NEXT_OFFSET).cast::<*mut u8>().write(next);
        tail_buffer
            .add(BUFFER_NEXT_OFFSET)
            .cast::<*mut u8>()
            .write(next_buffer);
        // Clear the terminal marker while this independently mapped frame is
        // a member of the aggregate. The final aggregate tail receives END
        // from `output.tail_buffer_flags` below.
        tail_buffer
            .cast::<u32>()
            .write(original_tail_buffer_flags[index] & !TX_BUFFER_END_BIT);
        index += 1;
    }

    first_payload.cast::<u32>().write(output.first_payload_word);
    first_descriptor
        .cast::<u32>()
        .write(output.first_descriptor_flags);
    first_descriptor
        .add(4)
        .cast::<u32>()
        .write(output.first_descriptor_word1);
    first
        .add(FRAME_REMAINING_LENGTH_OFFSET)
        .cast::<u16>()
        .write(output.first_remaining_length);
    tail_buffer.cast::<u32>().write(output.tail_buffer_flags);
    first_descriptor
        .add(DESCRIPTOR_TIMESTAMP_OFFSET)
        .cast::<u32>()
        .write(output.first_timestamp);
    // `ppCalTxAMPDULength` clears these three bytes, then increments each one
    // for every accepted MPDU before entering `ppAssembleAMPDU`. They are not
    // cosmetic scheduler state: `mac_tx_set_htsig` copies the two descriptor
    // bytes into the HT control register.
    first
        .add(FRAME_AGGREGATE_COUNT_OFFSET)
        .write(aggregate.subframes);
    first_descriptor
        .add(DESCRIPTOR_SPATIAL_COUNT_OFFSET)
        .write(aggregate.subframes);
    first_descriptor
        .add(DESCRIPTOR_CODING_COUNT_OFFSET)
        .write(aggregate.subframes);

    Ok(BasicHtAmpduChain {
        first,
        last,
        aggregate_length: aggregate.bytes,
        subframes: aggregate.subframes,
        frames: owned_frames,
        sequences,
        original_first_remaining_length,
        original_first_payload_word,
        original_first_descriptor_flags,
        original_first_descriptor_word1,
        original_first_timestamp,
        original_first_frame_count,
        original_first_spatial_count,
        original_first_coding_count,
        original_tail_buffer_flags,
    })
}

/// Validate and undo the exact mutations made by
/// `prepare_basic_ht_ampdu_chain`.
///
/// Validation covers the complete fixed frame/buffer topology before the
/// first write. Restoration then detaches both linked representations and
/// reinstates every scalar changed by the assembly oracle. It neither
/// recycles nor retries a frame; the executor remains the sole owner of all
/// returned MPDUs and can process one completion per wake.
///
/// # Safety
///
/// `chain` must still exclusively own every SRAM pointer captured during
/// preparation. Hardware must be idle for the corresponding queue and no
/// frame may be recycled or relinked concurrently.
#[cfg(target_arch = "riscv32")]
#[link_section = ".rwtext.wifi_strict.tx_ampdu_restore"]
pub unsafe fn restore_basic_ht_ampdu_chain(
    chain: &BasicHtAmpduChain,
) -> Result<(), BasicHtAmpduRestoreError> {
    const FRAME_FIRST_BUFFER_OFFSET: usize = 0x04;
    const FRAME_TAIL_BUFFER_OFFSET: usize = 0x08;
    const FRAME_REMAINING_LENGTH_OFFSET: usize = 0x16;
    const FRAME_AGGREGATE_COUNT_OFFSET: usize = 0x26;
    const FRAME_NEXT_OFFSET: usize = 0x30;
    const FRAME_DESCRIPTOR_OFFSET: usize = 0x34;
    const BUFFER_DATA_OFFSET: usize = 0x04;
    const BUFFER_NEXT_OFFSET: usize = 0x08;
    const DESCRIPTOR_TIMESTAMP_OFFSET: usize = 0x18;
    const DESCRIPTOR_SPATIAL_COUNT_OFFSET: usize = 0x2a;
    const DESCRIPTOR_CODING_COUNT_OFFSET: usize = 0x2e;

    if chain.subframes < 2 || usize::from(chain.subframes) > TX_AMPDU_SLOT_CAPACITY {
        return Err(BasicHtAmpduRestoreError::InvalidCount(chain.subframes));
    }
    let count = usize::from(chain.subframes);
    let mut index = 0_usize;
    while index < count {
        let frame = chain.frames[index];
        if frame.is_null() {
            return Err(BasicHtAmpduRestoreError::NullFrame(index as u8));
        }
        let expected_next = if index + 1 < count {
            chain.frames[index + 1]
        } else {
            core::ptr::null_mut()
        };
        let actual_next = frame.add(FRAME_NEXT_OFFSET).cast::<*mut u8>().read();
        if actual_next != expected_next {
            return Err(BasicHtAmpduRestoreError::FrameLinkMismatch(index as u8));
        }
        let tail_buffer = frame.add(FRAME_TAIL_BUFFER_OFFSET).cast::<*mut u8>().read();
        if tail_buffer.is_null() {
            return Err(BasicHtAmpduRestoreError::NullBufferDescriptor(index as u8));
        }
        let expected_next_buffer = if expected_next.is_null() {
            core::ptr::null_mut()
        } else {
            expected_next
                .add(FRAME_FIRST_BUFFER_OFFSET)
                .cast::<*mut u8>()
                .read()
        };
        let actual_next_buffer = tail_buffer.add(BUFFER_NEXT_OFFSET).cast::<*mut u8>().read();
        if actual_next_buffer != expected_next_buffer {
            return Err(BasicHtAmpduRestoreError::BufferLinkMismatch(index as u8));
        }
        index += 1;
    }

    let first_descriptor = chain
        .first
        .add(FRAME_DESCRIPTOR_OFFSET)
        .cast::<*mut u8>()
        .read();
    if first_descriptor.is_null() {
        return Err(BasicHtAmpduRestoreError::NullDescriptor);
    }
    let flags = first_descriptor.cast::<u32>().read();
    if flags & TX_DESCRIPTOR_AMPDU_BIT == 0 {
        return Err(BasicHtAmpduRestoreError::AggregateStateMissing(flags));
    }
    let frame_count = chain.first.add(FRAME_AGGREGATE_COUNT_OFFSET).read();
    if frame_count != chain.subframes {
        return Err(BasicHtAmpduRestoreError::FrameCountMismatch(frame_count));
    }
    let spatial_count = first_descriptor.add(DESCRIPTOR_SPATIAL_COUNT_OFFSET).read();
    let coding_count = first_descriptor.add(DESCRIPTOR_CODING_COUNT_OFFSET).read();
    if spatial_count != chain.subframes || coding_count != chain.subframes {
        return Err(BasicHtAmpduRestoreError::DescriptorCountMismatch {
            spatial: spatial_count,
            coding: coding_count,
        });
    }
    let first_buffer = chain
        .first
        .add(FRAME_FIRST_BUFFER_OFFSET)
        .cast::<*mut u8>()
        .read();
    let tail_buffer = chain
        .last
        .add(FRAME_TAIL_BUFFER_OFFSET)
        .cast::<*mut u8>()
        .read();
    if first_buffer.is_null() || tail_buffer.is_null() {
        return Err(BasicHtAmpduRestoreError::NullBufferDescriptor(0));
    }
    let first_payload = first_buffer
        .add(BUFFER_DATA_OFFSET)
        .cast::<*mut u8>()
        .read();
    if first_payload.is_null() {
        return Err(BasicHtAmpduRestoreError::NullPayload);
    }
    let tail_flags = tail_buffer.cast::<u32>().read();
    if tail_flags & TX_BUFFER_END_BIT == 0 {
        return Err(BasicHtAmpduRestoreError::TailStateMissing(tail_flags));
    }

    index = 0;
    while index < count {
        let frame = chain.frames[index];
        let tail_buffer = frame.add(FRAME_TAIL_BUFFER_OFFSET).cast::<*mut u8>().read();
        frame
            .add(FRAME_NEXT_OFFSET)
            .cast::<*mut u8>()
            .write(core::ptr::null_mut());
        tail_buffer
            .add(BUFFER_NEXT_OFFSET)
            .cast::<*mut u8>()
            .write(core::ptr::null_mut());
        tail_buffer
            .cast::<u32>()
            .write(chain.original_tail_buffer_flags[index]);
        index += 1;
    }
    first_payload
        .cast::<u32>()
        .write(chain.original_first_payload_word);
    first_descriptor
        .cast::<u32>()
        .write(chain.original_first_descriptor_flags);
    first_descriptor
        .add(4)
        .cast::<u32>()
        .write(chain.original_first_descriptor_word1);
    first_descriptor
        .add(DESCRIPTOR_TIMESTAMP_OFFSET)
        .cast::<u32>()
        .write(chain.original_first_timestamp);
    chain
        .first
        .add(FRAME_REMAINING_LENGTH_OFFSET)
        .cast::<u16>()
        .write(chain.original_first_remaining_length);
    chain
        .first
        .add(FRAME_AGGREGATE_COUNT_OFFSET)
        .write(chain.original_first_frame_count);
    first_descriptor
        .add(DESCRIPTOR_SPATIAL_COUNT_OFFSET)
        .write(chain.original_first_spatial_count);
    first_descriptor
        .add(DESCRIPTOR_CODING_COUNT_OFFSET)
        .write(chain.original_first_coding_count);
    Ok(())
}

/// Apply one BlockAck disposition to an already restored basic-HT MPDU.
///
/// This leaf only changes the three fields proven by the partial-BlockAck
/// oracle. It does not recycle, queue, encrypt, submit, retry, or invoke rate
/// control; the executor retains ownership and advances one MPDU per event.
///
/// # Safety
///
/// `frame` and its descriptor/buffer/payload pointers must remain exclusively
/// owned writable SRAM. `restore_basic_ht_ampdu_chain` must have detached it
/// from both aggregate linked representations before this call.
#[cfg(target_arch = "riscv32")]
#[link_section = ".rwtext.wifi_strict.tx_ampdu_completion"]
pub unsafe fn apply_basic_ht_ampdu_completion(
    frame: *mut u8,
    response: u8,
    acknowledged: bool,
) -> Result<(), BasicHtAmpduFrameCompletionError> {
    const FRAME_FIRST_BUFFER_OFFSET: usize = 0x04;
    const FRAME_TAIL_BUFFER_OFFSET: usize = 0x08;
    const FRAME_LAYOUT_FLAGS_OFFSET: usize = 0x24;
    const FRAME_NEXT_OFFSET: usize = 0x30;
    const FRAME_DESCRIPTOR_OFFSET: usize = 0x34;
    const BUFFER_DATA_OFFSET: usize = 0x04;
    const BUFFER_NEXT_OFFSET: usize = 0x08;
    const DESCRIPTOR_RESPONSE_OFFSET: usize = 0x0d;
    const DESCRIPTOR_QUEUE_WORD_OFFSET: usize = 0x10;
    const DESCRIPTOR_REASON_OFFSET: usize = 0x13;

    if frame.is_null() {
        return Err(BasicHtAmpduFrameCompletionError::NullFrame);
    }
    if !frame
        .add(FRAME_NEXT_OFFSET)
        .cast::<*mut u8>()
        .read()
        .is_null()
    {
        return Err(BasicHtAmpduFrameCompletionError::FrameStillLinked);
    }
    let descriptor = frame.add(FRAME_DESCRIPTOR_OFFSET).cast::<*mut u8>().read();
    if descriptor.is_null() {
        return Err(BasicHtAmpduFrameCompletionError::NullDescriptor);
    }
    let descriptor_flags = descriptor.cast::<u32>().read();
    if descriptor_flags & (TX_DESCRIPTOR_HE_BIT | TX_DESCRIPTOR_BAR_BIT | TX_DESCRIPTOR_AMPDU_BIT)
        != 0
    {
        return Err(BasicHtAmpduFrameCompletionError::UnsupportedDescriptor(
            descriptor_flags,
        ));
    }
    let first_buffer = frame
        .add(FRAME_FIRST_BUFFER_OFFSET)
        .cast::<*mut u8>()
        .read();
    let tail_buffer = frame.add(FRAME_TAIL_BUFFER_OFFSET).cast::<*mut u8>().read();
    if first_buffer.is_null() || tail_buffer.is_null() {
        return Err(BasicHtAmpduFrameCompletionError::NullBufferDescriptor);
    }
    if !tail_buffer
        .add(BUFFER_NEXT_OFFSET)
        .cast::<*mut u8>()
        .read()
        .is_null()
    {
        return Err(BasicHtAmpduFrameCompletionError::BufferStillLinked);
    }
    let mut header = first_buffer
        .add(BUFFER_DATA_OFFSET)
        .cast::<*mut u8>()
        .read();
    if header.is_null() {
        return Err(BasicHtAmpduFrameCompletionError::NullPayload);
    }
    if frame.add(FRAME_LAYOUT_FLAGS_OFFSET).cast::<u16>().read() & 0x2000 != 0 {
        header = header.add(8);
    }
    let frame_control = header.cast::<u16>().read_unaligned();
    if frame_control & 0x000c != 0x0008 {
        return Err(BasicHtAmpduFrameCompletionError::UnsupportedFrameControl(
            frame_control,
        ));
    }
    let descriptor_queue_word = descriptor
        .add(DESCRIPTOR_QUEUE_WORD_OFFSET)
        .cast::<u32>()
        .read();
    let output = basic_ht_ampdu_completion(BasicHtAmpduCompletionInput {
        descriptor_flags,
        descriptor_queue_word,
        frame_control,
        acknowledged,
    });

    descriptor.cast::<u32>().write(output.descriptor_flags);
    descriptor
        .add(DESCRIPTOR_QUEUE_WORD_OFFSET)
        .cast::<u32>()
        .write(output.descriptor_queue_word);
    header.cast::<u16>().write_unaligned(output.frame_control);
    if acknowledged {
        descriptor.add(DESCRIPTOR_RESPONSE_OFFSET).write(response);
        descriptor.add(DESCRIPTOR_REASON_OFFSET).write(1);
    }
    Ok(())
}

const BA_PARAMETER_AMSDU: u16 = 1;
const BA_PARAMETER_IMMEDIATE: u16 = 1 << 1;
const BA_PARAMETER_TID_SHIFT: u32 = 2;
const BA_PARAMETER_TID_MASK: u16 = 0x0f << BA_PARAMETER_TID_SHIFT;
const BA_PARAMETER_WINDOW_SHIFT: u32 = 6;
const BA_PARAMETER_WINDOW_MASK: u16 = 0x03ff << BA_PARAMETER_WINDOW_SHIFT;
const SEQUENCE_NUMBER_MASK: u16 = 0x0fff;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TxBlockAckError {
    InvalidTid(u8),
    InvalidWindow(u16),
    ZeroTimeout,
    DeadlineOverflow,
    MalformedResponse,
    UnexpectedResponse,
    DelayedPolicyUnsupported,
    WindowExceedsCapacity(u16),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TxBlockAckConfig {
    pub tid: u8,
    pub window: u16,
    pub timeout_tu: u16,
    pub negotiation_timeout_us: u32,
    pub amsdu: bool,
}

impl TxBlockAckConfig {
    pub const fn validate(self) -> Result<Self, TxBlockAckError> {
        if self.tid > 15 {
            return Err(TxBlockAckError::InvalidTid(self.tid));
        }
        if self.window == 0 || self.window > TX_BLOCK_ACK_MAX_WINDOW {
            return Err(TxBlockAckError::InvalidWindow(self.window));
        }
        if self.negotiation_timeout_us == 0 {
            return Err(TxBlockAckError::ZeroTimeout);
        }
        Ok(self)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TxBlockAckAlarm {
    pub generation: u32,
    pub deadline_us: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AddbaRequest {
    pub generation: u32,
    pub dialog_token: u8,
    pub starting_sequence: u16,
    pub body: [u8; ADDBA_ACTION_BODY_LEN],
    pub alarm: TxBlockAckAlarm,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OperationalTxBlockAck {
    pub tid: u8,
    pub window: u16,
    pub timeout_tu: u16,
    pub starting_sequence: u16,
    pub amsdu: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TxBlockAckResponse {
    Operational(OperationalTxBlockAck),
    Rejected(u16),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TxBlockAckPhase {
    Idle,
    Awaiting {
        dialog_token: u8,
        starting_sequence: u16,
    },
    Operational(OperationalTxBlockAck),
}

/// One statically owned TX BlockAck agreement for one QoS TID.
///
/// Every method performs a fixed number of loads/stores. Timer expiry is an
/// externally delivered edge; this type never reads time, sleeps, or retries.
pub struct TxBlockAckSession {
    config: TxBlockAckConfig,
    generation: u32,
    next_dialog_token: u8,
    phase: TxBlockAckPhase,
}

/// Opaque index of one statically owned TX frame.
///
/// The strict S31 data path has exactly 32 fixed TX slots. Keeping only their
/// indices here prevents the BlockAck state machine from owning raw pointers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TxAmpduSlot(u8);

impl TxAmpduSlot {
    pub const fn new(index: u8) -> Option<Self> {
        if (index as usize) < TX_AMPDU_SLOT_CAPACITY {
            Some(Self(index))
        } else {
            None
        }
    }

    pub const fn index(self) -> u8 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TxAmpduMpdu {
    pub slot: TxAmpduSlot,
    pub sequence: u16,
}

/// BlockAck information read from the MAC completion registers.
///
/// Bit zero acknowledges `starting_sequence`, bit one the following sequence,
/// and so on. The S31 completion block exposes 64 bits even though strict mode
/// deliberately negotiates a window of at most 32 frames.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TxBlockAckBitmap {
    pub starting_sequence: u16,
    pub bitmap: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HtBlockAckRegisters {
    pub control: u8,
    pub block_ack: TxBlockAckBitmap,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HtBlockAckReadError {
    InvalidHardwareQueue(u8),
}

/// Decode the fixed three-register result used by
/// `hal_mac_tx_get_blockack` on S31.
#[inline(always)]
pub const fn decode_ht_block_ack_registers(
    control_and_sequence: u32,
    bitmap_low: u32,
    bitmap_high: u32,
) -> HtBlockAckRegisters {
    HtBlockAckRegisters {
        control: ((control_and_sequence >> 16) & 0x0f) as u8,
        block_ack: TxBlockAckBitmap::new(
            ((control_and_sequence >> 4) & 0x0fff) as u16,
            (bitmap_low as u64) | ((bitmap_high as u64) << 32),
        ),
    }
}

/// Read one completed HT BlockAck without entering the vendor completion
/// graph. The body is three PAC reads and fixed bit decoding.
#[link_section = ".rwtext.wifi_strict.tx_block_ack"]
pub fn read_ht_block_ack(
    mmio: &RadioRegisters,
    hardware_queue: u8,
) -> Result<HtBlockAckRegisters, HtBlockAckReadError> {
    let registers = mmio
        .read_tx_block_ack_registers(hardware_queue)
        .ok_or(HtBlockAckReadError::InvalidHardwareQueue(hardware_queue))?;
    Ok(decode_ht_block_ack_registers(
        registers.control_and_sequence,
        registers.bitmap_low,
        registers.bitmap_high,
    ))
}

impl TxBlockAckBitmap {
    #[inline(always)]
    pub const fn new(starting_sequence: u16, bitmap: u64) -> Self {
        Self {
            starting_sequence: starting_sequence & SEQUENCE_NUMBER_MASK,
            bitmap,
        }
    }

    pub const fn acknowledges(self, sequence: u16) -> bool {
        let distance = sequence.wrapping_sub(self.starting_sequence) & SEQUENCE_NUMBER_MASK;
        distance < 64 && self.bitmap & (1_u64 << distance) != 0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TxAmpduDisposition {
    Acknowledged,
    Retry,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TxAmpduCompletion {
    pub mpdu: TxAmpduMpdu,
    pub disposition: TxAmpduDisposition,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TxAmpduBatchError {
    Busy,
    NotBuilding,
    Empty,
    InvalidWindow(u8),
    InvalidSlot(u8),
    DuplicateSlot(u8),
    DuplicateSequence(u16),
    Full,
}

#[derive(Clone, Copy)]
enum TxAmpduBatchPhase {
    Idle,
    Building,
    Completing(Option<TxBlockAckBitmap>),
}

/// One fixed TX A-MPDU batch owned by the Rust radio task.
///
/// `next_completion` returns at most one frame on every call. The executor can
/// therefore recycle or retry one MPDU and yield, instead of running the
/// vendor linked-list drains inside one PP event. There is no allocation,
/// clock read, retry loop, lock, or raw-pointer ownership in this type.
pub struct TxAmpduBatch {
    entries: [Option<TxAmpduMpdu>; TX_AMPDU_SLOT_CAPACITY],
    phase: TxAmpduBatchPhase,
    starting_sequence: u16,
    window: u8,
    count: u8,
    completion_index: u8,
    slot_mask: u32,
}

impl TxAmpduBatch {
    pub const fn new() -> Self {
        Self {
            entries: [None; TX_AMPDU_SLOT_CAPACITY],
            phase: TxAmpduBatchPhase::Idle,
            starting_sequence: 0,
            window: 0,
            count: 0,
            completion_index: 0,
            slot_mask: 0,
        }
    }

    pub fn begin(&mut self, starting_sequence: u16, window: u8) -> Result<(), TxAmpduBatchError> {
        if !matches!(self.phase, TxAmpduBatchPhase::Idle) {
            return Err(TxAmpduBatchError::Busy);
        }
        if window == 0 || usize::from(window) > TX_AMPDU_SLOT_CAPACITY {
            return Err(TxAmpduBatchError::InvalidWindow(window));
        }
        self.starting_sequence = starting_sequence & SEQUENCE_NUMBER_MASK;
        self.window = window;
        self.count = 0;
        self.completion_index = 0;
        self.slot_mask = 0;
        self.phase = TxAmpduBatchPhase::Building;
        Ok(())
    }

    /// Append one statically owned frame and assign its consecutive QoS
    /// sequence number. Duplicate slot ownership is rejected in O(1).
    pub fn push(&mut self, slot: u8) -> Result<TxAmpduMpdu, TxAmpduBatchError> {
        let sequence =
            self.starting_sequence.wrapping_add(u16::from(self.count)) & SEQUENCE_NUMBER_MASK;
        self.push_sequence(slot, sequence)
    }

    /// Append a statically owned frame whose sequence was already assigned by
    /// the finite PP framing leaf.
    ///
    /// This is the path used for a prepared hardware A-MPDU. It preserves the
    /// exact per-MPDU sequence numbers, including retry aggregates with holes,
    /// so BlockAck completion never depends on an inferred order.
    pub fn push_sequence(
        &mut self,
        slot: u8,
        sequence: u16,
    ) -> Result<TxAmpduMpdu, TxAmpduBatchError> {
        if !matches!(self.phase, TxAmpduBatchPhase::Building) {
            return Err(TxAmpduBatchError::NotBuilding);
        }
        let slot = TxAmpduSlot::new(slot).ok_or(TxAmpduBatchError::InvalidSlot(slot))?;
        let slot_bit = 1_u32 << slot.index();
        if self.slot_mask & slot_bit != 0 {
            return Err(TxAmpduBatchError::DuplicateSlot(slot.index()));
        }
        if self.count >= self.window {
            return Err(TxAmpduBatchError::Full);
        }

        let sequence = sequence & SEQUENCE_NUMBER_MASK;
        let mut index = 0_usize;
        while index < usize::from(self.count) {
            if self.entries[index].is_some_and(|entry| entry.sequence == sequence) {
                return Err(TxAmpduBatchError::DuplicateSequence(sequence));
            }
            index += 1;
        }
        let mpdu = TxAmpduMpdu { slot, sequence };
        self.entries[usize::from(self.count)] = Some(mpdu);
        self.count += 1;
        self.slot_mask |= slot_bit;
        Ok(mpdu)
    }

    pub fn complete_with_block_ack(
        &mut self,
        block_ack: TxBlockAckBitmap,
    ) -> Result<(), TxAmpduBatchError> {
        self.begin_completion(Some(block_ack))
    }

    /// Complete a hardware timeout/error edge. Every submitted MPDU is
    /// returned as `Retry`, one per `next_completion` call.
    pub fn complete_without_block_ack(&mut self) -> Result<(), TxAmpduBatchError> {
        self.begin_completion(None)
    }

    fn begin_completion(
        &mut self,
        block_ack: Option<TxBlockAckBitmap>,
    ) -> Result<(), TxAmpduBatchError> {
        if !matches!(self.phase, TxAmpduBatchPhase::Building) {
            return Err(TxAmpduBatchError::NotBuilding);
        }
        if self.count == 0 {
            return Err(TxAmpduBatchError::Empty);
        }
        self.completion_index = 0;
        self.phase = TxAmpduBatchPhase::Completing(block_ack);
        Ok(())
    }

    /// Consume exactly one completion result. Returning the last result also
    /// returns the batch to idle; no separate drain or cleanup loop exists.
    pub fn next_completion(&mut self) -> Option<TxAmpduCompletion> {
        let TxAmpduBatchPhase::Completing(block_ack) = self.phase else {
            return None;
        };
        if self.completion_index >= self.count {
            self.reset();
            return None;
        }

        let index = usize::from(self.completion_index);
        let mpdu = self.entries[index].take()?;
        self.completion_index += 1;
        self.slot_mask &= !(1_u32 << mpdu.slot.index());
        let disposition = if block_ack.is_some_and(|ack| ack.acknowledges(mpdu.sequence)) {
            TxAmpduDisposition::Acknowledged
        } else {
            TxAmpduDisposition::Retry
        };
        let completion = TxAmpduCompletion { mpdu, disposition };
        if self.completion_index == self.count {
            self.reset();
        }
        Some(completion)
    }

    pub const fn len(&self) -> usize {
        self.count as usize
    }

    pub const fn is_idle(&self) -> bool {
        matches!(self.phase, TxAmpduBatchPhase::Idle)
    }

    fn reset(&mut self) {
        self.phase = TxAmpduBatchPhase::Idle;
        self.window = 0;
        self.count = 0;
        self.completion_index = 0;
        self.slot_mask = 0;
    }
}

impl Default for TxAmpduBatch {
    fn default() -> Self {
        Self::new()
    }
}

impl TxBlockAckSession {
    pub const fn new(config: TxBlockAckConfig) -> Result<Self, TxBlockAckError> {
        let config = match config.validate() {
            Ok(config) => config,
            Err(error) => return Err(error),
        };
        Ok(Self {
            config,
            generation: 0,
            next_dialog_token: 1,
            phase: TxBlockAckPhase::Idle,
        })
    }

    pub fn begin(
        &mut self,
        starting_sequence: u16,
        now_us: u64,
    ) -> Result<AddbaRequest, TxBlockAckError> {
        let deadline_us = now_us
            .checked_add(u64::from(self.config.negotiation_timeout_us))
            .ok_or(TxBlockAckError::DeadlineOverflow)?;
        self.generation = next_generation(self.generation);
        let dialog_token = self.next_dialog_token;
        self.next_dialog_token = next_dialog_token(dialog_token);
        let starting_sequence = starting_sequence & SEQUENCE_NUMBER_MASK;
        self.phase = TxBlockAckPhase::Awaiting {
            dialog_token,
            starting_sequence,
        };

        let parameters =
            encode_ba_parameters(self.config.tid, self.config.window, self.config.amsdu);
        let sequence_control = starting_sequence << 4;
        let mut body = [0_u8; ADDBA_ACTION_BODY_LEN];
        body[0] = BLOCK_ACK_CATEGORY;
        body[1] = ADDBA_REQUEST_ACTION;
        body[2] = dialog_token;
        body[3..5].copy_from_slice(&parameters.to_le_bytes());
        body[5..7].copy_from_slice(&self.config.timeout_tu.to_le_bytes());
        body[7..9].copy_from_slice(&sequence_control.to_le_bytes());

        let alarm = TxBlockAckAlarm {
            generation: self.generation,
            deadline_us,
        };
        Ok(AddbaRequest {
            generation: self.generation,
            dialog_token,
            starting_sequence,
            body,
            alarm,
        })
    }

    pub fn on_response(&mut self, body: &[u8]) -> Result<TxBlockAckResponse, TxBlockAckError> {
        if body.len() != ADDBA_ACTION_BODY_LEN
            || body[0] != BLOCK_ACK_CATEGORY
            || body[1] != ADDBA_RESPONSE_ACTION
        {
            return Err(TxBlockAckError::MalformedResponse);
        }
        let TxBlockAckPhase::Awaiting {
            dialog_token,
            starting_sequence,
        } = self.phase
        else {
            return Err(TxBlockAckError::UnexpectedResponse);
        };
        if body[2] != dialog_token {
            return Err(TxBlockAckError::UnexpectedResponse);
        }

        let status = u16::from_le_bytes([body[3], body[4]]);
        if status != 0 {
            self.phase = TxBlockAckPhase::Idle;
            self.generation = next_generation(self.generation);
            return Ok(TxBlockAckResponse::Rejected(status));
        }

        let parameters = u16::from_le_bytes([body[5], body[6]]);
        if parameters & BA_PARAMETER_IMMEDIATE == 0 {
            return Err(TxBlockAckError::DelayedPolicyUnsupported);
        }
        let tid = ((parameters & BA_PARAMETER_TID_MASK) >> BA_PARAMETER_TID_SHIFT) as u8;
        if tid != self.config.tid {
            return Err(TxBlockAckError::UnexpectedResponse);
        }
        let window = (parameters & BA_PARAMETER_WINDOW_MASK) >> BA_PARAMETER_WINDOW_SHIFT;
        if window == 0 || window > self.config.window || window > TX_BLOCK_ACK_MAX_WINDOW {
            return Err(TxBlockAckError::WindowExceedsCapacity(window));
        }
        let agreement = OperationalTxBlockAck {
            tid,
            window,
            timeout_tu: u16::from_le_bytes([body[7], body[8]]),
            starting_sequence,
            amsdu: self.config.amsdu && parameters & BA_PARAMETER_AMSDU != 0,
        };
        self.phase = TxBlockAckPhase::Operational(agreement);
        self.generation = next_generation(self.generation);
        Ok(TxBlockAckResponse::Operational(agreement))
    }

    /// Consume one exact async timer edge. Returns true only when it cancelled
    /// the currently outstanding negotiation.
    pub fn on_alarm(&mut self, alarm: TxBlockAckAlarm) -> bool {
        if alarm.generation != self.generation
            || !matches!(self.phase, TxBlockAckPhase::Awaiting { .. })
        {
            return false;
        }
        self.phase = TxBlockAckPhase::Idle;
        self.generation = next_generation(self.generation);
        true
    }

    pub fn stop(&mut self) {
        self.phase = TxBlockAckPhase::Idle;
        self.generation = next_generation(self.generation);
    }

    pub const fn operational(&self) -> Option<OperationalTxBlockAck> {
        match self.phase {
            TxBlockAckPhase::Operational(agreement) => Some(agreement),
            _ => None,
        }
    }

    pub const fn is_awaiting(&self) -> bool {
        matches!(self.phase, TxBlockAckPhase::Awaiting { .. })
    }
}

const fn encode_ba_parameters(tid: u8, window: u16, amsdu: bool) -> u16 {
    ((amsdu as u16) * BA_PARAMETER_AMSDU)
        | BA_PARAMETER_IMMEDIATE
        | ((tid as u16) << BA_PARAMETER_TID_SHIFT)
        | (window << BA_PARAMETER_WINDOW_SHIFT)
}

const fn next_generation(current: u32) -> u32 {
    let next = current.wrapping_add(1);
    if next == 0 {
        1
    } else {
        next
    }
}

const fn next_dialog_token(current: u8) -> u8 {
    let next = current.wrapping_add(1);
    if next == 0 {
        1
    } else {
        next
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONFIG: TxBlockAckConfig = TxBlockAckConfig {
        tid: 7,
        window: 32,
        timeout_tu: 0,
        negotiation_timeout_us: 100_000,
        amsdu: true,
    };

    #[test]
    fn parses_block_ack_action_bodies_without_state() {
        assert_eq!(
            parse_block_ack_action(&[3, 0, 7, 0x87, 0x07, 0, 0, 0x30, 0x12]),
            Some(BlockAckAction::AddbaRequest {
                dialog_token: 7,
                tid: 1,
                immediate: true,
                amsdu: true,
                window: 30,
                timeout_tu: 0,
                starting_sequence: 0x123,
            })
        );
        assert_eq!(
            parse_block_ack_action(&[3, 1, 7, 0, 0, 0x86, 0x07, 5, 0]),
            Some(BlockAckAction::AddbaResponse {
                dialog_token: 7,
                status: 0,
                tid: 1,
                immediate: true,
                amsdu: false,
                window: 30,
                timeout_tu: 5,
            })
        );
        assert_eq!(
            parse_block_ack_action(&[3, 2, 0, 0x58, 39, 0]),
            Some(BlockAckAction::Delba {
                tid: 5,
                initiator: true,
                reason: 39,
            })
        );
        assert_eq!(parse_block_ack_action(&[4, 0, 0]), None);
    }

    #[test]
    fn ht_ampdu_length_matches_the_s31_six_mpdu_oracle() {
        let mut length = HtAmpduLengthAccumulator::new(32, u16::MAX).unwrap();
        for sequence in 0x15_u32..=0x1a {
            // The HIL payload metadata was 0x00ss0612 followed by a zero
            // delimiter byte: six 1,554-byte MPDUs with two-byte padding.
            length.push((sequence << 16) | 0x0612, 0).unwrap();
        }
        assert_eq!(
            length.finish(),
            Ok(HtAmpduLength {
                bytes: 9_358,
                subframes: 6,
            })
        );
    }

    #[test]
    fn ht_ampdu_length_is_bounded_and_removes_only_the_tail_trailer() {
        let mut length = HtAmpduLengthAccumulator::new(2, 4_096).unwrap();
        length.push(1_001, 2).unwrap();
        length.push(1_002, 1).unwrap();
        // First: 1001 + 3 padding + 8 empty + 4 mandatory.
        // Last: 1002 + 4 mandatory; its 2 padding + 4 empty bytes are removed.
        assert_eq!(length.finish().unwrap().bytes, 2_022);
        assert_eq!(length.push(1, 0), Err(HtAmpduLengthError::WindowFull));

        let mut too_short = HtAmpduLengthAccumulator::new(1, 1_000).unwrap();
        assert_eq!(
            too_short.push(1_001, 0),
            Err(HtAmpduLengthError::AggregateTooLong(1_005))
        );
        assert!(matches!(
            HtAmpduLengthAccumulator::new(0, 1),
            Err(HtAmpduLengthError::InvalidLimits)
        ));
    }

    #[test]
    fn block_ack_register_decode_matches_the_pinned_leaf_layout() {
        let decoded = decode_ht_block_ack_registers(0x000a_bc50, 0x89ab_cdef, 0x0123_4567);
        assert_eq!(decoded.control, 0x0a);
        assert_eq!(decoded.block_ack.starting_sequence, 0x0bc5);
        assert_eq!(decoded.block_ack.bitmap, 0x0123_4567_89ab_cdef);
    }

    #[test]
    fn basic_ht_assembly_matches_the_s31_hardware_oracle() {
        assert_eq!(
            basic_ht_ampdu_assembly(BasicHtAmpduAssemblyInput {
                aggregate_length: 9_358,
                first_header_length: 34,
                first_payload_word: 0xff12_3456,
                first_descriptor_flags: 0x0004_2009,
                first_descriptor_word1: 0xa5a5_0020,
                first_rate: 33,
                tail_buffer_flags: 0xa186_8612,
                tail_timestamp: 0x1234_5678,
            }),
            Ok(BasicHtAmpduAssemblyOutput {
                first_remaining_length: 9_324,
                first_payload_word: 0xfe12_3456,
                first_descriptor_flags: 0x004c_2009,
                first_descriptor_word1: 0x20,
                tail_buffer_flags: 0xe186_8612,
                first_timestamp: 0x1234_5678,
            })
        );
    }

    #[test]
    fn partial_block_ack_mutations_match_the_s31_retry_oracle() {
        let base = BasicHtAmpduCompletionInput {
            descriptor_flags: 0x0004_2009,
            descriptor_queue_word: 0x00a0_0304,
            frame_control: 0x4188,
            acknowledged: true,
        };
        assert_eq!(
            basic_ht_ampdu_completion(base),
            BasicHtAmpduCompletionOutput {
                descriptor_flags: 0x0044_2009,
                descriptor_queue_word: 0x01a0_0304,
                frame_control: 0x4188,
            }
        );
        assert_eq!(
            basic_ht_ampdu_completion(BasicHtAmpduCompletionInput {
                acknowledged: false,
                ..base
            }),
            BasicHtAmpduCompletionOutput {
                descriptor_flags: 0x0004_2009,
                descriptor_queue_word: 0x00a0_0304,
                frame_control: 0x4988,
            }
        );
    }

    #[test]
    fn basic_ht_assembly_rejects_he_bar_ampdu_and_bad_lengths_before_mutation() {
        let input = BasicHtAmpduAssemblyInput {
            aggregate_length: 1_500,
            first_header_length: 34,
            first_payload_word: 0,
            first_descriptor_flags: 0x0004_2009,
            first_descriptor_word1: 0x20,
            first_rate: 33,
            tail_buffer_flags: 0xa186_8612,
            tail_timestamp: 0,
        };
        assert_eq!(
            basic_ht_ampdu_assembly(BasicHtAmpduAssemblyInput {
                first_descriptor_flags: input.first_descriptor_flags | TX_DESCRIPTOR_HE_BIT,
                ..input
            }),
            Err(BasicHtAmpduAssemblyError::UnsupportedDescriptor(
                0x8004_2009
            ))
        );
        assert_eq!(
            basic_ht_ampdu_assembly(BasicHtAmpduAssemblyInput {
                first_rate: 15,
                ..input
            }),
            Err(BasicHtAmpduAssemblyError::UnsupportedRate(15))
        );
        assert_eq!(
            basic_ht_ampdu_assembly(BasicHtAmpduAssemblyInput {
                aggregate_length: 33,
                ..input
            }),
            Err(BasicHtAmpduAssemblyError::AggregateShorterThanHeader)
        );
        assert_eq!(
            basic_ht_ampdu_assembly(BasicHtAmpduAssemblyInput {
                tail_buffer_flags: input.tail_buffer_flags | TX_BUFFER_END_BIT,
                ..input
            }),
            Err(BasicHtAmpduAssemblyError::TailAlreadyTerminated(
                0xe186_8612
            ))
        );
    }

    #[test]
    fn prepared_chain_exposes_only_its_validated_frame_prefix() {
        let mut frames = [core::ptr::null_mut(); TX_AMPDU_SLOT_CAPACITY];
        frames[0] = 0x1000_usize as *mut u8;
        frames[1] = 0x2000_usize as *mut u8;
        let mut sequences = [0_u16; TX_AMPDU_SLOT_CAPACITY];
        sequences[0] = 0x014;
        sequences[1] = 0x015;
        let chain = BasicHtAmpduChain {
            first: frames[0],
            last: frames[1],
            aggregate_length: 3_000,
            subframes: 2,
            frames,
            sequences,
            original_first_remaining_length: 1_466,
            original_first_payload_word: 0x0100_0612,
            original_first_descriptor_flags: 0x0004_2009,
            original_first_descriptor_word1: 0xa5a5_0020,
            original_first_timestamp: 0x1234_5678,
            original_first_frame_count: 1,
            original_first_spatial_count: 1,
            original_first_coding_count: 1,
            original_tail_buffer_flags: {
                let mut flags = [0_u32; TX_AMPDU_SLOT_CAPACITY];
                flags[1] = 0xa186_8612;
                flags
            },
        };
        assert_eq!(chain.frame(0), Some(0x1000_usize as *mut u8));
        assert_eq!(chain.frame(1), Some(0x2000_usize as *mut u8));
        assert_eq!(chain.frame(2), None);
        assert_eq!(chain.frame(u8::MAX), None);
        assert_eq!(chain.sequence(0), Some(0x014));
        assert_eq!(chain.sequence(1), Some(0x015));
        assert_eq!(chain.sequence(2), None);
    }

    #[test]
    fn protection_spacing_matches_every_recovered_density_branch() {
        let expected = [20, 20, 20, 20, 20, 40, 76, 148];
        for (density, expected) in expected.into_iter().enumerate() {
            assert_eq!(
                basic_ht_ampdu_protection_spacing((density as u8) << 2),
                expected
            );
        }
        // Maximum A-MPDU length exponent and reserved high bits do not alter
        // the minimum-spacing field.
        assert_eq!(basic_ht_ampdu_protection_spacing(0xf7), 40);
    }

    #[test]
    fn request_encoding_is_exact_and_bounded() {
        let mut session = TxBlockAckSession::new(CONFIG).unwrap();
        let request = session.begin(0x1abc, 50).unwrap();
        assert_eq!(request.starting_sequence, 0x0abc);
        assert_eq!(request.alarm.deadline_us, 100_050);
        assert_eq!(request.body, [3, 0, 1, 0x1f, 0x08, 0, 0, 0xc0, 0xab]);
    }

    #[test]
    fn matching_response_commits_only_the_static_window() {
        let mut session = TxBlockAckSession::new(CONFIG).unwrap();
        let request = session.begin(0x123, 0).unwrap();
        let response = [3, 1, request.dialog_token, 0, 0, 0x1f, 0x08, 0, 0];
        assert_eq!(
            session.on_response(&response),
            Ok(TxBlockAckResponse::Operational(OperationalTxBlockAck {
                tid: 7,
                window: 32,
                timeout_tu: 0,
                starting_sequence: 0x123,
                amsdu: true,
            }))
        );
        assert_eq!(
            session.operational(),
            Some(OperationalTxBlockAck {
                tid: 7,
                window: 32,
                timeout_tu: 0,
                starting_sequence: 0x123,
                amsdu: true,
            })
        );
        assert!(!session.on_alarm(request.alarm));
    }

    #[test]
    fn stale_alarm_cannot_cancel_a_new_generation() {
        let mut session = TxBlockAckSession::new(CONFIG).unwrap();
        let stale = session.begin(1, 0).unwrap().alarm;
        let current = session.begin(2, 10).unwrap().alarm;
        assert!(!session.on_alarm(stale));
        assert!(session.on_alarm(current));
        assert_eq!(session.operational(), None);
    }

    #[test]
    fn response_cannot_expand_the_static_capacity() {
        let mut session = TxBlockAckSession::new(CONFIG).unwrap();
        let request = session.begin(0, 0).unwrap();
        let parameters = encode_ba_parameters(7, 64, false).to_le_bytes();
        let response = [
            3,
            1,
            request.dialog_token,
            0,
            0,
            parameters[0],
            parameters[1],
            0,
            0,
        ];
        assert_eq!(
            session.on_response(&response),
            Err(TxBlockAckError::WindowExceedsCapacity(64))
        );
    }

    #[test]
    fn rejected_response_returns_to_idle_without_a_timer_retry() {
        let mut session = TxBlockAckSession::new(CONFIG).unwrap();
        let request = session.begin(0, 0).unwrap();
        let response = [3, 1, request.dialog_token, 37, 0, 0, 0, 0, 0];
        assert_eq!(
            session.on_response(&response),
            Ok(TxBlockAckResponse::Rejected(37))
        );
        assert_eq!(session.operational(), None);
        assert!(!session.on_alarm(request.alarm));
    }

    #[test]
    fn block_ack_bitmap_handles_sequence_wrap() {
        let ack = TxBlockAckBitmap::new(0x0ffe, 0b1101);
        assert!(ack.acknowledges(0x0ffe));
        assert!(!ack.acknowledges(0x0fff));
        assert!(ack.acknowledges(0));
        assert!(ack.acknowledges(1));
        assert!(!ack.acknowledges(2));
    }

    #[test]
    fn batch_returns_one_block_ack_result_per_step() {
        let mut batch = TxAmpduBatch::new();
        batch.begin(0x0ffe, 4).unwrap();
        for slot in 3..7 {
            batch.push(slot).unwrap();
        }
        batch
            .complete_with_block_ack(TxBlockAckBitmap::new(0x0ffe, 0b1101))
            .unwrap();

        for (slot, sequence, disposition) in [
            (3, 0x0ffe, TxAmpduDisposition::Acknowledged),
            (4, 0x0fff, TxAmpduDisposition::Retry),
            (5, 0, TxAmpduDisposition::Acknowledged),
            (6, 1, TxAmpduDisposition::Acknowledged),
        ] {
            assert_eq!(
                batch.next_completion(),
                Some(TxAmpduCompletion {
                    mpdu: TxAmpduMpdu {
                        slot: TxAmpduSlot::new(slot).unwrap(),
                        sequence,
                    },
                    disposition,
                })
            );
        }
        assert!(batch.is_idle());
        assert_eq!(batch.next_completion(), None);
    }

    #[test]
    fn missing_block_ack_retries_every_mpdu_without_a_drain() {
        let mut batch = TxAmpduBatch::new();
        batch.begin(9, 2).unwrap();
        batch.push(0).unwrap();
        batch.push(31).unwrap();
        batch.complete_without_block_ack().unwrap();
        assert_eq!(
            batch.next_completion().unwrap().disposition,
            TxAmpduDisposition::Retry
        );
        assert!(!batch.is_idle());
        assert_eq!(
            batch.next_completion().unwrap().disposition,
            TxAmpduDisposition::Retry
        );
        assert!(batch.is_idle());
    }

    #[test]
    fn batch_rejects_duplicate_static_slot_ownership() {
        let mut batch = TxAmpduBatch::new();
        batch.begin(0, 32).unwrap();
        batch.push(17).unwrap();
        assert_eq!(batch.push(17), Err(TxAmpduBatchError::DuplicateSlot(17)));
    }

    #[test]
    fn batch_preserves_nonconsecutive_hardware_sequences() {
        let mut batch = TxAmpduBatch::new();
        batch.begin(0x120, 4).unwrap();
        assert_eq!(batch.push_sequence(3, 0x120).unwrap().sequence, 0x120);
        assert_eq!(batch.push_sequence(4, 0x123).unwrap().sequence, 0x123);
        assert_eq!(
            batch.push_sequence(5, 0x1123),
            Err(TxAmpduBatchError::DuplicateSequence(0x123))
        );
        batch
            .complete_with_block_ack(TxBlockAckBitmap::new(0x120, 0b1001))
            .unwrap();
        assert_eq!(
            batch.next_completion().unwrap().disposition,
            TxAmpduDisposition::Acknowledged
        );
        assert_eq!(
            batch.next_completion().unwrap().disposition,
            TxAmpduDisposition::Acknowledged
        );
        assert!(batch.is_idle());
    }

    #[test]
    fn batch_never_exceeds_negotiated_or_static_window() {
        let mut batch = TxAmpduBatch::new();
        assert_eq!(batch.begin(0, 0), Err(TxAmpduBatchError::InvalidWindow(0)));
        assert_eq!(
            batch.begin(0, 33),
            Err(TxAmpduBatchError::InvalidWindow(33))
        );
        batch.begin(0, 1).unwrap();
        batch.push(0).unwrap();
        assert_eq!(batch.push(1), Err(TxAmpduBatchError::Full));
    }
}
