//! Fixed receive staging ownership recovered from the vendor ESF path.
//!
//! The vendor implementation does not retain Wi-Fi DMA buffers while upper
//! layers parse a frame. Complete `libpp.a[wdev.o]::wDev_IndicateFrame`
//! allocates a kind-7 ESF object, copies the completed RX unit, calls
//! `wDev_DiscardFrame`, and only then calls `lmacRxDone`. Complete
//! `libpp.a[lmac.o]::lmacRxDone` enqueues that independent object and
//! posts PP event 17.
//!
//! Complete `libpp.a[esf_buf.o]::esf_buf_setup` and
//! `esf_buf_alloc_dynamic` establish the ordinary large-RX profile: 32
//! kind-7 objects backed by internal memory. The vendor ABI uses a 0x90-byte
//! header plus a 1,700-byte payload. This module preserves the exact
//! `Free -> Radio -> Network -> Free` ownership and 32-by-1,700 geometry,
//! while omitting the C-only ESF header and intrusive pointers.

use core::marker::PhantomData;

use crate::rx::{RxPhyInfo, decode_normalized_rx_metadata};
use open_esp_radio_dma::{RxHandoffPool, RxNetworkLease, RxRadioLease};
#[cfg(any(not(target_pointer_width = "32"), feature = "validation-raw-dma"))]
use open_esp_radio_esp32s31_wifi_dma::descriptor::length as descriptor_length;
#[cfg(any(not(target_pointer_width = "32"), feature = "validation-raw-dma"))]
use open_esp_radio_esp32s31_wifi_dma::rx_ring::RxCompletedDescriptor;
use open_esp_radio_esp32s31_wifi_dma::{
    rx_dma::RxDma,
    rx_ring::{RxCompletedUnit, RxRingError, RxRingLive, RxSegment},
    rx_storage::RxDmaCompletedUnit,
};
use open_esp_radio_wifi_softmac::MacRxMetadata;

