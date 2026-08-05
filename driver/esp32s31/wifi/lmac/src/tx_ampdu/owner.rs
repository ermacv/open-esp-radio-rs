//! Capability-bound A-MPDU metadata, descriptor and backing ownership.

#[cfg(not(target_pointer_width = "32"))]
extern crate alloc;

#[cfg(not(target_pointer_width = "32"))]
use alloc::boxed::Box;
use core::{ops::Deref, pin::Pin};

use open_esp_radio_dma::StableDmaBacking;
use open_esp_radio_esp32s31_registers::{MacTxDetachOutcome, MacTxDetachReason};
use open_esp_radio_esp32s31_wifi_dma::tx_ampdu_storage::{
    AmpduDmaState, AmpduDmaStorage, AmpduDmaStorageError, AmpduExternalDescriptor,
    PinnedAmpduDmaStorage, RetainedAmpduDma,
};

use crate::tx::{
    HeAmpduTxConfig, HeEdcaTxopLimit, HeRate, HtAmpduDensity, HtAmpduTxConfig, HtRate,
    LegacyTxQueue, TxCookie, TxSlotState,
};

use super::{
    HtAmpduHardware, HtAmpduLength, HtAmpduTxCompletion, HtAmpduTxError, HtAmpduTxStorage,
    TX_AMPDU_METADATA_SIZE,
};

/// Idle resources required by the safe external-buffer A-MPDU path.
///
/// Protocol metadata and DMA descriptors deliberately have separate owners:
/// [`HtAmpduTxStorage`] contains recovered MAC semantics, while
/// [`AmpduDmaStorage`] is the only layer allowed to publish raw descriptors.
/// Keeping them in one handoff value prevents reconnect/teardown code from
/// accidentally restoring one half without the other.
pub struct HtAmpduTxResources<'storage, const SLOTS: usize, const BUFFER_SIZE: usize> {
    metadata: Pin<&'storage mut HtAmpduTxStorage<SLOTS, BUFFER_SIZE>>,
    dma: PinnedAmpduDmaStorage<SLOTS, 0>,
}

impl<'storage, const SLOTS: usize, const BUFFER_SIZE: usize>
    HtAmpduTxResources<'storage, SLOTS, BUFFER_SIZE>
{
    pub fn new(
        metadata: Pin<&'storage mut HtAmpduTxStorage<SLOTS, BUFFER_SIZE>>,
        dma: PinnedAmpduDmaStorage<SLOTS, 0>,
    ) -> Result<Self, HtAmpduTxError> {
        if metadata.state() != TxSlotState::Free {
            return Err(HtAmpduTxError::NotFree(metadata.state()));
        }
        if dma.state() != AmpduDmaState::Free {
            return Err(HtAmpduTxError::DmaStorage(AmpduDmaStorageError::State));
        }
        Ok(Self { metadata, dma })
    }

    /// Supply a deterministic descriptor address for host state-machine tests.
    #[cfg(not(target_pointer_width = "32"))]
    pub fn new_model(
        metadata: Pin<&'storage mut HtAmpduTxStorage<SLOTS, BUFFER_SIZE>>,
    ) -> Result<Self, HtAmpduTxError> {
        const MODEL_DESCRIPTOR_BASE: u32 = 0x2f00_1000;
        let dma = Box::leak(Box::new(AmpduDmaStorage::<SLOTS, 0>::new()));
        Self::new(
            metadata,
            AmpduDmaStorage::pin_static_model(dma, MODEL_DESCRIPTOR_BASE, 0)?,
        )
    }
}

impl<const SLOTS: usize, const BUFFER_SIZE: usize> Deref
    for HtAmpduTxResources<'_, SLOTS, BUFFER_SIZE>
{
    type Target = HtAmpduTxStorage<SLOTS, BUFFER_SIZE>;

    fn deref(&self) -> &Self::Target {
        self.metadata.as_ref().get_ref()
    }
}

#[cfg(target_pointer_width = "32")]
impl<const SLOTS: usize, const BUFFER_SIZE: usize> HtAmpduTxResources<'static, SLOTS, BUFFER_SIZE> {
    /// Bind both statically allocated target arenas at their final addresses.
    pub fn pin_static(
        metadata: &'static mut HtAmpduTxStorage<SLOTS, BUFFER_SIZE>,
        dma: &'static mut AmpduDmaStorage<SLOTS, 0>,
    ) -> Result<Self, HtAmpduTxError> {
        Self::new(
            HtAmpduTxStorage::pin_static(metadata),
            AmpduDmaStorage::pin_static(dma)?,
        )
    }
}

