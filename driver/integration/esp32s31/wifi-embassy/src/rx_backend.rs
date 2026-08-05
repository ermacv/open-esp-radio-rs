//! Production ownership of the ESP32-S31 Wi-Fi RX descriptor ring.
//!
//! DMA storage, the live descriptor frontier and independent staging storage
//! are kept in one finite service. No DMA pointer escapes this owner: a
//! completed unit is copied and recycled before its staging lease is handed to
//! the separate protocol consumer.

#![allow(unsafe_code, reason = "RX DMA ownership transition")]

use core::future::Future;

use embassy_sync::channel::{Sender, TrySendError};
use open_esp_radio_embassy_net::RawMutex;
use open_esp_radio_esp32s31_wifi_lmac::{
    rx::{RxDma, RxReloadObservation, RxRingError, RxRingHalted, RxRingLive, RxRingStopped},
    rx_pool::{
        RxStageError, RxStagePool, RxStageTransactionError, VENDOR_LARGE_RX_PAYLOAD_CAPACITY,
        VENDOR_LARGE_RX_SLOT_COUNT,
    },
    rx_storage::{RxDmaBuffer, RxDmaStorage},
};

use crate::{
    connected_runner::WifiRxProgress,
    connected_services::Esp32s31ConnectedRxService,
    embassy_rx::{RxReloadDelay, await_staged_rx_reload},
    rx_observer::{
        RxPipelineObservation, RxPipelineObserver, RxServiceObservation, RxStageDiscard,
    },
    staged_rx::Esp32s31StagedRxFrame,
};

/// Descriptor count and allocation geometry qualified by the ordinary S31
/// large-RX profile.
pub const ESP32S31_RX_DESCRIPTOR_COUNT: usize = 32;
pub const ESP32S31_RX_BUFFER_SIZE: usize = 4_608;
pub const ESP32S31_RX_BUFFER_STORAGE_SIZE: usize = ESP32S31_RX_BUFFER_SIZE + 4;
/// Platform settle edge between stopped-ring publication and walker enable.
pub const ESP32S31_RX_WALKER_ENABLE_SETTLE_US: u32 = 5;
/// Qualified large-RX profile aliases over the executor-independent LMAC arena.
pub type Esp32s31RxDmaBuffer<
    const BUFFER_SIZE: usize = ESP32S31_RX_BUFFER_SIZE,
    const STORAGE_SIZE: usize = ESP32S31_RX_BUFFER_STORAGE_SIZE,
> = RxDmaBuffer<BUFFER_SIZE, STORAGE_SIZE>;

pub type Esp32s31RxDmaStorage<
    const COUNT: usize = ESP32S31_RX_DESCRIPTOR_COUNT,
    const BUFFER_SIZE: usize = ESP32S31_RX_BUFFER_SIZE,
    const STORAGE_SIZE: usize = ESP32S31_RX_BUFFER_STORAGE_SIZE,
> = RxDmaStorage<COUNT, BUFFER_SIZE, STORAGE_SIZE>;

/// Complete production RX owner for one running descriptor-ring epoch.
pub struct Esp32s31ConnectedRx<
    'storage,
    'pool,
    'queue,
    D,
    M: RawMutex,
    const QUEUE_DEPTH: usize = VENDOR_LARGE_RX_SLOT_COUNT,
    const COUNT: usize = ESP32S31_RX_DESCRIPTOR_COUNT,
    const STAGE_CAPACITY: usize = VENDOR_LARGE_RX_PAYLOAD_CAPACITY,
    const STAGE_SLOTS: usize = VENDOR_LARGE_RX_SLOT_COUNT,
    const DMA_BUFFER_SIZE: usize = ESP32S31_RX_BUFFER_SIZE,
    const DMA_STORAGE_SIZE: usize = ESP32S31_RX_BUFFER_STORAGE_SIZE,
> {
    ring: RxRingLive<'storage, COUNT>,
    storage: &'storage Esp32s31RxDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
    pool: &'pool RxStagePool<STAGE_SLOTS, STAGE_CAPACITY>,
    frames:
        Sender<'queue, M, Esp32s31StagedRxFrame<'pool, STAGE_CAPACITY, STAGE_SLOTS>, QUEUE_DEPTH>,
    delay: D,
    pipeline_observer: Option<&'pool dyn RxPipelineObserver>,
}

/// Connected RX resources after the DMA walker is confirmed stopped.
///
/// This owner deliberately retains the queue sender, staging pool and reload
/// delay together with the halted descriptor storage. A later station epoch
/// can therefore reconstruct the same production RX service without stealing
/// static resources or retaining any frontier from the previous peer.
pub struct Esp32s31StoppedRx<
    'storage,
    'pool,
    'queue,
    D,
    M: RawMutex,
    const QUEUE_DEPTH: usize = VENDOR_LARGE_RX_SLOT_COUNT,
    const COUNT: usize = ESP32S31_RX_DESCRIPTOR_COUNT,
    const STAGE_CAPACITY: usize = VENDOR_LARGE_RX_PAYLOAD_CAPACITY,
    const STAGE_SLOTS: usize = VENDOR_LARGE_RX_SLOT_COUNT,
    const DMA_BUFFER_SIZE: usize = ESP32S31_RX_BUFFER_SIZE,
    const DMA_STORAGE_SIZE: usize = ESP32S31_RX_BUFFER_STORAGE_SIZE,
> {
    ring: RxRingHalted<'storage, COUNT>,
    storage: &'storage Esp32s31RxDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
    pool: &'pool RxStagePool<STAGE_SLOTS, STAGE_CAPACITY>,
    frames:
        Sender<'queue, M, Esp32s31StagedRxFrame<'pool, STAGE_CAPACITY, STAGE_SLOTS>, QUEUE_DEPTH>,
    delay: D,
    pipeline_observer: Option<&'pool dyn RxPipelineObserver>,
}

