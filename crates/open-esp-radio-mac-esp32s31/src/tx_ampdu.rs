//! Allocation-free TX BlockAck negotiation state in the live MAC crate.
//!
//! The stock `ieee80211_ampdu_request` allocates one vendor agreement object
//! per TID and arms an OS timer. This module owns the protocol state and its
//! deadline instead. A caller sends the returned action body through the
//! fixed management-frame pool and programs `TxBlockAckAlarm::deadline_us`
//! into a Rust async timer.

use core::{cell::UnsafeCell, marker::PhantomPinned, mem::MaybeUninit, pin::Pin, ptr};

use open_esp_radio_pac_esp32s31::{
    MacHeTxProgram, MacHtAmpduCompletionRegisters, MacHtTxProgram, RadioRegisters,
};

use crate::{
    descriptor::{descriptor_address_valid, dma_range_valid, tx_owned_word, Descriptor, BIT_30},
    tx::{
        decode_tx_completion, HeAmpduTxConfig, HeSmpduTxConfig, HtAmpduTxConfig,
        HtProtectionSpacing, LegacyTxQueue, TxCompletion, TxCookie, TxHardware, TxSlotState,
    },
};

pub const BLOCK_ACK_CATEGORY: u8 = 3;
pub const ADDBA_REQUEST_ACTION: u8 = 0;
pub const ADDBA_RESPONSE_ACTION: u8 = 1;
pub const DELBA_ACTION: u8 = 2;
pub const ADDBA_ACTION_BODY_LEN: usize = 9;
pub const TX_BLOCK_ACK_MAX_WINDOW: u16 = 32;
pub const TX_AMPDU_SLOT_CAPACITY: usize = TX_BLOCK_ACK_MAX_WINDOW as usize;
pub const TX_AMPDU_METADATA_SIZE: usize = 8;
const TX_FCS_SIZE: u16 = 4;
const HE_SMPDU_VENDOR_DMA_CAPACITY: u32 = 64;
const TX_AMPDU_DEFAULT_MAX_BYTES: u16 = 0x1fff;
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

/// Hardware authority needed specifically by an aggregate completion.
///
/// A normal [`TxHardware`] completion may acknowledge the edge immediately.
/// A-MPDU must first sample the queue's three BlockAck words, so the ordering
/// is a separate trait operation and cannot accidentally be replaced with
/// the single-MPDU completion method.
pub trait HtAmpduHardware: TxHardware {
    fn take_ht_ampdu_completion(&mut self, queue: u8) -> Option<MacHtAmpduCompletionRegisters>;
}