/// Descriptor owner coupled to every external DMA backing it references.
///
/// Unlike a bare [`HtAmpduTxStorage`], this value cannot return or drop a
/// network allocation while hardware can still follow its descriptor. Safe
/// callers append owned [`StableDmaBacking`] leases; release is admitted only
/// after the underlying storage has returned to [`TxSlotState::Free`].
pub struct RetainedDmaAmpduTx<'storage, B, const SLOTS: usize, const BUFFER_SIZE: usize> {
    storage: Option<Pin<&'storage mut HtAmpduTxStorage<SLOTS, BUFFER_SIZE>>>,
    dma: Option<RetainedAmpduDma<B, SLOTS, 0>>,
}

impl<'storage, B, const SLOTS: usize, const BUFFER_SIZE: usize>
    RetainedDmaAmpduTx<'storage, B, SLOTS, BUFFER_SIZE>
{
    pub fn new(resources: HtAmpduTxResources<'storage, SLOTS, BUFFER_SIZE>) -> Self {
        let HtAmpduTxResources { metadata, dma } = resources;
        Self {
            storage: Some(metadata),
            dma: Some(RetainedAmpduDma::new(dma)),
        }
    }

    #[cfg(not(target_pointer_width = "32"))]
    pub fn new_model(
        storage: Pin<&'storage mut HtAmpduTxStorage<SLOTS, BUFFER_SIZE>>,
    ) -> Result<Self, HtAmpduTxError> {
        Ok(Self::new(HtAmpduTxResources::new_model(storage)?))
    }

    pub fn as_ref(&self) -> Pin<&HtAmpduTxStorage<SLOTS, BUFFER_SIZE>> {
        self.storage
            .as_ref()
            .expect("retained DMA owner keeps storage until teardown")
            .as_ref()
    }

    fn metadata_mut(&mut self) -> Pin<&mut HtAmpduTxStorage<SLOTS, BUFFER_SIZE>> {
        self.storage
            .as_mut()
            .expect("retained DMA owner keeps storage until teardown")
            .as_mut()
    }

    fn dma_mut(&mut self) -> &mut RetainedAmpduDma<B, SLOTS, 0> {
        self.dma
            .as_mut()
            .expect("retained DMA owner keeps descriptor storage until teardown")
    }

    pub fn held_backing_count(&self) -> usize {
        self.dma
            .as_ref()
            .expect("retained DMA owner keeps descriptor storage until teardown")
            .held_backing_count()
    }

    /// Confirm that every lease has already crossed a safe release edge.
    pub fn release_free_backings(&mut self) -> Result<(), HtAmpduTxError> {
        if self.state() != TxSlotState::Free
            || self.dma_mut().state() != AmpduDmaState::Free
            || self.held_backing_count() != 0
        {
            return Err(HtAmpduTxError::NotFree(self.state()));
        }
        Ok(())
    }

    /// Quarantine every retained lease when hardware ownership is unknowable.
    pub fn forget_backings(&mut self) {
        self.quarantine();
    }

    /// Recover the complete metadata/descriptor handoff only when idle.
    #[allow(clippy::result_large_err)]
    pub fn try_into_resources(
        mut self,
    ) -> Result<HtAmpduTxResources<'storage, SLOTS, BUFFER_SIZE>, Self> {
        if self.state() != TxSlotState::Free || self.held_backing_count() != 0 {
            return Err(self);
        }
        let dma_owner = self
            .dma
            .take()
            .expect("retained DMA owner contains descriptor storage");
        let dma = match dma_owner.try_into_dma() {
            Ok(dma) => dma,
            Err(dma_owner) => {
                self.dma = Some(dma_owner);
                return Err(self);
            }
        };
        let metadata = self
            .storage
            .take()
            .expect("retained DMA owner contains teardown metadata");
        Ok(HtAmpduTxResources { metadata, dma })
    }

    /// Begin a fresh aggregate in both protocol and descriptor owners.
    pub fn begin(&mut self) -> Result<TxCookie, HtAmpduTxError> {
        self.dma_mut().begin()?;
        match self.metadata_mut().begin() {
            Ok(cookie) => Ok(cookie),
            Err(error) => {
                self.dma_mut().cancel()?;
                Err(error)
            }
        }
    }

    pub fn configure_max_aggregate_bytes(
        &mut self,
        max_aggregate_bytes: u16,
    ) -> Result<(), HtAmpduTxError> {
        self.metadata_mut()
            .configure_max_aggregate_bytes(max_aggregate_bytes)
    }

    pub fn cancel(&mut self, cookie: TxCookie) -> Result<(), HtAmpduTxError> {
        self.metadata_mut().cancel(cookie)?;
        if let Err(error) = self.dma_mut().cancel() {
            self.quarantine();
            return Err(error.into());
        }
        Ok(())
    }

    fn quarantine(&mut self) {
        if let Some(storage) = self.storage.as_mut() {
            *storage.as_mut().project().state = TxSlotState::ResetRequired;
        }
        if let Some(dma) = self.dma.as_mut() {
            dma.quarantine();
        }
    }
}