pub const VENDOR_LARGE_RX_SLOT_COUNT: usize = 32;
pub const VENDOR_LARGE_RX_PAYLOAD_CAPACITY: usize = 1_700;
const RX_STAGE_MAX_SLOTS: usize = 2 * usize::BITS as usize;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RxStageError {
    InvalidPool,
    Exhausted,
    Empty,
    TooLong,
    SourceTooShort,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RxStageTransactionError {
    Stage(RxStageError),
    Ring(RxRingError),
    Ownership,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RxInPlaceEthernetError {
    PayloadOutsideFrame,
    InsufficientHeadroom,
}

/// Result after a completed DMA unit has been returned to the live walker.
pub enum RxDmaStageUnitOutcome<'pool, const SLOTS: usize, const CAPACITY: usize> {
    Staged(RxStageReloadPending<'pool, SLOTS, CAPACITY>),
    /// Malformed length metadata was discarded after descriptor recycle.
    Discarded(RxStageError),
}

/// Result after a completed DMA unit has been copied while its descriptor is
/// retained for the service call's frozen-LAST reclaim.
pub enum RxDmaDeferredStageUnitOutcome<'pool, const SLOTS: usize, const CAPACITY: usize> {
    Staged(NetworkRxFrame<'pool, SLOTS, CAPACITY>),
    /// Malformed length metadata was discarded while the descriptor remained
    /// in the CPU-owned observed prefix.
    Discarded(RxStageError),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct StagedMetadata {
    descriptor_address: u32,
    descriptor_word0: u32,
    next_descriptor_address: u32,
    length: usize,
}

#[cfg(any(not(target_pointer_width = "32"), feature = "validation-raw-dma"))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CompletedDescriptorObservation {
    descriptor_address: u32,
    descriptor_word0: u32,
    next_descriptor_address: u32,
}

#[cfg(any(not(target_pointer_width = "32"), feature = "validation-raw-dma"))]
impl From<RxCompletedDescriptor> for CompletedDescriptorObservation {
    fn from(completed: RxCompletedDescriptor) -> Self {
        Self {
            descriptor_address: completed.descriptor_address(),
            descriptor_word0: completed.word0(),
            next_descriptor_address: completed.next_descriptor_address(),
        }
    }
}

/// Fixed staging storage with explicit vendor-equivalent ownership states.
///
/// Place `RxStagePool<32, 1700>` in internal SRAM. It is ordinary CPU-owned
/// memory and must never be published to the Wi-Fi DMA walker. Two native
/// machine words define the qualified upper bound, permitting a platform
/// profile to add burst elasticity beyond the 32-slot vendor default without
/// requiring 64-bit atomics on RV32.
pub struct RxStagePool<const SLOTS: usize, const CAPACITY: usize> {
    storage: RxHandoffPool<CAPACITY, SLOTS>,
}

impl<const SLOTS: usize, const CAPACITY: usize> RxStagePool<SLOTS, CAPACITY> {
    pub const fn new() -> Self {
        Self {
            storage: RxHandoffPool::new(),
        }
    }

    #[cfg(any(not(target_pointer_width = "32"), feature = "validation-raw-dma"))]
    fn try_stage<'pool>(
        &'pool self,
        completed: CompletedDescriptorObservation,
        source: &[u8],
    ) -> Result<RadioRxFrame<'pool, SLOTS, CAPACITY>, RxStageError> {
        if SLOTS == 0 || SLOTS > RX_STAGE_MAX_SLOTS || CAPACITY == 0 {
            return Err(RxStageError::InvalidPool);
        }
        let length = usize::try_from(descriptor_length(completed.descriptor_word0))
            .map_err(|_| RxStageError::TooLong)?;
        if length == 0 {
            return Err(RxStageError::Empty);
        }
        if length > CAPACITY {
            return Err(RxStageError::TooLong);
        }
        let source = source.get(..length).ok_or(RxStageError::SourceTooShort)?;
        let mut lease = self
            .storage
            .try_claim_radio()
            .ok_or(RxStageError::Exhausted)?;
        lease.frame_prefix_mut(length).copy_from_slice(source);
        Ok(RadioRxFrame {
            lease: Some(lease),
            _slots: PhantomData,
            metadata: StagedMetadata {
                descriptor_address: completed.descriptor_address,
                descriptor_word0: completed.descriptor_word0,
                next_descriptor_address: completed.next_descriptor_address,
                length,
            },
        })
    }

    fn try_stage_unit<'pool, F>(
        &'pool self,
        unit: &RxCompletedUnit,
        maximum_length: usize,
        mut copy_segment: F,
    ) -> Result<RadioRxFrame<'pool, SLOTS, CAPACITY>, RxStageError>
    where
        F: FnMut(usize, &mut [u8]) -> Result<(), RxStageError>,
    {
        if SLOTS == 0 || SLOTS > RX_STAGE_MAX_SLOTS || CAPACITY == 0 {
            return Err(RxStageError::InvalidPool);
        }
        let length = unit.total_length();
        if length == 0 {
            return Err(RxStageError::Empty);
        }
        if length > CAPACITY.min(maximum_length) {
            return Err(RxStageError::TooLong);
        }
        let mut lease = self
            .storage
            .try_claim_radio()
            .ok_or(RxStageError::Exhausted)?;

        let copied = (|| {
            let mut offset = 0_usize;
            for step in 0..unit.descriptor_count() {
                let segment_length = unit
                    .segment_length(step)
                    .ok_or(RxStageError::SourceTooShort)?;
                let end = offset
                    .checked_add(segment_length)
                    .filter(|&end| end <= length)
                    .ok_or(RxStageError::SourceTooShort)?;
                let destination = &mut lease.frame_prefix_mut(length)[offset..end];
                copy_segment(step, destination)?;
                offset = end;
            }
            (offset == length)
                .then_some(())
                .ok_or(RxStageError::SourceTooShort)
        })();
        copied?;

        Ok(RadioRxFrame {
            lease: Some(lease),
            _slots: PhantomData,
            metadata: StagedMetadata {
                descriptor_address: unit.descriptor_address(),
                descriptor_word0: unit.staged_word0(),
                next_descriptor_address: 0,
                length,
            },
        })
    }

    open_esp_radio_esp32s31_wifi_dma::place_rx_hot_path! {
    /// Copy one completed DMA frame and begin returning its descriptor to
    /// hardware.
    ///
    /// This is the complete ownership order in
    /// `libpp.a[wdev.o]::wDev_IndicateFrame`: allocate/copy,
    /// `wDev_DiscardFrame`/`wDev_AppendRxBlocks`, then `lmacRxDone`. Neither
    /// protocol parsing nor the network queue is allowed to retain the DMA
    /// descriptor. The caller supplies only the storage-specific rearm
    /// operation. The returned [`RxStageReloadPending`] owns the copied frame
    /// until the caller observes reload completion with an async scheduling
    /// edge between pending samples.
    ///
    /// SOURCE\[HIL_OPEN_HE20_RX_OWNERSHIP_2026_07_30]: in the
    /// `psram-code-psram-data` profile, two reset-separated bidirectional
    /// HE20/MCS9 runs completed with `BUFFER_FULL=0` and `FIFO_OVERFLOW=0`.
    /// The proof run performed 5,147 RX-priority scheduling handoffs during
    /// TX preparation and delivered 10.036-Mbit/s RX plus 67.942-Mbit/s TX.
    /// The preceding parse-before-recycle path produced `BUFFER_FULL`.
    #[inline(never)]
    #[cfg(any(not(target_pointer_width = "32"), feature = "validation-raw-dma"))]
    pub fn stage_recycle<'pool, const COUNT: usize, M, F>(
        &'pool self,
        completed: RxCompletedDescriptor,
        source: &[u8],
        mmio: &mut M,
        ring: &mut RxRingLive<'_, COUNT>,
        prepare_buffer: F,
    ) -> Result<RxStageReloadPending<'pool, SLOTS, CAPACITY>, RxStageTransactionError>
    where
        M: RxDma,
        F: FnMut(usize) -> Result<(), RxRingError>,
    {
        let radio_frame = self
            .try_stage(completed.into(), source)
            .map_err(RxStageTransactionError::Stage)?;
        let append = ring
            .recycle_completed_prefix::<1, _, _>(mmio, prepare_buffer)
            .map_err(RxStageTransactionError::Ring)?
            .ok_or(RxStageTransactionError::Ring(RxRingError::Busy))?;
        if append.descriptor_count != 1 {
            return Err(RxStageTransactionError::Ring(RxRingError::Corrupt));
        }
        Ok(RxStageReloadPending {
            frame: Some(radio_frame),
        })
    }}

    /// Copy one complete RX unit, then atomically recycle all DMA descriptors
    /// that carried it. `copy_segment` receives the zero-based unit segment
    /// and an exactly sized destination range in the independent stage slot.
    #[cfg(any(not(target_pointer_width = "32"), feature = "validation-raw-dma"))]
    pub fn stage_unit_recycle<'pool, const COUNT: usize, M, C, F>(
        &'pool self,
        unit: RxCompletedUnit,
        mut copy_segment: C,
        mmio: &mut M,
        ring: &mut RxRingLive<'_, COUNT>,
        prepare_buffer: F,
    ) -> Result<RxStageReloadPending<'pool, SLOTS, CAPACITY>, RxStageTransactionError>
    where
        M: RxDma,
        C: FnMut(usize, &mut [u8]) -> Result<(), RxStageError>,
        F: FnMut(usize) -> Result<(), RxRingError>,
    {
        let descriptor_count = unit.descriptor_count();
        let radio_frame = self
            .try_stage_unit(&unit, CAPACITY, |step, destination| {
                copy_segment(step, destination)
            })
            .map_err(RxStageTransactionError::Stage)?;
        let append = ring
            .recycle_completed_unit(mmio, descriptor_count, prepare_buffer)
            .map_err(RxStageTransactionError::Ring)?
            .ok_or(RxStageTransactionError::Ring(RxRingError::Busy))?;
        if append.descriptor_count != descriptor_count {
            return Err(RxStageTransactionError::Ring(RxRingError::Corrupt));
        }
        Ok(RxStageReloadPending {
            frame: Some(radio_frame),
        })
    }

    /// Copy a storage-bound completed unit and recycle its descriptors.
    ///
    /// The completion token keeps the live ring mutably borrowed throughout
    /// the copy. Empty and oversized units follow the vendor discard path:
    /// they are not published to the protocol queue, but their descriptors
    /// are still promptly returned to DMA.
    pub fn stage_dma_unit_recycle<
        'pool,
        const COUNT: usize,
        const BUFFER_SIZE: usize,
        const STORAGE_SIZE: usize,
        M: RxDma,
    >(
        &'pool self,
        completed: RxDmaCompletedUnit<'_, '_, COUNT, BUFFER_SIZE, STORAGE_SIZE>,
        mmio: &mut M,
    ) -> Result<RxDmaStageUnitOutcome<'pool, SLOTS, CAPACITY>, RxStageTransactionError> {
        self.stage_dma_unit_recycle_bounded(completed, mmio, CAPACITY)
    }

    /// Copy and recycle one completed unit subject to a runtime admission
    /// limit no wider than this pool's physical slot capacity.
    ///
    /// A unit above the selected limit follows the same recoverable discard
    /// transaction as a unit above `CAPACITY`: its descriptors are recycled,
    /// no staging lease is published, and the caller must still observe the
    /// asynchronous reload edge. This lets an integration qualify or enforce
    /// a negotiated ingress bound without modifying untrusted descriptor
    /// fields or bypassing the real DMA ownership path.
    pub fn stage_dma_unit_recycle_bounded<
        'pool,
        const COUNT: usize,
        const BUFFER_SIZE: usize,
        const STORAGE_SIZE: usize,
        M: RxDma,
    >(
        &'pool self,
        completed: RxDmaCompletedUnit<'_, '_, COUNT, BUFFER_SIZE, STORAGE_SIZE>,
        mmio: &mut M,
        maximum_length: usize,
    ) -> Result<RxDmaStageUnitOutcome<'pool, SLOTS, CAPACITY>, RxStageTransactionError> {
        let descriptor_count = completed.descriptor_count();
        let staged =
            self.try_stage_unit(completed.metadata(), maximum_length, |step, destination| {
                let source = completed
                    .segment(step)
                    .ok_or(RxStageError::SourceTooShort)?;
                if source.len() != destination.len() {
                    return Err(RxStageError::SourceTooShort);
                }
                destination.copy_from_slice(source);
                Ok(())
            });

        let radio_frame = match staged {
            Ok(frame) => Some(frame),
            Err(error @ (RxStageError::Empty | RxStageError::TooLong)) => {
                let append = completed
                    .recycle(mmio)
                    .map_err(RxStageTransactionError::Ring)?
                    .ok_or(RxStageTransactionError::Ring(RxRingError::Busy))?;
                if append.descriptor_count != descriptor_count {
                    return Err(RxStageTransactionError::Ring(RxRingError::Corrupt));
                }
                return Ok(RxDmaStageUnitOutcome::Discarded(error));
            }
            Err(error) => return Err(RxStageTransactionError::Stage(error)),
        };

        let append = completed
            .recycle(mmio)
            .map_err(RxStageTransactionError::Ring)?
            .ok_or(RxStageTransactionError::Ring(RxRingError::Busy))?;
        if append.descriptor_count != descriptor_count {
            return Err(RxStageTransactionError::Ring(RxRingError::Corrupt));
        }
        Ok(RxDmaStageUnitOutcome::Staged(RxStageReloadPending {
            frame: radio_frame,
        }))
    }

    /// Copy one completed unit without immediately rewriting its descriptor.
    ///
    /// The independent frame can be published immediately, while descriptor
    /// ownership remains recorded in the ring until the producer returns this
    /// exact unit through its frozen-LAST proof. That append invalidates the
    /// cursor before any descriptor address can acquire a new generation.
    pub fn stage_dma_unit_deferred_bounded<
        'pool,
        const COUNT: usize,
        const BUFFER_SIZE: usize,
        const STORAGE_SIZE: usize,
    >(
        &'pool self,
        completed: RxDmaCompletedUnit<'_, '_, COUNT, BUFFER_SIZE, STORAGE_SIZE>,
        maximum_length: usize,
    ) -> Result<RxDmaDeferredStageUnitOutcome<'pool, SLOTS, CAPACITY>, RxStageTransactionError>
    {
        let staged =
            self.try_stage_unit(completed.metadata(), maximum_length, |step, destination| {
                let source = completed
                    .segment(step)
                    .ok_or(RxStageError::SourceTooShort)?;
                if source.len() != destination.len() {
                    return Err(RxStageError::SourceTooShort);
                }
                destination.copy_from_slice(source);
                Ok(())
            });

        match staged {
            Ok(frame) => {
                completed.retain_for_deferred_recycle();
                Ok(RxDmaDeferredStageUnitOutcome::Staged(frame.publish()))
            }
            Err(error @ (RxStageError::Empty | RxStageError::TooLong)) => {
                completed.retain_for_deferred_recycle();
                Ok(RxDmaDeferredStageUnitOutcome::Discarded(error))
            }
            Err(error) => Err(RxStageTransactionError::Stage(error)),
        }
    }

    pub fn claimed_slots(&self) -> u32 {
        self.storage.claimed_slots() as u32
    }

    /// Number of slots that can still accept an independent DMA copy.
    ///
    /// A Network-owned slot remains claimed until its unique token is dropped,
    /// so this is the complete producer credit rather than merely the number
    /// of entries absent from an intermediate queue.
    pub fn available_slots(&self) -> usize {
        SLOTS.saturating_sub(self.claimed_slots() as usize)
    }

    pub fn network_slots(&self) -> u32 {
        self.storage.network_slots() as u32
    }

    /// Shared handoff storage used by an upper network-device consumer.
    ///
    /// The pool still exposes no raw bytes: external users can only perform
    /// the same state-checked lease transitions as this staging wrapper.
    pub fn handoff_pool(&self) -> &RxHandoffPool<CAPACITY, SLOTS> {
        &self.storage
    }
}