impl HtAmpduHardware for RadioRegisters {
    fn take_ht_ampdu_completion(&mut self, queue: u8) -> Option<MacHtAmpduCompletionRegisters> {
        self.take_mac_ht_ampdu_completion(queue)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HtAmpduTxError {
    Busy,
    Stale,
    InvalidGeometry,
    FrameTooLong,
    TooFewFrames,
    AggregateFull,
    Length(HtAmpduLengthError),
    RegisterImageMismatch,
    QueueActive,
    DetachFailed,
    TimeoutNotPending,
    ResetRequired,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HtAmpduTxCompletion {
    pub tx: TxCompletion,
    pub block_ack: HtBlockAckRegisters,
}

impl HtAmpduTxCompletion {
    /// Return whether a completed A-MPDU positively acknowledges one MPDU.
    ///
    /// A nonzero TX status means no valid BlockAck was received. The hardware
    /// result registers are not cleared as a separate transaction and may
    /// still contain the preceding successful bitmap, so those bits must not
    /// suppress an individual retry.
    ///
    /// SOURCE[HIL_OPEN_HT_AMPDU_PARTIAL_2026_07_29]: live HT40 MCS7 SGI
    /// four-stream TX load produced successful partial BlockAck completions
    /// and status-five completions with stale nonzero bitmap words.
    pub const fn acknowledges(self, sequence: u16) -> bool {
        self.tx.status == 0 && self.block_ack.block_ack.acknowledges(sequence)
    }
}

#[repr(C, align(16))]
struct HtAmpduDmaBuffer<const BUFFER_SIZE: usize>(UnsafeCell<[u8; BUFFER_SIZE]>);

impl<const BUFFER_SIZE: usize> HtAmpduDmaBuffer<BUFFER_SIZE> {
    const fn new() -> Self {
        Self(UnsafeCell::new([0; BUFFER_SIZE]))
    }
}

// The storage owner serializes CPU access with the hardware-owned phase.
// Moving the enclosing pool after pinning is prevented by `PhantomPinned`.
unsafe impl<const BUFFER_SIZE: usize> Send for HtAmpduDmaBuffer<BUFFER_SIZE> {}

/// Statically owned direct-DMA pool for one basic-HT A-MPDU.
///
/// This is deliberately a sibling of the single-frame [`crate::tx::TxSlot`].
/// It owns a distinct descriptor and buffer for every MPDU, links the exact
/// 12-byte Wi-Fi DMA descriptors, and retains the entire pool until the
/// BlockAck and queue-detach edges complete. It does not use the vendor PP
/// scheduler, allocate, or expose raw pointers to the application.
///
/// SOURCE[HIL_OPEN_HT_AMPDU_DIRECT_2026_07_29]: ESP32-S31 rev0,
/// psram-code-psram-data, open PHY/MAC, HT40 MCS7 SGI. Four observed
/// two-MPDU submissions from this pool each returned a BlockAck bitmap ending
/// in `0x000f`, with no aggregate hardware timeout.
pub struct HtAmpduTxStorage<const SLOTS: usize, const BUFFER_SIZE: usize> {
    descriptors: [Descriptor; SLOTS],
    buffers: [HtAmpduDmaBuffer<BUFFER_SIZE>; SLOTS],
    frame_lengths: [u16; SLOTS],
    hardware_mic_lengths: [u8; SLOTS],
    psdu_lengths: [u16; SLOTS],
    empty_delimiters: [u8; SLOTS],
    descriptor_capacities: [u16; SLOTS],
    state: TxSlotState,
    generation_cursor: u32,
    active: TxCookie,
    queue: LegacyTxQueue,
    count: u8,
    prepared_length: u16,
    aggregate_length: u16,
    max_aggregate_bytes: u16,
    detached: bool,
    _pin: PhantomPinned,
}

impl<const SLOTS: usize, const BUFFER_SIZE: usize> HtAmpduTxStorage<SLOTS, BUFFER_SIZE> {
    pub const fn new() -> Self {
        Self {
            descriptors: [const { Descriptor::new() }; SLOTS],
            buffers: [const { HtAmpduDmaBuffer::new() }; SLOTS],
            frame_lengths: [0; SLOTS],
            hardware_mic_lengths: [0; SLOTS],
            psdu_lengths: [0; SLOTS],
            empty_delimiters: [0; SLOTS],
            descriptor_capacities: [0; SLOTS],
            state: TxSlotState::Free,
            generation_cursor: 0,
            active: TxCookie(0),
            queue: LegacyTxQueue::BestEffort,
            count: 0,
            prepared_length: 0,
            aggregate_length: 0,
            // Conservative HT A-MPDU exponent zero until the peer capability
            // is installed by the association owner.
            max_aggregate_bytes: TX_AMPDU_DEFAULT_MAX_BYTES,
            detached: false,
            _pin: PhantomPinned,
        }
    }

    /// Initialize a large DMA pool directly in its final static allocation.
    ///
    /// Passing [`Self::new`] to `StaticCell::init` materializes the whole pool
    /// on the embedded stack before moving it. A 32 x 1,700-byte pool therefore
    /// consumed more than 55 KiB of stack in HIL. This method never creates a
    /// complete by-value temporary.
    pub fn init_in_place(storage: &mut MaybeUninit<Self>) -> &mut Self {
        let storage = storage.as_mut_ptr();

        // SAFETY: `storage` is exclusively borrowed, correctly aligned
        // uninitialized memory. Descriptor words, DMA byte buffers, length
        // arrays, counters, cookies and `false` all accept zero. The two enum
        // fields and the pin marker are written with valid Rust values before
        // a reference to the complete object is formed.
        unsafe {
            storage
                .cast::<u8>()
                .write_bytes(0, core::mem::size_of::<Self>());
            ptr::addr_of_mut!((*storage).state).write(TxSlotState::Free);
            ptr::addr_of_mut!((*storage).queue).write(LegacyTxQueue::BestEffort);
            ptr::addr_of_mut!((*storage).max_aggregate_bytes).write(TX_AMPDU_DEFAULT_MAX_BYTES);
            ptr::addr_of_mut!((*storage)._pin).write(PhantomPinned);
            &mut *storage
        }
    }

    /// Consume a unique static owner and permanently pin its DMA addresses.
    pub fn pin_static(storage: &'static mut Self) -> Pin<&'static mut Self> {
        // SAFETY: the unique `&'static mut` is consumed by the returned
        // `'static` pin. `PhantomPinned` prevents safe extraction or movement
        // for the remainder of the program.
        unsafe { Pin::new_unchecked(storage) }
    }

    pub const fn state(&self) -> TxSlotState {
        self.state
    }

    pub const fn frame_count(&self) -> u8 {
        self.count
    }

    pub const fn aggregate_length(&self) -> u16 {
        self.aggregate_length
    }

    /// Installs the peer-advertised maximum A-MPDU byte length.
    ///
    /// This must be done while software owns an idle pool. The four HT values
    /// are 0x1fff, 0x3fff, 0x7fff and 0xffff for capability exponents 0..=3.
    pub fn configure_max_aggregate_bytes(
        self: Pin<&mut Self>,
        max_aggregate_bytes: u16,
    ) -> Result<(), HtAmpduTxError> {
        // SAFETY: this changes only scalar policy state and does not move any
        // pinned descriptor or DMA buffer.
        let storage = unsafe { self.get_unchecked_mut() };
        if storage.state != TxSlotState::Free {
            return Err(HtAmpduTxError::Busy);
        }
        if max_aggregate_bytes == 0 {
            return Err(HtAmpduTxError::Length(HtAmpduLengthError::InvalidLimits));
        }
        storage.max_aggregate_bytes = max_aggregate_bytes;
        Ok(())
    }

    /// Calculate the exact A-MPDU byte count for the committed software-owned
    /// prefix, before any DMA address is published.
    pub fn prepared_aggregate(&self, cookie: TxCookie) -> Result<HtAmpduLength, HtAmpduTxError> {
        if self.state != TxSlotState::Reserved || self.active != cookie {
            return Err(HtAmpduTxError::Stale);
        }
        // The byte-accounting contract is also used by the HE owner, whose
        // rate-dependent duration limit can deliberately retain a
        // one-subframe A-MPDU. The ordinary HT submit method still enforces
        // its separate minimum-two batching policy.
        if self.count == 0 {
            return Err(HtAmpduTxError::TooFewFrames);
        }
        self.calculate_aggregate()
    }

    /// Borrow one detached, completed encoded MPDU for an individual retry.
    ///
    /// The returned slice excludes the private eight-byte DMA metadata prefix
    /// and the hardware-generated MIC/FCS trailer. Sequence Control and CCMP
    /// header are retained exactly as originally submitted.
    pub fn completed_frame(
        &self,
        cookie: TxCookie,
        index: u8,
    ) -> Result<(&[u8], u8), HtAmpduTxError> {
        if self.state != TxSlotState::Completed || self.active != cookie || !self.detached {
            return Err(HtAmpduTxError::Stale);
        }
        let index = usize::from(index);
        if index >= usize::from(self.count) {
            return Err(HtAmpduTxError::InvalidGeometry);
        }
        let frame_length = usize::from(self.frame_lengths[index]);
        // SAFETY: the completed state has returned DMA ownership to this
        // unique pool owner. The slice is immutable and remains bounded to the
        // initialized encoded frame, excluding hardware trailer bytes.
        let buffer = unsafe { &*self.buffers[index].0.get() };
        Ok((
            &buffer[TX_AMPDU_METADATA_SIZE..TX_AMPDU_METADATA_SIZE + frame_length],
            self.hardware_mic_lengths[index],
        ))
    }

    /// Retain only selected completed MPDUs for another A-MPDU attempt.
    ///
    /// Bit `n` of `retry_mask` selects completed slot `n`. Selected frames are
    /// compacted toward slot zero, keep their Sequence Control and CCMP PN,
    /// and gain only the IEEE 802.11 Retry bit. This is the owned-array form
    /// of the finite mutation observed in
    /// `_oracles/libpp.a[pp.o]::ppResortTxAMPDU`: its partial-BlockAck path
    /// detached the old links, preserved the encoded missing MPDU, set
    /// Frame Control.Retry, and placed it at the head of a new aggregate.
    ///
    /// The queue must already be detached, so hardware cannot observe the
    /// compaction. A mask with fewer than two frames is rejected because that
    /// case belongs to the separate single-MPDU retry owner.
    pub fn retain_for_ampdu_retry(
        self: Pin<&mut Self>,
        cookie: TxCookie,
        retry_mask: u32,
    ) -> Result<HtAmpduLength, HtAmpduTxError> {
        // SAFETY: the detached completion state proves exclusive CPU
        // ownership. The pinned allocations themselves are not moved; bytes
        // are copied between their fixed buffers and descriptors are rebuilt
        // by the next `submit`.
        let storage = unsafe { self.get_unchecked_mut() };
        if storage.state != TxSlotState::Completed || storage.active != cookie || !storage.detached
        {
            return Err(HtAmpduTxError::Stale);
        }
        let old_count = usize::from(storage.count);
        let valid_mask = if old_count == 32 {
            u32::MAX
        } else {
            (1_u32 << old_count) - 1
        };
        if retry_mask & !valid_mask != 0 || retry_mask.count_ones() < 2 {
            return Err(HtAmpduTxError::InvalidGeometry);
        }
        for source in 0..old_count {
            if retry_mask & (1_u32 << source) != 0 && storage.frame_lengths[source] < 2 {
                return Err(HtAmpduTxError::InvalidGeometry);
            }
        }

        let mut destination = 0_usize;
        for source in 0..old_count {
            if retry_mask & (1_u32 << source) == 0 {
                continue;
            }
            if destination != source {
                let bytes = usize::from(storage.descriptor_capacities[source]);
                // SAFETY: every committed descriptor capacity was validated
                // against BUFFER_SIZE. Source and destination are distinct
                // fixed array elements and therefore do not overlap.
                unsafe {
                    ptr::copy_nonoverlapping(
                        storage.buffers[source].0.get().cast::<u8>(),
                        storage.buffers[destination].0.get().cast::<u8>(),
                        bytes,
                    );
                }
                storage.frame_lengths[destination] = storage.frame_lengths[source];
                storage.hardware_mic_lengths[destination] = storage.hardware_mic_lengths[source];
                storage.psdu_lengths[destination] = storage.psdu_lengths[source];
                storage.empty_delimiters[destination] = storage.empty_delimiters[source];
                storage.descriptor_capacities[destination] = storage.descriptor_capacities[source];
            }
            // Frame Control byte one starts after the private metadata prefix;
            // bit three is IEEE 802.11 Retry.
            let buffer = unsafe { &mut *storage.buffers[destination].0.get() };
            buffer[TX_AMPDU_METADATA_SIZE + 1] |= 0x08;
            destination += 1;
        }
        storage.count = u8::try_from(destination).map_err(|_| HtAmpduTxError::InvalidGeometry)?;
        storage.prepared_length = 0;
        storage.aggregate_length = 0;
        storage.detached = false;
        storage.state = TxSlotState::Reserved;
        storage.recalculate_prepared_length()
    }

    /// Begin constructing one aggregate in the software-owned pool.
    pub fn begin(self: Pin<&mut Self>) -> Result<TxCookie, HtAmpduTxError> {
        // SAFETY: scalar mutation and buffer preparation do not move the
        // pinned descriptors or buffers.
        let storage = unsafe { self.get_unchecked_mut() };
        if storage.state != TxSlotState::Free {
            return Err(HtAmpduTxError::Busy);
        }
        if SLOTS < 2
            || SLOTS > TX_AMPDU_SLOT_CAPACITY
            || BUFFER_SIZE <= TX_AMPDU_METADATA_SIZE + TX_FCS_SIZE as usize
        {
            return Err(HtAmpduTxError::InvalidGeometry);
        }
        let generation = storage
            .generation_cursor
            .checked_add(1)
            .ok_or(HtAmpduTxError::ResetRequired)?;
        storage.generation_cursor = generation;
        storage.active = TxCookie(generation);
        storage.count = 0;
        storage.prepared_length = 0;
        storage.aggregate_length = 0;
        storage.detached = false;
        storage.state = TxSlotState::Reserved;
        Ok(storage.active)
    }

    /// Return the payload area for the next MPDU while software owns it.
    ///
    /// The eight-byte S31 TX metadata prefix remains private and is published
    /// by [`commit_frame`](Self::commit_frame).
    pub fn next_frame_buffer(
        self: Pin<&mut Self>,
        cookie: TxCookie,
    ) -> Result<&mut [u8], HtAmpduTxError> {
        // SAFETY: the state check below proves hardware cannot access this
        // buffer, and the unique pinned borrow prevents a second CPU owner.
        let storage = unsafe { self.get_unchecked_mut() };
        if storage.state != TxSlotState::Reserved || storage.active != cookie {
            return Err(HtAmpduTxError::Stale);
        }
        let index = usize::from(storage.count);
        if index >= SLOTS {
            return Err(HtAmpduTxError::Busy);
        }
        let buffer = unsafe { &mut *storage.buffers[index].0.get() };
        Ok(&mut buffer[TX_AMPDU_METADATA_SIZE..])
    }

    /// Check whether one more encoded MPDU fits both its static slot and the
    /// peer-advertised aggregate-byte ceiling.
    ///
    /// This does not reserve a slot or mutate the batch. A scheduler can
    /// therefore stop at the byte limit without consuming a CCMP PN or
    /// Sequence Control value for a frame that belongs in the next PPDU.
    pub fn can_commit_frame(
        &self,
        cookie: TxCookie,
        frame_length: usize,
        hardware_mic_length: u8,
        _empty_delimiters: u8,
    ) -> Result<bool, HtAmpduTxError> {
        if self.state != TxSlotState::Reserved || self.active != cookie {
            return Err(HtAmpduTxError::Stale);
        }
        if usize::from(self.count) >= SLOTS {
            return Ok(false);
        }
        let Some(psdu_length) = frame_length
            .checked_add(usize::from(hardware_mic_length))
            .and_then(|length| length.checked_add(usize::from(TX_FCS_SIZE)))
            .and_then(|length| u16::try_from(length).ok())
            .filter(|length| *length != 0 && *length <= 0x3fff)
        else {
            return Err(HtAmpduTxError::FrameTooLong);
        };
        let Some(descriptor_capacity) = TX_AMPDU_METADATA_SIZE
            .checked_add(usize::from(psdu_length))
            .and_then(|length| length.checked_add(3))
            .map(|length| length & !3)
        else {
            return Err(HtAmpduTxError::FrameTooLong);
        };
        if descriptor_capacity > BUFFER_SIZE {
            return Err(HtAmpduTxError::FrameTooLong);
        }

        match self.length_after_append(psdu_length) {
            Ok(_) => Ok(true),
            Err(HtAmpduTxError::Length(HtAmpduLengthError::AggregateTooLong(_))) => Ok(false),
            Err(error) => Err(error),
        }
    }

    /// Commit the next encoded MPDU and its hardware-generated trailer.
    ///
    /// `frame_length` is the encoded 802.11 MPDU before the hardware MIC and
    /// FCS. The resulting metadata length is the complete PSDU length used by
    /// the recovered `ppCalTxAMPDULength` accounting.
    pub fn commit_frame(
        self: Pin<&mut Self>,
        cookie: TxCookie,
        frame_length: usize,
        hardware_mic_length: u8,
        empty_delimiters: u8,
    ) -> Result<(), HtAmpduTxError> {
        // SAFETY: no address-bearing field is moved.
        let storage = unsafe { self.get_unchecked_mut() };
        if storage.state != TxSlotState::Reserved || storage.active != cookie {
            return Err(HtAmpduTxError::Stale);
        }
        if !storage.can_commit_frame(cookie, frame_length, hardware_mic_length, empty_delimiters)? {
            return Err(HtAmpduTxError::AggregateFull);
        }
        let index = usize::from(storage.count);
        if index >= SLOTS {
            return Err(HtAmpduTxError::Busy);
        }
        let psdu_length = frame_length
            .checked_add(usize::from(hardware_mic_length))
            .and_then(|length| length.checked_add(usize::from(TX_FCS_SIZE)))
            .and_then(|length| u16::try_from(length).ok())
            .filter(|length| *length != 0 && *length <= 0x3fff)
            .ok_or(HtAmpduTxError::FrameTooLong)?;
        let transfer_length = TX_AMPDU_METADATA_SIZE
            .checked_add(usize::from(psdu_length))
            .ok_or(HtAmpduTxError::FrameTooLong)?;
        let descriptor_capacity = transfer_length
            .checked_add(3)
            .map(|length| length & !3)
            .filter(|length| *length <= BUFFER_SIZE)
            .and_then(|length| u16::try_from(length).ok())
            .ok_or(HtAmpduTxError::FrameTooLong)?;

        let buffer = unsafe { &mut *storage.buffers[index].0.get() };
        buffer[..4].copy_from_slice(&u32::from(psdu_length).to_le_bytes());
        buffer[4] = empty_delimiters;
        buffer[5..TX_AMPDU_METADATA_SIZE].fill(0);
        let trailer_start = TX_AMPDU_METADATA_SIZE + frame_length;
        buffer[trailer_start..transfer_length].fill(0);
        buffer[transfer_length..usize::from(descriptor_capacity)].fill(0);
        storage.frame_lengths[index] =
            u16::try_from(frame_length).map_err(|_| HtAmpduTxError::FrameTooLong)?;
        storage.hardware_mic_lengths[index] = hardware_mic_length;
        storage.psdu_lengths[index] = psdu_length;
        storage.empty_delimiters[index] = empty_delimiters;
        storage.descriptor_capacities[index] = descriptor_capacity;
        storage.prepared_length = storage.length_after_append(psdu_length)?;
        storage.count += 1;
        Ok(())
    }

    /// Discard a software-owned partial batch.
    pub fn cancel(self: Pin<&mut Self>, cookie: TxCookie) -> Result<(), HtAmpduTxError> {
        // SAFETY: the Reserved state has not published any descriptor.
        let storage = unsafe { self.get_unchecked_mut() };
        if storage.state != TxSlotState::Reserved || storage.active != cookie {
            return Err(HtAmpduTxError::Stale);
        }
        storage.release();
        Ok(())
    }

    /// Publish and start one A-MPDU while retaining every backing allocation.
    pub fn submit<H: HtAmpduHardware>(
        self: Pin<&mut Self>,
        hardware: &mut H,
        cookie: TxCookie,
        queue: LegacyTxQueue,
        config: HtAmpduTxConfig,
    ) -> Result<(), HtAmpduTxError> {
        // SAFETY: the pinned pool retains stable descriptor/buffer addresses
        // through completion and only scalar ownership fields are mutated.
        let storage = unsafe { self.get_unchecked_mut() };
        if storage.state != TxSlotState::Reserved || storage.active != cookie {
            return Err(HtAmpduTxError::Stale);
        }
        if storage.count < 2 {
            return Err(HtAmpduTxError::TooFewFrames);
        }

        let aggregate = storage.calculate_aggregate()?;
        let count = usize::from(storage.count);
        if config.aggregate_length != aggregate.bytes || config.subframes != aggregate.subframes {
            return Err(HtAmpduTxError::RegisterImageMismatch);
        }

        let first_descriptor = core::ptr::addr_of!(storage.descriptors[0]).addr() as u32;
        for index in 0..count {
            let descriptor_address = core::ptr::addr_of!(storage.descriptors[index]).addr() as u32;
            let buffer_address = storage.buffers[index].0.get().addr() as u32;
            let capacity = u32::from(storage.descriptor_capacities[index]);
            let transfer_length =
                (TX_AMPDU_METADATA_SIZE as u32) + u32::from(storage.psdu_lengths[index]);
            if !descriptor_address_valid(descriptor_address)
                || !dma_range_valid(buffer_address, capacity)
            {
                return Err(HtAmpduTxError::InvalidGeometry);
            }
            let next_address = if index + 1 < count {
                core::ptr::addr_of!(storage.descriptors[index + 1]).addr() as u32
            } else {
                0
            };
            let mut word0 =
                tx_owned_word(capacity, transfer_length).ok_or(HtAmpduTxError::InvalidGeometry)?;
            if index + 1 < count {
                word0 &= !BIT_30;
            }
            storage.descriptors[index].publish(word0, buffer_address, next_address);
        }

        let image = crate::tx::ht_ampdu_q0_image(first_descriptor, config)
            .ok_or(HtAmpduTxError::InvalidGeometry)?;
        let program = MacHtTxProgram {
            plcp0: image.plcp0,
            plcp1: image.plcp1,
            ht_signal: image.ht_signal,
            data_length: image.data_length,
            power: image.power,
            length_control: image.length_control,
            descriptor_count_a: image.descriptor_count_a,
            descriptor_count_b: image.descriptor_count_b,
            protection_spacing: image.protection_spacing,
            timeout: config.timeout,
            scheduler_priority: config.scheduler_priority,
            packet_priority: config.pti,
            priority_count: config.pti_count,
            aifsn: config.aifsn,
            contention_window: config.contention_window,
            interface: config.interface,
        };
        let queue_index = queue.index();
        if !hardware.prepare_ht_tx(queue_index, program) {
            return Err(HtAmpduTxError::QueueActive);
        }
        storage.queue = queue;
        storage.aggregate_length = aggregate.bytes;
        storage.detached = false;
        storage.state = TxSlotState::HardwareOwned;
        hardware.start_ht_tx(queue_index, image.plcp0);
        Ok(())
    }

    /// Publish and start one HE20 SU A-MPDU with the same pinned ownership
    /// and completion/BlockAck ordering as [`Self::submit`].
    pub fn submit_he<H: HtAmpduHardware>(
        self: Pin<&mut Self>,
        hardware: &mut H,
        cookie: TxCookie,
        queue: LegacyTxQueue,
        config: HeAmpduTxConfig,
    ) -> Result<(), HtAmpduTxError> {
        // SAFETY: the pinned pool retains stable descriptor/buffer addresses
        // through completion and only scalar ownership fields are mutated.
        let storage = unsafe { self.get_unchecked_mut() };
        if storage.state != TxSlotState::Reserved || storage.active != cookie {
            return Err(HtAmpduTxError::Stale);
        }
        // Unlike the ordinary HT batching policy, HE may intentionally use a
        // one-subframe A-MPDU. Complete `ppCalTxHEAMPDULength` and
        // `ppCheckTxHEAMPDUlength` retain this representation when the
        // rate-dependent APEP/duration limit admits only one MPDU. This is
        // required at the lowest DCM rates, where forcing two full-size MPDUs
        // can exceed the HE PPDU-duration limit.
        if storage.count == 0 {
            return Err(HtAmpduTxError::TooFewFrames);
        }

        let aggregate = storage.calculate_aggregate()?;
        let count = usize::from(storage.count);
        if config.aggregate_length != aggregate.bytes || config.subframes != aggregate.subframes {
            return Err(HtAmpduTxError::RegisterImageMismatch);
        }

        let first_descriptor = core::ptr::addr_of!(storage.descriptors[0]).addr() as u32;
        for index in 0..count {
            let descriptor_address = core::ptr::addr_of!(storage.descriptors[index]).addr() as u32;
            let buffer_address = storage.buffers[index].0.get().addr() as u32;
            let capacity = u32::from(storage.descriptor_capacities[index]);
            let transfer_length =
                (TX_AMPDU_METADATA_SIZE as u32) + u32::from(storage.psdu_lengths[index]);
            if !descriptor_address_valid(descriptor_address)
                || !dma_range_valid(buffer_address, capacity)
            {
                return Err(HtAmpduTxError::InvalidGeometry);
            }
            let next_address = if index + 1 < count {
                core::ptr::addr_of!(storage.descriptors[index + 1]).addr() as u32
            } else {
                0
            };
            let mut word0 =
                tx_owned_word(capacity, transfer_length).ok_or(HtAmpduTxError::InvalidGeometry)?;
            if index + 1 < count {
                word0 &= !BIT_30;
            }
            storage.descriptors[index].publish(word0, buffer_address, next_address);
        }

        let image = crate::tx::he_ampdu_q0_image(first_descriptor, config)
            .ok_or(HtAmpduTxError::InvalidGeometry)?;
        let program = MacHeTxProgram {
            plcp0: image.plcp0,
            plcp1: image.plcp1,
            he_signal_a1: image.he_signal_a1,
            he_signal_a2_length: image.he_signal_a2_length,
            power: image.power,
            length_control: image.length_control,
            descriptor_count_a: image.descriptor_count_a,
            descriptor_count_b: image.descriptor_count_b,
            protection_spacing: image.protection_spacing,
            timeout: config.timeout,
            scheduler_priority: config.scheduler_priority,
            packet_priority: config.pti,
            priority_count: config.pti_count,
            aifsn: config.aifsn,
            contention_window: config.contention_window,
            interface: config.interface,
        };
        let queue_index = queue.index();
        if !hardware.prepare_he_tx(queue_index, program) {
            return Err(HtAmpduTxError::QueueActive);
        }
        storage.queue = queue;
        storage.aggregate_length = aggregate.bytes;
        storage.detached = false;
        storage.state = TxSlotState::HardwareOwned;
        hardware.start_he_tx(queue_index, image.plcp0);
        Ok(())
    }

    /// Publish and start one HE20 SU S-MPDU.
    ///
    /// This deliberately has a separate entry point from [`Self::submit_he`].
    /// The PP blob gives HE single MPDU its own DMA-metadata and HE-SIG-A2
    /// layout, even though both paths reuse the same pinned descriptor and
    /// eight-byte private metadata storage.
    pub fn submit_he_smpdu<H: HtAmpduHardware>(
        self: Pin<&mut Self>,
        hardware: &mut H,
        cookie: TxCookie,
        queue: LegacyTxQueue,
        config: HeSmpduTxConfig,
    ) -> Result<(), HtAmpduTxError> {
        // SAFETY: the pinned pool retains stable descriptor/buffer addresses
        // through completion and only scalar ownership fields are mutated.
        let storage = unsafe { self.get_unchecked_mut() };
        if storage.state != TxSlotState::Reserved || storage.active != cookie {
            return Err(HtAmpduTxError::Stale);
        }
        if storage.count != 1
            || storage.frame_lengths[0] != config.mpdu_length
            || storage.hardware_mic_lengths[0] != 0
        {
            return Err(HtAmpduTxError::RegisterImageMismatch);
        }

        let descriptor_address = core::ptr::addr_of!(storage.descriptors[0]).addr() as u32;
        let buffer_address = storage.buffers[0].0.get().addr() as u32;
        // HIL_VENDOR_HE20_MCS0_DCM_RAW_2026_07_29 captured the complete
        // vendor DMA word c0090040: capacity 64 and used length 36. Preserve
        // that bounded single-MPDU allocation geometry even though the
        // statically owned Rust buffer is larger.
        let capacity =
            u32::from(storage.descriptor_capacities[0]).max(HE_SMPDU_VENDOR_DMA_CAPACITY);
        let transfer_length = (TX_AMPDU_METADATA_SIZE as u32) + u32::from(storage.psdu_lengths[0]);
        if !descriptor_address_valid(descriptor_address)
            || !dma_range_valid(buffer_address, capacity)
        {
            return Err(HtAmpduTxError::InvalidGeometry);
        }

        // `commit_frame` already wrote MPDU+FCS length and reserved the four
        // trailing hardware-FCS bytes. The live vendor S-MPDU buffer started
        // with 0x0100_001c for a 24-byte frame: retain length 28, set metadata
        // bit 24, keep empty delimiters and byte-seven's optional term zero.
        let buffer = unsafe { &mut *storage.buffers[0].0.get() };
        let metadata_length = u32::from(storage.psdu_lengths[0]);
        buffer[..4].copy_from_slice(&(metadata_length | 0x0100_0000).to_le_bytes());
        buffer[4] = 0;
        buffer[7] = 0;

        let word0 =
            tx_owned_word(capacity, transfer_length).ok_or(HtAmpduTxError::InvalidGeometry)?;
        storage.descriptors[0].publish(word0, buffer_address, 0);

        let image = crate::tx::he_smpdu_q0_image(descriptor_address, config)
            .ok_or(HtAmpduTxError::InvalidGeometry)?;
        let program = MacHeTxProgram {
            plcp0: image.plcp0,
            plcp1: image.plcp1,
            he_signal_a1: image.he_signal_a1,
            he_signal_a2_length: image.he_signal_a2_length,
            power: image.power,
            length_control: image.length_control,
            descriptor_count_a: image.descriptor_count_a,
            descriptor_count_b: image.descriptor_count_b,
            protection_spacing: image.protection_spacing,
            timeout: config.timeout,
            scheduler_priority: config.scheduler_priority,
            packet_priority: config.pti,
            priority_count: config.pti_count,
            aifsn: config.aifsn,
            contention_window: config.contention_window,
            interface: config.interface,
        };
        let queue_index = queue.index();
        if !hardware.prepare_he_tx(queue_index, program) {
            return Err(HtAmpduTxError::QueueActive);
        }
        storage.queue = queue;
        storage.aggregate_length = config.apep_length();
        storage.detached = false;
        storage.state = TxSlotState::HardwareOwned;
        hardware.start_he_tx(queue_index, image.plcp0);
        Ok(())
    }

    /// Sample BlockAck and transfer a completed aggregate back to software.
    pub fn acknowledge_completion<H: HtAmpduHardware>(
        self: Pin<&mut Self>,
        hardware: &mut H,
    ) -> Result<Option<HtAmpduTxCompletion>, HtAmpduTxError> {
        // SAFETY: no pinned allocation is moved.
        let storage = unsafe { self.get_unchecked_mut() };
        let Some(registers) = hardware.take_ht_ampdu_completion(storage.queue.index()) else {
            return Ok(None);
        };
        if storage.state != TxSlotState::HardwareOwned {
            storage.state = TxSlotState::ResetRequired;
            return Err(HtAmpduTxError::Stale);
        }
        storage.state = TxSlotState::Completed;
        Ok(Some(HtAmpduTxCompletion {
            tx: decode_tx_completion(storage.active, registers.tx),
            block_ack: decode_ht_block_ack_registers(
                registers.block_ack_control_and_sequence,
                registers.block_ack_bitmap_low,
                registers.block_ack_bitmap_high,
            ),
        }))
    }

    /// Transfer one HE S-MPDU ACK completion back to software.
    ///
    /// Unlike A-MPDU, this path intentionally does not sample BlockAck words.
    /// Its success criterion is the ordinary per-queue ACK status, matching
    /// the vendor raw-TX completion callback.
    pub fn acknowledge_he_smpdu_completion<H: TxHardware>(
        self: Pin<&mut Self>,
        hardware: &mut H,
    ) -> Result<Option<TxCompletion>, HtAmpduTxError> {
        // SAFETY: no pinned allocation is moved.
        let storage = unsafe { self.get_unchecked_mut() };
        let Some(registers) = hardware.take_tx_completion(storage.queue.index()) else {
            return Ok(None);
        };
        if storage.state != TxSlotState::HardwareOwned || storage.count != 1 {
            storage.state = TxSlotState::ResetRequired;
            return Err(HtAmpduTxError::Stale);
        }
        storage.state = TxSlotState::Completed;
        Ok(Some(decode_tx_completion(storage.active, registers)))
    }

    pub fn begin_timeout_abort<H: HtAmpduHardware>(
        self: Pin<&mut Self>,
        hardware: &mut H,
        cookie: TxCookie,
    ) -> Result<bool, HtAmpduTxError> {
        // SAFETY: scalar state only.
        let storage = unsafe { self.get_unchecked_mut() };
        if storage.state != TxSlotState::HardwareOwned || storage.active != cookie {
            return Err(HtAmpduTxError::Stale);
        }
        Ok(hardware.begin_tx_timeout_abort(storage.queue.index()))
    }

    pub fn finish_timeout_abort<H: HtAmpduHardware>(
        self: Pin<&mut Self>,
        hardware: &mut H,
        cookie: TxCookie,
    ) -> Result<(), HtAmpduTxError> {
        // SAFETY: scalar state only; hardware has stopped walking the pool.
        let storage = unsafe { self.get_unchecked_mut() };
        if storage.state != TxSlotState::HardwareOwned || storage.active != cookie {
            return Err(HtAmpduTxError::Stale);
        }
        let Some(detached) = hardware.finish_tx_timeout_abort(storage.queue.index()) else {
            return Err(HtAmpduTxError::TimeoutNotPending);
        };
        if !detached {
            storage.state = TxSlotState::ResetRequired;
            return Err(HtAmpduTxError::DetachFailed);
        }
        storage.release();
        Ok(())
    }

    pub fn detach_completed<H: HtAmpduHardware>(
        self: Pin<&mut Self>,
        hardware: &mut H,
        cookie: TxCookie,
    ) -> Result<(), HtAmpduTxError> {
        // SAFETY: the completion edge returned the pool to software; the
        // detach readback proves the queue no longer references its head.
        let storage = unsafe { self.get_unchecked_mut() };
        if storage.state != TxSlotState::Completed || storage.active != cookie {
            return Err(HtAmpduTxError::Stale);
        }
        if !hardware.detach_completed_tx(storage.queue.index()) {
            storage.state = TxSlotState::ResetRequired;
            return Err(HtAmpduTxError::DetachFailed);
        }
        // Keep the completed MPDUs and their lengths alive. BlockAck handling
        // may now copy any missing MPDU into the single-frame TX owner without
        // reconstructing Sequence Control or the CCMP PN. `release_completed`
        // is the explicit final ownership edge.
        storage.detached = true;
        Ok(())
    }

    /// Release a detached completed batch after BlockAck processing and any
    /// individual retries have copied the retained MPDUs.
    pub fn release_completed(self: Pin<&mut Self>, cookie: TxCookie) -> Result<(), HtAmpduTxError> {
        // SAFETY: the queue has already been detached and this only clears
        // scalar ownership metadata. Pinned descriptor and buffer addresses
        // remain in place for the next reservation.
        let storage = unsafe { self.get_unchecked_mut() };
        if storage.state != TxSlotState::Completed || storage.active != cookie || !storage.detached
        {
            return Err(HtAmpduTxError::Stale);
        }
        storage.release();
        Ok(())
    }

    fn release(&mut self) {
        self.active = TxCookie(0);
        self.count = 0;
        self.prepared_length = 0;
        self.aggregate_length = 0;
        self.detached = false;
        self.state = TxSlotState::Free;
    }

    /// Return the final A-MPDU length after appending one PSDU.
    ///
    /// The previous final MPDU gains its four-byte alignment and requested
    /// empty delimiters only when another MPDU follows it. The new final MPDU
    /// contributes one delimiter and its exact PSDU length. Keeping this
    /// prefix total makes each append O(1); the former validation replayed all
    /// preceding lengths on every `commit_frame`, making a 32-MPDU build
    /// O(n²).
    ///
    /// SOURCE: complete `_oracles/libpp.a[pp.o]::{ppCalSubFrameLength,
    /// ppCalTxAMPDULength}` and the equivalent finite rules in
    /// [`HtAmpduLengthAccumulator`].
    fn length_after_append(&self, psdu_length: u16) -> Result<u16, HtAmpduTxError> {
        if psdu_length == 0 {
            return Err(HtAmpduTxError::Length(HtAmpduLengthError::ZeroMpduLength));
        }
        let mut next = u32::from(self.prepared_length);
        if self.count != 0 {
            let previous = usize::from(self.count - 1);
            let previous_length = u32::from(self.psdu_lengths[previous]);
            next = next
                .checked_add((4 - (previous_length & 3)) & 3)
                .and_then(|length| {
                    length.checked_add(u32::from(self.empty_delimiters[previous]) * 4)
                })
                .ok_or(HtAmpduTxError::Length(
                    HtAmpduLengthError::AggregateTooLong(u32::MAX),
                ))?;
        }
        next = next
            .checked_add(4 + u32::from(psdu_length))
            .ok_or(HtAmpduTxError::Length(
                HtAmpduLengthError::AggregateTooLong(u32::MAX),
            ))?;
        if next > u32::from(self.max_aggregate_bytes) {
            return Err(HtAmpduTxError::Length(
                HtAmpduLengthError::AggregateTooLong(next),
            ));
        }
        u16::try_from(next)
            .map_err(|_| HtAmpduTxError::Length(HtAmpduLengthError::AggregateTooLong(next)))
    }

    fn recalculate_prepared_length(&mut self) -> Result<HtAmpduLength, HtAmpduTxError> {
        let mut length = HtAmpduLengthAccumulator::new(self.count, self.max_aggregate_bytes)
            .map_err(HtAmpduTxError::Length)?;
        for index in 0..usize::from(self.count) {
            length
                .push(
                    u32::from(self.psdu_lengths[index]),
                    self.empty_delimiters[index],
                )
                .map_err(HtAmpduTxError::Length)?;
        }
        let aggregate = length.finish().map_err(HtAmpduTxError::Length)?;
        self.prepared_length = aggregate.bytes;
        Ok(aggregate)
    }

    fn calculate_aggregate(&self) -> Result<HtAmpduLength, HtAmpduTxError> {
        if self.count == 0 {
            return Err(HtAmpduTxError::Length(HtAmpduLengthError::Empty));
        }
        if self.prepared_length == 0 {
            return Err(HtAmpduTxError::Length(HtAmpduLengthError::ZeroMpduLength));
        }
        Ok(HtAmpduLength {
            bytes: self.prepared_length,
            subframes: self.count,
        })
    }
}

impl<const SLOTS: usize, const BUFFER_SIZE: usize> Default
    for HtAmpduTxStorage<SLOTS, BUFFER_SIZE>
{
    fn default() -> Self {
        Self::new()
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
    HtProtectionSpacing::from_ampdu_parameters(parameters).hardware_value()
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
        // The nine-byte ADDBA response is a fixed prefix, not the complete
        // action-body length. An HE peer may append an ADDBA Extension IE
        // (element 159). Linux `net/mac80211/agg-rx.c`::
        // `ieee80211_send_addba_resp` does exactly that after the fixed
        // response fields. The controlled AX211 HE20 HIL reached
        // `parse_block_ack_action(AddbaResponse)` and then failed only this
        // former exact-length check. We deliberately consume only the fixed
        // prefix here: the negotiated low ten-bit window remains bounded by
        // `self.config.window` and `TX_BLOCK_ACK_MAX_WINDOW`; a future owner
        // of extended (>1024) windows must parse the IE separately.
        if body.len() < ADDBA_ACTION_BODY_LEN
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
    fn owned_dma_pool_builds_two_mpdu_length_without_publishing_hardware() {
        let mut storage = HtAmpduTxStorage::<4, 256>::new();
        // SAFETY: the local is not moved until the pin is dropped at the end
        // of this test, and no hardware ownership is published.
        let mut storage = unsafe { Pin::new_unchecked(&mut storage) };
        let cookie = storage.as_mut().begin().unwrap();

        storage.as_mut().next_frame_buffer(cookie).unwrap()[..100].fill(0xa5);
        storage.as_mut().commit_frame(cookie, 100, 8, 0).unwrap();
        storage.as_mut().next_frame_buffer(cookie).unwrap()[..101].fill(0x5a);
        storage.as_mut().commit_frame(cookie, 101, 8, 0).unwrap();

        assert_eq!(
            storage.prepared_aggregate(cookie).unwrap(),
            HtAmpduLength {
                // First PSDU: delimiter 4 + length 112. Final PSDU:
                // delimiter 4 + length 113, with its three padding bytes
                // removed by the recovered tail rule.
                bytes: 233,
                subframes: 2,
            }
        );
        assert_eq!(storage.frame_count(), 2);
        assert_eq!(storage.state(), TxSlotState::Reserved);
        storage.as_mut().cancel(cookie).unwrap();
        assert_eq!(storage.state(), TxSlotState::Free);
    }

    #[test]
    fn owned_dma_pool_preserves_one_subframe_he_ampdu_length() {
        let mut storage = HtAmpduTxStorage::<2, 256>::new();
        // SAFETY: no address is published and the local is not moved while
        // this pin exists.
        let mut storage = unsafe { Pin::new_unchecked(&mut storage) };
        let cookie = storage.as_mut().begin().unwrap();
        storage.as_mut().next_frame_buffer(cookie).unwrap()[..100].fill(0xa5);
        storage.as_mut().commit_frame(cookie, 100, 8, 0).unwrap();

        assert_eq!(
            storage.prepared_aggregate(cookie).unwrap(),
            HtAmpduLength {
                // One delimiter plus MPDU, hardware MIC and FCS. The 112-byte
                // PSDU is already aligned, so the tail rule removes nothing.
                bytes: 116,
                subframes: 1,
            }
        );
    }

    #[test]
    fn incremental_pool_length_matches_blob_accumulator_for_full_window() {
        let mut storage = HtAmpduTxStorage::<32, 256>::new();
        // SAFETY: no address is published and the local is not moved while
        // this pin exists.
        let mut storage = unsafe { Pin::new_unchecked(&mut storage) };
        storage
            .as_mut()
            .configure_max_aggregate_bytes(u16::MAX)
            .unwrap();
        let cookie = storage.as_mut().begin().unwrap();
        let mut oracle = HtAmpduLengthAccumulator::new(32, u16::MAX).unwrap();

        for index in 0..32_u8 {
            let frame_length = 100 + usize::from(index % 7);
            let empty_delimiters = index % 3;
            storage.as_mut().next_frame_buffer(cookie).unwrap()[..frame_length].fill(index);
            storage
                .as_mut()
                .commit_frame(cookie, frame_length, 8, empty_delimiters)
                .unwrap();
            oracle
                .push(
                    (frame_length + 8 + usize::from(TX_FCS_SIZE)) as u32,
                    empty_delimiters,
                )
                .unwrap();
            if index != 0 {
                assert_eq!(
                    storage.prepared_aggregate(cookie).unwrap(),
                    oracle.finish().unwrap()
                );
            }
        }
    }

    #[test]
    fn completed_pool_retains_mpdu_until_explicit_release() {
        let mut storage = HtAmpduTxStorage::<2, 256>::new();
        // SAFETY: the local is not moved while this pin exists and this test
        // does not publish an address to hardware.
        let mut storage = unsafe { Pin::new_unchecked(&mut storage) };
        let cookie = storage.as_mut().begin().unwrap();
        storage.as_mut().next_frame_buffer(cookie).unwrap()[..32].fill(0x5a);
        storage.as_mut().commit_frame(cookie, 32, 8, 0).unwrap();

        // Model the two hardware ownership edges independently: completion
        // alone must not expose a buffer that the queue still references.
        unsafe { storage.as_mut().get_unchecked_mut() }.state = TxSlotState::Completed;
        assert_eq!(
            storage.completed_frame(cookie, 0),
            Err(HtAmpduTxError::Stale)
        );
        unsafe { storage.as_mut().get_unchecked_mut() }.detached = true;
        let (frame, mic_length) = storage.completed_frame(cookie, 0).unwrap();
        assert_eq!(frame, &[0x5a; 32]);
        assert_eq!(mic_length, 8);

        storage.as_mut().release_completed(cookie).unwrap();
        assert_eq!(storage.state(), TxSlotState::Free);
        assert_eq!(
            storage.completed_frame(cookie, 0),
            Err(HtAmpduTxError::Stale)
        );
    }

    #[test]
    fn detached_pool_compacts_only_missing_frames_for_ampdu_retry() {
        let mut storage = HtAmpduTxStorage::<4, 256>::new();
        // SAFETY: no address is published and the local is not moved while
        // this pin exists.
        let mut storage = unsafe { Pin::new_unchecked(&mut storage) };
        let cookie = storage.as_mut().begin().unwrap();
        for index in 0..4_u8 {
            let frame = storage.as_mut().next_frame_buffer(cookie).unwrap();
            frame[..32].fill(index);
            frame[1] = 0x41;
            storage.as_mut().commit_frame(cookie, 32, 8, 0).unwrap();
        }
        unsafe {
            let storage = storage.as_mut().get_unchecked_mut();
            storage.state = TxSlotState::Completed;
            storage.detached = true;
        }

        let aggregate = storage
            .as_mut()
            .retain_for_ampdu_retry(cookie, 0b1010)
            .unwrap();
        assert_eq!(aggregate.subframes, 2);
        assert_eq!(storage.frame_count(), 2);
        assert_eq!(storage.state(), TxSlotState::Reserved);
        unsafe {
            let storage = storage.as_ref().get_ref();
            let first = &*storage.buffers[0].0.get();
            let second = &*storage.buffers[1].0.get();
            assert_eq!(first[TX_AMPDU_METADATA_SIZE], 1);
            assert_eq!(second[TX_AMPDU_METADATA_SIZE], 3);
            assert_eq!(first[TX_AMPDU_METADATA_SIZE + 1], 0x49);
            assert_eq!(second[TX_AMPDU_METADATA_SIZE + 1], 0x49);
        }
        storage.as_mut().cancel(cookie).unwrap();
    }

    #[test]
    fn full_window_and_byte_ceiling_are_independent() {
        let mut storage = HtAmpduTxStorage::<32, 1700>::new();
        // SAFETY: no address is published and the local is not moved while
        // the pin exists.
        let mut storage = unsafe { Pin::new_unchecked(&mut storage) };
        storage
            .as_mut()
            .configure_max_aggregate_bytes(0x7fff)
            .unwrap();
        let cookie = storage.as_mut().begin().unwrap();
        for _ in 0..20 {
            assert!(storage.can_commit_frame(cookie, 1600, 8, 0).unwrap());
            storage.as_mut().commit_frame(cookie, 1600, 8, 0).unwrap();
        }
        assert_eq!(storage.frame_count(), 20);
        assert!(!storage.can_commit_frame(cookie, 1600, 8, 0).unwrap());
        assert_eq!(
            storage.as_mut().commit_frame(cookie, 1600, 8, 0),
            Err(HtAmpduTxError::AggregateFull)
        );
        assert_eq!(storage.frame_count(), 20);
        storage.as_mut().cancel(cookie).unwrap();

        storage
            .as_mut()
            .configure_max_aggregate_bytes(0x1fff)
            .unwrap();
        let cookie = storage.as_mut().begin().unwrap();
        for _ in 0..32 {
            assert!(storage.can_commit_frame(cookie, 100, 8, 0).unwrap());
            storage.as_mut().commit_frame(cookie, 100, 8, 0).unwrap();
        }
        assert_eq!(storage.frame_count(), 32);
        assert!(!storage.can_commit_frame(cookie, 100, 8, 0).unwrap());
        storage.as_mut().cancel(cookie).unwrap();
    }

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
    fn matching_he_response_accepts_an_addba_extension_ie() {
        let mut session = TxBlockAckSession::new(CONFIG).unwrap();
        let request = session.begin(0x123, 0).unwrap();
        let response = [
            3,
            1,
            request.dialog_token,
            0,
            0,
            0x1f,
            0x08,
            0,
            0,
            159,
            1,
            0,
        ];
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