impl<B: StableDmaBacking, const SLOTS: usize, const BUFFER_SIZE: usize>
    RetainedDmaAmpduTx<'_, B, SLOTS, BUFFER_SIZE>
{
    /// Borrow a detached completed MPDU from its retained stable lease.
    ///
    /// The descriptor address is treated only as an identity to resolve the
    /// owning lease. It is never converted back into a Rust reference.
    pub fn completed_frame(
        &mut self,
        cookie: TxCookie,
        index: u8,
    ) -> Result<(&[u8], u8), HtAmpduTxError> {
        let layout = self
            .storage
            .as_ref()
            .expect("retained DMA owner keeps storage until teardown")
            .as_ref()
            .get_ref()
            .completed_frame_layout(cookie, index)?;
        let bytes = self
            .dma_mut()
            .detached_region_mut(layout.buffer_address, layout.capacity)
            .map_err(|_| HtAmpduTxError::BackingUnavailable { index })?;
        Ok((
            &bytes[layout.frame_start..layout.frame_end],
            layout.hardware_mic_length,
        ))
    }

    /// Retain selected MPDUs and set their Retry bit through the allocation
    /// owner rather than by reconstructing slices from descriptor addresses.
    pub fn retain_for_ampdu_retry(
        &mut self,
        cookie: TxCookie,
        retry_mask: u32,
    ) -> Result<HtAmpduLength, HtAmpduTxError> {
        let locations = self
            .storage
            .as_ref()
            .expect("retained DMA owner keeps storage until teardown")
            .as_ref()
            .retry_frame_locations(cookie, retry_mask)?;

        // Resolve every selected frame before mutating any of them. A stale
        // address therefore fails closed without a partially rewritten batch.
        for location in locations.iter().flatten() {
            if self
                .dma_mut()
                .detached_region_mut(location.buffer_address, location.capacity)
                .is_err()
            {
                return Err(HtAmpduTxError::BackingUnavailable {
                    index: location.index as u8,
                });
            }
        }

        for location in locations.iter().flatten() {
            let bytes = self
                .dma_mut()
                .detached_region_mut(location.buffer_address, location.capacity)
                .map_err(|_| HtAmpduTxError::BackingUnavailable {
                    index: location.index as u8,
                })?;
            bytes[TX_AMPDU_METADATA_SIZE + 1] |= 0x08;
        }
        self.dma_mut().begin_retry()?;
        match self.metadata_mut().compact_retry_metadata(locations) {
            Ok(length) => Ok(length),
            Err(error) => {
                self.quarantine();
                Err(error)
            }
        }
    }

    /// Commit one HT frame and retain its stable backing in the same owner.
    pub fn commit_ht(
        &mut self,
        cookie: TxCookie,
        backing: B,
        dma_offset: usize,
        frame_length: usize,
        hardware_mic_length: u8,
        empty_delimiters: u8,
        rate: HtRate,
    ) -> Result<(), HtAmpduTxError> {
        let (storage, dma) = (
            self.storage
                .as_mut()
                .expect("retained DMA owner keeps storage until teardown"),
            self.dma
                .as_mut()
                .expect("retained DMA owner keeps descriptor storage until teardown"),
        );
        let backing_index = dma.push_backing(backing)?;
        let result = (|| {
            let backing = dma.reserved_backing_mut(backing_index)?;
            let mut region = backing.stable_dma_region();
            let dma_storage = region
                .as_mut_slice()
                .get_mut(dma_offset..)
                .ok_or(HtAmpduTxError::FrameTooLong)?;
            storage.as_mut().commit_referenced_ht_frame(
                cookie,
                dma_storage,
                frame_length,
                hardware_mic_length,
                empty_delimiters,
                rate,
            )
        })();
        if let Err(error) = result {
            drop(dma.pop_last_backing(backing_index)?);
            return Err(error);
        }
        Ok(())
    }

    /// Commit one HE frame under the exact TXOP policy and retain its backing.
    pub fn commit_he_with_txop(
        &mut self,
        cookie: TxCookie,
        backing: B,
        dma_offset: usize,
        frame_length: usize,
        hardware_mic_length: u8,
        rate: HeRate,
        density: HtAmpduDensity,
        txop_limit: HeEdcaTxopLimit,
    ) -> Result<(), HtAmpduTxError> {
        let (storage, dma) = (
            self.storage
                .as_mut()
                .expect("retained DMA owner keeps storage until teardown"),
            self.dma
                .as_mut()
                .expect("retained DMA owner keeps descriptor storage until teardown"),
        );
        let backing_index = dma.push_backing(backing)?;
        let result = (|| {
            let backing = dma.reserved_backing_mut(backing_index)?;
            let mut region = backing.stable_dma_region();
            let dma_storage = region
                .as_mut_slice()
                .get_mut(dma_offset..)
                .ok_or(HtAmpduTxError::FrameTooLong)?;
            storage.as_mut().commit_referenced_he_frame_with_txop(
                cookie,
                dma_storage,
                frame_length,
                hardware_mic_length,
                rate,
                density,
                txop_limit,
            )
        })();
        if let Err(error) = result {
            drop(dma.pop_last_backing(backing_index)?);
            return Err(error);
        }
        Ok(())
    }

    fn external_descriptors(
        storage: &HtAmpduTxStorage<SLOTS, BUFFER_SIZE>,
        dma: &mut RetainedAmpduDma<B, SLOTS, 0>,
    ) -> Result<([AmpduExternalDescriptor; SLOTS], usize), HtAmpduTxError> {
        let count = usize::from(storage.count);
        let mut entries = [AmpduExternalDescriptor {
            backing_index: 0,
            dma_offset: 0,
            buffer_capacity: 0,
            transfer_length: 0,
        }; SLOTS];
        for (index, entry) in entries[..count].iter_mut().enumerate() {
            *entry = dma.reserved_external_descriptor(
                storage.buffer_addresses[index],
                u32::from(storage.descriptor_capacities[index]),
                (TX_AMPDU_METADATA_SIZE as u32) + u32::from(storage.psdu_lengths[index]),
            )?;
        }
        Ok((entries, count))
    }

    /// Publish and start an HT A-MPDU through the lower descriptor owner.
    pub fn submit<H: HtAmpduHardware>(
        &mut self,
        hardware: &mut H,
        cookie: TxCookie,
        queue: LegacyTxQueue,
        config: HtAmpduTxConfig,
    ) -> Result<(), HtAmpduTxError> {
        let (storage, dma) = (
            self.storage
                .as_mut()
                .expect("retained DMA owner keeps storage until teardown"),
            self.dma
                .as_mut()
                .expect("retained DMA owner keeps descriptor storage until teardown"),
        );
        let prepared = storage.as_ref().get_ref().prepared_ht_submission(
            dma.descriptor_head(),
            cookie,
            config,
        )?;
        let (entries, count) = Self::external_descriptors(storage.as_ref().get_ref(), dma)?;
        let publication = dma.publish_external_chain(&entries[..count])?;
        let queue_index = queue.index();
        if !hardware.prepare_bound_ht_tx(&publication, queue_index, prepared.program) {
            return Err(HtAmpduTxError::QueueActive);
        }
        let metadata = storage.as_mut().project();
        *metadata.queue = queue;
        *metadata.aggregate_length = prepared.aggregate.bytes;
        *metadata.detached = false;
        *metadata.state = TxSlotState::HardwareOwned;
        publication.commit(|dma| hardware.start_bound_ht_tx(dma, queue_index, prepared.plcp0));
        Ok(())
    }

    /// Publish and start an HE A-MPDU through the lower descriptor owner.
    pub fn submit_he<H: HtAmpduHardware>(
        &mut self,
        hardware: &mut H,
        cookie: TxCookie,
        queue: LegacyTxQueue,
        config: HeAmpduTxConfig,
    ) -> Result<(), HtAmpduTxError> {
        let (storage, dma) = (
            self.storage
                .as_mut()
                .expect("retained DMA owner keeps storage until teardown"),
            self.dma
                .as_mut()
                .expect("retained DMA owner keeps descriptor storage until teardown"),
        );
        let prepared = storage.as_ref().get_ref().prepared_he_submission(
            dma.descriptor_head(),
            cookie,
            queue,
            config,
        )?;
        let (entries, count) = Self::external_descriptors(storage.as_ref().get_ref(), dma)?;
        let publication = dma.publish_external_chain(&entries[..count])?;
        let queue_index = queue.index();
        if !hardware.prepare_bound_he_tx(&publication, queue_index, prepared.program) {
            return Err(HtAmpduTxError::QueueActive);
        }
        let mut trigger_snapshot = None;
        if let Some(trigger) = prepared.trigger {
            trigger_snapshot = Some(
                hardware
                    .prepare_he_trigger_based_queue(
                        trigger.policy,
                        trigger.reservation,
                        trigger.tid,
                        &storage.as_ref().get_ref().psdu_lengths[..count],
                        trigger.queued_msdu_bytes,
                    )
                    .map_err(HtAmpduTxError::TriggerBased)?,
            );
        }
        let metadata = storage.as_mut().project();
        *metadata.trigger_reservation = prepared.trigger.map(|trigger| trigger.reservation);
        *metadata.trigger_publication_snapshot = trigger_snapshot;
        *metadata.queue = queue;
        *metadata.aggregate_length = prepared.aggregate.bytes;
        *metadata.detached = false;
        *metadata.state = TxSlotState::HardwareOwned;
        publication.commit(|dma| hardware.start_bound_he_tx(dma, queue_index, prepared.plcp0));
        Ok(())
    }

    /// Sample BlockAck and synchronize both ownership state machines.
    pub fn acknowledge_completion<H: HtAmpduHardware>(
        &mut self,
        hardware: &mut H,
    ) -> Result<Option<HtAmpduTxCompletion>, HtAmpduTxError> {
        let completion = self.metadata_mut().acknowledge_completion(hardware)?;
        if completion.is_some() && self.dma_mut().mark_completed().is_err() {
            self.quarantine();
            return Err(HtAmpduTxError::ResetRequired);
        }
        Ok(completion)
    }

    pub fn begin_timeout_abort<H: HtAmpduHardware>(
        &mut self,
        hardware: &mut H,
        cookie: TxCookie,
    ) -> Result<bool, HtAmpduTxError> {
        self.metadata_mut().begin_timeout_abort(hardware, cookie)
    }

    pub fn abort_collision<H: HtAmpduHardware>(
        &mut self,
        hardware: &mut H,
        cookie: TxCookie,
    ) -> Result<bool, HtAmpduTxError> {
        let queue = {
            let storage = self.as_ref().get_ref();
            if storage.state != TxSlotState::HardwareOwned || storage.active != cookie {
                return Err(HtAmpduTxError::Stale);
            }
            storage.queue.index()
        };
        let descriptor_head = self.dma_mut().descriptor_head();
        let outcome = {
            let dma = self.dma_mut();
            hardware.with_tx_queue_detached(
                queue,
                descriptor_head,
                MacTxDetachReason::Collision,
                |detached| dma.release_aborted(detached),
            )
        };
        match outcome {
            MacTxDetachOutcome::NoEvent => return Ok(false),
            MacTxDetachOutcome::Detached(Ok(())) => {}
            MacTxDetachOutcome::Failed | MacTxDetachOutcome::Detached(Err(_)) => {
                self.quarantine();
                return Err(HtAmpduTxError::DetachFailed);
            }
        }
        if let Some(reservation) = self.metadata_mut().project().trigger_reservation.take() {
            hardware.clear_he_trigger_based_queue(reservation);
        }
        self.metadata_mut().release();
        Ok(true)
    }

    pub fn finish_timeout_abort<H: HtAmpduHardware>(
        &mut self,
        hardware: &mut H,
        cookie: TxCookie,
    ) -> Result<(), HtAmpduTxError> {
        let queue = {
            let storage = self.as_ref().get_ref();
            if storage.state != TxSlotState::HardwareOwned || storage.active != cookie {
                return Err(HtAmpduTxError::Stale);
            }
            storage.queue.index()
        };
        let descriptor_head = self.dma_mut().descriptor_head();
        let outcome = {
            let dma = self.dma_mut();
            hardware.with_tx_queue_detached(
                queue,
                descriptor_head,
                MacTxDetachReason::Timeout,
                |detached| dma.release_aborted(detached),
            )
        };
        match outcome {
            MacTxDetachOutcome::NoEvent => return Err(HtAmpduTxError::TimeoutNotPending),
            MacTxDetachOutcome::Detached(Ok(())) => {}
            MacTxDetachOutcome::Failed | MacTxDetachOutcome::Detached(Err(_)) => {
                self.quarantine();
                return Err(HtAmpduTxError::DetachFailed);
            }
        }
        if let Some(reservation) = self.metadata_mut().project().trigger_reservation.take() {
            hardware.clear_he_trigger_based_queue(reservation);
        }
        self.metadata_mut().release();
        Ok(())
    }

    pub fn detach_completed<H: HtAmpduHardware>(
        &mut self,
        hardware: &mut H,
        cookie: TxCookie,
    ) -> Result<(), HtAmpduTxError> {
        let queue = {
            let storage = self.as_ref().get_ref();
            if storage.state != TxSlotState::Completed || storage.active != cookie {
                return Err(HtAmpduTxError::Stale);
            }
            storage.queue.index()
        };
        let descriptor_head = self.dma_mut().descriptor_head();
        let outcome = {
            let dma = self.dma_mut();
            hardware.with_tx_queue_detached(
                queue,
                descriptor_head,
                MacTxDetachReason::Completed,
                |detached| dma.mark_detached(detached),
            )
        };
        match outcome {
            MacTxDetachOutcome::Detached(Ok(())) => {
                *self.metadata_mut().project().detached = true;
                Ok(())
            }
            MacTxDetachOutcome::NoEvent
            | MacTxDetachOutcome::Failed
            | MacTxDetachOutcome::Detached(Err(_)) => {
                self.quarantine();
                Err(HtAmpduTxError::DetachFailed)
            }
        }
    }

    pub fn release_completed(&mut self, cookie: TxCookie) -> Result<(), HtAmpduTxError> {
        {
            let storage = self.as_ref().get_ref();
            if storage.state != TxSlotState::Completed
                || storage.active != cookie
                || !storage.detached
            {
                return Err(HtAmpduTxError::Stale);
            }
        }
        if let Err(error) = self.dma_mut().release_detached() {
            self.quarantine();
            return Err(error.into());
        }
        self.metadata_mut().release();
        Ok(())
    }

    pub fn require_reset(&mut self, cookie: TxCookie) -> Result<(), HtAmpduTxError> {
        self.metadata_mut().require_reset(cookie)?;
        self.dma_mut().quarantine();
        Ok(())
    }
}

