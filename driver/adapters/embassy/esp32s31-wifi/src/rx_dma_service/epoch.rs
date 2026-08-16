use super::*;
use crate::{rx_frontier::Esp32s31RxFrontierSchedulerSnapshot, wdev::services::WdevRxService};

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
    report: Esp32s31StagedRxProducerReport,
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
        let resources = Esp32s31RxEpochResources::new(storage, pool, frames, delay);
        Self {
            state: Esp32s31StagedRxEpochState::Stopped(resources.with_halted_ring(ring)),
            report: Esp32s31StagedRxProducerReport::default(),
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
    ) -> Result<WdevRxProgress, RxStageTransactionError> {
        let Esp32s31StagedRxEpochState::Live(live) = &mut self.state else {
            return Err(RxStageTransactionError::Ring(RxRingError::Busy));
        };
        let progress = WdevRxService::service(live, hardware).await?;
        self.report = live.report();
        Ok(progress)
    }

    pub fn stop<H: RxDma>(&mut self, hardware: &mut H) -> Result<(), RxStageTransactionError> {
        let state = core::mem::replace(&mut self.state, Esp32s31StagedRxEpochState::Vacant);
        let Esp32s31StagedRxEpochState::Live(live) = state else {
            self.state = state;
            return Err(RxStageTransactionError::Ring(RxRingError::Busy));
        };
        self.report = live.report();
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

    pub const fn report(&self) -> Esp32s31StagedRxProducerReport {
        self.report
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
        let Self { state, report } = self;
        let Esp32s31StagedRxEpochState::Stopped(stopped) = state else {
            return Err(Self { state, report });
        };
        let (ring, resources) = stopped.into_epoch_parts();
        drop(resources);
        Ok(ring)
    }
}