/// Peer-independent resources retained while a halted ring is used by a
/// finite Authentication/Association/WPA2 receive epoch.
///
/// Splitting these resources from [`RxRingHalted`] lets the station lifecycle
/// pass the descriptor frontier through its pre-connected type state without
/// discarding the production staging pool, queue sender, reload delay or
/// telemetry binding. Reassembly consumes both owners, so it cannot create a
/// second connected RX service for the same static storage.
pub struct Esp32s31RxEpochResources<
    'storage,
    'pool,
    'queue,
    D,
    M: RawMutex,
    const QUEUE_DEPTH: usize = VENDOR_LARGE_RX_SLOT_COUNT,
    const COUNT: usize = ESP32S31_RX_DESCRIPTOR_COUNT,
    const STAGE_CAPACITY: usize = VENDOR_LARGE_RX_PAYLOAD_CAPACITY,
    const STAGE_SLOTS: usize = VENDOR_LARGE_RX_SLOT_COUNT,
    const DMA_BUFFER_SIZE: usize = ESP32S31_RX_BUFFER_SIZE,
    const DMA_STORAGE_SIZE: usize = ESP32S31_RX_BUFFER_STORAGE_SIZE,
> {
    storage: &'storage Esp32s31RxDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
    pool: &'pool RxStagePool<STAGE_SLOTS, STAGE_CAPACITY>,
    frames:
        Sender<'queue, M, Esp32s31StagedRxFrame<'pool, STAGE_CAPACITY, STAGE_SLOTS>, QUEUE_DEPTH>,
    delay: D,
    pipeline_observer: Option<&'pool dyn RxPipelineObserver>,
}

/// Connected RX resources after descriptor rebuild but before walker enable.
///
/// Keeping this state distinct preserves the platform settle edge and returns
/// the complete owner if walker activation fails. A reconnecting station can
/// retry or reset without stealing descriptor, buffer, queue or pool storage.
pub struct Esp32s31PreparedRx<
    'storage,
    'pool,
    'queue,
    D,
    M: RawMutex,
    const QUEUE_DEPTH: usize = VENDOR_LARGE_RX_SLOT_COUNT,
    const COUNT: usize = ESP32S31_RX_DESCRIPTOR_COUNT,
    const STAGE_CAPACITY: usize = VENDOR_LARGE_RX_PAYLOAD_CAPACITY,
    const STAGE_SLOTS: usize = VENDOR_LARGE_RX_SLOT_COUNT,
    const DMA_BUFFER_SIZE: usize = ESP32S31_RX_BUFFER_SIZE,
    const DMA_STORAGE_SIZE: usize = ESP32S31_RX_BUFFER_STORAGE_SIZE,
> {
    ring: RxRingStopped<'storage, COUNT>,
    storage: &'storage Esp32s31RxDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
    pool: &'pool RxStagePool<STAGE_SLOTS, STAGE_CAPACITY>,
    frames:
        Sender<'queue, M, Esp32s31StagedRxFrame<'pool, STAGE_CAPACITY, STAGE_SLOTS>, QUEUE_DEPTH>,
    delay: D,
    pipeline_observer: Option<&'pool dyn RxPipelineObserver>,
}

impl<
    'storage,
    'pool,
    'queue,
    D,
    M: RawMutex,
    const QUEUE_DEPTH: usize,
    const COUNT: usize,
    const STAGE_CAPACITY: usize,
    const STAGE_SLOTS: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
>
    Esp32s31StoppedRx<
        'storage,
        'pool,
        'queue,
        D,
        M,
        QUEUE_DEPTH,
        COUNT,
        STAGE_CAPACITY,
        STAGE_SLOTS,
        DMA_BUFFER_SIZE,
        DMA_STORAGE_SIZE,
    >
{
    pub const fn ring(&self) -> &RxRingHalted<'storage, COUNT> {
        &self.ring
    }

    pub const fn buffers(
        &self,
    ) -> &'storage [Esp32s31RxDmaBuffer<DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>; COUNT] {
        self.storage.buffers()
    }

    pub const fn storage(
        &self,
    ) -> &'storage Esp32s31RxDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE> {
        self.storage
    }

    pub const fn pool(&self) -> &'pool RxStagePool<STAGE_SLOTS, STAGE_CAPACITY> {
        self.pool
    }

    pub const fn delay(&self) -> &D {
        &self.delay
    }

    pub fn delay_mut(&mut self) -> &mut D {
        &mut self.delay
    }

    pub const fn pipeline_observer(&self) -> Option<&'pool dyn RxPipelineObserver> {
        self.pipeline_observer
    }

    pub fn queued_frames(&self) -> usize {
        self.frames.len()
    }

    /// Separate the peer-specific halted frontier from persistent connected
    /// RX resources for a finite pre-connected protocol epoch.
    pub fn into_epoch_parts(
        self,
    ) -> (
        RxRingHalted<'storage, COUNT>,
        Esp32s31RxEpochResources<
            'storage,
            'pool,
            'queue,
            D,
            M,
            QUEUE_DEPTH,
            COUNT,
            STAGE_CAPACITY,
            STAGE_SLOTS,
            DMA_BUFFER_SIZE,
            DMA_STORAGE_SIZE,
        >,
    ) {
        let Self {
            ring,
            storage,
            pool,
            frames,
            delay,
            pipeline_observer,
        } = self;
        (
            ring,
            Esp32s31RxEpochResources {
                storage,
                pool,
                frames,
                delay,
                pipeline_observer,
            },
        )
    }

    /// Rebuild descriptor and buffer state for a fresh association epoch.
    ///
    /// Hardware is already confirmed stopped by this type. On every failure
    /// the complete halted owner is reconstructed, including its queue sender
    /// and delay implementation.
    #[allow(clippy::result_large_err)]
    pub fn prepare<H: RxDma>(
        self,
        hardware: &mut H,
    ) -> Result<
        Esp32s31PreparedRx<
            'storage,
            'pool,
            'queue,
            D,
            M,
            QUEUE_DEPTH,
            COUNT,
            STAGE_CAPACITY,
            STAGE_SLOTS,
            DMA_BUFFER_SIZE,
            DMA_STORAGE_SIZE,
        >,
        (Self, RxRingError),
    > {
        if DMA_BUFFER_SIZE > u32::MAX as usize {
            return Err((self, RxRingError::Size));
        }
        let Self {
            ring,
            storage,
            pool,
            frames,
            delay,
            pipeline_observer,
        } = self;
        match storage.prepare_halted(ring, hardware) {
            Ok(ring) => Ok(Esp32s31PreparedRx {
                ring,
                storage,
                pool,
                frames,
                delay,
                pipeline_observer,
            }),
            Err((ring, error)) => Err((
                Self {
                    ring,
                    storage,
                    pool,
                    frames,
                    delay,
                    pipeline_observer,
                },
                error,
            )),
        }
    }
}

