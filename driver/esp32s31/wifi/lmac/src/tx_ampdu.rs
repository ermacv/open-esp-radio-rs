//! Allocation-free TX BlockAck negotiation state in the live MAC crate.
//!
//! The stock `ieee80211_ampdu_request` allocates one vendor agreement object
//! per TID and arms an OS timer. This module owns the protocol state and its
//! deadline instead. A caller sends the returned action body through the
//! fixed management-frame pool and programs `TxBlockAckAlarm::deadline_us`
//! into a Rust async timer.

use core::{marker::PhantomPinned, pin::Pin};

use open_esp_radio_esp32s31_registers::{
    MacHeTbLinkReservation, MacHeTbProgramError, MacHeTbTidLimit, MacHeTid,
    MacHeTriggerTxQueueSnapshot, MacHeTxProgram, MacHtAmpduCompletionRegisters, MacHtTxProgram,
    RadioRegisters,
};
use pin_project::pin_project;

#[cfg(target_arch = "riscv32")]
use crate::descriptor::{descriptor_address_valid, dma_range_valid};
use crate::{
    descriptor::{BIT_30, Descriptor, tx_owned_word},
    tx::{
        HeAmpduTxConfig, HeEdcaTxopLimit, HeRate, HeSmpduTxConfig, HtAmpduDensity, HtAmpduTxConfig,
        HtRate, LegacyTxQueue, TxCompletion, TxCookie, TxHardware, TxSlotState,
        decode_tx_completion,
    },
};

#[cfg(test)]
use crate::tx::HtProtectionSpacing;

pub const BLOCK_ACK_CATEGORY: u8 = 3;
pub const ADDBA_REQUEST_ACTION: u8 = 0;
pub const ADDBA_RESPONSE_ACTION: u8 = 1;
pub const DELBA_ACTION: u8 = 2;
pub const ADDBA_ACTION_BODY_LEN: usize = 9;
pub const TX_BLOCK_ACK_MAX_WINDOW: u16 = 32;
pub const TX_AMPDU_SLOT_CAPACITY: usize = TX_BLOCK_ACK_MAX_WINDOW as usize;
pub const TX_AMPDU_METADATA_SIZE: usize = 8;
const TX_FCS_SIZE: u16 = 4;
const HARDWARE_HE_CONTROL_LENGTH: u16 = 4;
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
        self.push_with_hardware_he_control(payload_word, empty_delimiters, false)
    }

    /// Add one MPDU whose HE-Control field may be inserted by MAC hardware.
    ///
    /// SOURCE: complete `libpp.a[pp_he.o]::
    /// ppCalSubFrameLength` reads `metadata[7] & 1`, multiplies it by four,
    /// and adds it after the delimiter and rounded metadata length. The low
    /// fourteen-bit MPDU length remains unchanged.
    pub const fn push_with_hardware_he_control(
        &mut self,
        payload_word: u32,
        empty_delimiters: u8,
        hardware_he_control: bool,
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
        let inserted_bytes = if hardware_he_control {
            HARDWARE_HE_CONTROL_LENGTH as u32
        } else {
            0
        };
        let contribution = mpdu_bytes + padding + empty_bytes + 4 + inserted_bytes;
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

    /// Prepare one queue for a future AP Trigger before publishing TX enable.
    ///
    /// Implementations must validate every fallible input before the first
    /// hardware write so an error cannot leave a partially published queue.
    fn prepare_he_trigger_based_queue(
        &mut self,
        policy: MacHeTbTidLimit,
        reservation: MacHeTbLinkReservation,
        tid: MacHeTid,
        mpdu_lengths: &[u16],
        queued_msdu_bytes: u32,
    ) -> Result<MacHeTriggerTxQueueSnapshot, MacHeTbProgramError>;

    /// Remove Trigger eligibility only after DMA ownership has returned.
    fn clear_he_trigger_based_queue(&mut self, reservation: MacHeTbLinkReservation);

    /// Read back a live Trigger queue while the aggregate owner still retains
    /// the reservation. Test doubles may return `None`.
    fn he_trigger_based_queue_snapshot(
        &self,
        _reservation: MacHeTbLinkReservation,
    ) -> Option<MacHeTriggerTxQueueSnapshot> {
        None
    }
}

impl HtAmpduHardware for RadioRegisters {
    fn take_ht_ampdu_completion(&mut self, queue: u8) -> Option<MacHtAmpduCompletionRegisters> {
        self.take_mac_ht_ampdu_completion(queue)
    }

    fn prepare_he_trigger_based_queue(
        &mut self,
        policy: MacHeTbTidLimit,
        reservation: MacHeTbLinkReservation,
        tid: MacHeTid,
        mpdu_lengths: &[u16],
        queued_msdu_bytes: u32,
    ) -> Result<MacHeTriggerTxQueueSnapshot, MacHeTbProgramError> {
        RadioRegisters::prepare_he_trigger_based_queue(
            self,
            policy,
            reservation,
            tid,
            mpdu_lengths,
            queued_msdu_bytes,
        )
    }

    fn clear_he_trigger_based_queue(&mut self, reservation: MacHeTbLinkReservation) {
        RadioRegisters::clear_he_trigger_based_queue(self, reservation);
    }

