use core::future::Future;

use embassy_sync::channel::Receiver;
use open_esp_radio_embassy_net::RawMutex;
use open_esp_radio_esp32s31_wifi_mac::{
    rx::{RxDescriptorSnapshot, RxDma, RxRingHalted, RxSegment},
    rx_pool::{RxStagePool, RxStageTransactionError},
};

use crate::{
    connected_rx_protocol::{Esp32s31StagedRxFrame, Esp32s31StagedRxQueue},
    embassy_rx::RxDmaObservationDelay,
    rx_dma_service::{Esp32s31RxDmaStorage, Esp32s31StagedRxEpoch, Esp32s31StagedRxProducerReport},
    rx_frontier::Esp32s31RxFrontierSchedulerSnapshot,
    wdev::WdevRxProgress,
};

#[doc(hidden)]
pub trait AccessPointStagedRxFrame {
    fn segment(&self) -> RxSegment<'_>;
}

impl<const CAPACITY: usize, const SLOTS: usize> AccessPointStagedRxFrame
    for Esp32s31StagedRxFrame<'_, CAPACITY, SLOTS>
{
    fn segment(&self) -> RxSegment<'_> {
        Esp32s31StagedRxFrame::segment(self)
    }
}

#[doc(hidden)]
pub trait AccessPointRxObservation<const COUNT: usize> {
    fn try_receive(&self) -> Option<Self::Frame>;
    type Frame: AccessPointStagedRxFrame;
    fn queued_frames(&self) -> usize;
    fn discard_queued(&self) -> usize;
    fn report(&self) -> Esp32s31StagedRxProducerReport;
    fn descriptor_snapshot(&self, index: usize) -> Option<RxDescriptorSnapshot>;
    fn scheduler_snapshot(&self) -> Option<Esp32s31RxFrontierSchedulerSnapshot>;
}

#[doc(hidden)]
pub trait AccessPointRxPipeline<H, const COUNT: usize>: AccessPointRxObservation<COUNT> {
    fn start(
        &mut self,
        hardware: &mut H,
    ) -> impl Future<Output = Result<(), RxStageTransactionError>>;
    fn stage_completed(
        &mut self,
        hardware: &mut H,
    ) -> impl Future<Output = Result<WdevRxProgress, RxStageTransactionError>>;
    fn stop(&mut self, hardware: &mut H) -> Result<(), RxStageTransactionError>;
}

/// One AP epoch's binding to the common role-neutral staged-RX producer.
///
/// The producer and consumer endpoints are kept together so an AP cannot
/// publish DMA ownership into a queue for which it does not own a consumer.
pub struct Esp32s31AccessPointRxPipeline<
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
    producer: Esp32s31StagedRxEpoch<
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
    frames:
        Receiver<'queue, M, Esp32s31StagedRxFrame<'pool, STAGE_CAPACITY, STAGE_SLOTS>, QUEUE_DEPTH>,
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
    Esp32s31AccessPointRxPipeline<
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
        queue: &'queue Esp32s31StagedRxQueue<'pool, M, QUEUE_DEPTH, STAGE_CAPACITY, STAGE_SLOTS>,
        delay: D,
    ) -> Self {
        let (sender, frames) = queue.split();
        Self {
            producer: Esp32s31StagedRxEpoch::from_halted(ring, storage, pool, sender, delay),
            frames,
        }
    }

    pub fn try_into_halted(self) -> Result<RxRingHalted<'storage, COUNT>, Self> {
        let Self { producer, frames } = self;
        match producer.try_into_halted() {
            Ok(ring) => {
                let _ = frames;
                Ok(ring)
            }
            Err(producer) => Err(Self { producer, frames }),
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
> AccessPointRxObservation<COUNT>
    for Esp32s31AccessPointRxPipeline<
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
    type Frame = Esp32s31StagedRxFrame<'pool, STAGE_CAPACITY, STAGE_SLOTS>;

    fn try_receive(&self) -> Option<Self::Frame> {
        self.frames.try_receive().ok()
    }

    fn queued_frames(&self) -> usize {
        self.frames.len()
    }

    fn discard_queued(&self) -> usize {
        let mut discarded = 0_usize;
        while let Ok(frame) = self.frames.try_receive() {
            drop(frame);
            discarded = discarded.saturating_add(1);
        }
        discarded
    }

    fn report(&self) -> Esp32s31StagedRxProducerReport {
        self.producer.report()
    }

    fn descriptor_snapshot(&self, index: usize) -> Option<RxDescriptorSnapshot> {
        self.producer.descriptor_snapshot(index)
    }

    fn scheduler_snapshot(&self) -> Option<Esp32s31RxFrontierSchedulerSnapshot> {
        self.producer.scheduler_snapshot()
    }
}

impl<
    'storage,
    'pool,
    'queue,
    D,
    M: RawMutex,
    H: RxDma,
    const QUEUE_DEPTH: usize,
    const COUNT: usize,
    const STAGE_CAPACITY: usize,
    const STAGE_SLOTS: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
> AccessPointRxPipeline<H, COUNT>
    for Esp32s31AccessPointRxPipeline<
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
    async fn start(&mut self, hardware: &mut H) -> Result<(), RxStageTransactionError> {
        self.producer.start(hardware).await
    }

    async fn stage_completed(
        &mut self,
        hardware: &mut H,
    ) -> Result<WdevRxProgress, RxStageTransactionError> {
        self.producer.service(hardware).await
    }

    fn stop(&mut self, hardware: &mut H) -> Result<(), RxStageTransactionError> {
        self.producer.stop(hardware)
    }
}
