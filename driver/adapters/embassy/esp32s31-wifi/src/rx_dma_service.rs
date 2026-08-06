//! Production ownership of the ESP32-S31 Wi-Fi RX descriptor ring.
//!
//! DMA storage, the live descriptor frontier and independent staging storage
//! are kept in one finite service. No DMA pointer escapes this owner: a
//! completed unit is copied and recycled before its staging lease is handed to
//! the separate protocol consumer.

#![forbid(unsafe_code)]

use core::future::Future;

use embassy_sync::channel::{Sender, TrySendError};
use open_esp_radio_embassy_net::RawMutex;
use open_esp_radio_esp32s31_wifi_mac::{
    rx::{RxDma, RxReloadObservation, RxRingError, RxRingHalted, RxRingLive, RxRingStopped},
    rx_pool::{
        RxDmaStageUnitOutcome, RxStageError, RxStagePool, RxStageTransactionError,
        VENDOR_LARGE_RX_PAYLOAD_CAPACITY, VENDOR_LARGE_RX_SLOT_COUNT,
    },
    rx_storage::{RxDmaBuffer, RxDmaStorage},
};

use crate::{
    connected_runner::WifiRxProgress,
    connected_rx_protocol::Esp32s31StagedRxFrame,
    connected_services::Esp32s31ConnectedRxService,
    embassy_rx::{RxReloadDelay, await_staged_rx_reload},
    rx_pipeline_observer::{
        RxPipelineObservation, RxPipelineObserver, RxServiceObservation, RxStageDiscard,
    },
};

/// Descriptor count and allocation geometry qualified by the ordinary S31
/// large-RX profile.
pub const ESP32S31_RX_DESCRIPTOR_COUNT: usize = 32;
pub const ESP32S31_RX_BUFFER_SIZE: usize = 4_608;
pub const ESP32S31_RX_BUFFER_STORAGE_SIZE: usize = ESP32S31_RX_BUFFER_SIZE + 4;
/// Platform settle edge between stopped-ring publication and walker enable.
pub const ESP32S31_RX_WALKER_ENABLE_SETTLE_US: u32 = 5;
/// Qualified large-RX profile aliases over the executor-independent MAC arena.
pub type Esp32s31RxDmaBuffer<
    const BUFFER_SIZE: usize = ESP32S31_RX_BUFFER_SIZE,
    const STORAGE_SIZE: usize = ESP32S31_RX_BUFFER_STORAGE_SIZE,
> = RxDmaBuffer<BUFFER_SIZE, STORAGE_SIZE>;

pub type Esp32s31RxDmaStorage<
    const COUNT: usize = ESP32S31_RX_DESCRIPTOR_COUNT,
    const BUFFER_SIZE: usize = ESP32S31_RX_BUFFER_SIZE,
    const STORAGE_SIZE: usize = ESP32S31_RX_BUFFER_STORAGE_SIZE,
> = RxDmaStorage<COUNT, BUFFER_SIZE, STORAGE_SIZE>;

/// Value-only description of one real completed DMA unit before staging.
///
/// The policy never receives a descriptor pointer, payload view or ring
/// capability. It can narrow the admitted payload length, but ownership and
/// descriptor recycle remain exclusively inside [`Esp32s31ConnectedRx`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Esp32s31RxCompletedUnit {
    pub head_index: usize,
    pub descriptor_count: usize,
    pub payload_length: usize,
}

/// A completed ingress transaction observed after its ownership edge.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31RxIngressObservation {
    /// The unit was discarded and its descriptor reload has settled.
    DiscardReloaded {
        unit: Esp32s31RxCompletedUnit,
        reason: RxStageDiscard,
    },
    /// The copied unit was published into the independent staging queue.
    Staged(Esp32s31RxCompletedUnit),
}

/// Admission policy at the completed-DMA-unit/staging boundary.
///
/// This hook is intentionally narrower than a general RX observer. It may
/// only lower the maximum staged payload for the current real unit and then
/// observe the completed transaction. It cannot mutate descriptor metadata,
/// recycle buffers, fabricate frames or retain hardware ownership.
pub trait Esp32s31RxStageAdmissionPolicy {
    fn maximum_payload_length(
        &self,
        _unit: Esp32s31RxCompletedUnit,
        physical_capacity: usize,
    ) -> usize {
        physical_capacity
    }

    fn observe(&self, _observation: Esp32s31RxIngressObservation) {}
}

impl<T: Esp32s31RxStageAdmissionPolicy + ?Sized> Esp32s31RxStageAdmissionPolicy for &T {
    fn maximum_payload_length(
        &self,
        unit: Esp32s31RxCompletedUnit,
        physical_capacity: usize,
    ) -> usize {
        T::maximum_payload_length(*self, unit, physical_capacity)
    }

    fn observe(&self, observation: Esp32s31RxIngressObservation) {
        T::observe(*self, observation);
    }
}

/// Zero-sized production policy admitting the complete physical stage slot.
#[derive(Clone, Copy, Debug, Default)]
pub struct FullRxStageAdmission;

impl Esp32s31RxStageAdmissionPolicy for FullRxStageAdmission {}

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
    P = FullRxStageAdmission,
> {
    ring: RxRingLive<'storage, COUNT>,
    storage: &'storage Esp32s31RxDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
    pool: &'pool RxStagePool<STAGE_SLOTS, STAGE_CAPACITY>,
    frames:
        Sender<'queue, M, Esp32s31StagedRxFrame<'pool, STAGE_CAPACITY, STAGE_SLOTS>, QUEUE_DEPTH>,
    delay: D,
    pipeline_observer: Option<&'pool dyn RxPipelineObserver>,
    admission: P,
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

mod lifecycle;
mod service;

#[cfg(test)]
mod tests;
