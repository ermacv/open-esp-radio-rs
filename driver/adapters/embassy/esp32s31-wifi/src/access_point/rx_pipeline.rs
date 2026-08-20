use core::future::Future;

use embassy_sync::channel::Receiver;
use open_esp_radio_embassy_net::RawMutex;
use open_esp_radio_esp32s31_wifi_mac::{
    rx::{PUBLIC_HEADER_SIZE, RxDescriptorSnapshot, RxDma, RxRingHalted, RxSegment},
    rx_pool::{RxStagePool, RxStageTransactionError},
};

use crate::{
    connected_rx_protocol::{
        Esp32s31StagedRxFrame, Esp32s31StagedRxQueue, StagedEthernetPublication,
    },
    embassy_rx::RxDmaObservationDelay,
    rx_dma_service::{Esp32s31RxDmaStorage, Esp32s31StagedRxEpoch, Esp32s31StagedRxProducerReport},
    rx_frontier::Esp32s31RxFrontierSchedulerSnapshot,
    rx_pipeline_observer::RxPipelineObserver,
    wdev::WdevRxProgress,
};

#[doc(hidden)]
pub trait AccessPointStagedRxFrame {
    fn segment(&self) -> RxSegment<'_>;

    fn publish_ethernet_in_place(self, ethernet: StagedEthernetPublication) -> Result<u8, Self>
    where
        Self: Sized;
}

impl<const CAPACITY: usize, const SLOTS: usize> AccessPointStagedRxFrame
    for Esp32s31StagedRxFrame<'_, CAPACITY, SLOTS>
{
    fn segment(&self) -> RxSegment<'_> {
        Esp32s31StagedRxFrame::segment(self)
    }

    #[inline(always)]
    fn publish_ethernet_in_place(self, ethernet: StagedEthernetPublication) -> Result<u8, Self> {
        Esp32s31StagedRxFrame::publish_ethernet_in_place(
            self,
            ethernet.destination,
            ethernet.source,
            ethernet.ether_type,
            ethernet.payload_offset,
            ethernet.payload_length,
        )
        .map_err(|(frame, _)| frame)
    }
}

#[doc(hidden)]
pub trait AccessPointRxProtocolConsumer {
    fn try_receive(&mut self) -> Option<Self::Frame>;
    /// Consume only protected data while the radio owner has an active TX
    /// transaction. A management or EAPOL frame remains the exact ordered
    /// head and is returned later by [`Self::try_receive`].
    fn try_receive_protected_data(&mut self) -> Option<Self::Frame>;
    type Frame: AccessPointStagedRxFrame;
    fn queued_frames(&self) -> usize;
    fn discard_queued(&mut self) -> usize;
}

#[doc(hidden)]
pub trait AccessPointRxProducerObservation<const COUNT: usize> {
    fn report(&self) -> Esp32s31StagedRxProducerReport;
    fn descriptor_snapshot(&self, index: usize) -> Option<RxDescriptorSnapshot>;
    fn scheduler_snapshot(&self) -> Option<Esp32s31RxFrontierSchedulerSnapshot>;
}

