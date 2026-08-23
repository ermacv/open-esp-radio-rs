//! Production ownership of the ESP32-S31 Wi-Fi RX descriptor ring.
//!
//! DMA storage, the live descriptor frontier and independent staging storage
//! are kept in one finite service. No DMA pointer escapes this owner: a
//! completed unit is copied into independent staging before its lease is handed
//! to the protocol consumer. The service freezes LAST before copying and
//! returns each copied complete unit in its own append/reload transaction;
//! no fixed descriptor suffix is withheld from the live list.

#![forbid(unsafe_code)]

use core::future::Future;

use embassy_sync::channel::{Sender, TrySendError};
use open_esp_radio_embassy_net::RawMutex;
use open_esp_radio_esp32s31_wifi_dma::rx_storage::{RxDmaBuffer, RxDmaStorage};
use open_esp_radio_esp32s31_wifi_mac::{
    rx::{
        PUBLIC_HEADER_SIZE, RxDescriptorSnapshot, RxDma, RxRingError, RxRingHalted, RxRingLive,
        RxRingStopped,
    },
    rx_pool::{
        RxDmaDeferredStageUnitOutcome, RxStageError, RxStagePool, RxStageTransactionError,
        VENDOR_LARGE_RX_PAYLOAD_CAPACITY, VENDOR_LARGE_RX_SLOT_COUNT,
    },
};
use open_esp_radio_ieee80211::vif::{StaApRxRoute, StaApVif, classify_sta_ap_rx};

#[cfg(any(feature = "diagnostics", test))]
use crate::diagnostics::rx_pipeline::{
    RxPipelineObservation, RxPipelineObserver, RxServiceObservation,
};
use crate::{
    datapath::DatapathRxProgress, datapath::rx::hardware::RxDmaObservationDelay,
    datapath::rx::staging::Esp32s31StagedRxFrame, datapath::services::DatapathRxService,
    diagnostics::rx_pipeline::RxStageDiscard, roles::concurrent::Esp32s31StaApStagedRxFrame,
};

/// Descriptor count and allocation geometry qualified by the ordinary S31
/// large-RX profile.
pub const ESP32S31_RX_DESCRIPTOR_COUNT: usize = 64;
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
/// descriptor reclaim remain exclusively inside [`Esp32s31StagedRxProducer`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Esp32s31RxCompletedUnit {
    pub head_index: usize,
    pub descriptor_count: usize,
    pub payload_length: usize,
}

/// Fact-only traffic class visible at the DMA/staging admission boundary.
///
/// Only an IEEE 802.11 frame-control value copied from the completed unit is
/// interpreted. No association, authorization or hardware meaning is
/// inferred here. Protected data is the sole bulk class; management, control
/// and unprotected data (including pre-key EAPOL) remain critical.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31RxIngressClass {
    BulkProtectedData,
    Critical,
    Unclassified,
}

/// Fact-only logical route for overload accounting.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31RxIngressRoute {
    Standalone,
    Station,
    AccessPoint,
    Foreign,
    Ambiguous,
    Malformed,
}

/// Value-only preview used before staging ownership is transferred.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Esp32s31RxCompletedUnitPreview {
    pub unit: Esp32s31RxCompletedUnit,
    pub frame_control: Option<u16>,
    pub class: Esp32s31RxIngressClass,
    pub route: Esp32s31RxIngressRoute,
}

/// Policy decision when ordinary staging credits are unavailable.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RxStageUnavailableDisposition {
    /// Preserve the final staging credit for control/management/EAPOL input.
    PreserveForCriticalAdmission,
    /// Drop the upper copy but return the completed descriptor immediately.
    DiscardAndRecycle,
}

/// A completed ingress transaction observed after its ownership edge.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31RxIngressObservation {
    /// The unit was discarded and retained until the frozen-LAST reclaim.
    DiscardRetained {
        unit: Esp32s31RxCompletedUnit,
        reason: RxStageDiscard,
    },
    /// The copied unit was published into the independent staging queue.
    Staged(Esp32s31RxCompletedUnit),
    /// A bulk unit could not acquire an upper-layer credit and followed the
    /// reviewed vendor discard/append path.
    OverloadDiscardedAndRecycled(Esp32s31RxCompletedUnitPreview),
    /// A critical unit consumed the reserved final staging credit.
    CriticalReserveAdmitted(Esp32s31RxCompletedUnitPreview),
    /// No reserved credit remained for a critical unit. Descriptor ownership
    /// was deliberately not transferred, so a later capacity wake retries it.
    CriticalAdmissionBlocked(Esp32s31RxCompletedUnitPreview),
}