/// Unique staged-frame owner while the RX append doorbell is pending.
///
/// Dropping this value discards only the independent staged copy. The DMA
/// descriptor has already been rearmed and remains owned by the ring's
/// pending-tail transaction.
#[must_use = "the staged frame must be polled to reload completion or explicitly dropped"]
pub struct RxStageReloadPending<'pool, const SLOTS: usize, const CAPACITY: usize> {
    frame: Option<RadioRxFrame<'pool, SLOTS, CAPACITY>>,
}

impl<'pool, const SLOTS: usize, const CAPACITY: usize>
    RxStageReloadPending<'pool, SLOTS, CAPACITY>
{
    /// Complete the exact vendor reload transaction before exposing the
    /// independent staged copy to the network.
    pub fn complete_reload<const COUNT: usize, M: RxDma>(
        &mut self,
        mmio: &mut M,
        ring: &mut RxRingLive<'_, COUNT>,
    ) -> Result<NetworkRxFrame<'pool, SLOTS, CAPACITY>, RxStageTransactionError> {
        if self.frame.is_none() {
            return Err(RxStageTransactionError::Ownership);
        }
        if let Err(error) = ring.complete_pending_reload(mmio) {
            self.frame.take();
            return Err(RxStageTransactionError::Ring(error));
        }
        let frame = self
            .frame
            .take()
            .ok_or(RxStageTransactionError::Ownership)?;
        Ok(frame.publish())
    }
}