impl<
    'storage,
    'pool,
    'queue,
    D,
    M: RawMutex,
    const QUEUE_DEPTH: usize,
    const COUNT: usize,
    const STAGE_CAPACITY: usize,
    const STAGE_SLOTS: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
>
    Esp32s31RxEpochResources<
        'storage,
        'pool,
        'queue,
        D,
        M,
        QUEUE_DEPTH,
        COUNT,
        STAGE_CAPACITY,
        STAGE_SLOTS,
        DMA_BUFFER_SIZE,
        DMA_STORAGE_SIZE,
    >
{
    /// Bind board-allocated DMA/staging resources before the first connected
    /// epoch. Later epochs recover this same owner from [`Esp32s31StoppedRx`].
    pub fn new(
        storage: &'storage Esp32s31RxDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
        pool: &'pool RxStagePool<STAGE_SLOTS, STAGE_CAPACITY>,
        frames: Sender<
            'queue,
            M,
            Esp32s31StagedRxFrame<'pool, STAGE_CAPACITY, STAGE_SLOTS>,
            QUEUE_DEPTH,
        >,
        delay: D,
    ) -> Self {
        Self {
            storage,
            pool,
            frames,
            delay,
            pipeline_observer: None,
        }
    }

    pub fn with_pipeline_observer(mut self, observer: &'pool dyn RxPipelineObserver) -> Self {
        self.pipeline_observer = Some(observer);
        self
    }

    pub const fn buffers(
        &self,
    ) -> &'storage [Esp32s31RxDmaBuffer<DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>; COUNT] {
        self.storage.buffers()
    }

    pub const fn storage(
        &self,
    ) -> &'storage Esp32s31RxDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE> {
        self.storage
    }

    pub fn delay_mut(&mut self) -> &mut D {
        &mut self.delay
    }

    pub fn queued_frames(&self) -> usize {
        self.frames.len()
    }

    /// Reassemble the stopped production owner after a finite join attempt.
    pub fn with_halted_ring(
        self,
        ring: RxRingHalted<'storage, COUNT>,
    ) -> Esp32s31StoppedRx<
        'storage,
        'pool,
        'queue,
        D,
        M,
        QUEUE_DEPTH,
        COUNT,
        STAGE_CAPACITY,
        STAGE_SLOTS,
        DMA_BUFFER_SIZE,
        DMA_STORAGE_SIZE,
    > {
        Esp32s31StoppedRx {
            ring,
            storage: self.storage,
            pool: self.pool,
            frames: self.frames,
            delay: self.delay,
            pipeline_observer: self.pipeline_observer,
        }
    }

    /// Promote the same persistent resources into a connected RX service
    /// after Association/WPA2 returns the live ring frontier.
    pub fn with_live_ring(
        self,
        ring: RxRingLive<'storage, COUNT>,
    ) -> Esp32s31ConnectedRx<
        'storage,
        'pool,
        'queue,
        D,
        M,
        QUEUE_DEPTH,
        COUNT,
        STAGE_CAPACITY,
        STAGE_SLOTS,
        DMA_BUFFER_SIZE,
        DMA_STORAGE_SIZE,
    > {
        Esp32s31ConnectedRx {
            ring,
            storage: self.storage,
            pool: self.pool,
            frames: self.frames,
            delay: self.delay,
            pipeline_observer: self.pipeline_observer,
        }
    }
}

impl<
    'storage,
    'pool,
    'queue,
    D,
    M: RawMutex,
    const QUEUE_DEPTH: usize,
    const COUNT: usize,
    const STAGE_CAPACITY: usize,
    const STAGE_SLOTS: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
>
    Esp32s31PreparedRx<
        'storage,
        'pool,
        'queue,
        D,
        M,
        QUEUE_DEPTH,
        COUNT,
        STAGE_CAPACITY,
        STAGE_SLOTS,
        DMA_BUFFER_SIZE,
        DMA_STORAGE_SIZE,
    >
{
    pub const fn ring(&self) -> &RxRingStopped<'storage, COUNT> {
        &self.ring
    }

    pub fn queued_frames(&self) -> usize {
        self.frames.len()
    }

    /// Observe the required settle delay and open a fresh live RX epoch.
    ///
    /// A rejected walker-enable readback returns this prepared owner intact,
    /// so a higher-level reset/retry policy never loses static resources.
    #[allow(clippy::result_large_err)]
    pub async fn start<H: RxDma>(
        self,
        hardware: &mut H,
    ) -> Result<
        Esp32s31ConnectedRx<
            'storage,
            'pool,
            'queue,
            D,
            M,
            QUEUE_DEPTH,
            COUNT,
            STAGE_CAPACITY,
            STAGE_SLOTS,
            DMA_BUFFER_SIZE,
            DMA_STORAGE_SIZE,
        >,
        (Self, RxRingError),
    >
    where
        D: RxReloadDelay,
    {
        let Self {
            ring,
            storage,
            pool,
            frames,
            mut delay,
            pipeline_observer,
        } = self;
        delay
            .after_micros(ESP32S31_RX_WALKER_ENABLE_SETTLE_US)
            .await;
        match ring.try_start(hardware) {
            Ok(ring) => Ok(Esp32s31ConnectedRx {
                ring,
                storage,
                pool,
                frames,
                delay,
                pipeline_observer,
            }),
            Err((ring, error)) => Err((
                Self {
                    ring,
                    storage,
                    pool,
                    frames,
                    delay,
                    pipeline_observer,
                },
                error,
            )),
        }
    }
}

impl<
    'storage,
    'pool,
    'queue,
    D,
    M: RawMutex,
    const QUEUE_DEPTH: usize,
    const COUNT: usize,
    const STAGE_CAPACITY: usize,
    const STAGE_SLOTS: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
