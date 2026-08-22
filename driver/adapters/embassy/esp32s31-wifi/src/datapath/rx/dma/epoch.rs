#![expect(
    clippy::result_large_err,
    reason = "no-alloc RX transitions return the exact staged DMA owner on failure"
)]

use super::*;
use crate::{
    datapath::rx::frontier::Esp32s31RxFrontierSchedulerSnapshot,
    datapath::services::DatapathRxService,
};

enum Esp32s31StagedRxEpochState<
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
> {
    Stopped(
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
    ),
    Prepared(
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
    ),
    Live(
        Esp32s31StagedRxProducer<
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
    ),
    Vacant,
}

/// Role-neutral halted/live lifecycle around the common staged-RX producer.
///
/// The owner is intentionally independent of STA/AP protocol semantics. Its
/// only live operation copies a finite LAST-bounded DMA frontier into unique
/// staging leases, republishes safe descriptors, and reports ownership facts.
pub struct Esp32s31StagedRxEpoch<
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
> {
    state: Esp32s31StagedRxEpochState<
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
    Esp32s31StagedRxEpoch<
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
    D: RxDmaObservationDelay,
{
    pub fn from_halted(
        ring: RxRingHalted<'storage, COUNT>,
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
        Self::from_halted_resources(
            ring,
            Esp32s31RxEpochResources::new(storage, pool, frames, delay),
        )
    }

    /// Build the sole descriptor epoch for a same-channel STA+AP pair.
    /// Routing is recorded in one ordered queue after hardware normalization;
    /// role protocol consumers never receive DMA ownership.
    pub fn from_halted_sta_ap(
        ring: RxRingHalted<'storage, COUNT>,
        storage: &'storage Esp32s31RxDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
        pool: &'pool RxStagePool<STAGE_SLOTS, STAGE_CAPACITY>,
        frames: Sender<
            'queue,
            M,
            crate::roles::concurrent::Esp32s31StaApStagedRxFrame<
                'pool,
                STAGE_CAPACITY,
                STAGE_SLOTS,
            >,
            QUEUE_DEPTH,
        >,
        ingress: open_esp_radio_esp32s31_wifi_mac::rx::RxIngressConfig,
        addresses: open_esp_radio_ieee80211::vif::StaApRxAddresses,
        delay: D,
    ) -> Self {
        Self::from_halted_resources(
            ring,
            Esp32s31RxEpochResources::new_sta_ap(storage, pool, frames, ingress, addresses, delay),
        )
    }

    /// Replace an empty standalone producer endpoint with the one ordered
    /// STA/AP endpoint while preserving the halted ring, storage, pool,
    /// observation delay, and pipeline observer.
    ///
    /// The conversion fails closed if a standalone staging lease is still
    /// queued. Such a lease must be consumed by the station protocol before
    /// the route-tagged epoch can become the sole producer.
    pub fn try_from_stopped_sta_ap(
        stopped: Esp32s31StoppedRx<
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
        frames: Sender<
            'queue,
            M,
            crate::roles::concurrent::Esp32s31StaApStagedRxFrame<
                'pool,
                STAGE_CAPACITY,
                STAGE_SLOTS,
            >,
            QUEUE_DEPTH,
        >,
        ingress: open_esp_radio_esp32s31_wifi_mac::rx::RxIngressConfig,
        addresses: open_esp_radio_ieee80211::vif::StaApRxAddresses,
    ) -> Result<
        Self,
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
    > {
        if stopped.queued_frames() != 0 {
            return Err(stopped);
        }
        let Esp32s31StoppedRx {
            ring,
            storage,
            pool,
            frames: _,
            delay,
            pipeline_observer,
        } = stopped;
        let resources =
            Esp32s31RxEpochResources::new_sta_ap(storage, pool, frames, ingress, addresses, delay);
        let resources = match pipeline_observer {
            Some(observer) => resources.with_pipeline_observer(observer),
            None => resources,
        };
        Ok(Self::from_halted_resources(ring, resources))
    }

    /// Bind the same value-only pipeline observer used by a connected STA to
    /// a role-neutral halted RX epoch.
    pub fn from_halted_with_pipeline_observer(
        ring: RxRingHalted<'storage, COUNT>,
        storage: &'storage Esp32s31RxDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
        pool: &'pool RxStagePool<STAGE_SLOTS, STAGE_CAPACITY>,
        frames: Sender<
            'queue,
            M,
            Esp32s31StagedRxFrame<'pool, STAGE_CAPACITY, STAGE_SLOTS>,
            QUEUE_DEPTH,
        >,
        delay: D,
        observer: &'pool dyn RxPipelineObserver,
    ) -> Self {
        Self::from_halted_resources(
            ring,
            Esp32s31RxEpochResources::new(storage, pool, frames, delay)
                .with_pipeline_observer(observer),
        )
    }

    fn from_halted_resources(
        ring: RxRingHalted<'storage, COUNT>,
        resources: Esp32s31RxEpochResources<
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
    ) -> Self {
        Self {
            state: Esp32s31StagedRxEpochState::Stopped(resources.with_halted_ring(ring)),
        }
    }

    pub async fn start<H: RxDma>(
        &mut self,
        hardware: &mut H,
    ) -> Result<(), RxStageTransactionError> {
        let state = core::mem::replace(&mut self.state, Esp32s31StagedRxEpochState::Vacant);
        let prepared = match state {
            Esp32s31StagedRxEpochState::Stopped(stopped) => match stopped.prepare(hardware) {
                Ok(prepared) => prepared,
                Err((stopped, error)) => {
                    self.state = Esp32s31StagedRxEpochState::Stopped(stopped);
                    return Err(RxStageTransactionError::Ring(error));
                }
            },
            Esp32s31StagedRxEpochState::Prepared(prepared) => prepared,
            state => {
                self.state = state;
                return Err(RxStageTransactionError::Ring(RxRingError::Busy));
            }
        };
        match prepared.start(hardware).await {
            Ok(live) => {
                self.state = Esp32s31StagedRxEpochState::Live(live);
                Ok(())
            }
            Err((prepared, error)) => {
                self.state = Esp32s31StagedRxEpochState::Prepared(prepared);
                Err(RxStageTransactionError::Ring(error))
            }
        }
    }

    pub async fn service<H: RxDma>(
        &mut self,
        hardware: &mut H,
    ) -> Result<DatapathRxProgress, RxStageTransactionError> {
        let Esp32s31StagedRxEpochState::Live(live) = &mut self.state else {
            return Err(RxStageTransactionError::Ring(RxRingError::Busy));
        };
        DatapathRxService::service(live, hardware).await
    }

    pub fn stop<H: RxDma>(&mut self, hardware: &mut H) -> Result<(), RxStageTransactionError> {
        let state = core::mem::replace(&mut self.state, Esp32s31StagedRxEpochState::Vacant);
        let Esp32s31StagedRxEpochState::Live(live) = state else {
            self.state = state;
            return Err(RxStageTransactionError::Ring(RxRingError::Busy));
        };
        match live.try_stop(hardware) {
            Ok(stopped) => {
                self.state = Esp32s31StagedRxEpochState::Stopped(stopped);
                Ok(())
            }
            Err((live, error)) => {
                self.state = Esp32s31StagedRxEpochState::Live(live);
                Err(RxStageTransactionError::Ring(error))
            }
        }
    }

    pub fn serviced_descriptors(&self) -> u64 {
        match &self.state {
            Esp32s31StagedRxEpochState::Live(owner) => owner.serviced_descriptors(),
            Esp32s31StagedRxEpochState::Stopped(_)
            | Esp32s31StagedRxEpochState::Prepared(_)
            | Esp32s31StagedRxEpochState::Vacant => 0,
        }
    }

    pub fn descriptor_snapshot(&self, index: usize) -> Option<RxDescriptorSnapshot> {
        match &self.state {
            Esp32s31StagedRxEpochState::Stopped(owner) => owner.ring().descriptor_snapshot(index),
            Esp32s31StagedRxEpochState::Prepared(owner) => owner.ring().descriptor_snapshot(index),
            Esp32s31StagedRxEpochState::Live(owner) => owner.ring().descriptor_snapshot(index),
            Esp32s31StagedRxEpochState::Vacant => None,
        }
    }

    pub fn scheduler_snapshot(&self) -> Option<Esp32s31RxFrontierSchedulerSnapshot> {
        let Esp32s31StagedRxEpochState::Live(owner) = &self.state else {
            return None;
        };
        let ring = owner.ring();
        Some(Esp32s31RxFrontierSchedulerSnapshot {
            recycle_start: ring.recycle_start(),
            accepted_tail: ring.accepted_tail(),
            observed_mask: ring.observed_mask(),
            topology: ring.topology_snapshot(),
        })
    }

    pub fn try_into_halted(self) -> Result<RxRingHalted<'storage, COUNT>, Self> {
        let Self { state } = self;
        let Esp32s31StagedRxEpochState::Stopped(stopped) = state else {
            return Err(Self { state });
        };
        let (ring, resources) = stopped.into_epoch_parts();
        drop(resources);
        Ok(ring)
    }

    /// Return a stopped paired epoch to the standalone station publisher.
    ///
    /// Every route-tagged lease must already have left the paired queue. The
    /// returned owner retains the same halted hardware and observation state,
    /// so standalone resume does not rebuild ownership from raw addresses.
    pub fn try_into_standalone_stopped(
        self,
        frames: Sender<
            'queue,
            M,
            Esp32s31StagedRxFrame<'pool, STAGE_CAPACITY, STAGE_SLOTS>,
            QUEUE_DEPTH,
        >,
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
        Self,
    > {
        let Self { state } = self;
        let Esp32s31StagedRxEpochState::Stopped(stopped) = state else {
            return Err(Self { state });
        };
        if stopped.queued_frames() != 0 {
            return Err(Self {
                state: Esp32s31StagedRxEpochState::Stopped(stopped),
            });
        }
        let Esp32s31StoppedRx {
            ring,
            storage,
            pool,
            frames: _,
            delay,
            pipeline_observer,
        } = stopped;
        Ok(Esp32s31StoppedRx {
            ring,
            storage,
            pool,
            frames: Esp32s31StagedRxPublisher::standalone(frames),
            delay,
            pipeline_observer,
        })
    }
}
