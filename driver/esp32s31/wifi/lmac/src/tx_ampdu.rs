//! ESP32-S31 A-MPDU formatting, lifecycle and retained-DMA composition.
//!
//! Generic IEEE 802.11 BlockAck negotiation lives in the portable protocol
//! crate. The [`block_ack`] adapter retains only chip policy and completion
//! geometry. This facade preserves the existing public API while the
//! formatter itself owns no timer, executor or management-frame policy.

#![forbid(unsafe_code)]

use core::{marker::PhantomPinned, pin::Pin};

use open_esp_radio_esp32s31_registers::{
    MacHeTbLinkReservation, MacHeTbProgramError, MacHeTriggerTxQueueSnapshot,
};
use open_esp_radio_esp32s31_wifi_dma::tx_ampdu_storage::AmpduDmaStorageError;
use pin_project::pin_project;

pub mod block_ack;
mod hardware;
mod length;
mod lifecycle;
#[cfg(not(target_pointer_width = "32"))]
mod model;
mod owner;
mod request;
mod retry;
mod submission;

pub use block_ack::{
    ADDBA_ACTION_BODY_LEN, ADDBA_REQUEST_ACTION, ADDBA_RESPONSE_ACTION, AddbaRequest,
    BLOCK_ACK_CATEGORY, BlockAckAction, DELBA_ACTION, HtBlockAckRegisters, OperationalTxBlockAck,
    STA_TX_BLOCK_ACK_TIDS, StaTxBlockAckResponse, StaTxBlockAckSessions,
    StaTxBlockAckSessionsError, TX_AMPDU_SLOT_CAPACITY, TX_BLOCK_ACK_MAX_WINDOW, TxAmpduBatch,
    TxAmpduBatchError, TxAmpduCompletion, TxAmpduDisposition, TxAmpduMpdu, TxAmpduSlot,
    TxBlockAckAlarm, TxBlockAckBitmap, TxBlockAckConfig, TxBlockAckDialogToken,
    TxBlockAckDialogTokenSequence, TxBlockAckError, TxBlockAckResponse, TxBlockAckSession,
    decode_ht_block_ack_registers, parse_block_ack_action,
};
pub use hardware::HtAmpduHardware;
pub use length::{HtAmpduLength, HtAmpduLengthAccumulator, HtAmpduLengthError};
#[cfg(not(target_pointer_width = "32"))]
pub use model::{
    BasicHtAmpduAssemblyError, BasicHtAmpduAssemblyInput, BasicHtAmpduAssemblyOutput,
    BasicHtAmpduCompletionInput, BasicHtAmpduCompletionOutput, basic_ht_ampdu_assembly,
    basic_ht_ampdu_completion,
};
#[cfg(test)]
pub(crate) use model::{
    TX_BUFFER_END_BIT, TX_DESCRIPTOR_HE_BIT, basic_ht_ampdu_protection_spacing,
};
pub use owner::{HtAmpduTxResources, RetainedDmaAmpduTx};
pub use request::{
    AmpduFrameLayout, AmpduFrameSize, HeAmpduFrameRequest, HeAmpduPolicy, HtAmpduFrameRequest,
};

#[cfg(not(target_pointer_width = "32"))]
use crate::tx::{HeEdcaTxopLimit, HeRate, HtAmpduDensity};
use crate::tx::{HtRate, LegacyTxQueue, TxCompletion, TxCookie, TxSlotState};
use length::HARDWARE_HE_CONTROL_LENGTH;

pub const TX_AMPDU_METADATA_SIZE: usize = 8;
const TX_FCS_SIZE: u16 = 4;
const TX_AMPDU_DEFAULT_MAX_BYTES: u16 = 0x1fff;

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
    /// Stored descriptor metadata did not resolve to any currently retained
    /// stable backing or to the storage's own fixed DMA buffer.
    BackingUnavailable {
        index: u8,
    },
    SlotCountOverflow {
        count: usize,
    },
    InvalidStorageGeometry {
        slots: usize,
        buffer_size: usize,
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
    /// The lower descriptor/backing owner rejected a lifecycle or range edge.
    DmaStorage(AmpduDmaStorageError),
}