impl<const SLOTS: usize, const CAPACITY: usize> Default for RxStagePool<SLOTS, CAPACITY> {
    fn default() -> Self {
        Self::new()
    }
}

/// Unique frame owner between the DMA copy and queue publication.
struct RadioRxFrame<'pool, const SLOTS: usize, const CAPACITY: usize> {
    lease: Option<RxRadioLease<'pool, CAPACITY>>,
    _slots: PhantomData<[(); SLOTS]>,
    metadata: StagedMetadata,
}

impl<'pool, const SLOTS: usize, const CAPACITY: usize> RadioRxFrame<'pool, SLOTS, CAPACITY> {
    /// Transfer the staged frame to the upper receive queue.
    fn publish(mut self) -> NetworkRxFrame<'pool, SLOTS, CAPACITY> {
        let lease = self
            .lease
            .take()
            .expect("live staged RX frame owns its radio lease")
            .into_network(self.metadata.length);
        NetworkRxFrame {
            lease,
            _slots: PhantomData,
            metadata: self.metadata,
        }
    }
}

/// Unique upper-layer owner of one staged receive frame.
pub struct NetworkRxFrame<'pool, const SLOTS: usize, const CAPACITY: usize> {
    lease: RxNetworkLease<'pool, CAPACITY>,
    _slots: PhantomData<[(); SLOTS]>,
    metadata: StagedMetadata,
}