/// Admission policy at the completed-DMA-unit/staging boundary.
///
/// This hook is intentionally narrower than a general RX observer. It may
/// only lower the maximum staged payload for the current real unit and then
/// observe the completed transaction. It cannot mutate descriptor metadata,
/// reclaim buffers, fabricate frames or retain hardware ownership.
pub trait Esp32s31RxStageAdmissionPolicy {
    fn maximum_payload_length(
        &self,
        _unit: Esp32s31RxCompletedUnit,
        physical_capacity: usize,
    ) -> usize {
        physical_capacity
    }

    fn observe(&self, _observation: Esp32s31RxIngressObservation) {}

    /// Number of staging/queue credits unavailable to ordinary bulk data.
    fn critical_reserved_credits(&self) -> usize {
        1
    }

    /// Decide whether a unit may be discarded when only the critical reserve
    /// remains. The default is deliberately conservative for unknown input.
    fn unavailable_disposition(
        &self,
        preview: Esp32s31RxCompletedUnitPreview,
    ) -> RxStageUnavailableDisposition {
        match preview.class {
            Esp32s31RxIngressClass::BulkProtectedData => {
                RxStageUnavailableDisposition::DiscardAndRecycle
            }
            Esp32s31RxIngressClass::Critical | Esp32s31RxIngressClass::Unclassified => {
                RxStageUnavailableDisposition::PreserveForCriticalAdmission
            }
        }
    }
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

    fn critical_reserved_credits(&self) -> usize {
        T::critical_reserved_credits(*self)
    }

    fn unavailable_disposition(
        &self,
        preview: Esp32s31RxCompletedUnitPreview,
    ) -> RxStageUnavailableDisposition {
        T::unavailable_disposition(*self, preview)
    }
}

/// Zero-sized production policy admitting the complete physical stage slot.
#[derive(Clone, Copy, Debug, Default)]
pub struct FullRxStageAdmission;

impl Esp32s31RxStageAdmissionPolicy for FullRxStageAdmission {}

/// Permanent publication strategy owned by the single physical RX producer.
///
/// Standalone STA/AP epochs publish an unclassified lease to their sole
/// protocol consumer. A same-channel STA+AP epoch instead publishes exactly
/// one ordered stream carrying the fact-only VIF route. Neither variant owns
/// protocol policy, and neither can manufacture or duplicate a staging lease.
pub enum Esp32s31StagedRxPublisher<
    'pool,
    'queue,
    M: RawMutex,
    const QUEUE_DEPTH: usize,
    const STAGE_CAPACITY: usize,
    const STAGE_SLOTS: usize,
> {
    Standalone(
        Sender<'queue, M, Esp32s31StagedRxFrame<'pool, STAGE_CAPACITY, STAGE_SLOTS>, QUEUE_DEPTH>,
    ),
    StaAp {
        frames: Sender<
            'queue,
            M,
            Esp32s31StaApStagedRxFrame<'pool, STAGE_CAPACITY, STAGE_SLOTS>,
            QUEUE_DEPTH,
        >,
        ingress: open_esp_radio_esp32s31_wifi_mac::rx::RxIngressConfig,
        addresses: open_esp_radio_ieee80211::vif::StaApRxAddresses,
    },
}

impl<
    'pool,
    'queue,
    M: RawMutex,
    const QUEUE_DEPTH: usize,
    const STAGE_CAPACITY: usize,
    const STAGE_SLOTS: usize,