#[doc(hidden)]
pub trait AccessPointRxProducer<H, const COUNT: usize>:
    AccessPointRxProducerObservation<COUNT>
{
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

/// DMA-only AP endpoint. It can publish staged owners but cannot parse or
/// consume them.
pub struct Esp32s31AccessPointRxProducer<
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
    inner: Esp32s31StagedRxEpoch<
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

/// Protocol-only AP endpoint. It owns staged frame leases but has no DMA,
/// descriptor, PAC, or interrupt capability.
pub struct Esp32s31AccessPointRxConsumer<
    'pool,
    'queue,
    M: RawMutex,
    const QUEUE_DEPTH: usize,
    const STAGE_CAPACITY: usize,
    const STAGE_SLOTS: usize,
> {
    frames:
        Receiver<'queue, M, Esp32s31StagedRxFrame<'pool, STAGE_CAPACITY, STAGE_SLOTS>, QUEUE_DEPTH>,
    deferred: Option<Esp32s31StagedRxFrame<'pool, STAGE_CAPACITY, STAGE_SLOTS>>,
}

pub(super) fn is_protected_data(segment: RxSegment<'_>) -> bool {
    let Some(frame_control) = segment
        .buffer
        .get(PUBLIC_HEADER_SIZE..PUBLIC_HEADER_SIZE + 2)
        .map(|bytes| u16::from_le_bytes([bytes[0], bytes[1]]))
    else {
        return false;
    };
    frame_control & 0x000c == 0x0008 && frame_control & 0x4000 != 0
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
    Esp32s31AccessPointRxProducer<
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
    ) -> (
        Self,
        Esp32s31AccessPointRxConsumer<'pool, 'queue, M, QUEUE_DEPTH, STAGE_CAPACITY, STAGE_SLOTS>,
    ) {
        let (sender, frames) = queue.split();
        (
            Self {
                inner: Esp32s31StagedRxEpoch::from_halted(ring, storage, pool, sender, delay),
            },
            Esp32s31AccessPointRxConsumer {
                frames,
                deferred: None,
            },
        )
    }

    /// Attach value-only pipeline observations without exposing descriptor
    /// ownership to the AP protocol layer.
    pub fn from_halted_with_pipeline_observer(
        ring: RxRingHalted<'storage, COUNT>,
        storage: &'storage Esp32s31RxDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
        pool: &'pool RxStagePool<STAGE_SLOTS, STAGE_CAPACITY>,
        queue: &'queue Esp32s31StagedRxQueue<'pool, M, QUEUE_DEPTH, STAGE_CAPACITY, STAGE_SLOTS>,
        delay: D,
        observer: &'pool dyn RxPipelineObserver,
    ) -> (
        Self,
        Esp32s31AccessPointRxConsumer<'pool, 'queue, M, QUEUE_DEPTH, STAGE_CAPACITY, STAGE_SLOTS>,
    ) {
        let (sender, frames) = queue.split();
        (
            Self {
                inner: Esp32s31StagedRxEpoch::from_halted_with_pipeline_observer(
                    ring, storage, pool, sender, delay, observer,
                ),
            },
            Esp32s31AccessPointRxConsumer {
                frames,
                deferred: None,
            },
        )
    }

    pub fn try_into_halted(self) -> Result<RxRingHalted<'storage, COUNT>, Self> {
        let Self { inner } = self;
        match inner.try_into_halted() {
            Ok(ring) => Ok(ring),
            Err(inner) => Err(Self { inner }),
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
> AccessPointRxProducerObservation<COUNT>
    for Esp32s31AccessPointRxProducer<
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
    fn report(&self) -> Esp32s31StagedRxProducerReport {
        self.inner.report()
    }

    fn descriptor_snapshot(&self, index: usize) -> Option<RxDescriptorSnapshot> {
        self.inner.descriptor_snapshot(index)
    }

    fn scheduler_snapshot(&self) -> Option<Esp32s31RxFrontierSchedulerSnapshot> {
        self.inner.scheduler_snapshot()
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
> AccessPointRxProducer<H, COUNT>
    for Esp32s31AccessPointRxProducer<
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
        self.inner.start(hardware).await
    }

    async fn stage_completed(
        &mut self,
        hardware: &mut H,
    ) -> Result<WdevRxProgress, RxStageTransactionError> {
        self.inner.service(hardware).await
    }

    fn stop(&mut self, hardware: &mut H) -> Result<(), RxStageTransactionError> {
        self.inner.stop(hardware)
    }
}

impl<
    'pool,
    'queue,
    M: RawMutex,
    const QUEUE_DEPTH: usize,
    const STAGE_CAPACITY: usize,
    const STAGE_SLOTS: usize,
> AccessPointRxProtocolConsumer
    for Esp32s31AccessPointRxConsumer<'pool, 'queue, M, QUEUE_DEPTH, STAGE_CAPACITY, STAGE_SLOTS>
{
    type Frame = Esp32s31StagedRxFrame<'pool, STAGE_CAPACITY, STAGE_SLOTS>;

    #[inline(always)]
    fn try_receive(&mut self) -> Option<Self::Frame> {
        self.deferred
            .take()
            .or_else(|| self.frames.try_receive().ok())
    }

    #[inline(always)]
    fn try_receive_protected_data(&mut self) -> Option<Self::Frame> {
        if self.deferred.is_some() {
            return None;
        }
        let frame = self.frames.try_receive().ok()?;
        if is_protected_data(frame.segment()) {
            Some(frame)
        } else {
            self.deferred = Some(frame);
            None
        }
    }

    fn queued_frames(&self) -> usize {
        self.frames.len() + usize::from(self.deferred.is_some())
    }

    fn discard_queued(&mut self) -> usize {
        let mut discarded = usize::from(self.deferred.take().is_some());
        while let Ok(frame) = self.frames.try_receive() {
            drop(frame);
            discarded = discarded.saturating_add(1);
        }
        discarded
    }
}

#[cfg(test)]
mod classification_tests {
    use super::*;

    #[test]
    fn active_tx_admits_only_protected_data_to_protocol_processing() {
        let mut protected = [0_u8; PUBLIC_HEADER_SIZE + 2];
        protected[PUBLIC_HEADER_SIZE..].copy_from_slice(&0x4008_u16.to_le_bytes());
        assert!(is_protected_data(RxSegment {
            descriptor_address: 0,
            descriptor_word0: 0,
            buffer: &protected,
            next_descriptor_address: 0,
        }));

        let mut management = protected;
        management[PUBLIC_HEADER_SIZE..].copy_from_slice(&0_u16.to_le_bytes());
        assert!(!is_protected_data(RxSegment {
            descriptor_address: 0,
            descriptor_word0: 0,
            buffer: &management,
            next_descriptor_address: 0,
        }));
    }
}