impl<const SLOTS: usize, const CAPACITY: usize> NetworkRxFrame<'_, SLOTS, CAPACITY> {
    /// Stable index of this unique lease in its integration-owned pool.
    ///
    /// The index is metadata, not an address. It lets an allocation-free
    /// reorder state refer back to this token without exposing pool storage.
    pub fn slot(&self) -> usize {
        self.lease.index()
    }

    pub fn segment(&self) -> RxSegment<'_> {
        RxSegment {
            descriptor_address: self.metadata.descriptor_address,
            descriptor_word0: self.metadata.descriptor_word0,
            buffer: self.lease.frame(),
            next_descriptor_address: self.metadata.next_descriptor_address,
        }
    }

    /// Hardware-observed portable metadata copied with this staged lease.
    ///
    /// Crypto and A-MSDU remain explicitly unavailable here. Complete vendor
    /// debug code independently proves the adjacent hardware
    /// IEEE S-MPDU status and the HT-SIG Aggregation bit when applicable.
    pub fn normalized_metadata(&self) -> Option<MacRxMetadata<RxPhyInfo>> {
        decode_normalized_rx_metadata(self.segment().buffer)
    }

    pub const fn length(&self) -> usize {
        self.metadata.length
    }

    /// Replace the dead 802.11/CCMP/LLC prefix with an Ethernet header and
    /// republish the same slot to the network-device consumer.
    ///
    /// `payload_offset` is relative to [`Self::segment`]'s buffer. It must
    /// identify an already initialized payload range and leave fourteen bytes
    /// before it. No payload byte is copied.
    pub fn publish_ethernet_in_place(
        self,
        destination: [u8; 6],
        source: [u8; 6],
        ether_type: u16,
        payload_offset: usize,
        payload_length: usize,
    ) -> Result<u8, (Self, RxInPlaceEthernetError)> {
        let Some(payload_end) = payload_offset.checked_add(payload_length) else {
            return Err((self, RxInPlaceEthernetError::PayloadOutsideFrame));
        };
        if payload_end > self.metadata.length {
            return Err((self, RxInPlaceEthernetError::PayloadOutsideFrame));
        }
        let Some(frame_offset) = payload_offset.checked_sub(14) else {
            return Err((self, RxInPlaceEthernetError::InsufficientHeadroom));
        };
        let ethernet_length = 14 + payload_length;
        let NetworkRxFrame {
            mut lease,
            _slots: _,
            metadata: _,
        } = self;
        lease.with_frame(|raw| {
            raw[frame_offset..frame_offset + 6].copy_from_slice(&destination);
            raw[frame_offset + 6..frame_offset + 12].copy_from_slice(&source);
            raw[frame_offset + 12..frame_offset + 14].copy_from_slice(&ether_type.to_be_bytes());
        });
        Ok(lease.republish(frame_offset, ethernet_length))
    }
}