    fn he_trigger_based_queue_snapshot(
        &self,
        reservation: MacHeTbLinkReservation,
    ) -> Option<MacHeTriggerTxQueueSnapshot> {
        Some(RadioRegisters::he_trigger_based_queue_snapshot(
            self,
            reservation,
        ))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HtAmpduTxError {
    /// A storage transition requiring software-owned idle state observed a
    /// different lifecycle state.
    NotFree(TxSlotState),
    /// A commit attempted to advance beyond the statically bounded slot set.
    SlotCapacity,
    Stale,
    FrameIndexOutOfRange {
        index: u8,
        count: u8,
    },
    InvalidRetryMask {
        mask: u32,
        count: u8,
    },
    RetainedFrameTooShort {
        index: u8,
        length: u16,
    },
    SlotCountOverflow {
        count: usize,
    },
    InvalidStorageGeometry {
        slots: usize,
        buffer_size: usize,
    },
    InvalidDmaRange {
        descriptor_address: u32,
        buffer_address: u32,
        capacity: u32,
    },
    InvalidDescriptorLength {
        capacity: u32,
        transfer_length: u32,
    },
    TxImageUnavailable {
        format: HtAmpduTxFormat,
    },
    InvalidTriggerReservation {
        queue: u8,
        subframes: u8,
    },
    FrameTooLong,
    TooFewFrames,
    AggregateFull,
    Length(HtAmpduLengthError),
    RegisterImageMismatch,
    QueueActive,
    TriggerMsduLengthUnavailable,
    TriggerBased(MacHeTbProgramError),
    DetachFailed,
    TimeoutNotPending,
    ResetRequired,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HtAmpduTxFormat {
    HtAmpdu,
    HeAmpdu,
    HeSmpdu,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HtAmpduTxCompletion {
    pub tx: TxCompletion,
    pub block_ack: HtBlockAckRegisters,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PreparedHeTrigger {
    policy: MacHeTbTidLimit,
    reservation: MacHeTbLinkReservation,
    tid: MacHeTid,
    queued_msdu_bytes: u32,
}

impl HtAmpduTxCompletion {
    /// Return whether a completed A-MPDU positively acknowledges one MPDU.
    ///
    /// A nonzero TX status means no valid BlockAck was received. The hardware
    /// result registers are not cleared as a separate transaction and may
    /// still contain the preceding successful bitmap, so those bits must not
    /// suppress an individual retry.
    ///
    /// SOURCE\[HIL_OPEN_HT_AMPDU_PARTIAL_2026_07_29]: live HT40 MCS7 SGI
    /// four-stream TX load produced successful partial BlockAck completions
    /// and status-five completions with stale nonzero bitmap words.
    pub const fn acknowledges(self, sequence: u16) -> bool {
        self.tx.status == 0 && self.block_ack.block_ack.acknowledges(sequence)
    }
}

#[repr(C, align(16))]
struct HtAmpduDmaBuffer<const BUFFER_SIZE: usize>([u8; BUFFER_SIZE]);

impl<const BUFFER_SIZE: usize> HtAmpduDmaBuffer<BUFFER_SIZE> {
    const fn new() -> Self {
        Self([0; BUFFER_SIZE])
    }
}

/// Statically owned direct-DMA pool for one basic-HT A-MPDU.
///
/// This is deliberately a sibling of the single-frame [`crate::tx::TxSlot`].
/// It owns a distinct descriptor and buffer for every MPDU, links the exact
/// 12-byte Wi-Fi DMA descriptors, and retains the entire pool until the
/// BlockAck and queue-detach edges complete. It does not use the vendor PP
/// scheduler, allocate, or expose raw pointers to the application.
///
/// SOURCE\[HIL_OPEN_HT_AMPDU_DIRECT_2026_07_29]: ESP32-S31 rev0,
/// psram-code-psram-data, open PHY/MAC, HT40 MCS7 SGI. Four observed
/// two-MPDU submissions from this pool each returned a BlockAck bitmap ending
/// in `0x000f`, with no aggregate hardware timeout.
#[pin_project]
pub struct HtAmpduTxStorage<const SLOTS: usize, const BUFFER_SIZE: usize> {
    descriptors: [Descriptor; SLOTS],
    buffers: [HtAmpduDmaBuffer<BUFFER_SIZE>; SLOTS],
    /// Stable backing allocation selected for each committed MPDU.
    ///
    /// Ordinary commits point into `buffers`; cache-TX commits point into a
    /// separately pinned lease retained by the safe runtime wrapper.
    buffer_addresses: [usize; SLOTS],
    frame_lengths: [u16; SLOTS],
    hardware_mic_lengths: [u8; SLOTS],
    /// Original MSDU length corresponding to each encoded MPDU.
    ///
    /// Zero means the upper MAC did not supply this vendor-visible value and
    /// therefore the frame cannot be published as Trigger-eligible.
    msdu_lengths: [u16; SLOTS],
    psdu_lengths: [u16; SLOTS],
    /// MAC inserts one four-byte HE-Control field for this MPDU.
    ///
    /// The encoded frame and low fourteen-bit metadata length exclude those
    /// bytes; DMA metadata byte seven bit zero owns the insertion.
    hardware_he_control: [bool; SLOTS],
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
    trigger_reservation: Option<MacHeTbLinkReservation>,
    trigger_publication_snapshot: Option<MacHeTriggerTxQueueSnapshot>,
    detached: bool,
    #[pin]
    _pin: PhantomPinned,
}

impl<const SLOTS: usize, const BUFFER_SIZE: usize> HtAmpduTxStorage<SLOTS, BUFFER_SIZE> {
    pub const fn new() -> Self {
        Self {
            descriptors: [const { Descriptor::new() }; SLOTS],
            buffers: [const { HtAmpduDmaBuffer::new() }; SLOTS],
            buffer_addresses: [0; SLOTS],
            frame_lengths: [0; SLOTS],
            hardware_mic_lengths: [0; SLOTS],
            msdu_lengths: [0; SLOTS],
            psdu_lengths: [0; SLOTS],
            hardware_he_control: [false; SLOTS],
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
            trigger_reservation: None,
            trigger_publication_snapshot: None,
            detached: false,
            _pin: PhantomPinned,
        }
    }

    /// Consume a unique static owner and permanently pin its DMA addresses.
    pub fn pin_static(storage: &'static mut Self) -> Pin<&'static mut Self> {
        Pin::static_mut(storage)
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
        let storage = self.project();
        if *storage.state != TxSlotState::Free {
            return Err(HtAmpduTxError::NotFree(*storage.state));
        }
        if max_aggregate_bytes == 0 {
            return Err(HtAmpduTxError::Length(HtAmpduLengthError::InvalidLimits));
        }
        *storage.max_aggregate_bytes = max_aggregate_bytes;
        Ok(())
    }

    /// Calculate the exact A-MPDU byte count for the committed software-owned
    /// prefix, before any DMA address is published.
    pub fn prepared_aggregate(&self, cookie: TxCookie) -> Result<HtAmpduLength, HtAmpduTxError> {
        if self.state != TxSlotState::Reserved || self.active != cookie {
            return Err(HtAmpduTxError::Stale);
        }
        // Both HT and HE can retain a one-subframe A-MPDU. Complete
        // `libpp.a[pp.o]::ppAssembleAMPDU` starts from the supplied
        // chain head and iterates until its next pointer is null; it has no
        // minimum-two branch.
        if self.count == 0 {
            return Err(HtAmpduTxError::TooFewFrames);
        }
        self.calculate_aggregate()
    }

    /// Read the committed metadata-byte-four value while software still owns
    /// the aggregate.
    ///
    /// This bounded observation is intended for formatter validation and HIL
    /// reporting. It does not expose the DMA buffer or permit mutation.
    pub fn prepared_empty_delimiters(
        &self,
        cookie: TxCookie,
        index: u8,
    ) -> Result<u8, HtAmpduTxError> {
        if self.state != TxSlotState::Reserved || self.active != cookie {
            return Err(HtAmpduTxError::Stale);
        }
        let index = usize::from(index);
        if index >= usize::from(self.count) {
            return Err(HtAmpduTxError::FrameIndexOutOfRange {
                index: index as u8,
                count: self.count,
            });
        }
        Ok(self.empty_delimiters[index])
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
            return Err(HtAmpduTxError::FrameIndexOutOfRange {
                index: index as u8,
                count: self.count,
            });
        }
        let frame_length = usize::from(self.frame_lengths[index]);
        let buffer_address = self.buffer_addresses[index];
        let capacity = usize::from(self.descriptor_capacities[index]);
        // SAFETY: an internal commit uses this pool's pinned allocation. A
        // referenced commit requires its caller to retain the external pinned
        // lease through completion/retry. The completed state has returned DMA
        // ownership, and the slice is bounded to the validated descriptor
        // capacity.
        let buffer = unsafe { core::slice::from_raw_parts(buffer_address as *const u8, capacity) };
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
    /// `libpp.a[pp.o]::ppResortTxAMPDU`: its partial-BlockAck path
    /// detached the old links, preserved the encoded missing MPDU, set
    /// Frame Control.Retry, and placed it at the head of a new aggregate.
    ///
    /// The queue must already be detached, so hardware cannot observe the
    /// compaction. A one-frame mask is valid because HE keeps a failed
    /// one-member A-MPDU under this same owner; converting it to the ordinary
    /// queue first requires the distinct `ppHEAMPDU2Normal` metadata
    /// transition.
    ///
    /// The retained aggregate also keeps its original PHY-rate image.
    /// Complete `libpp.a[lmac.o]::lmacRetryTxFrame` compares state
    /// byte `+0x12` with four and skips `rcGetRate` in that state; complete
    /// `lmacProcessLongRetryFail` writes four immediately before entering the
    /// A-MPDU retry leaf. Only the compacted byte/subframe count and the next
    /// EDCA backoff are expected to change before republication.
    pub fn retain_for_ampdu_retry(
        mut self: Pin<&mut Self>,
        cookie: TxCookie,
        retry_mask: u32,
    ) -> Result<HtAmpduLength, HtAmpduTxError> {
        let storage = self.as_ref().get_ref();
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
        if retry_mask == 0 || retry_mask & !valid_mask != 0 {
            return Err(HtAmpduTxError::InvalidRetryMask {
                mask: retry_mask,
                count: storage.count,
            });
        }
        for source in 0..old_count {
            if retry_mask & (1_u32 << source) != 0 && storage.frame_lengths[source] < 2 {
                return Err(HtAmpduTxError::RetainedFrameTooShort {
                    index: source as u8,
                    length: storage.frame_lengths[source],
                });
            }
        }

        let storage = self.as_mut().project();
        let mut destination = 0_usize;
        for source in 0..old_count {
            if retry_mask & (1_u32 << source) == 0 {
                continue;
            }
            if destination != source {
                storage.buffer_addresses[destination] = storage.buffer_addresses[source];
                storage.frame_lengths[destination] = storage.frame_lengths[source];
                storage.hardware_mic_lengths[destination] = storage.hardware_mic_lengths[source];
                storage.msdu_lengths[destination] = storage.msdu_lengths[source];
                storage.psdu_lengths[destination] = storage.psdu_lengths[source];
                storage.hardware_he_control[destination] = storage.hardware_he_control[source];
                storage.empty_delimiters[destination] = storage.empty_delimiters[source];
                storage.descriptor_capacities[destination] = storage.descriptor_capacities[source];
            }
            // Frame Control byte one starts after the private metadata prefix;
            // bit three is IEEE 802.11 Retry.
            let buffer_address = storage.buffer_addresses[destination];
            let capacity = usize::from(storage.descriptor_capacities[destination]);
            // SAFETY: the detached completion state returned every retained
            // backing allocation to software. Internal buffers are pinned by
            // this owner; referenced buffers remain pinned by the wrapper
            // required by `commit_referenced_frame`.
            let buffer =
                unsafe { core::slice::from_raw_parts_mut(buffer_address as *mut u8, capacity) };
            buffer[TX_AMPDU_METADATA_SIZE + 1] |= 0x08;
            destination += 1;
        }
        *storage.count = u8::try_from(destination)
            .map_err(|_| HtAmpduTxError::SlotCountOverflow { count: destination })?;
        *storage.prepared_length = 0;
        *storage.aggregate_length = 0;
        *storage.trigger_reservation = None;
        *storage.trigger_publication_snapshot = None;
        *storage.detached = false;
        *storage.state = TxSlotState::Reserved;
        self.recalculate_prepared_length()
    }

    /// Begin constructing one aggregate in the software-owned pool.
    pub fn begin(self: Pin<&mut Self>) -> Result<TxCookie, HtAmpduTxError> {
        let storage = self.project();
        if *storage.state != TxSlotState::Free {
            return Err(HtAmpduTxError::NotFree(*storage.state));
        }
        if SLOTS < 2
            || SLOTS > TX_AMPDU_SLOT_CAPACITY
            || (BUFFER_SIZE != 0 && BUFFER_SIZE <= TX_AMPDU_METADATA_SIZE + TX_FCS_SIZE as usize)
        {
            return Err(HtAmpduTxError::InvalidStorageGeometry {
                slots: SLOTS,
                buffer_size: BUFFER_SIZE,
            });
        }
        let generation = (*storage.generation_cursor)
            .checked_add(1)
            .ok_or(HtAmpduTxError::ResetRequired)?;
        *storage.generation_cursor = generation;
        *storage.active = TxCookie(generation);
        *storage.count = 0;
        *storage.prepared_length = 0;
        *storage.aggregate_length = 0;
        *storage.trigger_reservation = None;
        *storage.trigger_publication_snapshot = None;
        *storage.detached = false;
        *storage.state = TxSlotState::Reserved;
        Ok(*storage.active)
    }

    /// Return the payload area for the next MPDU while software owns it.
    ///
    /// The eight-byte S31 TX metadata prefix remains private and is published
    /// by [`commit_frame`](Self::commit_frame).
    pub fn next_frame_buffer(
        self: Pin<&mut Self>,
        cookie: TxCookie,
    ) -> Result<&mut [u8], HtAmpduTxError> {
        let storage = self.project();
        if *storage.state != TxSlotState::Reserved || *storage.active != cookie {
            return Err(HtAmpduTxError::Stale);
        }
        let index = usize::from(*storage.count);
        if index >= SLOTS {
            return Err(HtAmpduTxError::SlotCapacity);
        }
        if BUFFER_SIZE <= TX_AMPDU_METADATA_SIZE {
            return Err(HtAmpduTxError::InvalidStorageGeometry {
                slots: SLOTS,
                buffer_size: BUFFER_SIZE,
            });
        }
        let buffer = &mut storage.buffers[index].0;
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
        self.can_commit_frame_in_capacity(
            cookie,
            frame_length,
            hardware_mic_length,
            false,
            BUFFER_SIZE,
        )
    }

    /// Check a frame against a separately owned DMA allocation.
    ///
    /// A descriptor-only storage uses `BUFFER_SIZE == 0`; its external
    /// allocation capacity is supplied by the pinned lease instead of being
    /// charged twice to the aggregate owner.
    pub fn can_commit_referenced_frame(
        &self,
        cookie: TxCookie,
        frame_length: usize,
        hardware_mic_length: u8,
        dma_capacity: usize,
    ) -> Result<bool, HtAmpduTxError> {
        self.can_commit_frame_in_capacity(
            cookie,
            frame_length,
            hardware_mic_length,
            false,
            dma_capacity,
        )
    }

    /// Check one HT MPDU against both negotiated storage and the exact
    /// rate-dependent `rx11NRate2AMPDULimit` ceiling.
    pub fn can_commit_ht_frame(
        &self,
        cookie: TxCookie,
        frame_length: usize,
        hardware_mic_length: u8,
        empty_delimiters: u8,
        rate: HtRate,
    ) -> Result<bool, HtAmpduTxError> {
        if !self.can_commit_frame(cookie, frame_length, hardware_mic_length, empty_delimiters)? {
            return Ok(false);
        }
        let psdu_length = frame_length
            .checked_add(usize::from(hardware_mic_length))
            .and_then(|length| length.checked_add(usize::from(TX_FCS_SIZE)))
            .and_then(|length| u16::try_from(length).ok())
            .filter(|length| *length != 0 && *length <= 0x3fff)
            .ok_or(HtAmpduTxError::FrameTooLong)?;
        match rate.vendor_ampdu_byte_limit() {
            Some(limit) => Ok(self.length_after_append(psdu_length, false)? <= limit),
            None => Ok(true),
        }
    }

    /// Descriptor-only counterpart of [`Self::can_commit_ht_frame`].
    pub fn can_commit_referenced_ht_frame(
        &self,
        cookie: TxCookie,
        frame_length: usize,
        hardware_mic_length: u8,
        _empty_delimiters: u8,
        rate: HtRate,
        dma_capacity: usize,
    ) -> Result<bool, HtAmpduTxError> {
        if !self.can_commit_referenced_frame(
            cookie,
            frame_length,
            hardware_mic_length,
            dma_capacity,
        )? {
            return Ok(false);
        }
        let psdu_length = frame_length
            .checked_add(usize::from(hardware_mic_length))
            .and_then(|length| length.checked_add(usize::from(TX_FCS_SIZE)))
            .and_then(|length| u16::try_from(length).ok())
            .filter(|length| *length != 0 && *length <= 0x3fff)
            .ok_or(HtAmpduTxError::FrameTooLong)?;
        match rate.vendor_ampdu_byte_limit() {
            Some(limit) => Ok(self.length_after_append(psdu_length, false)? <= limit),
            None => Ok(true),
        }
    }

    /// Check one referenced HT frame against a fresh aggregate without
    /// reserving or cancelling the descriptor pool.
    ///
    /// A scheduler can use this to route an individually valid frame to the
    /// ordinary queue when it exceeds a peer/rate aggregate ceiling. Keeping
    /// the probe value-only avoids consuming a generation or mutating the
    /// caller-supplied association byte limit before the real batch begins.
    pub fn can_fit_fresh_referenced_ht_frame(
        &self,
        frame_length: usize,
        hardware_mic_length: u8,
        rate: HtRate,
        maximum_aggregate_bytes: u16,
        dma_capacity: usize,
    ) -> Result<bool, HtAmpduTxError> {
        let psdu_length =
            Self::fresh_referenced_psdu_length(frame_length, hardware_mic_length, dma_capacity)?;
        let aggregate_length = 4_u32 + u32::from(psdu_length);
        if maximum_aggregate_bytes == 0 || aggregate_length > u32::from(maximum_aggregate_bytes) {
            return Ok(false);
        }
        Ok(rate
            .vendor_ampdu_byte_limit()
            .is_none_or(|limit| aggregate_length <= u32::from(limit)))
    }

    fn can_commit_frame_with_hardware_he_control(
        &self,
        cookie: TxCookie,
        frame_length: usize,
        hardware_mic_length: u8,
        hardware_he_control: bool,
    ) -> Result<bool, HtAmpduTxError> {
        self.can_commit_frame_in_capacity(
            cookie,
            frame_length,
            hardware_mic_length,
            hardware_he_control,
            BUFFER_SIZE,
        )
    }

    fn can_commit_frame_in_capacity(
        &self,
        cookie: TxCookie,
        frame_length: usize,
        hardware_mic_length: u8,
        hardware_he_control: bool,
        buffer_capacity: usize,
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
        if descriptor_capacity > buffer_capacity {
            return Err(HtAmpduTxError::FrameTooLong);
        }

        match self.length_after_append(psdu_length, hardware_he_control) {
            Ok(_) => Ok(true),
            Err(HtAmpduTxError::Length(HtAmpduLengthError::AggregateTooLong(_))) => Ok(false),
            Err(error) => Err(error),
        }
    }

    /// Check one HE frame using the blob-derived delimiter padding policy.
    ///
    /// Unlike [`Self::can_commit_frame`], the caller cannot accidentally
    /// publish an HE short subframe with a guessed metadata-byte-four value.
    pub fn can_commit_he_frame(
        &self,
        cookie: TxCookie,
        frame_length: usize,
        hardware_mic_length: u8,
        rate: HeRate,
        density: HtAmpduDensity,
    ) -> Result<bool, HtAmpduTxError> {
        self.can_commit_he_frame_with_txop(
            cookie,
            frame_length,
            hardware_mic_length,
            rate,
            density,
            HeEdcaTxopLimit::DEFAULT,
        )
    }

    /// Check one HE frame against both peer and rate/TXOP APEP ceilings.
    pub fn can_commit_he_frame_with_txop(
        &self,
        cookie: TxCookie,
        frame_length: usize,
        hardware_mic_length: u8,
        rate: HeRate,
        density: HtAmpduDensity,
        txop_limit: HeEdcaTxopLimit,
    ) -> Result<bool, HtAmpduTxError> {
        let psdu_length = frame_length
            .checked_add(usize::from(hardware_mic_length))
            .and_then(|length| length.checked_add(usize::from(TX_FCS_SIZE)))
            .and_then(|length| u16::try_from(length).ok())
            .filter(|length| *length != 0 && *length <= 0x3fff)
            .ok_or(HtAmpduTxError::FrameTooLong)?;
        let empty_delimiters = rate
            .ampdu_empty_delimiters(psdu_length, density)
            .ok_or(HtAmpduTxError::FrameTooLong)?;
        if !self.can_commit_frame(cookie, frame_length, hardware_mic_length, empty_delimiters)? {
            return Ok(false);
        }
        Ok(u32::from(self.length_after_append(psdu_length, false)?)
            <= rate.maximum_apep_bytes(txop_limit))
    }

    /// Check one referenced HE frame using the blob-derived delimiter policy.
    ///
    /// This is the descriptor-only/cache-TX counterpart of
    /// [`Self::can_commit_he_frame`]. The frame capacity belongs to the pinned
    /// network lease rather than this aggregate owner.
    pub fn can_commit_referenced_he_frame(
        &self,
        cookie: TxCookie,
        frame_length: usize,
        hardware_mic_length: u8,
        rate: HeRate,
        density: HtAmpduDensity,
        dma_capacity: usize,
    ) -> Result<bool, HtAmpduTxError> {
        self.can_commit_referenced_he_frame_with_txop(
            cookie,
            frame_length,
            hardware_mic_length,
            rate,
            density,
            HeEdcaTxopLimit::DEFAULT,
            dma_capacity,
        )
    }

    /// Check one referenced HE frame against allocation, APEP and TXOP limits.
    pub fn can_commit_referenced_he_frame_with_txop(
        &self,
        cookie: TxCookie,
        frame_length: usize,
        hardware_mic_length: u8,
        rate: HeRate,
        density: HtAmpduDensity,
        txop_limit: HeEdcaTxopLimit,
        dma_capacity: usize,
    ) -> Result<bool, HtAmpduTxError> {
        let psdu_length = frame_length
            .checked_add(usize::from(hardware_mic_length))
            .and_then(|length| length.checked_add(usize::from(TX_FCS_SIZE)))
            .and_then(|length| u16::try_from(length).ok())
            .filter(|length| *length != 0 && *length <= 0x3fff)
            .ok_or(HtAmpduTxError::FrameTooLong)?;
        let _empty_delimiters = rate
            .ampdu_empty_delimiters(psdu_length, density)
            .ok_or(HtAmpduTxError::FrameTooLong)?;
        if !self.can_commit_frame_in_capacity(
            cookie,
            frame_length,
            hardware_mic_length,
            false,
            dma_capacity,
        )? {
            return Ok(false);
        }
        Ok(u32::from(self.length_after_append(psdu_length, false)?)
            <= rate.maximum_apep_bytes(txop_limit))
    }

    /// Check one referenced HE frame against a fresh aggregate without
    /// changing descriptor ownership.
    pub fn can_fit_fresh_referenced_he_frame_with_txop(
        &self,
        frame_length: usize,
        hardware_mic_length: u8,
        rate: HeRate,
        density: HtAmpduDensity,
        txop_limit: HeEdcaTxopLimit,
        maximum_aggregate_bytes: u16,
        dma_capacity: usize,
    ) -> Result<bool, HtAmpduTxError> {
        let psdu_length =
            Self::fresh_referenced_psdu_length(frame_length, hardware_mic_length, dma_capacity)?;
        let _empty_delimiters = rate
            .ampdu_empty_delimiters(psdu_length, density)
            .ok_or(HtAmpduTxError::FrameTooLong)?;
        let aggregate_length = 4_u32 + u32::from(psdu_length);
        Ok(maximum_aggregate_bytes != 0
            && aggregate_length <= u32::from(maximum_aggregate_bytes)
            && aggregate_length <= rate.maximum_apep_bytes(txop_limit))
    }

    fn fresh_referenced_psdu_length(
        frame_length: usize,
        hardware_mic_length: u8,
        dma_capacity: usize,
    ) -> Result<u16, HtAmpduTxError> {
        let psdu_length = frame_length
            .checked_add(usize::from(hardware_mic_length))
            .and_then(|length| length.checked_add(usize::from(TX_FCS_SIZE)))
            .and_then(|length| u16::try_from(length).ok())
            .filter(|length| *length != 0 && *length <= 0x3fff)
            .ok_or(HtAmpduTxError::FrameTooLong)?;
        let descriptor_capacity = TX_AMPDU_METADATA_SIZE
            .checked_add(usize::from(psdu_length))
            .and_then(|length| length.checked_add(3))
            .map(|length| length & !3)
            .ok_or(HtAmpduTxError::FrameTooLong)?;
        if descriptor_capacity > dma_capacity {
            return Err(HtAmpduTxError::FrameTooLong);
        }
        Ok(psdu_length)
    }

    /// Check one HE frame whose four-byte HE-Control is inserted by hardware.
    ///
    /// The DMA-resident frame length remains unchanged. Only APEP accounting
    /// gains four bytes through metadata byte seven bit zero.
    pub fn can_commit_hardware_he_control_frame(
        &self,
        cookie: TxCookie,
        frame_length: usize,
        hardware_mic_length: u8,
        rate: HeRate,
        density: HtAmpduDensity,
    ) -> Result<bool, HtAmpduTxError> {
        self.can_commit_hardware_he_control_frame_with_txop(
            cookie,
            frame_length,
            hardware_mic_length,
            rate,
            density,
            HeEdcaTxopLimit::DEFAULT,
        )
    }

    /// Check hardware HE-Control insertion under the same complete APEP gate.
    pub fn can_commit_hardware_he_control_frame_with_txop(
        &self,
        cookie: TxCookie,
        frame_length: usize,
        hardware_mic_length: u8,
        rate: HeRate,
        density: HtAmpduDensity,
        txop_limit: HeEdcaTxopLimit,
    ) -> Result<bool, HtAmpduTxError> {
        let psdu_length = frame_length
            .checked_add(usize::from(hardware_mic_length))
            .and_then(|length| length.checked_add(usize::from(TX_FCS_SIZE)))
            .and_then(|length| u16::try_from(length).ok())
            .filter(|length| *length != 0 && *length <= 0x3fff)
            .ok_or(HtAmpduTxError::FrameTooLong)?;
        let _empty_delimiters = rate
            .ampdu_empty_delimiters(psdu_length, density)
            .ok_or(HtAmpduTxError::FrameTooLong)?;
        if !self.can_commit_frame_with_hardware_he_control(
            cookie,
            frame_length,
            hardware_mic_length,
            true,
        )? {
            return Ok(false);
        }
        Ok(u32::from(self.length_after_append(psdu_length, true)?)
            <= rate.maximum_apep_bytes(txop_limit))
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
        self.commit_frame_with_hardware_he_control(
            cookie,
            frame_length,
            hardware_mic_length,
            empty_delimiters,
            false,
        )
    }

    /// Commit an MPDU already encoded in a separately pinned allocation.
    ///
    /// `dma_storage` begins with the private eight-byte S31 metadata prefix;
    /// the encoded 802.11 frame must already occupy
    /// `dma_storage[8..8 + frame_length]`. This is the descriptor half of the
    /// vendor cache-TX/type-nine ESF path and performs no payload copy.
    ///
    /// # Safety
    ///
    /// The caller must retain exclusive ownership of the same allocation at
    /// the same address until this batch has completed, detached, processed
    /// BlockAck/retries and reached [`Self::release_completed`] or
    /// [`Self::cancel`]. It must not mutate the allocation while hardware owns
    /// the batch. A safe runtime wrapper is expected to enforce this by owning
    /// the pinned frame lease beside this storage.
    ///
    /// SOURCE: complete `libnet80211.a[ieee80211_output.o]::
    /// ieee80211_alloc_tx_buf` cache-TX/type-nine branch retains the netstack
    /// buffer through `s_netstack_ref`; complete `libpp.a[pp.o]::
    /// ppAssembleAMPDU` links the existing ESF descriptors without copying
    /// their payloads.
    pub unsafe fn commit_referenced_frame(
        self: Pin<&mut Self>,
        cookie: TxCookie,
        dma_storage: &mut [u8],
        frame_length: usize,
        hardware_mic_length: u8,
        empty_delimiters: u8,
    ) -> Result<(), HtAmpduTxError> {
        let storage = self.as_ref().get_ref();
        if storage.state != TxSlotState::Reserved || storage.active != cookie {
            return Err(HtAmpduTxError::Stale);
        }
        if !storage.can_commit_referenced_frame(
            cookie,
            frame_length,
            hardware_mic_length,
            dma_storage.len(),
        )? {
            return Err(HtAmpduTxError::AggregateFull);
        }
        let index = usize::from(storage.count);
        if index >= SLOTS {
            return Err(HtAmpduTxError::SlotCapacity);
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
            .filter(|length| *length <= dma_storage.len())
            .and_then(|length| u16::try_from(length).ok())
            .ok_or(HtAmpduTxError::FrameTooLong)?;
        let prepared_length = storage.length_after_append(psdu_length, false)?;

        let storage = self.project();
        storage.buffer_addresses[index] = dma_storage.as_mut_ptr().addr();
        dma_storage[..4].copy_from_slice(&u32::from(psdu_length).to_le_bytes());
        dma_storage[4] = empty_delimiters;
        dma_storage[5..TX_AMPDU_METADATA_SIZE].fill(0);
        let trailer_start = TX_AMPDU_METADATA_SIZE + frame_length;
        dma_storage[trailer_start..transfer_length].fill(0);
        dma_storage[transfer_length..usize::from(descriptor_capacity)].fill(0);
        storage.frame_lengths[index] =
            u16::try_from(frame_length).map_err(|_| HtAmpduTxError::FrameTooLong)?;
        storage.hardware_mic_lengths[index] = hardware_mic_length;
        storage.msdu_lengths[index] = 0;
        storage.psdu_lengths[index] = psdu_length;
        storage.hardware_he_control[index] = false;
        storage.empty_delimiters[index] = empty_delimiters;
        storage.descriptor_capacities[index] = descriptor_capacity;
        *storage.prepared_length = prepared_length;
        *storage.count += 1;
        Ok(())
    }

    /// Commit one referenced HT MPDU under the exact rate-dependent ceiling.
    ///
    /// # Safety
    ///
    /// The caller must uphold the same pinned allocation invariant as
    /// [`Self::commit_referenced_frame`].
    pub unsafe fn commit_referenced_ht_frame(
        mut self: Pin<&mut Self>,
        cookie: TxCookie,
        dma_storage: &mut [u8],
        frame_length: usize,
        hardware_mic_length: u8,
        empty_delimiters: u8,
        rate: HtRate,
    ) -> Result<(), HtAmpduTxError> {
        if !self.as_ref().can_commit_referenced_ht_frame(
            cookie,
            frame_length,
            hardware_mic_length,
            empty_delimiters,
            rate,
            dma_storage.len(),
        )? {
            return Err(HtAmpduTxError::AggregateFull);
        }
        // SAFETY: the caller supplied the external allocation invariant, and
        // the complete HT rate ceiling was checked above.
        unsafe {
            self.as_mut().commit_referenced_frame(
                cookie,
                dma_storage,
                frame_length,
                hardware_mic_length,
                empty_delimiters,
            )
        }
    }

    fn commit_frame_with_hardware_he_control(
        self: Pin<&mut Self>,
        cookie: TxCookie,
        frame_length: usize,
        hardware_mic_length: u8,
        empty_delimiters: u8,
        hardware_he_control: bool,
    ) -> Result<(), HtAmpduTxError> {
        let storage = self.as_ref().get_ref();
        if storage.state != TxSlotState::Reserved || storage.active != cookie {
            return Err(HtAmpduTxError::Stale);
        }
        if !storage.can_commit_frame_with_hardware_he_control(
            cookie,
            frame_length,
            hardware_mic_length,
            hardware_he_control,
        )? {
            return Err(HtAmpduTxError::AggregateFull);
        }
        let index = usize::from(storage.count);
        if index >= SLOTS {
            return Err(HtAmpduTxError::SlotCapacity);
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
        let prepared_length = storage.length_after_append(psdu_length, hardware_he_control)?;

        let storage = self.project();
        let buffer = &mut storage.buffers[index].0;
        storage.buffer_addresses[index] = buffer.as_mut_ptr().addr();
        buffer[..4].copy_from_slice(&u32::from(psdu_length).to_le_bytes());
        buffer[4] = empty_delimiters;
        buffer[5..TX_AMPDU_METADATA_SIZE].fill(0);
        if hardware_he_control {
            // SOURCE: complete `libpp.a[pp_he.o]::
            // ppCalSubFrameLength` uses byte seven bit zero as the four-byte
            // on-air insertion term. HIL_VENDOR_HE_CONTROL_INSERTION_2026_07_30
            // captured vendor metadata word one as 0x0100_0000 while CCMP
            // remained immediately after the QoS header in DMA.
            buffer[7] = 1;
        }
        let trailer_start = TX_AMPDU_METADATA_SIZE + frame_length;
        buffer[trailer_start..transfer_length].fill(0);
        buffer[transfer_length..usize::from(descriptor_capacity)].fill(0);
        storage.frame_lengths[index] =
            u16::try_from(frame_length).map_err(|_| HtAmpduTxError::FrameTooLong)?;
        storage.hardware_mic_lengths[index] = hardware_mic_length;
        // The generic MPDU API does not know the pre-encapsulation MSDU
        // length. Clear a possibly retained value from the previous
        // generation so Trigger publication fails closed.
        storage.msdu_lengths[index] = 0;
        storage.psdu_lengths[index] = psdu_length;
        storage.hardware_he_control[index] = hardware_he_control;
        storage.empty_delimiters[index] = empty_delimiters;
        storage.descriptor_capacities[index] = descriptor_capacity;
        *storage.prepared_length = prepared_length;
        *storage.count += 1;
        Ok(())
    }

    /// Commit one HE MPDU with exact PP-blob delimiter padding.
    pub fn commit_he_frame(
        self: Pin<&mut Self>,
        cookie: TxCookie,
        frame_length: usize,
        hardware_mic_length: u8,
        rate: HeRate,
        density: HtAmpduDensity,
    ) -> Result<(), HtAmpduTxError> {
        self.commit_he_frame_with_txop(
            cookie,
            frame_length,
            hardware_mic_length,
            rate,
            density,
            HeEdcaTxopLimit::DEFAULT,
        )
    }

    /// Commit one HE MPDU under the complete rate/TXOP duration policy.
    pub fn commit_he_frame_with_txop(
        self: Pin<&mut Self>,
        cookie: TxCookie,
        frame_length: usize,
        hardware_mic_length: u8,
        rate: HeRate,
        density: HtAmpduDensity,
        txop_limit: HeEdcaTxopLimit,
    ) -> Result<(), HtAmpduTxError> {
        if !self.as_ref().can_commit_he_frame_with_txop(
            cookie,
            frame_length,
            hardware_mic_length,
            rate,
            density,
            txop_limit,
        )? {
            return Err(HtAmpduTxError::AggregateFull);
        }
        let psdu_length = frame_length
            .checked_add(usize::from(hardware_mic_length))
            .and_then(|length| length.checked_add(usize::from(TX_FCS_SIZE)))
            .and_then(|length| u16::try_from(length).ok())
            .filter(|length| *length != 0 && *length <= 0x3fff)
            .ok_or(HtAmpduTxError::FrameTooLong)?;
        let empty_delimiters = rate
            .ampdu_empty_delimiters(psdu_length, density)
            .ok_or(HtAmpduTxError::FrameTooLong)?;
        self.commit_frame(cookie, frame_length, hardware_mic_length, empty_delimiters)
    }

    /// Commit one HE MPDU already encoded in a separately pinned allocation.
    ///
    /// This preserves the exact HE delimiter and TXOP gates of
    /// [`Self::commit_he_frame_with_txop`] while using the vendor cache-TX
    /// ownership model represented by [`Self::commit_referenced_frame`].
    ///
    /// # Safety
    ///
    /// The caller must uphold the allocation lifetime and exclusivity
    /// invariant documented by [`Self::commit_referenced_frame`].
    pub unsafe fn commit_referenced_he_frame(
        self: Pin<&mut Self>,
        cookie: TxCookie,
        dma_storage: &mut [u8],
        frame_length: usize,
        hardware_mic_length: u8,
        rate: HeRate,
        density: HtAmpduDensity,
    ) -> Result<(), HtAmpduTxError> {
        // SAFETY: forwarded unchanged to the TXOP-aware implementation.
        unsafe {
            self.commit_referenced_he_frame_with_txop(
                cookie,
                dma_storage,
                frame_length,
                hardware_mic_length,
                rate,
                density,
                HeEdcaTxopLimit::DEFAULT,
            )
        }
    }

    /// Commit one referenced HE MPDU under the complete rate/TXOP policy.
    ///
    /// SOURCE: complete `libpp.a[pp_he.o]::{ppCalDeliNum,
    /// ppCalTxHEAMPDULength,ppCheckTxHEAMPDUlength}` supplies delimiter and
    /// duration accounting. Complete
    /// `libnet80211.a[ieee80211_output.o]::ieee80211_alloc_tx_buf`
    /// and `libpp.a[pp.o]::ppAssembleAMPDU` retain and link the
    /// cache-TX allocation instead of copying it into aggregate storage.
    ///
    /// # Safety
    ///
    /// The caller must retain exclusive ownership of `dma_storage` at its
    /// current address until the batch is detached and released or cancelled.
    pub unsafe fn commit_referenced_he_frame_with_txop(
        mut self: Pin<&mut Self>,
        cookie: TxCookie,
        dma_storage: &mut [u8],
        frame_length: usize,
        hardware_mic_length: u8,
        rate: HeRate,
        density: HtAmpduDensity,
        txop_limit: HeEdcaTxopLimit,
    ) -> Result<(), HtAmpduTxError> {
        if !self.as_ref().can_commit_referenced_he_frame_with_txop(
            cookie,
            frame_length,
            hardware_mic_length,
            rate,
            density,
            txop_limit,
            dma_storage.len(),
        )? {
            return Err(HtAmpduTxError::AggregateFull);
        }
        let psdu_length = frame_length
            .checked_add(usize::from(hardware_mic_length))
            .and_then(|length| length.checked_add(usize::from(TX_FCS_SIZE)))
            .and_then(|length| u16::try_from(length).ok())
            .filter(|length| *length != 0 && *length <= 0x3fff)
            .ok_or(HtAmpduTxError::FrameTooLong)?;
        let empty_delimiters = rate
            .ampdu_empty_delimiters(psdu_length, density)
            .ok_or(HtAmpduTxError::FrameTooLong)?;
        // SAFETY: the caller supplied the external allocation invariant, and
        // the complete HE policy was checked before forwarding the commit.
        unsafe {
            self.as_mut().commit_referenced_frame(
                cookie,
                dma_storage,
                frame_length,
                hardware_mic_length,
                empty_delimiters,
            )
        }
    }

    /// Commit one HE MPDU with a hardware-inserted HE-Control field.
    ///
    /// Unlike the former placeholder workaround, this does not move CCMP or
    /// increase the low fourteen-bit DMA MPDU length. It publishes the exact
    /// vendor metadata byte-seven bit and adds four only to assembled APEP
    /// accounting.
    pub fn commit_hardware_he_control_frame(
        self: Pin<&mut Self>,
        cookie: TxCookie,
        frame_length: usize,
        hardware_mic_length: u8,
        rate: HeRate,
        density: HtAmpduDensity,
    ) -> Result<(), HtAmpduTxError> {
        self.commit_hardware_he_control_frame_with_txop(
            cookie,
            frame_length,
            hardware_mic_length,
            rate,
            density,
            HeEdcaTxopLimit::DEFAULT,
        )
    }

    /// Commit hardware HE-Control insertion under the complete duration gate.
    pub fn commit_hardware_he_control_frame_with_txop(
        self: Pin<&mut Self>,
        cookie: TxCookie,
        frame_length: usize,
        hardware_mic_length: u8,
        rate: HeRate,
        density: HtAmpduDensity,
        txop_limit: HeEdcaTxopLimit,
    ) -> Result<(), HtAmpduTxError> {
        if !self
            .as_ref()
            .can_commit_hardware_he_control_frame_with_txop(
                cookie,
                frame_length,
                hardware_mic_length,
                rate,
                density,
                txop_limit,
            )?
        {
            return Err(HtAmpduTxError::AggregateFull);
        }
        let psdu_length = frame_length
            .checked_add(usize::from(hardware_mic_length))
            .and_then(|length| length.checked_add(usize::from(TX_FCS_SIZE)))
            .and_then(|length| u16::try_from(length).ok())
            .filter(|length| *length != 0 && *length <= 0x3fff)
            .ok_or(HtAmpduTxError::FrameTooLong)?;
        let empty_delimiters = rate
            .ampdu_empty_delimiters(psdu_length, density)
            .ok_or(HtAmpduTxError::FrameTooLong)?;
        self.commit_frame_with_hardware_he_control(
            cookie,
            frame_length,
            hardware_mic_length,
            empty_delimiters,
            true,
        )
    }

    /// Commit one HE MPDU together with its original MSDU byte count.
    ///
    /// SOURCE: complete `libpp.a[hal_debug.o]::dbg_read_tx_ppdu`
    /// names frame-state halfword `+0x22` `msdu_len`; complete
    /// `libpp.a[hal_mac_tx.o]::mac_tx_set_tb` sums that halfword
    /// across the linked aggregate and publishes it to `WDEVTXQBSR_SW`.
    /// Keeping the value beside the owned encoded MPDU prevents the Trigger
    /// path from substituting the larger 802.11 frame or PSDU length.
    pub fn commit_he_msdu_frame(
        mut self: Pin<&mut Self>,
        cookie: TxCookie,
        frame_length: usize,
        hardware_mic_length: u8,
        msdu_length: usize,
        rate: HeRate,
        density: HtAmpduDensity,
    ) -> Result<(), HtAmpduTxError> {
        let msdu_length = u16::try_from(msdu_length)
            .ok()
            .filter(|length| *length != 0)
            .ok_or(HtAmpduTxError::TriggerMsduLengthUnavailable)?;
        let index = usize::from(self.count);
        self.as_mut()
            .commit_he_frame(cookie, frame_length, hardware_mic_length, rate, density)?;
        self.project().msdu_lengths[index] = msdu_length;
        Ok(())
    }

    /// Commit a Trigger-eligible HE MPDU with hardware HE-Control insertion.
    pub fn commit_hardware_he_control_msdu_frame(
        self: Pin<&mut Self>,
        cookie: TxCookie,
        frame_length: usize,
        hardware_mic_length: u8,
        msdu_length: usize,
        rate: HeRate,
        density: HtAmpduDensity,
    ) -> Result<(), HtAmpduTxError> {
        self.commit_hardware_he_control_msdu_frame_with_txop(
            cookie,
            frame_length,
            hardware_mic_length,
            msdu_length,
            rate,
            density,
            HeEdcaTxopLimit::DEFAULT,
        )
    }

    /// Commit Trigger metadata and HE-Control under a typed EDCA TXOP limit.
    pub fn commit_hardware_he_control_msdu_frame_with_txop(
        mut self: Pin<&mut Self>,
        cookie: TxCookie,
        frame_length: usize,
        hardware_mic_length: u8,
        msdu_length: usize,
        rate: HeRate,
        density: HtAmpduDensity,
        txop_limit: HeEdcaTxopLimit,
    ) -> Result<(), HtAmpduTxError> {
        let msdu_length = u16::try_from(msdu_length)
            .ok()
            .filter(|length| *length != 0)
            .ok_or(HtAmpduTxError::TriggerMsduLengthUnavailable)?;
        let index = usize::from(self.count);
        self.as_mut().commit_hardware_he_control_frame_with_txop(
            cookie,
            frame_length,
            hardware_mic_length,
            rate,
            density,
            txop_limit,
        )?;
        self.project().msdu_lengths[index] = msdu_length;
        Ok(())
    }

    /// Discard a software-owned partial batch.
    pub fn cancel(mut self: Pin<&mut Self>, cookie: TxCookie) -> Result<(), HtAmpduTxError> {
        let storage = self.as_ref().get_ref();
        if storage.state != TxSlotState::Reserved || storage.active != cookie {
            return Err(HtAmpduTxError::Stale);
        }
        self.as_mut().release();
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
        let storage = self.as_ref().get_ref();
        if storage.state != TxSlotState::Reserved || storage.active != cookie {
            return Err(HtAmpduTxError::Stale);
        }
        // Complete `libpp.a[pp.o]::ppAssembleAMPDU` accepts a
        // one-element descriptor chain and stops at its null next pointer.
        // Keep zero as the only invalid cardinality; batching two or more
        // MPDUs is a scheduler optimization rather than a formatter
        // invariant.
        if storage.count == 0 {
            return Err(HtAmpduTxError::TooFewFrames);
        }

        let aggregate = storage.calculate_aggregate()?;
        let count = usize::from(storage.count);
        if config.aggregate_length != aggregate.bytes || config.subframes != aggregate.subframes {
            return Err(HtAmpduTxError::RegisterImageMismatch);
        }
        let first_descriptor = hardware
            .tx_descriptor_address(core::ptr::addr_of!(storage.descriptors[0]).addr() as u32);
        for index in 0..count {
            #[cfg(target_arch = "riscv32")]
            let descriptor_address = hardware.tx_descriptor_address(
                core::ptr::addr_of!(storage.descriptors[index]).addr() as u32,
            );
            let buffer_address = u32::try_from(storage.buffer_addresses[index]).unwrap_or(u32::MAX);
            let capacity = u32::from(storage.descriptor_capacities[index]);
            let transfer_length =
                (TX_AMPDU_METADATA_SIZE as u32) + u32::from(storage.psdu_lengths[index]);
            #[cfg(target_arch = "riscv32")]
            {
                if !descriptor_address_valid(descriptor_address)
                    || !dma_range_valid(buffer_address, capacity)
                {
                    return Err(HtAmpduTxError::InvalidDmaRange {
                        descriptor_address,
                        buffer_address,
                        capacity,
                    });
                }
            }
            let next_address = if index + 1 < count {
                hardware.tx_descriptor_address(
                    core::ptr::addr_of!(storage.descriptors[index + 1]).addr() as u32,
                )
            } else {
                0
            };
            let mut word0 = tx_owned_word(capacity, transfer_length).ok_or(
                HtAmpduTxError::InvalidDescriptorLength {
                    capacity,
                    transfer_length,
                },
            )?;
            if index + 1 < count {
                word0 &= !BIT_30;
            }
            storage.descriptors[index].publish(word0, buffer_address, next_address);
        }

        let image = crate::tx::ht_ampdu_q0_image(first_descriptor, config).ok_or(
            HtAmpduTxError::TxImageUnavailable {
                format: HtAmpduTxFormat::HtAmpdu,
            },
        )?;
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
        let storage = self.project();
        *storage.queue = queue;
        *storage.aggregate_length = aggregate.bytes;
        *storage.detached = false;
        *storage.state = TxSlotState::HardwareOwned;
        hardware.start_ht_tx(queue_index, image.plcp0);
        Ok(())
    }

    /// Publish and start one HE20 SU A-MPDU with the same pinned ownership
    /// and completion/BlockAck ordering as [`Self::submit`].
    pub fn submit_he<H: HtAmpduHardware>(
        mut self: Pin<&mut Self>,
        hardware: &mut H,
        cookie: TxCookie,
        queue: LegacyTxQueue,
        config: HeAmpduTxConfig,
    ) -> Result<(), HtAmpduTxError> {
        let storage = self.as_ref().get_ref();
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
        let trigger = storage.prepared_he_trigger(queue, config)?;

        let first_descriptor = hardware
            .tx_descriptor_address(core::ptr::addr_of!(storage.descriptors[0]).addr() as u32);
        for index in 0..count {
            #[cfg(target_arch = "riscv32")]
            let descriptor_address = hardware.tx_descriptor_address(
                core::ptr::addr_of!(storage.descriptors[index]).addr() as u32,
            );
            let buffer_address = u32::try_from(storage.buffer_addresses[index]).unwrap_or(u32::MAX);
            let capacity = u32::from(storage.descriptor_capacities[index]);
            let transfer_length =
                (TX_AMPDU_METADATA_SIZE as u32) + u32::from(storage.psdu_lengths[index]);
            #[cfg(target_arch = "riscv32")]
            {
                if !descriptor_address_valid(descriptor_address)
                    || !dma_range_valid(buffer_address, capacity)
                {
                    return Err(HtAmpduTxError::InvalidDmaRange {
                        descriptor_address,
                        buffer_address,
                        capacity,
                    });
                }
            }
            let next_address = if index + 1 < count {
                hardware.tx_descriptor_address(
                    core::ptr::addr_of!(storage.descriptors[index + 1]).addr() as u32,
                )
            } else {
                0
            };
            let mut word0 = tx_owned_word(capacity, transfer_length).ok_or(
                HtAmpduTxError::InvalidDescriptorLength {
                    capacity,
                    transfer_length,
                },
            )?;
            if index + 1 < count {
                word0 &= !BIT_30;
            }
            storage.descriptors[index].publish(word0, buffer_address, next_address);
        }

        let image = crate::tx::he_ampdu_q0_image(first_descriptor, config).ok_or(
            HtAmpduTxError::TxImageUnavailable {
                format: HtAmpduTxFormat::HeAmpdu,
            },
        )?;
        let program = MacHeTxProgram {
            plcp0: image.plcp0,
            plcp1: image.plcp1,
            he_signal_a1: image.he_signal_a1,
            he_signal_a2_length: image.he_signal_a2_length,
            software_he_control: None,
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
        let psdu_lengths = &storage.psdu_lengths[..count];
        if let Some(trigger) = trigger {
            let publication_snapshot = hardware
                .prepare_he_trigger_based_queue(
                    trigger.policy,
                    trigger.reservation,
                    trigger.tid,
                    psdu_lengths,
                    trigger.queued_msdu_bytes,
                )
                .map_err(HtAmpduTxError::TriggerBased)?;
            let storage = self.as_mut().project();
            *storage.trigger_reservation = Some(trigger.reservation);
            // SOURCE: complete blob/ROM `mac_tx_set_tb` make the BSR valid
            // bitmap their final publication edge. The following ordinary
            // HE queue doorbell can immediately consume and clear both BSR
            // value and validity, as observed by
            // HIL_OPEN_HE_TB_QUEUE_PUBLICATION_2026_07_30. Preserve the
            // PAC-backed readback at the only deterministic boundary: after
            // publication and before `start_he_tx`.
            *storage.trigger_publication_snapshot = Some(publication_snapshot);
        }
        let storage = self.project();
        *storage.queue = queue;
        *storage.aggregate_length = aggregate.bytes;
        *storage.detached = false;
        *storage.state = TxSlotState::HardwareOwned;
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
        let storage = self.as_ref().get_ref();
        if storage.state != TxSlotState::Reserved || storage.active != cookie {
            return Err(HtAmpduTxError::Stale);
        }
        if storage.count != 1
            || storage.frame_lengths[0] != config.mpdu_length
            || storage.hardware_mic_lengths[0] != 0
        {
            return Err(HtAmpduTxError::RegisterImageMismatch);
        }

        let descriptor_address = hardware
            .tx_descriptor_address(core::ptr::addr_of!(storage.descriptors[0]).addr() as u32);
        let buffer_address = u32::try_from(storage.buffer_addresses[0]).unwrap_or(u32::MAX);
        // HIL_VENDOR_HE20_MCS0_DCM_RAW_2026_07_29 captured the complete
        // vendor DMA word c0090040: capacity 64 and used length 36. Preserve
        // that bounded single-MPDU allocation geometry even though the
        // statically owned Rust buffer is larger.
        let capacity =
            u32::from(storage.descriptor_capacities[0]).max(HE_SMPDU_VENDOR_DMA_CAPACITY);
        let transfer_length = (TX_AMPDU_METADATA_SIZE as u32) + u32::from(storage.psdu_lengths[0]);
        #[cfg(target_arch = "riscv32")]
        {
            if !descriptor_address_valid(descriptor_address)
                || !dma_range_valid(buffer_address, capacity)
            {
                return Err(HtAmpduTxError::InvalidDmaRange {
                    descriptor_address,
                    buffer_address,
                    capacity,
                });
            }
        }

        // `commit_frame` already wrote MPDU+FCS length and reserved the four
        // trailing hardware-FCS bytes. The live vendor S-MPDU buffer started
        // with 0x0100_001c for a 24-byte frame: retain length 28, set metadata
        // bit 24, keep empty delimiters and byte-seven's optional term zero.
        // SAFETY: the committed backing is pinned by this storage or by the
        // referenced-frame owner required by `commit_referenced_frame`.
        let buffer = unsafe {
            core::slice::from_raw_parts_mut(
                storage.buffer_addresses[0] as *mut u8,
                usize::from(storage.descriptor_capacities[0]),
            )
        };
        let metadata_length = u32::from(storage.psdu_lengths[0]);
        buffer[..4].copy_from_slice(&(metadata_length | 0x0100_0000).to_le_bytes());
        buffer[4] = 0;
        buffer[7] = 0;

        let word0 = tx_owned_word(capacity, transfer_length).ok_or(
            HtAmpduTxError::InvalidDescriptorLength {
                capacity,
                transfer_length,
            },
        )?;
        storage.descriptors[0].publish(word0, buffer_address, 0);

        let image = crate::tx::he_smpdu_q0_image(descriptor_address, config).ok_or(
            HtAmpduTxError::TxImageUnavailable {
                format: HtAmpduTxFormat::HeSmpdu,
            },
        )?;
        let program = MacHeTxProgram {
            plcp0: image.plcp0,
            plcp1: image.plcp1,
            he_signal_a1: image.he_signal_a1,
            he_signal_a2_length: image.he_signal_a2_length,
            software_he_control: None,
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
        let storage = self.project();
        *storage.queue = queue;
        *storage.aggregate_length = config.apep_length();
        *storage.detached = false;
        *storage.state = TxSlotState::HardwareOwned;
        hardware.start_he_tx(queue_index, image.plcp0);
        Ok(())
    }

    /// Sample BlockAck and transfer a completed aggregate back to software.
    pub fn acknowledge_completion<H: HtAmpduHardware>(
        self: Pin<&mut Self>,
        hardware: &mut H,
    ) -> Result<Option<HtAmpduTxCompletion>, HtAmpduTxError> {
        let storage = self.project();
        let Some(registers) = hardware.take_ht_ampdu_completion(storage.queue.index()) else {
            return Ok(None);
        };
        if *storage.state != TxSlotState::HardwareOwned {
            *storage.state = TxSlotState::ResetRequired;
            return Err(HtAmpduTxError::Stale);
        }
        if let Some(reservation) = storage.trigger_reservation.take() {
            hardware.clear_he_trigger_based_queue(reservation);
        }
        *storage.state = TxSlotState::Completed;
        Ok(Some(HtAmpduTxCompletion {
            tx: decode_tx_completion(*storage.active, registers.tx),
            block_ack: decode_ht_block_ack_registers(
                registers.block_ack_control_and_sequence,
                registers.block_ack_bitmap_low,
                registers.block_ack_bitmap_high,
            ),
        }))
    }

    /// Return the Trigger queue readback captured at its publication edge.
    ///
    /// The snapshot is sampled after the complete queue/BSR transaction and
    /// before the HE TX doorbell. Hardware may consume the BSR value and
    /// validity immediately after that doorbell, so a later live read is not
    /// an equivalent publication check.
    pub fn he_trigger_based_snapshot<H: HtAmpduHardware>(
        self: Pin<&Self>,
        _hardware: &H,
        cookie: TxCookie,
    ) -> Result<Option<MacHeTriggerTxQueueSnapshot>, HtAmpduTxError> {
        let storage = self.get_ref();
        if storage.state != TxSlotState::HardwareOwned || storage.active != cookie {
            return Err(HtAmpduTxError::Stale);
        }
        Ok(storage.trigger_publication_snapshot)
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
        let storage = self.project();
        let Some(registers) = hardware.take_tx_completion(storage.queue.index()) else {
            return Ok(None);
        };
        if *storage.state != TxSlotState::HardwareOwned || *storage.count != 1 {
            *storage.state = TxSlotState::ResetRequired;
            return Err(HtAmpduTxError::Stale);
        }
        *storage.state = TxSlotState::Completed;
        Ok(Some(decode_tx_completion(*storage.active, registers)))
    }

    pub fn begin_timeout_abort<H: HtAmpduHardware>(
        self: Pin<&mut Self>,
        hardware: &mut H,
        cookie: TxCookie,
    ) -> Result<bool, HtAmpduTxError> {
        let storage = self.project();
        if *storage.state != TxSlotState::HardwareOwned || *storage.active != cookie {
            return Err(HtAmpduTxError::Stale);
        }
        Ok(hardware.begin_tx_timeout_abort(storage.queue.index()))
    }

    /// Permanently quarantine an aggregate whose executor deadline expired
    /// without a qualified hardware completion/timeout edge.
    ///
    /// The referenced network leases must remain retained until the unique
    /// radio lifecycle owner resets the MAC; returning them would make
    /// potentially DMA-visible allocations writable by `embassy-net`.
    pub fn require_reset(self: Pin<&mut Self>, cookie: TxCookie) -> Result<(), HtAmpduTxError> {
        let storage = self.project();
        if *storage.state != TxSlotState::HardwareOwned || *storage.active != cookie {
            return Err(HtAmpduTxError::Stale);
        }
        *storage.state = TxSlotState::ResetRequired;
        Ok(())
    }

    /// Disable and release one collision-owned aggregate queue.
    pub fn abort_collision<H: HtAmpduHardware>(
        mut self: Pin<&mut Self>,
        hardware: &mut H,
        cookie: TxCookie,
    ) -> Result<bool, HtAmpduTxError> {
        let storage = self.as_ref().get_ref();
        if storage.state != TxSlotState::HardwareOwned || storage.active != cookie {
            return Err(HtAmpduTxError::Stale);
        }
        if !hardware.abort_tx_collision(storage.queue.index()) {
            return Ok(false);
        }
        if let Some(reservation) = self.as_mut().project().trigger_reservation.take() {
            hardware.clear_he_trigger_based_queue(reservation);
        }
        self.as_mut().release();
        Ok(true)
    }

    pub fn finish_timeout_abort<H: HtAmpduHardware>(
        mut self: Pin<&mut Self>,
        hardware: &mut H,
        cookie: TxCookie,
    ) -> Result<(), HtAmpduTxError> {
        let storage = self.as_ref().get_ref();
        if storage.state != TxSlotState::HardwareOwned || storage.active != cookie {
            return Err(HtAmpduTxError::Stale);
        }
        let Some(detached) = hardware.finish_tx_timeout_abort(storage.queue.index()) else {
            return Err(HtAmpduTxError::TimeoutNotPending);
        };
        if !detached {
            *self.as_mut().project().state = TxSlotState::ResetRequired;
            return Err(HtAmpduTxError::DetachFailed);
        }
        if let Some(reservation) = self.as_mut().project().trigger_reservation.take() {
            hardware.clear_he_trigger_based_queue(reservation);
        }
        self.as_mut().release();
        Ok(())
    }

    pub fn detach_completed<H: HtAmpduHardware>(
        self: Pin<&mut Self>,
        hardware: &mut H,
        cookie: TxCookie,
    ) -> Result<(), HtAmpduTxError> {
        let storage = self.project();
        if *storage.state != TxSlotState::Completed || *storage.active != cookie {
            return Err(HtAmpduTxError::Stale);
        }
        if !hardware.detach_completed_tx(storage.queue.index()) {
            *storage.state = TxSlotState::ResetRequired;
            return Err(HtAmpduTxError::DetachFailed);
        }
        // Keep the completed MPDUs and their lengths alive. BlockAck handling
        // may now copy any missing MPDU into the single-frame TX owner without
        // reconstructing Sequence Control or the CCMP PN. `release_completed`
        // is the explicit final ownership edge.
        *storage.detached = true;
        Ok(())
    }

    /// Release a detached completed batch after BlockAck processing and any
    /// individual retries have copied the retained MPDUs.
    pub fn release_completed(
        mut self: Pin<&mut Self>,
        cookie: TxCookie,
    ) -> Result<(), HtAmpduTxError> {
        let storage = self.as_ref().get_ref();
        if storage.state != TxSlotState::Completed || storage.active != cookie || !storage.detached
        {
            return Err(HtAmpduTxError::Stale);
        }
        self.as_mut().release();
        Ok(())
    }

    fn release(self: Pin<&mut Self>) {
        let storage = self.project();
        *storage.active = TxCookie(0);
        *storage.count = 0;
        *storage.prepared_length = 0;
        *storage.aggregate_length = 0;
        *storage.trigger_reservation = None;
        *storage.trigger_publication_snapshot = None;
        *storage.detached = false;
        *storage.state = TxSlotState::Free;
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
    /// SOURCE: complete `libpp.a[pp.o]::{ppCalSubFrameLength,
    /// ppCalTxAMPDULength}`, complete `libpp.a[pp_he.o]::
    /// ppCalSubFrameLength`, and the equivalent finite rules in
    /// [`HtAmpduLengthAccumulator`].
    fn length_after_append(
        &self,
        psdu_length: u16,
        hardware_he_control: bool,
    ) -> Result<u16, HtAmpduTxError> {
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
            .checked_add(
                4 + u32::from(psdu_length)
                    + if hardware_he_control {
                        u32::from(HARDWARE_HE_CONTROL_LENGTH)
                    } else {
                        0
                    },
            )
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

    fn recalculate_prepared_length(self: Pin<&mut Self>) -> Result<HtAmpduLength, HtAmpduTxError> {
        let storage = self.as_ref().get_ref();
        let mut length = HtAmpduLengthAccumulator::new(storage.count, storage.max_aggregate_bytes)
            .map_err(HtAmpduTxError::Length)?;
        for index in 0..usize::from(storage.count) {
            length
                .push_with_hardware_he_control(
                    u32::from(storage.psdu_lengths[index]),
                    storage.empty_delimiters[index],
                    storage.hardware_he_control[index],
                )
                .map_err(HtAmpduTxError::Length)?;
        }
        let aggregate = length.finish().map_err(HtAmpduTxError::Length)?;
        *self.project().prepared_length = aggregate.bytes;
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

    fn prepared_he_trigger(
        &self,
        queue: LegacyTxQueue,
        config: HeAmpduTxConfig,
    ) -> Result<Option<PreparedHeTrigger>, HtAmpduTxError> {
        let Some(trigger) = config.trigger_based() else {
            return Ok(None);
        };
        let count = self.count;
        let reservation =
            MacHeTbLinkReservation::for_queue(trigger.tid_limit(), queue.index(), count).ok_or(
                HtAmpduTxError::InvalidTriggerReservation {
                    queue: queue.index(),
                    subframes: count,
                },
            )?;
        if self.psdu_lengths[..usize::from(count)]
            .iter()
            .any(|length| *length == 0 || *length > 0x3fff)
        {
            return Err(HtAmpduTxError::TriggerBased(
                MacHeTbProgramError::InvalidMpduLength,
            ));
        }
        let mut queued_msdu_bytes = 0_u32;
        for msdu_length in &self.msdu_lengths[..usize::from(count)] {
            if *msdu_length == 0 {
                return Err(HtAmpduTxError::TriggerMsduLengthUnavailable);
            }
            queued_msdu_bytes = queued_msdu_bytes
                .checked_add(u32::from(*msdu_length))
                .ok_or(HtAmpduTxError::TriggerBased(
                    MacHeTbProgramError::QueuedBytesTooLarge,
                ))?;
        }
        if queued_msdu_bytes > 0x000f_ffff {
            return Err(HtAmpduTxError::TriggerBased(
                MacHeTbProgramError::QueuedBytesTooLarge,
            ));
        }
        Ok(Some(PreparedHeTrigger {
            policy: trigger.tid_limit(),
            reservation,
            tid: trigger.tid(),
            queued_msdu_bytes,
        }))
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
}

/// Inputs consumed by the finite HT `ppAssembleAMPDU` leaf.
///
/// This value form keeps the recovered bit transition host-testable without
/// exposing the vendor ESF pointer layout to the owned TX path.
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

/// Reproduce the `ni + 0x82` protection-spacing value written by the pinned
/// `rcUpdateAMPDUParam` body from the peer's HT A-MPDU Parameters byte.
///
/// Bits 2..=4 encode the IEEE 802.11 minimum MPDU start spacing. The hardware
/// consumes the recovered finite value in all three 10-bit protection fields.
#[cfg(test)]
pub(crate) const fn basic_ht_ampdu_protection_spacing(parameters: u8) -> u16 {
    HtProtectionSpacing::from_ampdu_parameters(parameters).hardware_value()
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

const BA_PARAMETER_AMSDU: u16 = 1;
const BA_PARAMETER_IMMEDIATE: u16 = 1 << 1;
const BA_PARAMETER_TID_SHIFT: u32 = 2;
const BA_PARAMETER_WINDOW_SHIFT: u32 = 6;
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

/// One vendor-compatible BlockAck action Dialog Token.
///
/// Keeping construction private prevents independent per-TID sessions from
/// accidentally reusing a token while their negotiations overlap.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TxBlockAckDialogToken(u8);

impl TxBlockAckDialogToken {
    pub const fn value(self) -> u8 {
        self.0
    }
}

/// Shared Dialog Token owner for a set of TX BlockAck sessions.
///
/// SOURCE: complete `libnet80211.a[wl_cnx.o]::cnx_auth_done`
/// invokes `ieee80211_ampdu_request` for TIDs 0, 7 and 5 in that order.
/// Complete `libnet80211.a[ieee80211_ht.o]::
/// ieee80211_ampdu_request` increments one archive-static token modulo 63
/// before constructing each request. The vendor HE20 air oracle consequently
/// carries tokens 1, 2 and 3 for those three negotiations.
pub struct TxBlockAckDialogTokenSequence {
    next: u8,
}

impl TxBlockAckDialogTokenSequence {
    pub const fn new() -> Self {
        Self { next: 1 }
    }

    pub fn take(&mut self) -> TxBlockAckDialogToken {
        let token = TxBlockAckDialogToken(self.next);
        self.next = next_vendor_block_ack_dialog_token(self.next);
        token
    }
}

impl Default for TxBlockAckDialogTokenSequence {
    fn default() -> Self {
        Self::new()
    }
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

/// TX BlockAck TIDs started by the vendor STA connection-complete path.
///
/// SOURCE: complete `libnet80211.a[wl_cnx.o]::cnx_auth_done`
/// invokes `ieee80211_ampdu_request` for TIDs 0, 7 and 5 in this order.
pub const STA_TX_BLOCK_ACK_TIDS: [u8; 3] = [0, 7, 5];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaTxBlockAckSessionsError {
    UnsupportedTid(u8),
    MalformedResponse,
    UnexpectedDialogToken(u8),
    Session { tid: u8, error: TxBlockAckError },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaTxBlockAckResponse {
    pub tid: u8,
    pub response: TxBlockAckResponse,
}

/// Fixed, allocation-free owner for all TX BlockAck agreements created when
/// an S31 station enters the connected state.
///
/// The three sessions deliberately share one Dialog Token sequence while
/// retaining independent negotiation generations and alarms. This is the
/// ownership boundary recovered from the vendor connection-complete path;
/// an executor supplies timestamps and transmits the returned action body.
pub struct StaTxBlockAckSessions {
    sessions: [TxBlockAckSession; 3],
    alarms: [Option<TxBlockAckAlarm>; 3],
    dialog_tokens: TxBlockAckDialogTokenSequence,
}

impl StaTxBlockAckSessions {
    pub const fn new(
        window: u16,
        negotiation_timeout_us: u32,
        tid0_amsdu: bool,
    ) -> Result<Self, TxBlockAckError> {
        let tid0 = match TxBlockAckSession::new(TxBlockAckConfig {
            tid: 0,
            window,
            timeout_tu: 0,
            negotiation_timeout_us,
            amsdu: tid0_amsdu,
        }) {
            Ok(session) => session,
            Err(error) => return Err(error),
        };
        let tid7 = match TxBlockAckSession::new(TxBlockAckConfig {
            tid: 7,
            window,
            timeout_tu: 0,
            negotiation_timeout_us,
            amsdu: false,
        }) {
            Ok(session) => session,
            Err(error) => return Err(error),
        };
        let tid5 = match TxBlockAckSession::new(TxBlockAckConfig {
            tid: 5,
            window,
            timeout_tu: 0,
            negotiation_timeout_us,
            amsdu: false,
        }) {
            Ok(session) => session,
            Err(error) => return Err(error),
        };
        Ok(Self {
            sessions: [tid0, tid7, tid5],
            alarms: [None; 3],
            dialog_tokens: TxBlockAckDialogTokenSequence::new(),
        })
    }

    /// Begin one of the three recovered STA negotiations.
    ///
    /// The returned request owns its encoded action body. Its alarm is stored
    /// internally so a caller cannot accidentally pair it with another TID.
    pub fn begin(
        &mut self,
        tid: u8,
        starting_sequence: u16,
        now_us: u64,
    ) -> Result<AddbaRequest, StaTxBlockAckSessionsError> {
        let index =
            sta_tx_block_ack_index(tid).ok_or(StaTxBlockAckSessionsError::UnsupportedTid(tid))?;
        let dialog_token = self.dialog_tokens.take();
        let request = self.sessions[index]
            .begin_with_dialog_token(starting_sequence, now_us, dialog_token)
            .map_err(|error| StaTxBlockAckSessionsError::Session { tid, error })?;
        self.alarms[index] = Some(request.alarm);
        Ok(request)
    }

    /// Route one ADDBA response by the shared Dialog Token and update exactly
    /// one session. A terminal response also consumes that session's alarm.
    pub fn on_response(
        &mut self,
        body: &[u8],
    ) -> Result<StaTxBlockAckResponse, StaTxBlockAckSessionsError> {
        let action =
            parse_block_ack_action(body).ok_or(StaTxBlockAckSessionsError::MalformedResponse)?;
        self.on_response_action(action)
    }

    /// Route an already parsed ADDBA response without retaining its borrowed
    /// management-frame body.
    ///
    /// This is the ownership boundary used by an async RX dispatcher: the
    /// fixed fields are copied into [`BlockAckAction`] while the staged frame
    /// is live, then protocol state can be updated after that storage is
    /// released.
    pub fn on_response_action(
        &mut self,
        action: BlockAckAction,
    ) -> Result<StaTxBlockAckResponse, StaTxBlockAckSessionsError> {
        let BlockAckAction::AddbaResponse {
            dialog_token: response_token,
            ..
        } = action
        else {
            return Err(StaTxBlockAckSessionsError::MalformedResponse);
        };
        let index = self
            .sessions
            .iter()
            .position(|session| session.awaiting_dialog_token() == Some(response_token))
            .ok_or(StaTxBlockAckSessionsError::UnexpectedDialogToken(
                response_token,
            ))?;
        let tid = STA_TX_BLOCK_ACK_TIDS[index];
        let response = self.sessions[index]
            .on_response_action(action)
            .map_err(|error| StaTxBlockAckSessionsError::Session { tid, error })?;
        self.alarms[index] = None;
        Ok(StaTxBlockAckResponse { tid, response })
    }

    /// Consume at most one due alarm. Repeated calls drain simultaneous
    /// expirations without placing an unbounded loop inside the state owner.
    pub fn expire_next(&mut self, now_us: u64) -> Option<u8> {
        for (index, tid) in STA_TX_BLOCK_ACK_TIDS.into_iter().enumerate() {
            let Some(alarm) = self.alarms[index] else {
                continue;
            };
            if now_us < alarm.deadline_us {
                continue;
            }
            self.alarms[index] = None;
            if self.sessions[index].on_alarm(alarm) {
                return Some(tid);
            }
        }
        None
    }

    /// Stop the recovered session for `tid` and invalidate its alarm.
    pub fn stop(&mut self, tid: u8) -> bool {
        let Some(index) = sta_tx_block_ack_index(tid) else {
            return false;
        };
        self.sessions[index].stop();
        self.alarms[index] = None;
        true
    }

    pub const fn operational(&self, tid: u8) -> Option<OperationalTxBlockAck> {
        let Some(index) = sta_tx_block_ack_index(tid) else {
            return None;
        };
        self.sessions[index].operational()
    }

    pub const fn alarm(&self, tid: u8) -> Option<TxBlockAckAlarm> {
        let Some(index) = sta_tx_block_ack_index(tid) else {
            return None;
        };
        self.alarms[index]
    }
}

const fn sta_tx_block_ack_index(tid: u8) -> Option<usize> {
    match tid {
        0 => Some(0),
        7 => Some(1),
        5 => Some(2),
        _ => None,
    }
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
#[unsafe(link_section = ".rwtext.wifi_strict.tx_block_ack")]
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
        let dialog_token = TxBlockAckDialogToken(self.next_dialog_token);
        self.next_dialog_token = next_dialog_token(dialog_token.value());
        self.begin_with_dialog_token(starting_sequence, now_us, dialog_token)
    }

    /// Start negotiation with a token supplied by a shared multi-TID owner.
    ///
    /// A caller that has more than one live TID must obtain this value from
    /// [`TxBlockAckDialogTokenSequence`]. The session still exclusively owns
    /// its timeout generation, starting sequence and operational agreement.
    pub fn begin_with_dialog_token(
        &mut self,
        starting_sequence: u16,
        now_us: u64,
        dialog_token: TxBlockAckDialogToken,
    ) -> Result<AddbaRequest, TxBlockAckError> {
        let deadline_us = now_us
            .checked_add(u64::from(self.config.negotiation_timeout_us))
            .ok_or(TxBlockAckError::DeadlineOverflow)?;
        self.generation = next_generation(self.generation);
        let dialog_token = dialog_token.value();
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
        let action = parse_block_ack_action(body).ok_or(TxBlockAckError::MalformedResponse)?;
        self.on_response_action(action)
    }

    /// Apply the fixed fields of an already parsed ADDBA response.
    pub fn on_response_action(
        &mut self,
        action: BlockAckAction,
    ) -> Result<TxBlockAckResponse, TxBlockAckError> {
        let BlockAckAction::AddbaResponse {
            dialog_token: response_dialog_token,
            status,
            tid,
            immediate,
            amsdu,
            window,
            timeout_tu,
        } = action
        else {
            return Err(TxBlockAckError::MalformedResponse);
        };
        let TxBlockAckPhase::Awaiting {
            dialog_token,
            starting_sequence,
        } = self.phase
        else {
            return Err(TxBlockAckError::UnexpectedResponse);
        };
        if response_dialog_token != dialog_token {
            return Err(TxBlockAckError::UnexpectedResponse);
        }

        if status != 0 {
            self.phase = TxBlockAckPhase::Idle;
            self.generation = next_generation(self.generation);
            return Ok(TxBlockAckResponse::Rejected(status));
        }

        if !immediate {
            return Err(TxBlockAckError::DelayedPolicyUnsupported);
        }
        if tid != self.config.tid {
            return Err(TxBlockAckError::UnexpectedResponse);
        }
        if window == 0 || window > self.config.window || window > TX_BLOCK_ACK_MAX_WINDOW {
            return Err(TxBlockAckError::WindowExceedsCapacity(window));
        }
        let agreement = OperationalTxBlockAck {
            tid,
            window,
            timeout_tu,
            starting_sequence,
            amsdu: self.config.amsdu && amsdu,
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

    /// Dialog Token currently owned by an outstanding negotiation.
    ///
    /// A multi-TID dispatcher can use this before calling [`Self::on_response`]
    /// so a response is delivered to exactly one session.
    pub const fn awaiting_dialog_token(&self) -> Option<u8> {
        match self.phase {
            TxBlockAckPhase::Awaiting { dialog_token, .. } => Some(dialog_token),
            _ => None,
        }
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
    if next == 0 { 1 } else { next }
}

const fn next_dialog_token(current: u8) -> u8 {
    let next = current.wrapping_add(1);
    if next == 0 { 1 } else { next }
}

const fn next_vendor_block_ack_dialog_token(current: u8) -> u8 {
    if current >= 62 { 0 } else { current + 1 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use open_esp_radio_esp32s31_registers::{
        MacHeTxVectorSnapshot, MacLegacyTxProgram, MacTxCompletionRegisters,
    };

    struct CompletionHardware {
        completion: Option<MacHtAmpduCompletionRegisters>,
        cleared: Option<MacHeTbLinkReservation>,
        trigger_snapshot: Option<MacHeTriggerTxQueueSnapshot>,
    }

    impl TxHardware for CompletionHardware {
        fn prepare_legacy_tx(&mut self, _: u8, _: MacLegacyTxProgram) -> bool {
            false
        }

        fn start_legacy_tx(&mut self, _: u8, _: u32) {}

        fn prepare_ht_tx(&mut self, _: u8, _: MacHtTxProgram) -> bool {
            false
        }

        fn start_ht_tx(&mut self, _: u8, _: u32) {}

        fn prepare_he_tx(&mut self, _: u8, _: MacHeTxProgram) -> bool {
            false
        }

        fn start_he_tx(&mut self, _: u8, _: u32) {}

        fn he_tx_vector_snapshot(&self, _: u8) -> Option<MacHeTxVectorSnapshot> {
            None
        }

        fn take_tx_completion(&mut self, _: u8) -> Option<MacTxCompletionRegisters> {
            None
        }

        fn begin_tx_timeout_abort(&mut self, _: u8) -> bool {
            false
        }

        fn finish_tx_timeout_abort(&mut self, _: u8) -> Option<bool> {
            None
        }

        fn detach_completed_tx(&mut self, _: u8) -> bool {
            false
        }
    }

    impl HtAmpduHardware for CompletionHardware {
        fn take_ht_ampdu_completion(&mut self, _: u8) -> Option<MacHtAmpduCompletionRegisters> {
            self.completion.take()
        }

        fn prepare_he_trigger_based_queue(
            &mut self,
            _: MacHeTbTidLimit,
            _: MacHeTbLinkReservation,
            _: MacHeTid,
            _: &[u16],
            _: u32,
        ) -> Result<MacHeTriggerTxQueueSnapshot, MacHeTbProgramError> {
            self.trigger_snapshot
                .ok_or(MacHeTbProgramError::LengthCountMismatch)
        }

        fn clear_he_trigger_based_queue(&mut self, reservation: MacHeTbLinkReservation) {
            self.cleared = Some(reservation);
        }

        fn he_trigger_based_queue_snapshot(
            &self,
            _: MacHeTbLinkReservation,
        ) -> Option<MacHeTriggerTxQueueSnapshot> {
            self.trigger_snapshot
        }
    }

    const CONFIG: TxBlockAckConfig = TxBlockAckConfig {
        tid: 7,
        window: 32,
        timeout_tu: 0,
        negotiation_timeout_us: 100_000,
        amsdu: true,
    };

    #[test]
    fn owned_dma_pool_builds_two_mpdu_length_without_publishing_hardware() {
        let storage = HtAmpduTxStorage::<4, 256>::new();
        let mut storage = core::pin::pin!(storage);
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
    fn referenced_commit_uses_the_retained_allocation_without_copying_payload() {
        let storage = HtAmpduTxStorage::<2, 256>::new();
        let mut external = [0xa5_u8; 256];
        external[TX_AMPDU_METADATA_SIZE..TX_AMPDU_METADATA_SIZE + 100].fill(0x5a);
        let mut storage = core::pin::pin!(storage);
        let cookie = storage.as_mut().begin().unwrap();
        unsafe {
            storage
                .as_mut()
                .commit_referenced_frame(cookie, &mut external, 100, 8, 0)
                .unwrap();
        }

        assert_eq!(
            storage.prepared_aggregate(cookie).unwrap(),
            HtAmpduLength {
                bytes: 116,
                subframes: 1,
            }
        );
        storage.as_mut().cancel(cookie).unwrap();
        assert_eq!(
            storage.buffer_addresses[0],
            external.as_ptr().addr(),
            "descriptor backing must remain the referenced allocation"
        );

        assert_eq!(u32::from_le_bytes(external[..4].try_into().unwrap()), 112);
        assert_eq!(
            &external[TX_AMPDU_METADATA_SIZE..TX_AMPDU_METADATA_SIZE + 100],
            &[0x5a; 100]
        );
        // The ordinary internal backing was not used as a staging buffer.
        assert_eq!(storage.buffers[0].0[TX_AMPDU_METADATA_SIZE], 0);
    }

    #[test]
    fn referenced_he_commit_uses_external_capacity_with_descriptor_only_storage() {
        let storage = HtAmpduTxStorage::<2, 0>::new();
        let mut first = [0xa5_u8; 256];
        let mut second = [0x5a_u8; 256];
        let rate = HeRate::bcc_dcm(
            crate::tx::HeBccDcmMcs::Mcs3,
            crate::rx::HeGuardIntervalAndLtf::TwoLtf800Ns,
        );
        let mut storage = core::pin::pin!(storage);
        let cookie = storage.as_mut().begin().unwrap();
        for external in [&mut first, &mut second] {
            unsafe {
                storage
                    .as_mut()
                    .commit_referenced_he_frame(
                        cookie,
                        external,
                        16,
                        8,
                        rate,
                        HtAmpduDensity::SixteenMicroseconds,
                    )
                    .unwrap();
            }
        }

        assert_eq!(storage.empty_delimiters[..2], [1, 1]);
        assert_eq!(
            storage.prepared_aggregate(cookie).unwrap(),
            HtAmpduLength {
                bytes: 68,
                subframes: 2,
            }
        );
        assert_eq!(first[4], 1);
        assert_eq!(second[4], 1);
        assert_eq!(storage.buffer_addresses[0], first.as_ptr().addr());
        assert_eq!(storage.buffer_addresses[1], second.as_ptr().addr());
        storage.as_mut().cancel(cookie).unwrap();
    }

    #[test]
    fn referenced_ht_commit_stops_at_the_vendor_rate_byte_ceiling() {
        let storage = HtAmpduTxStorage::<8, 0>::new();
        let mut external = [[0xa5_u8; 1_600]; 8];
        let mcs0_sgi = HtRate::new(
            crate::tx::HtMcs::Mcs0,
            crate::tx::HtGuardInterval::Short400Ns,
            crate::tx::HtChannelWidth::Mhz40,
        );
        let mut storage = core::pin::pin!(storage);
        storage
            .as_mut()
            .configure_max_aggregate_bytes(u16::MAX)
            .unwrap();
        let cookie = storage.as_mut().begin().unwrap();

        for frame in external.iter_mut().take(6) {
            unsafe {
                storage
                    .as_mut()
                    .commit_referenced_ht_frame(cookie, frame, 1_500, 8, 0, mcs0_sgi)
                    .unwrap();
            }
        }
        assert_eq!(
            storage.prepared_aggregate(cookie).unwrap(),
            HtAmpduLength {
                bytes: 9_096,
                subframes: 6,
            }
        );
        assert!(
            !storage
                .can_commit_referenced_ht_frame(cookie, 1_500, 8, 0, mcs0_sgi, external[6].len())
                .unwrap()
        );
        assert_eq!(
            unsafe {
                storage.as_mut().commit_referenced_ht_frame(
                    cookie,
                    &mut external[6],
                    1_500,
                    8,
                    0,
                    mcs0_sgi,
                )
            },
            Err(HtAmpduTxError::AggregateFull)
        );
        assert_eq!(storage.frame_count(), 6);
        storage.as_mut().cancel(cookie).unwrap();

        // The complete oracle table uses zero for SGI MCS7. That means this
        // particular leaf adds no ceiling: the peer/static limit still
        // permits a seventh MPDU.
        let mcs7_sgi = HtRate::new(
            crate::tx::HtMcs::Mcs7,
            crate::tx::HtGuardInterval::Short400Ns,
            crate::tx::HtChannelWidth::Mhz40,
        );
        let cookie = storage.as_mut().begin().unwrap();
        for frame in external.iter_mut().take(7) {
            unsafe {
                storage
                    .as_mut()
                    .commit_referenced_ht_frame(cookie, frame, 1_500, 8, 0, mcs7_sgi)
                    .unwrap();
            }
        }
        assert_eq!(storage.frame_count(), 7);
        storage.as_mut().cancel(cookie).unwrap();
    }

    #[test]
    fn owned_dma_pool_preserves_one_subframe_he_ampdu_length() {
        let storage = HtAmpduTxStorage::<2, 256>::new();
        let mut storage = core::pin::pin!(storage);
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
    fn owned_he_commit_derives_empty_delimiters_from_rate_and_peer_density() {
        let storage = HtAmpduTxStorage::<2, 256>::new();
        let mut storage = core::pin::pin!(storage);
        let cookie = storage.as_mut().begin().unwrap();
        let rate = HeRate::bcc_dcm(
            crate::tx::HeBccDcmMcs::Mcs3,
            crate::rx::HeGuardIntervalAndLtf::TwoLtf800Ns,
        );

        storage.as_mut().next_frame_buffer(cookie).unwrap()[..16].fill(0xa5);
        storage
            .as_mut()
            .commit_he_frame(cookie, 16, 8, rate, HtAmpduDensity::SixteenMicroseconds)
            .unwrap();
        storage.as_mut().next_frame_buffer(cookie).unwrap()[..16].fill(0x5a);
        storage
            .as_mut()
            .commit_he_frame(cookie, 16, 8, rate, HtAmpduDensity::SixteenMicroseconds)
            .unwrap();

        // frame + MIC + FCS is 28 bytes. At DCM MCS3/GI800 and 16 us the
        // blob minimum is 35 bytes, so ppCalDeliNum requests one empty
        // delimiter. The trailing delimiter of the final MPDU is omitted
        // from aggregate length exactly as ppEmptyDelimiterLength requires.
        assert_eq!(storage.empty_delimiters[..2], [1, 1]);
        assert_eq!(
            storage.prepared_aggregate(cookie).unwrap(),
            HtAmpduLength {
                bytes: 68,
                subframes: 2,
            }
        );
        let first = &storage.buffers[0].0;
        assert_eq!(first[4], 1);
    }

    #[test]
    fn hardware_he_control_uses_vendor_metadata_bit_without_dma_placeholder() {
        let storage = HtAmpduTxStorage::<2, 256>::new();
        let mut storage = core::pin::pin!(storage);
        let cookie = storage.as_mut().begin().unwrap();
        let rate = HeRate::new(
            crate::tx::HeMcs::Mcs9,
            crate::rx::HeGuardIntervalAndLtf::TwoLtf800Ns,
        );
        let frame = storage.as_mut().next_frame_buffer(cookie).unwrap();
        frame[..64].fill(0xa5);
        // Model the QoS/CCMP boundary: CCMP starts immediately at byte 26.
        frame[26..34].copy_from_slice(&[0x0f, 0, 0, 0x20, 0, 0, 0, 0]);
        storage
            .as_mut()
            .commit_hardware_he_control_frame(cookie, 64, 8, rate, HtAmpduDensity::NoRestriction)
            .unwrap();

        // Base MPDU length is frame + MIC + FCS = 76. Hardware HE-Control is
        // encoded only by metadata[7].bit0 and contributes four to APEP.
        assert_eq!(
            storage.prepared_aggregate(cookie).unwrap(),
            HtAmpduLength {
                bytes: 84,
                subframes: 1,
            }
        );
        let dma = &storage.buffers[0].0;
        assert_eq!(&dma[..4], &76_u32.to_le_bytes());
        assert_eq!(&dma[4..8], &0x0100_0000_u32.to_le_bytes());
        assert_eq!(
            &dma[TX_AMPDU_METADATA_SIZE + 26..TX_AMPDU_METADATA_SIZE + 34],
            &[0x0f, 0, 0, 0x20, 0, 0, 0, 0]
        );
    }

    #[test]
    fn he_trigger_preparation_uses_original_msdu_lengths_and_exact_link_range() {
        let storage = HtAmpduTxStorage::<4, 256>::new();
        let mut storage = core::pin::pin!(storage);
        let cookie = storage.as_mut().begin().unwrap();
        let rate = HeRate::new(
            crate::tx::HeMcs::Mcs9,
            crate::rx::HeGuardIntervalAndLtf::TwoLtf800Ns,
        );
        for (frame_length, msdu_length) in [(100, 80), (101, 81)] {
            storage.as_mut().next_frame_buffer(cookie).unwrap()[..frame_length].fill(0xa5);
            storage
                .as_mut()
                .commit_he_msdu_frame(
                    cookie,
                    frame_length,
                    8,
                    msdu_length,
                    rate,
                    HtAmpduDensity::NoRestriction,
                )
                .unwrap();
        }
        let aggregate = storage.prepared_aggregate(cookie).unwrap();
        let trigger = crate::tx::HeTriggerBasedTxConfig::new(
            MacHeTbTidLimit::Three,
            MacHeTid::new(0).unwrap(),
        )
        .unwrap();
        let config = HeAmpduTxConfig::new(
            rate,
            0,
            aggregate.bytes,
            aggregate.subframes,
            HtAmpduDensity::NoRestriction,
        )
        .unwrap()
        .with_trigger_based(trigger);
        let prepared = storage
            .prepared_he_trigger(LegacyTxQueue::BestEffort, config)
            .unwrap()
            .unwrap();

        assert_eq!(prepared.policy, MacHeTbTidLimit::Three);
        assert_eq!(prepared.tid, MacHeTid::new(0).unwrap());
        assert_eq!(prepared.reservation.queue(), 2);
        assert_eq!(prepared.reservation.first(), 0);
        assert_eq!(prepared.reservation.count(), 2);
        assert_eq!(prepared.queued_msdu_bytes, 161);
        assert_eq!(storage.psdu_lengths[..2], [112, 113]);
    }

    #[test]
    fn he_trigger_preparation_fails_closed_without_original_msdu_length() {
        let storage = HtAmpduTxStorage::<2, 256>::new();
        let mut storage = core::pin::pin!(storage);
        let cookie = storage.as_mut().begin().unwrap();
        let rate = HeRate::new(
            crate::tx::HeMcs::Mcs0,
            crate::rx::HeGuardIntervalAndLtf::TwoLtf800Ns,
        );
        storage.as_mut().next_frame_buffer(cookie).unwrap()[..64].fill(0x5a);
        storage
            .as_mut()
            .commit_he_frame(cookie, 64, 8, rate, HtAmpduDensity::NoRestriction)
            .unwrap();
        let aggregate = storage.prepared_aggregate(cookie).unwrap();
        let trigger = crate::tx::HeTriggerBasedTxConfig::new(
            MacHeTbTidLimit::Three,
            MacHeTid::new(0).unwrap(),
        )
        .unwrap();
        let config = HeAmpduTxConfig::new(
            rate,
            0,
            aggregate.bytes,
            aggregate.subframes,
            HtAmpduDensity::NoRestriction,
        )
        .unwrap()
        .with_trigger_based(trigger);

        assert_eq!(
            storage.prepared_he_trigger(LegacyTxQueue::BestEffort, config),
            Err(HtAmpduTxError::TriggerMsduLengthUnavailable)
        );
    }

    #[test]
    fn completion_exposes_publication_snapshot_then_clears_tb_enable() {
        let storage = HtAmpduTxStorage::<2, 256>::new();
        let mut storage = core::pin::pin!(storage);
        let cookie = storage.as_mut().begin().unwrap();
        let reservation = MacHeTbLinkReservation::for_queue(MacHeTbTidLimit::Three, 2, 2).unwrap();
        let snapshot = MacHeTriggerTxQueueSnapshot {
            logical_queue: 2,
            tid: 0,
            trigger_based_enabled: true,
            mu_edca_timer_select: 2,
            mu_edca_timer_enabled: true,
            first_link: 0,
            first_mpdu_length: 112,
            first_next_link: 1,
            tail_link: 1,
            programmed_msdu_bytes: 161,
            queued_msdu_bytes: 161,
            queue_valid: true,
        };
        {
            let storage = storage.as_mut().project();
            *storage.state = TxSlotState::HardwareOwned;
            *storage.queue = LegacyTxQueue::BestEffort;
            *storage.trigger_reservation = Some(reservation);
            *storage.trigger_publication_snapshot = Some(snapshot);
        }
        let mut hardware = CompletionHardware {
            completion: Some(MacHtAmpduCompletionRegisters {
                tx: MacTxCompletionRegisters {
                    aux_a: 0,
                    aux_b: 0,
                    aux_c: 0,
                    primary: 0,
                    alternate: 0,
                    trigger_flow: false,
                },
                block_ack_control_and_sequence: 0,
                block_ack_bitmap_low: 0,
                block_ack_bitmap_high: 0,
            }),
            cleared: None,
            trigger_snapshot: Some(snapshot),
        };

        assert_eq!(
            storage
                .as_ref()
                .he_trigger_based_snapshot(&hardware, cookie),
            Ok(Some(snapshot))
        );
        assert!(
            storage
                .as_mut()
                .acknowledge_completion(&mut hardware)
                .unwrap()
                .is_some()
        );
        assert_eq!(hardware.cleared, Some(reservation));
        assert_eq!(
            storage
                .as_ref()
                .he_trigger_based_snapshot(&hardware, cookie),
            Err(HtAmpduTxError::Stale)
        );
    }

    #[test]
    fn incremental_pool_length_matches_blob_accumulator_for_full_window() {
        let storage = HtAmpduTxStorage::<32, 256>::new();
        let mut storage = core::pin::pin!(storage);
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
        let storage = HtAmpduTxStorage::<2, 256>::new();
        let mut storage = core::pin::pin!(storage);
        let cookie = storage.as_mut().begin().unwrap();
        storage.as_mut().next_frame_buffer(cookie).unwrap()[..32].fill(0x5a);
        storage.as_mut().commit_frame(cookie, 32, 8, 0).unwrap();

        // Model the two hardware ownership edges independently: completion
        // alone must not expose a buffer that the queue still references.
        *storage.as_mut().project().state = TxSlotState::Completed;
        assert_eq!(
            storage.completed_frame(cookie, 0),
            Err(HtAmpduTxError::Stale)
        );
        *storage.as_mut().project().detached = true;
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
        let storage = HtAmpduTxStorage::<4, 256>::new();
        let mut storage = core::pin::pin!(storage);
        let cookie = storage.as_mut().begin().unwrap();
        for index in 0..4_u8 {
            let frame = storage.as_mut().next_frame_buffer(cookie).unwrap();
            frame[..32].fill(index);
            frame[1] = 0x41;
            storage.as_mut().commit_frame(cookie, 32, 8, 0).unwrap();
        }
        {
            let storage = storage.as_mut().project();
            *storage.state = TxSlotState::Completed;
            *storage.detached = true;
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
            let first = core::slice::from_raw_parts(
                storage.buffer_addresses[0] as *const u8,
                usize::from(storage.descriptor_capacities[0]),
            );
            let second = core::slice::from_raw_parts(
                storage.buffer_addresses[1] as *const u8,
                usize::from(storage.descriptor_capacities[1]),
            );
            assert_eq!(first[TX_AMPDU_METADATA_SIZE], 1);
            assert_eq!(second[TX_AMPDU_METADATA_SIZE], 3);
            assert_eq!(first[TX_AMPDU_METADATA_SIZE + 1], 0x49);
            assert_eq!(second[TX_AMPDU_METADATA_SIZE + 1], 0x49);
        }
        storage.as_mut().cancel(cookie).unwrap();
    }

    #[test]
    fn detached_he_pool_retains_one_missing_mpdu_at_the_original_rate() {
        let storage = HtAmpduTxStorage::<2, 256>::new();
        let mut storage = core::pin::pin!(storage);
        let cookie = storage.as_mut().begin().unwrap();
        let initial = HeRate::ldpc(
            crate::tx::HeMcs::Mcs9,
            crate::rx::HeGuardIntervalAndLtf::TwoLtf1600Ns,
        );
        storage.as_mut().next_frame_buffer(cookie).unwrap()[..32].fill(0x5a);
        storage
            .as_mut()
            .commit_he_frame(cookie, 32, 8, initial, HtAmpduDensity::EightMicroseconds)
            .unwrap();
        {
            let storage = storage.as_mut().project();
            *storage.state = TxSlotState::Completed;
            *storage.detached = true;
        }

        let retained = storage
            .as_mut()
            .retain_for_ampdu_retry(cookie, 0b1)
            .unwrap();
        assert_eq!(retained.subframes, 1);
        let buffer = &storage.as_ref().get_ref().buffers[0].0;
        assert_eq!(buffer[TX_AMPDU_METADATA_SIZE + 1] & 0x08, 0x08);

        assert_eq!(
            storage.prepared_empty_delimiters(cookie, 0).unwrap(),
            initial
                .ampdu_empty_delimiters(32 + 8 + TX_FCS_SIZE, HtAmpduDensity::EightMicroseconds)
                .unwrap()
        );
        storage.as_mut().cancel(cookie).unwrap();
    }

    #[test]
    fn full_window_and_byte_ceiling_are_independent() {
        let storage = HtAmpduTxStorage::<32, 1700>::new();
        let mut storage = core::pin::pin!(storage);
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
    fn he_rate_duration_gate_prevents_an_oversized_dma_publication() {
        let storage = HtAmpduTxStorage::<32, 1_600>::new();
        let mut storage = core::pin::pin!(storage);
        storage
            .as_mut()
            .configure_max_aggregate_bytes(u16::MAX)
            .unwrap();
        let density = HtAmpduDensity::NoRestriction;
        let gi_1600 = HeRate::ldpc(
            crate::tx::HeMcs::Mcs9,
            crate::rx::HeGuardIntervalAndLtf::TwoLtf1600Ns,
        );
        let cookie = storage.as_mut().begin().unwrap();
        for _ in 0..31 {
            assert!(
                storage
                    .can_commit_he_frame(cookie, 1_500, 8, gi_1600, density)
                    .unwrap()
            );
            storage
                .as_mut()
                .commit_he_frame(cookie, 1_500, 8, gi_1600, density)
                .unwrap();
        }
        assert_eq!(storage.prepared_aggregate(cookie).unwrap().bytes, 46_996);
        assert!(
            !storage
                .can_commit_he_frame(cookie, 1_500, 8, gi_1600, density)
                .unwrap()
        );
        assert_eq!(
            storage
                .as_mut()
                .commit_he_frame(cookie, 1_500, 8, gi_1600, density),
            Err(HtAmpduTxError::AggregateFull)
        );
        storage.as_mut().cancel(cookie).unwrap();

        let gi_800 = HeRate::ldpc(
            crate::tx::HeMcs::Mcs9,
            crate::rx::HeGuardIntervalAndLtf::TwoLtf800Ns,
        );
        let cookie = storage.as_mut().begin().unwrap();
        for _ in 0..32 {
            storage
                .as_mut()
                .commit_he_frame(cookie, 1_500, 8, gi_800, density)
                .unwrap();
        }
        assert_eq!(storage.prepared_aggregate(cookie).unwrap().bytes, 48_512);
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
    fn shared_dialog_tokens_reproduce_the_vendor_three_tid_order_and_modulus() {
        let mut tokens = TxBlockAckDialogTokenSequence::new();
        assert_eq!(tokens.take().value(), 1);
        assert_eq!(tokens.take().value(), 2);
        assert_eq!(tokens.take().value(), 3);

        tokens.next = 62;
        assert_eq!(tokens.take().value(), 62);
        assert_eq!(tokens.take().value(), 0);
        assert_eq!(tokens.take().value(), 1);
    }

    #[test]
    fn station_sessions_own_vendor_tid_order_response_routing_and_alarms() {
        let mut sessions = StaTxBlockAckSessions::new(32, 100_000, true).unwrap();
        let tid0 = sessions.begin(0, 0x100, 0).unwrap();
        let tid7 = sessions.begin(7, 0x200, 0).unwrap();
        let tid5 = sessions.begin(5, 0x300, 0).unwrap();
        assert_eq!(
            [tid0.dialog_token, tid7.dialog_token, tid5.dialog_token],
            [1, 2, 3]
        );
        assert_eq!(sessions.alarm(0), Some(tid0.alarm));
        assert_eq!(sessions.alarm(7), Some(tid7.alarm));
        assert_eq!(sessions.alarm(5), Some(tid5.alarm));

        let parameters = encode_ba_parameters(7, 16, false).to_le_bytes();
        let response = [
            3,
            1,
            tid7.dialog_token,
            0,
            0,
            parameters[0],
            parameters[1],
            0,
            0,
        ];
        assert_eq!(
            sessions.on_response(&response),
            Ok(StaTxBlockAckResponse {
                tid: 7,
                response: TxBlockAckResponse::Operational(OperationalTxBlockAck {
                    tid: 7,
                    window: 16,
                    timeout_tu: 0,
                    starting_sequence: 0x200,
                    amsdu: false,
                }),
            })
        );
        assert_eq!(sessions.alarm(7), None);
        assert_eq!(sessions.expire_next(100_000), Some(0));
        assert_eq!(sessions.expire_next(100_000), Some(5));
        assert_eq!(sessions.expire_next(100_000), None);
        assert!(sessions.operational(7).is_some());
    }

    #[test]
    fn parsed_response_can_cross_the_staged_rx_ownership_boundary() {
        let mut sessions = StaTxBlockAckSessions::new(32, 100_000, true).unwrap();
        let request = sessions.begin(0, 0x123, 0).unwrap();

        assert_eq!(
            sessions.on_response_action(BlockAckAction::AddbaResponse {
                dialog_token: request.dialog_token,
                status: 0,
                tid: 0,
                immediate: true,
                amsdu: true,
                window: 16,
                timeout_tu: 7,
            }),
            Ok(StaTxBlockAckResponse {
                tid: 0,
                response: TxBlockAckResponse::Operational(OperationalTxBlockAck {
                    tid: 0,
                    window: 16,
                    timeout_tu: 7,
                    starting_sequence: 0x123,
                    amsdu: true,
                }),
            })
        );
        assert_eq!(sessions.alarm(0), None);
    }

    #[test]
    fn station_sessions_reject_unowned_tid_and_stale_dialog_token() {
        let mut sessions = StaTxBlockAckSessions::new(32, 100_000, false).unwrap();
        assert_eq!(
            sessions.begin(3, 0, 0),
            Err(StaTxBlockAckSessionsError::UnsupportedTid(3))
        );
        assert_eq!(
            sessions.on_response(&[3, 1, 42, 0, 0, 0, 0, 0, 0]),
            Err(StaTxBlockAckSessionsError::UnexpectedDialogToken(42))
        );
        assert!(!sessions.stop(3));
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