> Esp32s31StagedRxPublisher<'pool, 'queue, M, QUEUE_DEPTH, STAGE_CAPACITY, STAGE_SLOTS>
{
    pub const fn standalone(
        frames: Sender<
            'queue,
            M,
            Esp32s31StagedRxFrame<'pool, STAGE_CAPACITY, STAGE_SLOTS>,
            QUEUE_DEPTH,
        >,
    ) -> Self {
        Self::Standalone(frames)
    }

    pub const fn sta_ap(
        frames: Sender<
            'queue,
            M,
            Esp32s31StaApStagedRxFrame<'pool, STAGE_CAPACITY, STAGE_SLOTS>,
            QUEUE_DEPTH,
        >,
        ingress: open_esp_radio_esp32s31_wifi_mac::rx::RxIngressConfig,
        addresses: open_esp_radio_ieee80211::vif::StaApRxAddresses,
    ) -> Self {
        Self::StaAp {
            frames,
            ingress,
            addresses,
        }
    }

    fn free_capacity(&self) -> usize {
        match self {
            Self::Standalone(frames) => frames.free_capacity(),
            Self::StaAp { frames, .. } => frames.free_capacity(),
        }
    }

    fn len(&self) -> usize {
        match self {
            Self::Standalone(frames) => frames.len(),
            Self::StaAp { frames, .. } => frames.len(),
        }
    }

    fn preview(
        &self,
        unit: Esp32s31RxCompletedUnit,
        bytes: [u8; 24],
    ) -> Esp32s31RxCompletedUnitPreview {
        let frame_control = Some(u16::from_le_bytes([bytes[0], bytes[1]]));
        let class = match frame_control {
            Some(value) if value & 0x000c == 0x0008 && value & 0x4000 != 0 => {
                Esp32s31RxIngressClass::BulkProtectedData
            }
            Some(_) => Esp32s31RxIngressClass::Critical,
            None => Esp32s31RxIngressClass::Unclassified,
        };
        let route = match self {
            Self::Standalone(_) => Esp32s31RxIngressRoute::Standalone,
            Self::StaAp { addresses, .. } => match classify_sta_ap_rx(&bytes, *addresses) {
                StaApRxRoute::Interface(StaApVif::Station) => Esp32s31RxIngressRoute::Station,
                StaApRxRoute::Interface(StaApVif::AccessPoint) => {
                    Esp32s31RxIngressRoute::AccessPoint
                }
                StaApRxRoute::Foreign => Esp32s31RxIngressRoute::Foreign,
                StaApRxRoute::Ambiguous => Esp32s31RxIngressRoute::Ambiguous,
                StaApRxRoute::Malformed => Esp32s31RxIngressRoute::Malformed,
            },
        };
        Esp32s31RxCompletedUnitPreview {
            unit,
            frame_control,
            class,
            route,
        }
    }

    const fn unclassified_preview(
        &self,
        unit: Esp32s31RxCompletedUnit,
    ) -> Esp32s31RxCompletedUnitPreview {
        Esp32s31RxCompletedUnitPreview {
            unit,
            frame_control: None,
            class: Esp32s31RxIngressClass::Unclassified,
            route: match self {
                Self::Standalone(_) => Esp32s31RxIngressRoute::Standalone,
                Self::StaAp { .. } => Esp32s31RxIngressRoute::Malformed,
            },
        }
    }

    fn try_send(
        &self,
        frame: Esp32s31StagedRxFrame<'pool, STAGE_CAPACITY, STAGE_SLOTS>,
    ) -> Result<(), Esp32s31StagedRxFrame<'pool, STAGE_CAPACITY, STAGE_SLOTS>> {
        match self {
            Self::Standalone(frames) => frames.try_send(frame).map_err(|error| match error {
                TrySendError::Full(frame) => frame,
            }),
            Self::StaAp {
                frames,
                ingress,
                addresses,
            } => frames
                .try_send(Esp32s31StaApStagedRxFrame::classify(
                    frame, *ingress, *addresses,
                ))
                .map_err(|error| match error {
                    TrySendError::Full(frame) => frame.into_frame(),
                }),
        }
    }
}

/// Complete production RX owner for one running descriptor-ring epoch.
pub struct Esp32s31StagedRxProducer<
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
    frames: Esp32s31StagedRxPublisher<'pool, 'queue, M, QUEUE_DEPTH, STAGE_CAPACITY, STAGE_SLOTS>,
    delay: D,
    #[cfg(any(feature = "diagnostics", test))]
    pipeline_observer: Option<&'pool dyn RxPipelineObserver>,
    admission: P,
    serviced_descriptors: u64,
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
    frames: Esp32s31StagedRxPublisher<'pool, 'queue, M, QUEUE_DEPTH, STAGE_CAPACITY, STAGE_SLOTS>,
    delay: D,
    #[cfg(any(feature = "diagnostics", test))]
    pipeline_observer: Option<&'pool dyn RxPipelineObserver>,
}

/// Peer-independent resources retained while a halted ring is used by a
/// finite Authentication/Association/WPA2 receive epoch.
///
/// Splitting these resources from [`RxRingHalted`] lets the station lifecycle
/// pass the descriptor frontier through its pre-connected type state without
/// discarding the production staging pool, queue sender, observation delay or
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
    frames: Esp32s31StagedRxPublisher<'pool, 'queue, M, QUEUE_DEPTH, STAGE_CAPACITY, STAGE_SLOTS>,
    delay: D,
    #[cfg(any(feature = "diagnostics", test))]
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
    frames: Esp32s31StagedRxPublisher<'pool, 'queue, M, QUEUE_DEPTH, STAGE_CAPACITY, STAGE_SLOTS>,
    delay: D,
    #[cfg(any(feature = "diagnostics", test))]
    pipeline_observer: Option<&'pool dyn RxPipelineObserver>,
}

mod epoch;
mod lifecycle;
mod service;

pub use epoch::Esp32s31StagedRxEpoch;

#[cfg(test)]
mod tests;