/// Bounded FIFO of independently owned RX frames.
///
/// The queue stores ownership tokens rather than DMA pointers. Consequently,
/// queueing and parsing can never extend hardware ownership of the original
/// descriptor. Its maximum useful depth is the staging-pool slot count.
pub struct RxFrameQueue<'pool, const SLOTS: usize, const CAPACITY: usize> {
    frames: [Option<NetworkRxFrame<'pool, SLOTS, CAPACITY>>; SLOTS],
    head: usize,
    tail: usize,
    len: usize,
}

impl<'pool, const SLOTS: usize, const CAPACITY: usize> RxFrameQueue<'pool, SLOTS, CAPACITY> {
    pub fn new() -> Self {
        assert!(SLOTS != 0, "RX frame queue requires at least one slot");
        Self {
            frames: [const { None }; SLOTS],
            head: 0,
            tail: 0,
            len: 0,
        }
    }

    pub const fn len(&self) -> usize {
        self.len
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub const fn is_full(&self) -> bool {
        self.len == SLOTS
    }

    pub fn try_push(
        &mut self,
        frame: NetworkRxFrame<'pool, SLOTS, CAPACITY>,
    ) -> Result<(), NetworkRxFrame<'pool, SLOTS, CAPACITY>> {
        if self.is_full() || self.frames[self.tail].is_some() {
            return Err(frame);
        }
        self.frames[self.tail] = Some(frame);
        self.tail = (self.tail + 1) % SLOTS;
        self.len += 1;
        Ok(())
    }