impl<B, const SLOTS: usize, const BUFFER_SIZE: usize> Deref
    for RetainedDmaAmpduTx<'_, B, SLOTS, BUFFER_SIZE>
{
    type Target = HtAmpduTxStorage<SLOTS, BUFFER_SIZE>;

    fn deref(&self) -> &Self::Target {
        self.as_ref().get_ref()
    }
}

impl<B, const SLOTS: usize, const BUFFER_SIZE: usize> Drop
    for RetainedDmaAmpduTx<'_, B, SLOTS, BUFFER_SIZE>
{
    fn drop(&mut self) {
        if self.storage.is_none() || self.dma.is_none() {
            return;
        }
        let metadata_state = self.state();
        let dma_state = self.dma_mut().state();
        let released = match (metadata_state, dma_state) {
            (TxSlotState::Free, AmpduDmaState::Free) => true,
            (TxSlotState::Reserved, AmpduDmaState::Reserved) => {
                let cookie = self.active;
                self.metadata_mut().cancel(cookie).is_ok() && self.dma_mut().cancel().is_ok()
            }
            (TxSlotState::Completed, AmpduDmaState::Detached) if self.detached => {
                let cookie = self.active;
                self.dma_mut().release_detached().is_ok()
                    && self.metadata_mut().release_completed(cookie).is_ok()
            }
            _ => false,
        };
        if !released {
            self.quarantine();
        }
    }
}