impl From<AmpduDmaStorageError> for HtAmpduTxError {
    fn from(error: AmpduDmaStorageError) -> Self {
        Self::DmaStorage(error)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HtAmpduTxFormat {
    HtAmpdu,
    HeAmpdu,
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
    /// SOURCE\[HIL_OPEN_HT_AMPDU_PARTIAL_2026_07_29]: live HT40 MCS7 SGI
    /// four-stream TX load produced successful partial BlockAck completions
    /// and status-five completions with stale nonzero bitmap words.
    pub const fn acknowledges(self, sequence: u16) -> bool {
        self.tx.status == 0 && self.block_ack.block_ack.acknowledges(sequence)
    }
}

#[cfg(not(target_pointer_width = "32"))]
#[repr(C, align(16))]
struct HtAmpduDmaBuffer<const BUFFER_SIZE: usize>([u8; BUFFER_SIZE]);

#[cfg(not(target_pointer_width = "32"))]
impl<const BUFFER_SIZE: usize> HtAmpduDmaBuffer<BUFFER_SIZE> {
    const fn new() -> Self {
        Self([0; BUFFER_SIZE])
    }
}

/// Pinned A-MPDU protocol state and bounded frame-formatting workspace.
///
/// This type records aggregate layout, completion state and retained frame
/// metadata. It cannot publish a descriptor chain or start hardware by itself:
/// production submission requires [`HtAmpduTxResources`], which couples this
/// state to the lower DMA crate's descriptor/backing owner. Native models also
/// retain fixed formatter buffers for oracle tests; those buffers and their
/// upper-only APIs are absent from 32-bit production builds.
///
/// SOURCE\[HIL_OPEN_HT_AMPDU_DIRECT_2026_07_29]: ESP32-S31 rev0,
/// psram-code-psram-data, open PHY/MAC, HT40 MCS7 SGI. Four observed
/// two-MPDU submissions from this pool each returned a BlockAck bitmap ending
/// in `0x000f`, with no aggregate hardware timeout.
#[pin_project]
pub struct HtAmpduTxStorage<const SLOTS: usize, const BUFFER_SIZE: usize> {
    #[cfg(not(target_pointer_width = "32"))]
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

// Production metadata size must not scale with the host-only formatter
// capacity. This is a compile-time target-layout assertion, not a runtime
// diagnostic.
#[cfg(target_pointer_width = "32")]
const _: [(); core::mem::size_of::<HtAmpduTxStorage<8, 0>>()] =
    [(); core::mem::size_of::<HtAmpduTxStorage<8, 4096>>()];

impl<const SLOTS: usize, const BUFFER_SIZE: usize> HtAmpduTxStorage<SLOTS, BUFFER_SIZE> {
    pub const fn new() -> Self {
        Self {
            #[cfg(not(target_pointer_width = "32"))]
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
    #[cfg(not(target_pointer_width = "32"))]
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
    #[cfg(not(target_pointer_width = "32"))]
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
    fn can_commit_referenced_frame(
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
    #[cfg(not(target_pointer_width = "32"))]
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

    #[cfg(not(target_pointer_width = "32"))]
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
    #[cfg(not(target_pointer_width = "32"))]
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
    #[cfg(not(target_pointer_width = "32"))]
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

    /// Check one referenced HE frame against allocation, APEP and TXOP limits.
    pub fn can_commit_referenced_he_frame(
        &self,
        cookie: TxCookie,
        frame_size: AmpduFrameSize,
        policy: HeAmpduPolicy,
        dma_capacity: usize,
    ) -> Result<bool, HtAmpduTxError> {
        let frame_length = frame_size.mpdu_length();
        let hardware_mic_length = frame_size.hardware_mic_length();
        let psdu_length = frame_length
            .checked_add(usize::from(hardware_mic_length))
            .and_then(|length| length.checked_add(usize::from(TX_FCS_SIZE)))
            .and_then(|length| u16::try_from(length).ok())
            .filter(|length| *length != 0 && *length <= 0x3fff)
            .ok_or(HtAmpduTxError::FrameTooLong)?;
        let _empty_delimiters = policy
            .rate()
            .ampdu_empty_delimiters(psdu_length, policy.density())
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
            <= policy.rate().maximum_apep_bytes(policy.txop_limit()))
    }

    /// Check one referenced HE frame against a fresh aggregate without
    /// changing descriptor ownership.
    pub fn can_fit_fresh_referenced_he_frame(
        &self,
        frame_size: AmpduFrameSize,
        policy: HeAmpduPolicy,
        maximum_aggregate_bytes: u16,
        dma_capacity: usize,
    ) -> Result<bool, HtAmpduTxError> {
        let psdu_length = Self::fresh_referenced_psdu_length(
            frame_size.mpdu_length(),
            frame_size.hardware_mic_length(),
            dma_capacity,
        )?;
        let _empty_delimiters = policy
            .rate()
            .ampdu_empty_delimiters(psdu_length, policy.density())
            .ok_or(HtAmpduTxError::FrameTooLong)?;
        let aggregate_length = 4_u32 + u32::from(psdu_length);
        Ok(maximum_aggregate_bytes != 0
            && aggregate_length <= u32::from(maximum_aggregate_bytes)
            && aggregate_length <= policy.rate().maximum_apep_bytes(policy.txop_limit()))
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
    #[cfg(not(target_pointer_width = "32"))]
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
    #[cfg(not(target_pointer_width = "32"))]
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
    #[cfg(not(target_pointer_width = "32"))]
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
    /// The private entry is reachable only through [`RetainedDmaAmpduTx`],
    /// which retains exclusive ownership of the same stable allocation until
    /// this batch has completed, detached and released or cancelled.
    ///
    /// SOURCE: complete `libnet80211.a[ieee80211_output.o]::
    /// ieee80211_alloc_tx_buf` cache-TX/type-nine branch retains the netstack
    /// buffer through `s_netstack_ref`; complete `libpp.a[pp.o]::
    /// ppAssembleAMPDU` links the existing ESF descriptors without copying
    /// their payloads.
    fn commit_referenced_frame(
        self: Pin<&mut Self>,
        cookie: TxCookie,
        dma_storage: &mut [u8],
        layout: AmpduFrameLayout,
        empty_delimiters: u8,
    ) -> Result<(), HtAmpduTxError> {
        let frame_length = layout.mpdu_length();
        let hardware_mic_length = layout.hardware_mic_length();
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
    /// The retained owner upholds the same stable-allocation invariant as
    /// [`Self::commit_referenced_frame`].
    fn commit_referenced_ht_frame(
        mut self: Pin<&mut Self>,
        cookie: TxCookie,
        dma_storage: &mut [u8],
        request: HtAmpduFrameRequest,
    ) -> Result<(), HtAmpduTxError> {
        let layout = request.layout();
        if !self.as_ref().can_commit_referenced_ht_frame(
            cookie,
            layout.mpdu_length(),
            layout.hardware_mic_length(),
            request.empty_delimiters(),
            request.rate(),
            dma_storage.len(),
        )? {
            return Err(HtAmpduTxError::AggregateFull);
        }
        self.as_mut().commit_referenced_frame(
            cookie,
            dma_storage,
            layout,
            request.empty_delimiters(),
        )
    }

    #[cfg(not(target_pointer_width = "32"))]
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
    #[cfg(not(target_pointer_width = "32"))]
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
    #[cfg(not(target_pointer_width = "32"))]
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

    /// Commit one referenced HE MPDU under the complete rate/TXOP policy.
    ///
    /// SOURCE: complete `libpp.a[pp_he.o]::{ppCalDeliNum,
    /// ppCalTxHEAMPDULength,ppCheckTxHEAMPDUlength}` supplies delimiter and
    /// duration accounting. Complete
    /// `libnet80211.a[ieee80211_output.o]::ieee80211_alloc_tx_buf`
    /// and `libpp.a[pp.o]::ppAssembleAMPDU` retain and link the
    /// cache-TX allocation instead of copying it into aggregate storage.
    ///
    /// The retained owner keeps `dma_storage` at its current address until the
    /// batch is detached and released or cancelled.
    fn commit_referenced_he_frame(
        mut self: Pin<&mut Self>,
        cookie: TxCookie,
        dma_storage: &mut [u8],
        request: HeAmpduFrameRequest,
    ) -> Result<(), HtAmpduTxError> {
        let layout = request.layout();
        if !self.as_ref().can_commit_referenced_he_frame(
            cookie,
            layout.frame_size(),
            request.policy(),
            dma_storage.len(),
        )? {
            return Err(HtAmpduTxError::AggregateFull);
        }
        let psdu_length = layout
            .mpdu_length()
            .checked_add(usize::from(layout.hardware_mic_length()))
            .and_then(|length| length.checked_add(usize::from(TX_FCS_SIZE)))
            .and_then(|length| u16::try_from(length).ok())
            .filter(|length| *length != 0 && *length <= 0x3fff)
            .ok_or(HtAmpduTxError::FrameTooLong)?;
        let policy = request.policy();
        let empty_delimiters = policy
            .rate()
            .ampdu_empty_delimiters(psdu_length, policy.density())
            .ok_or(HtAmpduTxError::FrameTooLong)?;
        self.as_mut()
            .commit_referenced_frame(cookie, dma_storage, layout, empty_delimiters)
    }

    /// Commit one HE MPDU with a hardware-inserted HE-Control field.
    ///
    /// Unlike the former placeholder workaround, this does not move CCMP or
    /// increase the low fourteen-bit DMA MPDU length. It publishes the exact
    /// vendor metadata byte-seven bit and adds four only to assembled APEP
    /// accounting.
    #[cfg(not(target_pointer_width = "32"))]
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
    #[cfg(not(target_pointer_width = "32"))]
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
    #[cfg(not(target_pointer_width = "32"))]
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
    #[cfg(not(target_pointer_width = "32"))]
    pub fn commit_hardware_he_control_msdu_frame(
        self: Pin<&mut Self>,
        cookie: TxCookie,
        frame_size: AmpduFrameSize,
        msdu_length: usize,
        rate: HeRate,
        density: HtAmpduDensity,
    ) -> Result<(), HtAmpduTxError> {
        self.commit_hardware_he_control_msdu_frame_with_policy(
            cookie,
            frame_size,
            msdu_length,
            HeAmpduPolicy::new(rate, density, HeEdcaTxopLimit::DEFAULT),
        )
    }

    /// Commit Trigger metadata and HE-Control under one HE duration policy.
    #[cfg(not(target_pointer_width = "32"))]
    pub fn commit_hardware_he_control_msdu_frame_with_policy(
        mut self: Pin<&mut Self>,
        cookie: TxCookie,
        frame_size: AmpduFrameSize,
        msdu_length: usize,
        policy: HeAmpduPolicy,
    ) -> Result<(), HtAmpduTxError> {
        let msdu_length = u16::try_from(msdu_length)
            .ok()
            .filter(|length| *length != 0)
            .ok_or(HtAmpduTxError::TriggerMsduLengthUnavailable)?;
        let index = usize::from(self.count);
        self.as_mut().commit_hardware_he_control_frame_with_txop(
            cookie,
            frame_size.mpdu_length(),
            frame_size.hardware_mic_length(),
            policy.rate(),
            policy.density(),
            policy.txop_limit(),
        )?;
        self.project().msdu_lengths[index] = msdu_length;
        Ok(())
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
}

impl<const SLOTS: usize, const BUFFER_SIZE: usize> Default
    for HtAmpduTxStorage<SLOTS, BUFFER_SIZE>
{
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