>
    Esp32s31ConnectedRx<
        'storage,
        'pool,
        'queue,
        D,
        M,
        QUEUE_DEPTH,
        COUNT,
        STAGE_CAPACITY,
        STAGE_SLOTS,
        DMA_BUFFER_SIZE,
        DMA_STORAGE_SIZE,
    >
{
    pub fn new(
        ring: RxRingLive<'storage, COUNT>,
        storage: &'storage Esp32s31RxDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
        pool: &'pool RxStagePool<STAGE_SLOTS, STAGE_CAPACITY>,
        delay: D,
        frames: Sender<
            'queue,
            M,
            Esp32s31StagedRxFrame<'pool, STAGE_CAPACITY, STAGE_SLOTS>,
            QUEUE_DEPTH,
        >,
    ) -> Self {
        Self {
            ring,
            storage,
            pool,
            frames,
            delay,
            pipeline_observer: None,
        }
    }

    pub fn with_pipeline_observer(mut self, observer: &'pool dyn RxPipelineObserver) -> Self {
        self.pipeline_observer = Some(observer);
        self
    }

    pub const fn ring(&self) -> &RxRingLive<'storage, COUNT> {
        &self.ring
    }

    pub fn queued_frames(&self) -> usize {
        self.frames.len()
    }

    /// Confirm that DMA released the ring and return a stopped RX owner.
    ///
    /// On failure the complete live owner is returned together with the
    /// hardware error; no staging, queue or delay capability is lost.
    #[allow(clippy::result_large_err)]
    pub fn try_stop<H: RxDma>(
        self,
        hardware: &mut H,
    ) -> Result<
        Esp32s31StoppedRx<
            'storage,
            'pool,
            'queue,
            D,
            M,
            QUEUE_DEPTH,
            COUNT,
            STAGE_CAPACITY,
            STAGE_SLOTS,
            DMA_BUFFER_SIZE,
            DMA_STORAGE_SIZE,
        >,
        (Self, RxRingError),
    > {
        let Self {
            ring,
            storage,
            pool,
            frames,
            delay,
            pipeline_observer,
        } = self;
        match ring.try_stop(hardware) {
            Ok(ring) => Ok(Esp32s31StoppedRx {
                ring,
                storage,
                pool,
                frames,
                delay,
                pipeline_observer,
            }),
            Err((ring, error)) => Err((
                Self {
                    ring,
                    storage,
                    pool,
                    frames,
                    delay,
                    pipeline_observer,
                },
                error,
            )),
        }
    }
}

impl<
    'storage,
    'pool,
    'queue,
    H,
    D,
    M: RawMutex,
    const QUEUE_DEPTH: usize,
    const COUNT: usize,
    const STAGE_CAPACITY: usize,
    const STAGE_SLOTS: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
> Esp32s31ConnectedRxService<H>
    for Esp32s31ConnectedRx<
        'storage,
        'pool,
        'queue,
        D,
        M,
        QUEUE_DEPTH,
        COUNT,
        STAGE_CAPACITY,
        STAGE_SLOTS,
        DMA_BUFFER_SIZE,
        DMA_STORAGE_SIZE,
    >