    pub fn pop(&mut self) -> Option<NetworkRxFrame<'pool, SLOTS, CAPACITY>> {
        if self.is_empty() {
            return None;
        }
        let frame = self.frames[self.head].take()?;
        self.head = (self.head + 1) % SLOTS;
        self.len -= 1;
        Some(frame)
    }
}

impl<'pool, const SLOTS: usize, const CAPACITY: usize> Default
    for RxFrameQueue<'pool, SLOTS, CAPACITY>
{
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use open_esp_radio_wifi_softmac::MacRxEvidence;

    use super::*;

    fn completed(length: u32) -> CompletedDescriptorObservation {
        CompletedDescriptorObservation {
            descriptor_address: 0x2f00_1000,
            descriptor_word0: 1_700 | (length << 14) | 0xc000_0000,
            next_descriptor_address: 0,
        }
    }

    #[test]
    fn ownership_moves_free_radio_network_free() {
        let pool = RxStagePool::<2, 32>::new();
        let radio = pool.try_stage(completed(4), &[1, 2, 3, 4]).unwrap();
        assert_eq!(pool.claimed_slots(), 1);
        assert_eq!(pool.network_slots(), 0);
        let network = radio.publish();
        assert_eq!(pool.network_slots(), 1);
        assert_eq!(network.slot(), 0);
        assert_eq!(network.segment().buffer, &[1, 2, 3, 4]);
        drop(network);
        assert_eq!(pool.claimed_slots(), 0);
        assert_eq!(pool.network_slots(), 0);
    }

    #[test]
    fn network_owner_exposes_the_metadata_copied_with_its_frame() {
        let mut bytes = [0_u8; 64];
        bytes[0] = (-52_i8) as u8;
        bytes[1] = 9;
        bytes[0x1c] = 11;
        bytes[0x1f] = 1;
        bytes[0x25] = 4 << 4;
        let pool = RxStagePool::<1, 64>::new();
        let network = pool.try_stage(completed(64), &bytes).unwrap().publish();

        let metadata = network.normalized_metadata().unwrap();
        assert_eq!(metadata.channel, MacRxEvidence::Unavailable);
        assert_eq!(metadata.rssi_dbm, MacRxEvidence::HardwareObserved(-52));
        assert_eq!(metadata.crypto, MacRxEvidence::Unavailable);
        assert_eq!(metadata.s_mpdu, MacRxEvidence::HardwareObserved(true));
        assert_eq!(metadata.ampdu, MacRxEvidence::ProtocolValidated(true));
        assert_eq!(metadata.amsdu, MacRxEvidence::Unavailable);
    }

    #[test]
    fn ordinary_ethernet_frame_reuses_the_staging_slot() {
        let mut bytes = [0_u8; 64];
        bytes[24..28].copy_from_slice(&[1, 2, 3, 4]);
        let pool = RxStagePool::<1, 64>::new();
        let staged = pool.try_stage(completed(64), &bytes).unwrap().publish();
        let index = staged
            .publish_ethernet_in_place([0, 1, 2, 3, 4, 5], [6, 7, 8, 9, 10, 11], 0x0800, 24, 4)
            .ok()
            .unwrap();

        let network = pool.handoff_pool().claim_network(index);
        assert_eq!(
            network.frame(),
            &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 0x08, 0x00, 1, 2, 3, 4]
        );
        drop(network);
        assert_eq!(pool.claimed_slots(), 0);
    }

    #[test]
    fn ownership_scales_past_one_machine_word() {
        const CROSS_WORD_SLOTS: usize = usize::BITS as usize + 1;
        let pool = RxStagePool::<CROSS_WORD_SLOTS, 4>::new();
        let mut frames = std::vec::Vec::new();

        for _ in 0..CROSS_WORD_SLOTS {
            let radio = pool.try_stage(completed(4), &[1, 2, 3, 4]).unwrap();
            frames.push(radio.publish());
        }

        assert_eq!(pool.claimed_slots(), CROSS_WORD_SLOTS as u32);
        assert_eq!(pool.network_slots(), CROSS_WORD_SLOTS as u32);
        assert_eq!(pool.available_slots(), 0);
        assert!(matches!(
            pool.try_stage(completed(4), &[1, 2, 3, 4]),
            Err(RxStageError::Exhausted)
        ));

        drop(frames);
        assert_eq!(pool.claimed_slots(), 0);
        assert_eq!(pool.network_slots(), 0);
        assert_eq!(pool.available_slots(), CROSS_WORD_SLOTS);
    }

    #[test]
    fn pool_exhaustion_is_bounded_and_does_not_alias() {
        let pool = RxStagePool::<1, 32>::new();
        let first = pool.try_stage(completed(4), &[1, 2, 3, 4]).unwrap();
        assert!(matches!(
            pool.try_stage(completed(4), &[5, 6, 7, 8]),
            Err(RxStageError::Exhausted)
        ));
        drop(first);
        assert!(pool.try_stage(completed(4), &[5, 6, 7, 8]).is_ok());
    }

    #[test]
    fn oversized_frame_never_claims_a_slot() {
        let pool = RxStagePool::<1, 3>::new();
        assert!(matches!(
            pool.try_stage(completed(4), &[1, 2, 3, 4]),
            Err(RxStageError::TooLong)
        ));
        assert_eq!(pool.claimed_slots(), 0);
    }

    #[test]
    fn frame_queue_preserves_fifo_ownership_and_releases_on_drop() {
        let pool = RxStagePool::<2, 32>::new();
        let first = pool.try_stage(completed(1), &[0x11]).unwrap().publish();
        let second = pool.try_stage(completed(1), &[0x22]).unwrap().publish();
        let mut queue = RxFrameQueue::new();
        queue.try_push(first).ok().unwrap();
        queue.try_push(second).ok().unwrap();
        assert!(queue.is_full());
        assert_eq!(queue.pop().unwrap().segment().buffer, &[0x11]);
        assert_eq!(queue.pop().unwrap().segment().buffer, &[0x22]);
        assert!(queue.is_empty());
        assert_eq!(pool.claimed_slots(), 0);
    }
}
