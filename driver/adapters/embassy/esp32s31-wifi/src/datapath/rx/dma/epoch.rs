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
        storage: &'static Esp32s31RxDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
        pool: &'pool RxStagePool<STAGE_SLOTS, STAGE_CAPACITY>,
        frames: Esp32s31StagedRxSender<'queue, 'pool, M, QUEUE_DEPTH, STAGE_CAPACITY, STAGE_SLOTS>,
        delay: D,
    ) -> Self {
        Self::from_halted_resources(
            ring,
            Esp32s31RxEpochResources::new(storage, pool, frames, delay),
        )
    }

    /// Adopt an already-running physical ring at a logical role boundary.
    /// No descriptor is rebuilt and the hardware walker is not toggled.
    pub fn from_live(
        ring: RxRingLive<'storage, COUNT>,
        storage: &'static Esp32s31RxDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
        pool: &'pool RxStagePool<STAGE_SLOTS, STAGE_CAPACITY>,
        frames: Esp32s31StagedRxSender<'queue, 'pool, M, QUEUE_DEPTH, STAGE_CAPACITY, STAGE_SLOTS>,
        delay: D,
    ) -> Self {
        Self {
            state: Esp32s31StagedRxEpochState::Live(Esp32s31StagedRxProducer::new(
                ring, storage, pool, delay, frames,
            )),
        }
    }

    /// Reattach a retained standalone publisher to its continuously running
    /// physical ring at a logical role boundary.
    ///
    /// Unlike [`Self::from_live`], this does not split a static queue or create
    /// a new producer endpoint. The caller must separately resume the sole
    /// protocol consumer from `resources` before moving the resources here.
    pub fn from_live_resources(
        ring: RxRingLive<'storage, COUNT>,
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
            state: Esp32s31StagedRxEpochState::Live(resources.with_live_ring(ring)),
        }
    }

    /// Reattach a retained standalone publisher to a halted physical ring.
    pub fn from_halted_resources(
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

    #[cfg(any(feature = "diagnostics", test))]
    pub fn from_live_with_pipeline_observer(
        ring: RxRingLive<'storage, COUNT>,
        storage: &'static Esp32s31RxDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
        pool: &'pool RxStagePool<STAGE_SLOTS, STAGE_CAPACITY>,
        frames: Esp32s31StagedRxSender<'queue, 'pool, M, QUEUE_DEPTH, STAGE_CAPACITY, STAGE_SLOTS>,
        delay: D,
        observer: &'pool dyn RxPipelineObserver,
    ) -> Self {
        Self {
            state: Esp32s31StagedRxEpochState::Live(
                Esp32s31StagedRxProducer::new(ring, storage, pool, delay, frames)
                    .with_pipeline_observer(observer),
            ),
        }
    }

    /// Build the sole descriptor epoch for a same-channel STA+AP pair.
    /// Routing is recorded in one ordered queue after hardware normalization;
    /// role protocol consumers never receive DMA ownership.
    pub fn from_halted_sta_ap(
        ring: RxRingHalted<'storage, COUNT>,
        storage: &'static Esp32s31RxDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
        pool: &'pool RxStagePool<STAGE_SLOTS, STAGE_CAPACITY>,
        frames: crate::roles::concurrent::Esp32s31StaApStagedRxSender<
            'pool,
            'queue,
            M,
            QUEUE_DEPTH,
            STAGE_CAPACITY,
            STAGE_SLOTS,
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
        frames: crate::roles::concurrent::Esp32s31StaApStagedRxSender<
            'pool,
            'queue,
            M,
            QUEUE_DEPTH,
            STAGE_CAPACITY,
            STAGE_SLOTS,
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
            #[cfg(any(feature = "diagnostics", test))]
            pipeline_observer,
        } = stopped;
        let resources =
            Esp32s31RxEpochResources::new_sta_ap(storage, pool, frames, ingress, addresses, delay);
        #[cfg(any(feature = "diagnostics", test))]
        let resources = match pipeline_observer {
            Some(observer) => resources.with_pipeline_observer(observer),
            None => resources,
        };
        Ok(Self::from_halted_resources(ring, resources))
    }

    /// Replace an empty standalone publisher with the paired STA/AP publisher
    /// without stopping or rebuilding the physical descriptor ring.
    pub fn try_from_live_sta_ap<P>(
        live: Esp32s31StagedRxProducer<
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
            P,
        >,
        frames: crate::roles::concurrent::Esp32s31StaApStagedRxSender<
            'pool,
            'queue,
            M,
            QUEUE_DEPTH,
            STAGE_CAPACITY,
            STAGE_SLOTS,
        >,
        ingress: open_esp_radio_esp32s31_wifi_mac::rx::RxIngressConfig,
        addresses: open_esp_radio_ieee80211::vif::StaApRxAddresses,
    ) -> Result<
        Self,
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
            P,
        >,
    > {
        let (ring, resources) = match live.try_into_live_epoch_parts() {
            Ok(parts) => parts,
            Err((live, _)) => return Err(live),
        };
        let Esp32s31RxEpochResources {
            ring_lifetime: _,
            storage,
            pool,
            frames: _,
            delay,
            #[cfg(any(feature = "diagnostics", test))]
            pipeline_observer,
        } = resources;
        let producer = Esp32s31StagedRxProducer::new_sta_ap(
            ring, storage, pool, delay, frames, ingress, addresses,
        );
        #[cfg(any(feature = "diagnostics", test))]
        let producer = match pipeline_observer {
            Some(observer) => producer.with_pipeline_observer(observer),
            None => producer,
        };
        Ok(Self {
            state: Esp32s31StagedRxEpochState::Live(producer),
        })
    }

    /// Bind the same value-only pipeline observer used by a connected STA to
    /// a role-neutral halted RX epoch.
    #[cfg(any(feature = "diagnostics", test))]
    pub fn from_halted_with_pipeline_observer(
        ring: RxRingHalted<'storage, COUNT>,
        storage: &'static Esp32s31RxDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
        pool: &'pool RxStagePool<STAGE_SLOTS, STAGE_CAPACITY>,
        frames: Esp32s31StagedRxSender<'queue, 'pool, M, QUEUE_DEPTH, STAGE_CAPACITY, STAGE_SLOTS>,
        delay: D,
        observer: &'pool dyn RxPipelineObserver,
    ) -> Self {
        Self::from_halted_resources(
            ring,
            Esp32s31RxEpochResources::new(storage, pool, frames, delay)
                .with_pipeline_observer(observer),
        )
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
            Esp32s31StagedRxEpochState::Live(live) => {
                self.state = Esp32s31StagedRxEpochState::Live(live);
                return Ok(());
            }
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

    /// Prove that all logical staging ownership has drained while leaving the
    /// physical descriptor epoch live for the next role.
    pub fn park_for_role_handoff(&self) -> Result<(), RxStageTransactionError> {
        let Esp32s31StagedRxEpochState::Live(live) = &self.state else {
            return Err(RxStageTransactionError::Ring(RxRingError::Busy));
        };
        if live.can_park_for_role_handoff() {
            Ok(())
        } else {
            Err(RxStageTransactionError::Ring(RxRingError::Busy))
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

    /// Preserve physical work accounting across the halted/live role wrapper.
    pub fn work_counters(&self) -> DatapathRxWorkCounters {
        match &self.state {
            Esp32s31StagedRxEpochState::Live(owner) => owner.work_counters(),
            Esp32s31StagedRxEpochState::Stopped(_)
            | Esp32s31StagedRxEpochState::Prepared(_)
            | Esp32s31StagedRxEpochState::Vacant => DatapathRxWorkCounters::default(),
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

    /// Observe instantaneous hardware credits without exposing descriptor
    /// ownership or publication authority.
    pub fn try_into_halted(self) -> Result<RxRingHalted<'storage, COUNT>, Self> {
        let Self { state } = self;
        let Esp32s31StagedRxEpochState::Stopped(stopped) = state else {
            return Err(Self { state });
        };
        let (ring, resources) = stopped.into_epoch_parts();
        drop(resources);
        Ok(ring)
    }

    /// Consume a parked logical epoch and return the continuously live ring.
    pub fn try_into_live_ring(self) -> Result<RxRingLive<'storage, COUNT>, Self> {
        let Self { state } = self;
        let Esp32s31StagedRxEpochState::Live(live) = state else {
            return Err(Self { state });
        };
        match live.try_into_live_epoch_parts() {
            Ok((ring, resources)) => {
                drop(resources);
                Ok(ring)
            }
            Err((live, _)) => Err(Self {
                state: Esp32s31StagedRxEpochState::Live(live),
            }),
        }
    }

    /// Return both the live ring and its retained standalone publication
    /// resources without ending the affine producer epoch.
    #[allow(clippy::type_complexity, clippy::result_large_err)]
    pub fn try_into_live_epoch_parts(
        self,
    ) -> Result<
        (
            RxRingLive<'storage, COUNT>,
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
        ),
        Self,
    > {
        let Self { state } = self;
        let Esp32s31StagedRxEpochState::Live(live) = state else {
            return Err(Self { state });
        };
        live.try_into_live_epoch_parts().map_err(|(live, _)| Self {
            state: Esp32s31StagedRxEpochState::Live(live),
        })
    }

    /// Return a stopped paired epoch to the standalone station publisher.
    ///
    /// Every route-tagged lease must already have left the paired queue. The
    /// returned owner retains the same halted hardware and observation state,
    /// so standalone resume does not rebuild ownership from raw addresses.
    pub fn try_into_standalone_stopped(
        self,
        frames: Esp32s31StagedRxSender<'queue, 'pool, M, QUEUE_DEPTH, STAGE_CAPACITY, STAGE_SLOTS>,
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
            #[cfg(any(feature = "diagnostics", test))]
            pipeline_observer,
        } = stopped;
        Ok(Esp32s31StoppedRx {
            ring,
            storage,
            pool,
            frames: Esp32s31StagedRxPublisher::standalone(frames),
            delay,
            #[cfg(any(feature = "diagnostics", test))]
            pipeline_observer,
        })
    }

    /// Replace the drained paired publisher with a standalone publisher while
    /// preserving the continuously running physical ring.
    pub fn try_into_standalone_live(
        self,
        frames: Esp32s31StagedRxSender<'queue, 'pool, M, QUEUE_DEPTH, STAGE_CAPACITY, STAGE_SLOTS>,
    ) -> Result<
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
        Self,
    > {
        let Self { state } = self;
        let Esp32s31StagedRxEpochState::Live(live) = state else {
            return Err(Self { state });
        };
        let (ring, resources) = match live.try_into_live_epoch_parts() {
            Ok(parts) => parts,
            Err((live, _)) => {
                return Err(Self {
                    state: Esp32s31StagedRxEpochState::Live(live),
                });
            }
        };
        let Esp32s31RxEpochResources {
            ring_lifetime: _,
            storage,
            pool,
            frames: _,
            delay,
            #[cfg(any(feature = "diagnostics", test))]
            pipeline_observer,
        } = resources;
        let producer = Esp32s31StagedRxProducer::new(ring, storage, pool, delay, frames);
        #[cfg(any(feature = "diagnostics", test))]
        let producer = match pipeline_observer {
            Some(observer) => producer.with_pipeline_observer(observer),
            None => producer,
        };
        Ok(producer)
    }
}