where
    H: RxDma,
    D: RxReloadDelay,
{
    type Error = RxStageTransactionError;

    fn service<'a>(
        &'a mut self,
        hardware: &'a mut H,
    ) -> impl Future<Output = Result<WifiRxProgress, Self::Error>> + 'a {
        async move {
            let service_started = self
                .pipeline_observer
                .map(|observer| observer.begin_service());
            let hardware_buffer_full = self
                .pipeline_observer
                .and_then(|_| hardware.buffer_full_count());
            // Freeze the completion frontier before any descriptor is rearmed.
            // A saturated producer can therefore only create a later epoch; it
            // cannot make this service call unbounded by refilling the ring.
            let frontier_snapshot = self.ring.completed_unit_frontier_with(|index| {
                // SAFETY: this is only a volatile guard observation. A word
                // different from the recycle sentinel proves that DMA has
                // begun consuming this non-terminal buffer; ownership is not
                // transferred until a later terminal descriptor is visible.
                self.storage.buffers()[index].leading_guard_overwritten()
            });
            let frontier = frontier_snapshot.unit_count;
            let pool_credits = self.pool.available_slots();
            let queue_credits = self.frames.free_capacity();
            let credits = pool_credits.min(queue_credits);
            let admitted = frontier.min(credits);
            let mut staged_bytes = 0_usize;
            let mut remaining_descriptors = frontier_snapshot.descriptor_count;

            for _ in 0..admitted {
                let unit = self
                    .ring
                    .take_completed_unit(remaining_descriptors)
                    .ok_or(RxStageTransactionError::Ring(RxRingError::Corrupt))?;
                let head_index = unit.head_index();
                let unit_descriptor_count = unit.descriptor_count();
                remaining_descriptors = remaining_descriptors
                    .checked_sub(unit_descriptor_count)
                    .ok_or(RxStageTransactionError::Ring(RxRingError::Corrupt))?;
                let pending = self.pool.stage_unit_recycle(
                    unit,
                    |step, destination| {
                        let index = (head_index + step) % COUNT;
                        // SAFETY: `take_completed_unit` transferred every
                        // segment through its terminal descriptor. The copy
                        // completes before the recycle closure can publish any
                        // of those buffers back to DMA.
                        let source = unsafe { self.storage.buffers()[index].completed() };
                        let source = source
                            .get(..destination.len())
                            .ok_or(RxStageError::SourceTooShort)?;
                        destination.copy_from_slice(source);
                        Ok(())
                    },
                    hardware,
                    &mut self.ring,
                    |recycled| {
                        // SAFETY: the ring invokes this only for its owned
                        // completed prefix immediately before publication.
                        unsafe { self.storage.buffers()[recycled].prepare_for_recycle() }
                    },
                );
                let pending = match pending {
                    Ok(pending) => pending,
                    Err(RxStageTransactionError::Stage(
                        error @ (RxStageError::Empty | RxStageError::TooLong),
                    )) => {
                        // Length is supplied by an untrusted receive unit. A
                        // malformed/FCS/oversize unit must not terminate the
                        // sole radio owner: the vendor path discards such a
                        // frame and immediately returns its descriptor to the
                        // DMA walker. Preserve that ownership order and the
                        // asynchronous reload edge without publishing a
                        // staging token.
                        if let Some(observer) = self.pipeline_observer {
                            let discard = match error {
                                RxStageError::Empty => RxStageDiscard::Empty,
                                RxStageError::TooLong => RxStageDiscard::TooLong,
                                _ => unreachable!("match arm admits only length discards"),
                            };
                            observer.observe(RxPipelineObservation::StageDiscarded(discard));
                        }
                        let append = self
                            .ring
                            .recycle_completed_unit(hardware, unit_descriptor_count, |recycled| {
                                // SAFETY: this is the same uniquely observed
                                // descriptor rejected before staging copied it.
                                unsafe { self.storage.buffers()[recycled].prepare_for_recycle() }
                            })
                            .map_err(RxStageTransactionError::Ring)?
                            .ok_or(RxStageTransactionError::Ring(RxRingError::Busy))?;
                        if append.descriptor_count != unit_descriptor_count {
                            return Err(RxStageTransactionError::Ring(RxRingError::Corrupt));
                        }
                        loop {
                            match self
                                .ring
                                .poll_pending_reload(hardware)
                                .map_err(RxStageTransactionError::Ring)?
                            {
                                RxReloadObservation::Pending => self.delay.after_micros(1).await,
                                RxReloadObservation::Settled => break,
                            }
                        }
                        continue;
                    }
                    Err(error) => return Err(error),
                };
                let frame =
                    await_staged_rx_reload(pending, hardware, &mut self.ring, &mut self.delay)
                        .await?;
                staged_bytes = staged_bytes.saturating_add(frame.length());
                self.frames.try_send(frame).map_err(|error| match error {
                    TrySendError::Full(_) => RxStageTransactionError::Ring(RxRingError::Corrupt),
                })?;
            }

            if let (Some(observer), Some(started)) = (self.pipeline_observer, service_started) {
                observer.observe(RxPipelineObservation::ServiceCompleted(
                    RxServiceObservation {
                        frontier,
                        pool_credits,
                        queue_credits,
                        admitted,
                        staged_bytes,
                        micros: observer.elapsed_micros_since(started),
                        hardware_buffer_full,
                    },
                ));
            }

            Ok(if admitted < frontier {
                WifiRxProgress::Backpressured
            } else {
                WifiRxProgress::Drained
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use core::{
        future::ready,
        sync::atomic::{AtomicU32, Ordering},
    };

    use embassy_sync::channel::TryReceiveError;
    use open_esp_radio_embassy_net::NoopRawMutex;
    use open_esp_radio_esp32s31_wifi_lmac::{
        connected_rx::{
            ConnectedRxConfig, ConnectedRxDispatcher, ConnectedRxEvent, ConnectedRxSink,
        },
        descriptor::{BIT_30, BIT_31, DESCRIPTOR_BYTES, LENGTH_SHIFT},
        rx::{PUBLIC_HEADER_SIZE, RxIngressConfig, RxRingStopped},
    };

    use super::*;
    use crate::{
        embassy_irq::EmbassyMacIrqRuntime,
        rx_reorder::{RxReorderCommand, RxReorderCommandResources, try_send_rx_reorder_command},
        staged_rx::{Esp32s31ConnectedRxProtocol, Esp32s31StagedRxQueue},
    };

    const BASE: u32 = 0x2f00_1000;

    #[derive(Default)]
    struct RecordingRxObserver {
        stage_too_long_discards: AtomicU32,
    }

    impl RxPipelineObserver for RecordingRxObserver {
        fn now_micros(&self) -> u64 {
            0
        }

        fn observe(&self, observation: RxPipelineObservation) {
            if matches!(
                observation,
                RxPipelineObservation::StageDiscarded(RxStageDiscard::TooLong)
            ) {
                self.stage_too_long_discards.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    #[derive(Default)]
    struct MockRxDma {
        walker: bool,
        descriptor_base: u32,
        fail_enable: bool,
    }

    impl RxDma for MockRxDma {
        fn last_descriptor_low(&mut self) -> u32 {
            0
        }
        fn next_descriptor_low(&mut self) -> u32 {
            BASE + DESCRIPTOR_BYTES
        }
        fn walker_enabled(&mut self) -> bool {
            self.walker
        }
        fn reload_pending(&mut self) -> bool {
            false
        }
        fn set_descriptor_high_window(&mut self, _address_high: u16) {}
        fn write_descriptor_base(&mut self, address: u32) {
            self.descriptor_base = address;
        }
        fn publish_walker_enable(&mut self) {
            self.walker = true;
        }
        fn request_reload(&mut self) {}
        fn try_enable_walker(&mut self) -> bool {
            if self.fail_enable {
                return false;
            }
            self.walker = true;
            true
        }
        fn try_disable_walker(&mut self) -> bool {
            self.walker = false;
            true
        }
        fn fence(&mut self) {}
    }

    struct NoDelay;

    impl RxReloadDelay for NoDelay {
        fn after_micros(&mut self, _micros: u32) -> impl Future<Output = ()> + '_ {
            ready(())
        }
    }

    #[derive(Default)]
    struct Observer(u32);

    impl ConnectedRxSink for Observer {
        fn publish(&mut self, _event: ConnectedRxEvent<'_>) {
            self.0 += 1;
        }
    }

    #[derive(Default)]
    struct OrderObserver(std::vec::Vec<u16>);

    impl ConnectedRxSink for OrderObserver {
        fn publish(&mut self, event: ConnectedRxEvent<'_>) {
            if let ConnectedRxEvent::Ethernet { raw, .. } = event {
                let sequence_control = u16::from_le_bytes([
                    raw[PUBLIC_HEADER_SIZE + 22],
                    raw[PUBLIC_HEADER_SIZE + 23],
                ]);
                self.0.push(sequence_control >> 4);
            }
        }
    }

    fn dispatcher() -> ConnectedRxDispatcher {
        ConnectedRxDispatcher::new(ConnectedRxConfig {
            station_address: [2, 3, 4, 5, 6, 7],
            bssid: [8, 9, 10, 11, 12, 13],
            association_id: 1,
            ingress: RxIngressConfig {
                ring_entry_limit: 1,
                csi_config: 0,
                flags: 0,
            },
        })
    }

    #[test]
    fn finite_service_uses_queue_credits_and_protocol_dispatch_returns_ownership() {
        const COUNT: usize = 2;
        const STAGED_DEPTH: usize = 1;
        let storage = Esp32s31RxDmaStorage::<COUNT>::new();
        let addresses = [0x2f00_2000, 0x2f00_3200];
        let mut hardware = MockRxDma::default();
        let stopped = RxRingStopped::prepare(
            &mut hardware,
            storage.descriptors(),
            BASE,
            &addresses,
            ESP32S31_RX_BUFFER_SIZE as u32,
            |_| Ok(()),
        )
        .unwrap();
        let ring = stopped.start(&mut hardware).unwrap();
        storage.descriptors()[0]
            .write_word0(ESP32S31_RX_BUFFER_SIZE as u32 | (8 << LENGTH_SHIFT) | BIT_30 | BIT_31);
        storage.descriptors()[1]
            .write_word0(ESP32S31_RX_BUFFER_SIZE as u32 | (8 << LENGTH_SHIFT) | BIT_30 | BIT_31);
        let pool = RxStagePool::new();
        let queue = Esp32s31StagedRxQueue::<NoopRawMutex, STAGED_DEPTH>::new();
        let (sender, receiver) = queue.split();
        let irq = EmbassyMacIrqRuntime::<NoopRawMutex>::new();
        let mut mpdu = [0; ESP32S31_RX_BUFFER_SIZE];
        let mut ethernet = [0; ESP32S31_RX_BUFFER_SIZE];
        let mut service = Esp32s31ConnectedRx::new(ring, &storage, &pool, NoDelay, sender);
        let mut protocol = Esp32s31ConnectedRxProtocol::new(
            receiver,
            &irq,
            dispatcher(),
            crate::staged_rx::AlwaysReadyConnectedRxSink(Observer::default()),
            &mut mpdu,
            &mut ethernet,
        );

        assert_eq!(
            embassy_futures::block_on(service.service(&mut hardware)),
            Ok(WifiRxProgress::Backpressured),
        );
        assert_eq!(pool.claimed_slots(), 1);
        assert_eq!(pool.network_slots(), 1);
        assert_eq!(protocol.queue_len(), 1);
        embassy_futures::block_on(protocol.dispatch_next());
        assert_eq!(pool.claimed_slots(), 0);
        assert_eq!(pool.network_slots(), 0);
        assert_eq!(service.ring().recycle_start(), 1);
        assert_eq!(storage.descriptors()[0].word0() & BIT_30, 0);
        assert_ne!(storage.descriptors()[0].word0() & BIT_31, 0);

        assert_eq!(
            embassy_futures::block_on(service.service(&mut hardware)),
            Ok(WifiRxProgress::Drained),
        );
        assert_eq!(service.ring().recycle_start(), 0);
        assert_eq!(protocol.queue_len(), 1);
        embassy_futures::block_on(protocol.dispatch_next());
        assert_eq!(pool.claimed_slots(), 0);
        assert_eq!(pool.network_slots(), 0);
    }

    #[test]
    fn connected_rx_stop_confirms_walker_off_and_preserves_static_resources() {
        const COUNT: usize = 2;
        const STAGED_DEPTH: usize = 1;
        let storage = Esp32s31RxDmaStorage::<COUNT>::new();
        let addresses = [0x2f00_2000, 0x2f00_3200];
        let mut hardware = MockRxDma::default();
        let stopped = RxRingStopped::prepare(
            &mut hardware,
            storage.descriptors(),
            BASE,
            &addresses,
            ESP32S31_RX_BUFFER_SIZE as u32,
            |_| Ok(()),
        )
        .unwrap();
        let ring = stopped.start(&mut hardware).unwrap();
        let pool = RxStagePool::<STAGED_DEPTH, ESP32S31_RX_BUFFER_SIZE>::new();
        let queue = Esp32s31StagedRxQueue::<
            NoopRawMutex,
            STAGED_DEPTH,
            ESP32S31_RX_BUFFER_SIZE,
            STAGED_DEPTH,
        >::new();
        let (sender, _receiver) = queue.split();
        let service = Esp32s31ConnectedRx::new(ring, &storage, &pool, NoDelay, sender);
        assert!(hardware.walker);

        let stopped = match service.try_stop(&mut hardware) {
            Ok(stopped) => stopped,
            Err(_) => panic!("mock walker must confirm the stop edge"),
        };

        assert!(!hardware.walker);
        assert_eq!(stopped.ring().descriptor_base(), BASE);
        assert_eq!(stopped.ring().buffer_addresses(), &addresses);
        assert_eq!(stopped.queued_frames(), 0);

        let (ring, epoch_resources) = stopped.into_epoch_parts();
        assert_eq!(ring.descriptor_base(), BASE);
        assert_eq!(epoch_resources.queued_frames(), 0);
        let stopped = epoch_resources.with_halted_ring(ring);

        let prepared = match stopped.prepare(&mut hardware) {
            Ok(prepared) => prepared,
            Err(_) => panic!("halted owner must rebuild the next descriptor epoch"),
        };
        assert!(!hardware.walker);
        assert_eq!(prepared.ring().initial_start(), 0);
        hardware.fail_enable = true;
        let prepared = match embassy_futures::block_on(prepared.start(&mut hardware)) {
            Ok(_) => panic!("rejected walker enable must not create a live owner"),
            Err((prepared, error)) => {
                assert_eq!(error, RxRingError::Busy);
                prepared
            }
        };
        assert!(!hardware.walker);
        assert_eq!(prepared.ring().initial_start(), 0);
        assert_eq!(prepared.queued_frames(), 0);
        hardware.fail_enable = false;
        let restarted = match embassy_futures::block_on(prepared.start(&mut hardware)) {
            Ok(restarted) => restarted,
            Err(_) => panic!("prepared owner must reopen the mock walker"),
        };
        assert!(hardware.walker);
        assert_eq!(restarted.ring().descriptor_base(), BASE);
        assert_eq!(restarted.queued_frames(), 0);

        let stopped = match restarted.try_stop(&mut hardware) {
            Ok(stopped) => stopped,
            Err(_) => panic!("restarted owner must stop before the split test"),
        };
        let (ring, epoch_resources) = stopped.into_epoch_parts();
        let prepared = match ring.prepare(&mut hardware, ESP32S31_RX_BUFFER_SIZE as u32, |index| {
            // SAFETY: the halted ring proves the mock walker released
            // every matching test buffer before this preparation edge.
            unsafe { storage.buffers()[index].prepare_for_recycle() }
        }) {
            Ok(prepared) => prepared,
            Err(_) => panic!("split halted ring must rebuild"),
        };
        let ring = prepared.start(&mut hardware).unwrap();
        let restarted = epoch_resources.with_live_ring(ring);
        assert!(hardware.walker);
        assert_eq!(restarted.ring().descriptor_base(), BASE);
        assert_eq!(restarted.queued_frames(), 0);
    }

    #[test]
    fn finite_service_stages_a_descriptor_chain_as_one_contiguous_unit() {
        const COUNT: usize = 2;
        const STAGED_DEPTH: usize = 1;
        const STAGE_CAPACITY: usize = 16;
        let storage = Esp32s31RxDmaStorage::<COUNT>::new();
        let addresses = [0x2f00_2000, 0x2f00_3200];
        let mut hardware = MockRxDma::default();
        let stopped = RxRingStopped::prepare(
            &mut hardware,
            storage.descriptors(),
            BASE,
            &addresses,
            ESP32S31_RX_BUFFER_SIZE as u32,
            |_| Ok(()),
        )
        .unwrap();
        let ring = stopped.start(&mut hardware).unwrap();
        // SAFETY: the mock walker never accesses host storage; the test owns
        // both buffers until service copies and recycles the complete unit.
        unsafe {
            core::ptr::copy_nonoverlapping(
                [1, 2, 3, 4].as_ptr(),
                storage.buffers()[0].completed().as_ptr().cast_mut(),
                4,
            );
            core::ptr::copy_nonoverlapping(
                [5, 6, 7, 8].as_ptr(),
                storage.buffers()[1].completed().as_ptr().cast_mut(),
                4,
            );
        }
        storage.descriptors()[0]
            .write_word0(ESP32S31_RX_BUFFER_SIZE as u32 | (4 << LENGTH_SHIFT) | BIT_31);
        storage.descriptors()[1]
            .write_word0(ESP32S31_RX_BUFFER_SIZE as u32 | (4 << LENGTH_SHIFT) | BIT_30 | BIT_31);
        let pool = RxStagePool::<STAGED_DEPTH, STAGE_CAPACITY>::new();
        let queue =
            Esp32s31StagedRxQueue::<NoopRawMutex, STAGED_DEPTH, STAGE_CAPACITY, STAGED_DEPTH>::new(
            );
        let (sender, receiver) = queue.split();
        let mut service = Esp32s31ConnectedRx::new(ring, &storage, &pool, NoDelay, sender);

        assert_eq!(
            embassy_futures::block_on(service.service(&mut hardware)),
            Ok(WifiRxProgress::Drained),
        );
        let frame = receiver.try_receive().expect("one chained staged unit");
        assert_eq!(frame.segment().buffer, &[1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(service.ring().recycle_start(), 0);
        drop(frame);
        assert_eq!(pool.claimed_slots(), 0);
    }

    #[test]
    fn negotiated_rx_block_ack_releases_staged_leases_in_sequence_order() {
        const COUNT: usize = 4;
        const STAGED_DEPTH: usize = 3;
        const STAGE_CAPACITY: usize = 192;
        const MPDU: usize = 26 + 8 + 8 + 4 + 8;
        const SIGNAL: usize = MPDU + 4;
        const RECEIVED: usize = PUBLIC_HEADER_SIZE + SIGNAL;

        let storage = Esp32s31RxDmaStorage::<COUNT>::new();
        let addresses = [0x2f00_2000, 0x2f00_2200, 0x2f00_2400, 0x2f00_2600];
        let mut hardware = MockRxDma::default();
        let stopped = RxRingStopped::prepare(
            &mut hardware,
            storage.descriptors(),
            BASE,
            &addresses,
            ESP32S31_RX_BUFFER_SIZE as u32,
            |_| Ok(()),
        )
        .unwrap();
        let ring = stopped.start(&mut hardware).unwrap();

        let mut buffer = [0_u8; ESP32S31_RX_BUFFER_SIZE];
        for (index, sequence) in [102_u16, 100, 101].into_iter().enumerate() {
            buffer.fill(0);
            buffer[0x38..0x3c]
                .copy_from_slice(&(((SIGNAL + 4) as u32) << 16 | SIGNAL as u32).to_le_bytes());
            let frame = &mut buffer[PUBLIC_HEADER_SIZE..PUBLIC_HEADER_SIZE + MPDU];
            frame[..2].copy_from_slice(&0x4288_u16.to_le_bytes());
            frame[4..10].copy_from_slice(&[2, 3, 4, 5, 6, 7]);
            frame[10..16].copy_from_slice(&[8, 9, 10, 11, 12, 13]);
            frame[16..22].copy_from_slice(&[14, 15, 16, 17, 18, 19]);
            frame[22..24].copy_from_slice(&(sequence << 4).to_le_bytes());
            frame[24] = 0;
            frame[26..34].copy_from_slice(&[3, 0, 0, 0x20, 0, 0, 0, 0]);
            frame[34..42].copy_from_slice(&[0xaa, 0xaa, 0x03, 0, 0, 0, 0x08, 0x00]);
            frame[42..46].copy_from_slice(&sequence.to_be_bytes().repeat(2));
            // SAFETY: the test owns the stopped DMA storage before service
            // publishes any descriptor back to the mock walker.
            unsafe {
                core::ptr::copy_nonoverlapping(
                    buffer.as_ptr(),
                    storage.buffers()[index].completed().as_ptr().cast_mut(),
                    buffer.len(),
                );
            }
            storage.descriptors()[index].write_word0(
                STAGE_CAPACITY as u32 | ((RECEIVED as u32) << LENGTH_SHIFT) | BIT_30 | BIT_31,
            );
        }

        let pool = RxStagePool::<STAGED_DEPTH, STAGE_CAPACITY>::new();
        // Declared before the queue because protocol frame types borrow both
        // pools and Rust drops local owners in reverse declaration order.
        let reorder_storage = crate::rx_reorder::RxReorderFrameStorage::<STAGE_CAPACITY>::new();
        let queue =
            Esp32s31StagedRxQueue::<NoopRawMutex, STAGED_DEPTH, STAGE_CAPACITY, STAGED_DEPTH>::new(
            );
        let (sender, receiver) = queue.split();
        let reorder_resources = RxReorderCommandResources::<NoopRawMutex>::new();
        let (reorder_sender, reorder_receiver) = reorder_resources.split();
        try_send_rx_reorder_command(
            &reorder_sender,
            RxReorderCommand::Start {
                tid: 0,
                starting_sequence: 100,
                window: 8,
            },
        )
        .unwrap();
        let irq = EmbassyMacIrqRuntime::<NoopRawMutex>::new();
        let mut mpdu = [0; STAGE_CAPACITY];
        let mut ethernet = [0; STAGE_CAPACITY];
        let mut reorder_scratch = [0; STAGE_CAPACITY];
        let mut service = Esp32s31ConnectedRx::new(ring, &storage, &pool, NoDelay, sender);
        let mut protocol = Esp32s31ConnectedRxProtocol::new(
            receiver,
            &irq,
            dispatcher(),
            crate::staged_rx::AlwaysReadyConnectedRxSink(OrderObserver::default()),
            &mut mpdu,
            &mut ethernet,
        )
        .with_rx_reorder_commands(reorder_receiver)
        .with_rx_reorder_storage(&reorder_storage)
        .with_rx_reorder_scratch(&mut reorder_scratch);

        assert_eq!(
            embassy_futures::block_on(service.service(&mut hardware)),
            Ok(WifiRxProgress::Drained),
        );
        assert_eq!(pool.claimed_slots(), 3);
        embassy_futures::block_on(protocol.dispatch_next());
        assert_eq!(protocol.sink().0.0, [100]);
        // Sequence 102 is retained in the cold reorder backing while 100 has
        // been dispatched. The former implementation retained both staging
        // leases here and therefore reported two claimed SRAM slots.
        assert_eq!(pool.claimed_slots(), 1);
        assert_eq!(
            reorder_storage.available_slots(),
            crate::rx_reorder::RX_REORDER_BACKING_SLOT_COUNT - 1
        );
        embassy_futures::block_on(protocol.dispatch_next());
        assert_eq!(protocol.sink().0.0, [100, 101, 102]);
        assert_eq!(pool.claimed_slots(), 0);
        assert_eq!(
            reorder_storage.available_slots(),
            crate::rx_reorder::RX_REORDER_BACKING_SLOT_COUNT
        );
    }

    #[test]
    fn finite_service_discards_oversize_unit_and_keeps_the_ring_live() {
        const COUNT: usize = 2;
        const STAGED_DEPTH: usize = 1;
        let storage = Esp32s31RxDmaStorage::<COUNT>::new();
        let addresses = [0x2f00_2000, 0x2f00_3200];
        let mut hardware = MockRxDma::default();
        let stopped = RxRingStopped::prepare(
            &mut hardware,
            storage.descriptors(),
            BASE,
            &addresses,
            ESP32S31_RX_BUFFER_SIZE as u32,
            |_| Ok(()),
        )
        .unwrap();
        let ring = stopped.start(&mut hardware).unwrap();
        storage.descriptors()[0].write_word0(
            ESP32S31_RX_BUFFER_SIZE as u32
                | ((VENDOR_LARGE_RX_PAYLOAD_CAPACITY as u32 + 1) << LENGTH_SHIFT)
                | BIT_30
                | BIT_31,
        );
        let pool = RxStagePool::new();
        let observer = RecordingRxObserver::default();
        let queue = Esp32s31StagedRxQueue::<NoopRawMutex, STAGED_DEPTH>::new();
        let (sender, receiver) = queue.split();
        let mut service = Esp32s31ConnectedRx::new(ring, &storage, &pool, NoDelay, sender)
            .with_pipeline_observer(&observer);

        assert_eq!(
            embassy_futures::block_on(service.service(&mut hardware)),
            Ok(WifiRxProgress::Drained),
        );
        assert_eq!(service.ring().recycle_start(), 1);
        assert_eq!(storage.descriptors()[0].word0() & BIT_30, 0);
        assert_ne!(storage.descriptors()[0].word0() & BIT_31, 0);
        assert_eq!(pool.claimed_slots(), 0);
        assert!(matches!(
            receiver.try_receive(),
            Err(TryReceiveError::Empty)
        ));
        assert_eq!(observer.stage_too_long_discards.load(Ordering::Relaxed), 1);

        // The recovered discard path is not a reset frontier: the following
        // descriptor is still accepted, staged and returned to the caller.
        storage.descriptors()[1]
            .write_word0(ESP32S31_RX_BUFFER_SIZE as u32 | (4 << LENGTH_SHIFT) | BIT_30 | BIT_31);
        assert_eq!(
            embassy_futures::block_on(service.service(&mut hardware)),
            Ok(WifiRxProgress::Drained),
        );
        let next = receiver.try_receive().expect("post-discard frame");
        assert_eq!(next.length(), 4);
        drop(next);
        assert_eq!(pool.claimed_slots(), 0);
    }

    #[test]
    fn finite_service_accepts_a_unit_within_a_wider_negotiated_stage() {
        const COUNT: usize = 2;
        const STAGED_DEPTH: usize = 1;
        const WIDE_STAGE_CAPACITY: usize = VENDOR_LARGE_RX_PAYLOAD_CAPACITY + 1;
        let storage = Esp32s31RxDmaStorage::<COUNT>::new();
        let addresses = [0x2f00_2000, 0x2f00_3200];
        let mut hardware = MockRxDma::default();
        let stopped = RxRingStopped::prepare(
            &mut hardware,
            storage.descriptors(),
            BASE,
            &addresses,
            ESP32S31_RX_BUFFER_SIZE as u32,
            |_| Ok(()),
        )
        .unwrap();
        let ring = stopped.start(&mut hardware).unwrap();
        storage.descriptors()[0].write_word0(
            ESP32S31_RX_BUFFER_SIZE as u32
                | ((WIDE_STAGE_CAPACITY as u32) << LENGTH_SHIFT)
                | BIT_30
                | BIT_31,
        );
        let pool = RxStagePool::<VENDOR_LARGE_RX_SLOT_COUNT, WIDE_STAGE_CAPACITY>::new();
        let queue = Esp32s31StagedRxQueue::<NoopRawMutex, STAGED_DEPTH, WIDE_STAGE_CAPACITY>::new();
        let (sender, receiver) = queue.split();
        let mut service = Esp32s31ConnectedRx::new(ring, &storage, &pool, NoDelay, sender);

        assert_eq!(
            embassy_futures::block_on(service.service(&mut hardware)),
            Ok(WifiRxProgress::Drained),
        );
        let frame = receiver.try_receive().expect("wide staged frame");
        assert_eq!(frame.length(), WIDE_STAGE_CAPACITY);
        drop(frame);
        assert_eq!(pool.claimed_slots(), 0);
    }
}
