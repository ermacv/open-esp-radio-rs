#![expect(
    clippy::too_many_arguments,
    reason = "AP ingress and lifecycle boundaries expose independent borrowed owners without dynamic erasure"
)]

//! Embassy-owned AP MAC and network handoff service.
//!
//! The service handles beacons, management frames, WPA2 EAPOL and authorized
//! Ethernet traffic through one bounded RX/TX owner.

use core::{convert::Infallible, future::Future};

use embassy_futures::yield_now;
use embassy_sync::blocking_mutex::raw::RawMutex;
use embassy_time::{Instant, Timer};

use open_esp_radio_dma::StableDmaBacking;
use open_esp_radio_embassy_net::{
    FrameLengthError, LinkState, PinnedTxFrame, PinnedTxInterfaceConsumer, RxEnqueueError,
};

use open_esp_radio_esp32s31_wifi::{
    ampdu_tx::HtAmpduTxRolePolicy,
    ordinary_tx::{WifiTxEntropy, WifiTxPowerProfile, WifiTxResources, WifiTxTimer},
    tx::{WifiTxProgress, WifiTxWake},
};
#[cfg(any(feature = "diagnostics", test))]
use open_esp_radio_esp32s31_wifi_ap::mac::Esp32s31ApMacObservation;
use open_esp_radio_esp32s31_wifi_ap::protocol::{
    AccessPointServiceStatus, ApPeerClose, ApWpa2RetryProgress,
};
use open_esp_radio_esp32s31_wifi_ap::{
    ampdu::{
        Esp32s31ApAggregateAdmission, Esp32s31ApAmpduCompletion, Esp32s31ApAmpduError,
        Esp32s31ApAmpduProgress,
    },
    engine::{Esp32s31ApRuntimeHardware, Esp32s31ApWpa2Outcome},
    mac::{
        Esp32s31ApMac, Esp32s31ApMacError, Esp32s31ApMacParked, Esp32s31ApPeerDisconnectStage,
        Esp32s31ApTxCompletionAction,
    },
    rx::{
        Esp32s31ApRxConfig, Esp32s31ApRxDispatch, Esp32s31ApRxDispatcher, Esp32s31ApRxError,
        Esp32s31ApRxEvent, Esp32s31ApRxSink,
    },
};
#[cfg(any(feature = "diagnostics", test))]
use open_esp_radio_esp32s31_wifi_mac::rx::RxDescriptorSnapshot;
#[cfg(any(feature = "diagnostics", test))]
use open_esp_radio_esp32s31_wifi_mac::tx::HtMcs;
use open_esp_radio_esp32s31_wifi_mac::{
    MacInterface,
    init::MAC_COLD_RX_INTERRUPT_MASK,
    irq::MacInterruptRoute,
    rx::{RxDma, RxIngressConfig, view_normalized_rx_frame},
    rx_ampdu::{
        RX_BLOCK_ACK_MAX_WINDOW, RxBlockAckActivation, RxBlockAckRequest, RxBlockAckSessionsError,
        write_declined_addba_response,
    },
    rx_ampdu_hw::{RxBlockAckHardware, S31RxBlockAckAgreementError},
    tx::{HtChannelWidth, HtRate, TxHardware},
};
#[cfg(any(feature = "diagnostics", test))]
use open_esp_radio_ieee80211::data::EthernetFrameParts;
use open_esp_radio_ieee80211::data::{
    DataInterfaceRole, IEEE80211_LEGACY_DATA_HEADER_LEN, IEEE80211_QOS_DATA_HEADER_LEN,
    plan_data_decapsulation,
};
use open_esp_radio_ieee80211::{
    ap::{ApManagementRequest, parse_ap_management_request},
    block_ack::BlockAckAction,
};
use open_esp_radio_wifi_embassy::await_stack_boundary;
use open_esp_radio_wifi_softmac::MacRxEvidence;
use open_esp_radio_wpa2::{OwnedEapolFrame, Wpa2Interface};

#[cfg(any(feature = "diagnostics", test))]
use crate::datapath::irq::Esp32s31MacInterruptEpochDrain;
#[cfg(any(feature = "diagnostics", test))]
use crate::datapath::rx::frontier::Esp32s31RxFrontierSchedulerSnapshot;
#[cfg(any(feature = "diagnostics", test))]
use crate::diagnostics::access_point::{
    AccessPointObservationStorage, AccessPointTerminalObservation, AccessPointTerminalObserver,
};
#[cfg(feature = "diagnostics")]
use crate::diagnostics::network::{RxNetworkDeliveryEvent, RxNetworkDeliveryObserver};
use crate::{
    datapath::irq::{
        Esp32s31MacInterruptEpoch, Esp32s31MacInterruptEpochActivateError,
        Esp32s31MacInterruptEpochQuiesceError,
    },
    datapath::rx::reorder::{RX_REORDER_BACKING_SLOT_COUNT, RxReorderFrameStorage},
    datapath::rx::staging::StagedEthernetPublication,
    datapath::tx::aggregate::AggregateTxServiceEvent,
    datapath::{
        DatapathControlContext, DatapathControlProgress, DatapathRunner, DatapathRxProgress,
        DatapathRxServiceContext, DatapathServices, DatapathStopProgress,
        network::DatapathNetworkRx,
    },
    diagnostics::aggregate_tx::{AggregateBuildStop, AggregateTxObservation, AggregateTxObserver},
    roles::concurrent::Esp32s31StaApRxBlockAck,
};

const EAPOL_ETHERTYPE: u16 = 0x888e;
const EAPOL_CAPACITY: usize = 512;

fn observe_aggregate_rate(observer: &dyn AggregateTxObserver, rate: HtRate) {
    observer.observe(AggregateTxObservation::RateSelected {
        bandwidth_mhz: match rate.channel_width {
            HtChannelWidth::Mhz20 => 20,
            HtChannelWidth::Mhz40 => 40,
        },
        nominal_kbps: rate.nominal_kbps(),
    });
}

/// Avoid per-frame MMIO polling while preserving a batched producer refill.
/// The DATAPATH owner explicitly services DMA again at each protocol-quantum
/// boundary before yielding.
const fn should_observe_ap_rx_dma(protocol_blocked: bool, queued_frames: usize) -> bool {
    protocol_blocked || queued_frames == 0
}

/// An active TX keeps hardware out of the protocol consumer. The enclosing
/// radio owner remains responsible for executing the consumer's typed mailbox
/// actions after the protocol borrow ends.
const fn rx_protocol_consumer_has_hardware(tx_pending: bool) -> bool {
    !tx_pending
}

/// Keep one reorder release on a single ordered publication path after an
/// older cold frame has entered the deferred batch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AccessPointRxPublication {
    /// Lend the SRAM staging owner directly to the sole standalone endpoint.
    SharedStaging,
    /// Copy into the paired endpoint's PSRAM pool and release SRAM immediately.
    OwnedNetworkPool,
}

const fn can_publish_ap_rx_in_place(
    publication: AccessPointRxPublication,
    current_staging_owner: bool,
    current_is_amsdu: bool,
    deferred_bytes: usize,
) -> bool {
    matches!(publication, AccessPointRxPublication::SharedStaging)
        && current_staging_owner
        && !current_is_amsdu
        && deferred_bytes == 0
}

mod ampdu;
mod datapath;
pub mod network_tx;
mod protocol_mailbox;
pub mod runtime;
mod rx_pipeline;
mod rx_reorder;

pub use self::rx_reorder::{Esp32s31AccessPointRxReorder, Esp32s31AccessPointRxReorderError};
pub use ampdu::Esp32s31AccessPointAmpdu;
use datapath::{BlockAckObservationState, Esp32s31AccessPointDatapathServices};
use network_tx::Esp32s31AccessPointNetworkTx;
pub use protocol_mailbox::{
    Esp32s31AccessPointControlAction, Esp32s31AccessPointHardwareAction,
    Esp32s31AccessPointProtocolAction, Esp32s31AccessPointProtocolMailbox,
    Esp32s31AccessPointProtocolPublisher, Esp32s31AccessPointProtocolReceiver,
};
pub use runtime::{
    AccessPointRoleRuntime, Esp32s31StaApAccessPointFinishFailure,
    Esp32s31StaApAccessPointFinished, Esp32s31StaApAccessPointParkError,
    Esp32s31StaApAccessPointParkFailure, Esp32s31StaApAccessPointTxActive,
    Esp32s31StaApAccessPointTxParked, finish_sta_ap_access_point_role,
    park_sta_ap_access_point_role,
};
#[doc(hidden)]
pub use rx_pipeline::{
    AccessPointRxProducer, AccessPointRxProducerObservation, AccessPointRxProtocolConsumer,
    AccessPointStagedRxFrame, Esp32s31AccessPointRxConsumer, Esp32s31AccessPointRxProducer,
};

// One protected frame can produce a BlockAck-window reset and one peer
// activity update. The active-TX protocol quantum owns four frames, so the
// mailbox covers exactly one complete bounded turn.
const AP_PROTOCOL_ACTION_CAPACITY: usize = 8;
const AP_PROTOCOL_ACTIONS_PER_RX_FRAME: usize = 2;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Esp32s31AccessPointControlObservation {
    pub missed_beacon_intervals: u32,
    pub maximum_beacon_lateness_micros: u32,
    pub tx_interrupt_wakes: u32,
    pub tx_deadline_wakes: u32,
    pub maximum_tx_pending_micros: u32,
    /// Longest network-originated data transaction, excluding chained AP
    /// management, WPA2 and shutdown publications.
    pub maximum_network_tx_pending_micros: u32,
    /// Hardware publications made by the network frame which established
    /// `maximum_network_tx_pending_micros`.
    pub network_tx_attempts_at_maximum_pending: u8,
    pub maximum_rx_service_micros: u32,
    pub maximum_rx_dma_service_micros: u32,
    pub total_rx_dma_service_micros: u32,
    pub rx_dma_service_calls: u32,
    pub maximum_rx_protocol_service_micros: u32,
    pub maximum_rx_protected_data_service_micros: u32,
    pub total_rx_protected_data_service_micros: u32,
    pub maximum_rx_management_service_micros: u32,
    pub maximum_rx_eapol_service_micros: u32,
    pub maximum_network_backpressure_micros: u32,
    pub retained_rx_descriptors: u32,
    pub ignored_rx_frames: u32,
    pub rx_mic_failures: u32,
    pub rx_quarantined_frames: u32,
    pub rx_view_rejected: u32,
    pub control_frames_staged: u32,
    pub control_frames_dropped_while_busy: u32,
    pub ethernet_frames_staged: u32,
    pub ethernet_arp_requests_staged: u32,
    pub ethernet_tcp_frames_staged: u32,
    pub network_tx_frames_observed: u32,
    pub network_tx_arp_requests: u32,
    pub network_tx_arp_replies: u32,
    pub network_tx_rejected_no_peer: u32,
    pub network_tx_rejected_destination: u32,
    pub network_tx_frames_rejected: u32,
    /// Protected data MPDUs whose RX metadata identified an HT PPDU.
    pub rx_ht_data_frames: u32,
    /// Protected HT data MPDUs whose HT-SIG Aggregation bit was set.
    pub rx_ht_mpdus_with_aggregation_bit: u32,
    /// Protected data MPDUs with a hardware-observed RSSI sample.
    pub rx_rssi_samples: u32,
    /// Signed sum of hardware-observed RSSI samples in dBm.
    pub rx_rssi_sum_dbm: i32,
    pub rx_rssi_min_dbm: i8,
    pub rx_rssi_max_dbm: i8,
    /// Protected HT40 data MPDUs grouped by hardware-observed MCS0..MCS7.
    pub rx_ht40_mcs_frames: [u32; 8],
    /// Network A-MPDU transactions started with a typed HT rate.
    pub tx_ht_aggregates: u32,
    /// Network A-MPDU transactions started specifically at HT40 MCS7.
    pub tx_ht40_mcs7_aggregates: u32,
    pub protected_data_frames: u32,
    pub protected_data_unauthorized: u32,
    pub protected_data_foreign: u32,
    pub protected_data_duplicates: u32,
    pub rx_reorder_buffered_mpdus: u32,
    pub rx_reorder_dispatched_mpdus: u32,
    pub rx_reorder_hardware_window_resets: u32,
    pub rx_reorder_gap_timeouts: u32,
    pub protected_data_radio_rejected: u32,
    pub protected_data_protocol_rejected: u32,
}

#[cfg(any(feature = "diagnostics", test))]
macro_rules! observe_access_point {
    ($owner:expr, $observation:ident, $body:block) => {{
        let $observation = &mut $owner.observer.observation;
        $body
    }};
}

#[cfg(not(any(feature = "diagnostics", test)))]
macro_rules! observe_access_point {
    ($owner:expr, $observation:ident, $body:block) => {{}};
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AccessPointRxProtocolClass {
    ProtectedData,
    Management,
    Eapol,
    Other,
    Rejected,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31AccessPointControlError {
    Receive(open_esp_radio_esp32s31_wifi_mac::rx_pool::RxStageTransactionError),
    Mac(Esp32s31ApMacError),
    /// The caller-provided RX scratch cannot retain one fully decoded batch.
    ReceiveBatchCapacity,
    /// Protocol produced more value-only actions than one bounded turn owns.
    ProtocolActionCapacity,
    /// A non-data frame reached the protocol-only active-TX consumer.
    ProtocolFrameRequiresHardware,
    InvalidBeaconSchedule,
    RxBlockAckSession(RxBlockAckSessionsError),
    RxBlockAckHardware(S31RxBlockAckAgreementError),
    RxBlockAckReorder(Esp32s31AccessPointRxReorderError),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31AccessPointDatapathError {
    Control(Esp32s31AccessPointControlError),
    Network(FrameLengthError),
    Aggregate(Esp32s31ApAmpduError),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31AccessPointRunError<E> {
    Control(Esp32s31AccessPointControlError),
    InterruptActivate(Esp32s31MacInterruptEpochActivateError<E>),
    InterruptQuiesce(Esp32s31MacInterruptEpochQuiesceError<E>),
    Network(FrameLengthError),
    Aggregate(Esp32s31ApAmpduError),
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Esp32s31AccessPointRunObservation {
    #[cfg(any(feature = "diagnostics", test))]
    pub interrupt_drain: Esp32s31MacInterruptEpochDrain,
    #[cfg(any(feature = "diagnostics", test))]
    pub rx_scheduler: Option<Esp32s31RxFrontierSchedulerSnapshot>,
}

/// Complete reusable frontier after IRQ, RX, TX, keys and AP TSF stop.
pub struct Esp32s31AccessPointStopped<
    'storage,
    'beacon,
    'slot,
    P,
    E,
    T,
    R,
    C,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
    const TX_BUFFER_SIZE: usize,
> {
    pub receive: R,
    pub protocol_rx: C,
    pub transmit: WifiTxResources<'slot, P, E, T, TX_BUFFER_SIZE>,
    pub rx_frame: &'storage mut [u8],
    pub tx_frame: &'storage mut [u8],
    pub data_rx: &'storage mut Esp32s31ApRxDispatcher,
    pub rx_block_ack: &'storage Esp32s31StaApRxBlockAck,
    pub rx_reorder: &'storage mut Esp32s31AccessPointRxReorder<'storage, DMA_BUFFER_SIZE>,
    pub rx_reorder_storage:
        &'storage RxReorderFrameStorage<DMA_BUFFER_SIZE, RX_REORDER_BACKING_SLOT_COUNT>,
    #[cfg(feature = "diagnostics")]
    pub observation_storage: &'static mut AccessPointObservationStorage,
    pub engine: open_esp_radio_esp32s31_wifi_ap::engine::Esp32s31ApEngineStop<'beacon>,
}

/// Quiescent AP protocol owners returned by a paired STA+AP DATAPATH.
///
/// Physical RX is owned by the common paired producer and is intentionally
/// absent.  Ordinary TX is returned here so the paired boundary can rejoin it
/// with the shared physical owner before restoring the station graph.
pub struct Esp32s31AccessPointProtocolStopped<
    'storage,
    'beacon,
    'slot,
    P,
    E,
    T,
    const DMA_BUFFER_SIZE: usize,
    const TX_BUFFER_SIZE: usize,
> {
    pub transmit: WifiTxResources<'slot, P, E, T, TX_BUFFER_SIZE>,
    pub rx_frame: &'storage mut [u8],
    pub tx_frame: &'storage mut [u8],
    pub data_rx: &'storage mut Esp32s31ApRxDispatcher,
    pub rx_block_ack: &'storage Esp32s31StaApRxBlockAck,
    pub rx_reorder: &'storage mut Esp32s31AccessPointRxReorder<'storage, DMA_BUFFER_SIZE>,
    pub rx_reorder_storage:
        &'storage RxReorderFrameStorage<DMA_BUFFER_SIZE, RX_REORDER_BACKING_SLOT_COUNT>,
    #[cfg(feature = "diagnostics")]
    pub observation_storage: &'static mut AccessPointObservationStorage,
    pub engine: open_esp_radio_esp32s31_wifi_ap::engine::Esp32s31ApEngineStop<'beacon>,
}

/// AP role-local owners after ordinary TX has returned to the paired
/// physical owner.
pub struct Esp32s31AccessPointProtocolFinished<'storage, 'beacon, const DMA_BUFFER_SIZE: usize> {
    pub rx_frame: &'storage mut [u8],
    pub tx_frame: &'storage mut [u8],
    pub data_rx: &'storage mut Esp32s31ApRxDispatcher,
    pub rx_block_ack: &'storage Esp32s31StaApRxBlockAck,
    pub rx_reorder: &'storage mut Esp32s31AccessPointRxReorder<'storage, DMA_BUFFER_SIZE>,
    pub rx_reorder_storage:
        &'storage RxReorderFrameStorage<DMA_BUFFER_SIZE, RX_REORDER_BACKING_SLOT_COUNT>,
    #[cfg(feature = "diagnostics")]
    pub observation_storage: &'static mut AccessPointObservationStorage,
    pub engine: open_esp_radio_esp32s31_wifi_ap::engine::Esp32s31ApEngineStop<'beacon>,
}

impl<'storage, 'beacon, 'slot, P, E, T, const DMA_BUFFER_SIZE: usize, const TX_BUFFER_SIZE: usize>
    Esp32s31AccessPointProtocolStopped<
        'storage,
        'beacon,
        'slot,
        P,
        E,
        T,
        DMA_BUFFER_SIZE,
        TX_BUFFER_SIZE,
    >
{
    pub fn into_parts(
        self,
    ) -> (
        WifiTxResources<'slot, P, E, T, TX_BUFFER_SIZE>,
        Esp32s31AccessPointProtocolFinished<'storage, 'beacon, DMA_BUFFER_SIZE>,
    ) {
        (
            self.transmit,
            Esp32s31AccessPointProtocolFinished {
                rx_frame: self.rx_frame,
                tx_frame: self.tx_frame,
                data_rx: self.data_rx,
                rx_block_ack: self.rx_block_ack,
                rx_reorder: self.rx_reorder,
                rx_reorder_storage: self.rx_reorder_storage,
                #[cfg(feature = "diagnostics")]
                observation_storage: self.observation_storage,
                engine: self.engine,
            },
        )
    }
}

impl From<open_esp_radio_esp32s31_wifi_mac::rx_pool::RxStageTransactionError>
    for Esp32s31AccessPointControlError
{
    fn from(error: open_esp_radio_esp32s31_wifi_mac::rx_pool::RxStageTransactionError) -> Self {
        Self::Receive(error)
    }
}

impl From<Esp32s31ApMacError> for Esp32s31AccessPointControlError {
    fn from(error: Esp32s31ApMacError) -> Self {
        Self::Mac(error)
    }
}

impl From<RxBlockAckSessionsError> for Esp32s31AccessPointControlError {
    fn from(error: RxBlockAckSessionsError) -> Self {
        Self::RxBlockAckSession(error)
    }
}

impl From<S31RxBlockAckAgreementError> for Esp32s31AccessPointControlError {
    fn from(error: S31RxBlockAckAgreementError) -> Self {
        Self::RxBlockAckHardware(error)
    }
}

impl From<Esp32s31AccessPointRxReorderError> for Esp32s31AccessPointControlError {
    fn from(error: Esp32s31AccessPointRxReorderError) -> Self {
        Self::RxBlockAckReorder(error)
    }
}

impl From<open_esp_radio_esp32s31_wifi_ap::engine::Esp32s31ApEngineError>
    for Esp32s31AccessPointControlError
{
    fn from(error: open_esp_radio_esp32s31_wifi_ap::engine::Esp32s31ApEngineError) -> Self {
        Self::Mac(Esp32s31ApMacError::Engine(error))
    }
}

struct DeferredAccessPointRxSink<'storage> {
    frames: crate::datapath::rx::ethernet::PackedEthernetWriter<'storage>,
    exhausted: bool,
}

impl<'storage> DeferredAccessPointRxSink<'storage> {
    fn new(storage: &'storage mut [u8]) -> Self {
        Self {
            frames: crate::datapath::rx::ethernet::PackedEthernetWriter::new(storage),
            exhausted: false,
        }
    }

    const fn used(&self) -> usize {
        self.frames.used()
    }
}

impl Esp32s31ApRxSink for DeferredAccessPointRxSink<'_> {
    #[cfg_attr(
        target_arch = "riscv32",
        unsafe(link_section = ".hot.text.open_radio_ap_rx_sink")
    )]
    fn publish(&mut self, event: Esp32s31ApRxEvent<'_>) {
        if self.frames.push(event.frame).is_err() {
            self.exhausted = true;
        }
    }
}

/// Captures one ordinary Ethernet view as offsets inside its staging owner.
/// The owner is converted in place only after the AP dispatcher and reorder
/// state have accepted the frame.
struct InPlaceAccessPointRxSink {
    raw_start: usize,
    raw_length: usize,
    publication: Option<StagedEthernetPublication>,
    unsupported: bool,
}

impl InPlaceAccessPointRxSink {
    fn new(raw: &[u8]) -> Self {
        Self {
            raw_start: raw.as_ptr() as usize,
            raw_length: raw.len(),
            publication: None,
            unsupported: false,
        }
    }
}

impl Esp32s31ApRxSink for InPlaceAccessPointRxSink {
    fn publish(&mut self, event: Esp32s31ApRxEvent<'_>) {
        let payload_start = event.frame.payload.as_ptr() as usize;
        let payload_end = match payload_start.checked_add(event.frame.payload.len()) {
            Some(end) => end,
            None => {
                self.unsupported = true;
                return;
            }
        };
        let raw_end = self.raw_start.saturating_add(self.raw_length);
        if event.amsdu
            || self.publication.is_some()
            || payload_start < self.raw_start
            || payload_end > raw_end
        {
            self.unsupported = true;
            return;
        }
        self.publication = Some(StagedEthernetPublication {
            destination: event.frame.destination,
            source: event.frame.source,
            ether_type: event.frame.ether_type,
            payload_offset: payload_start - self.raw_start,
            payload_length: event.frame.payload.len(),
            metadata: event.metadata,
        });
    }
}

#[cfg(test)]
mod in_place_rx_sink_tests {
    use open_esp_radio_wifi_softmac::MacRxMetadata;

    use super::*;

    #[test]
    fn active_tx_protocol_consumer_has_no_hardware_capability() {
        assert!(rx_protocol_consumer_has_hardware(false));
        assert!(!rx_protocol_consumer_has_hardware(true));
    }

    fn event<'a>(payload: &'a [u8], amsdu: bool) -> Esp32s31ApRxEvent<'a> {
        Esp32s31ApRxEvent {
            frame: EthernetFrameParts {
                destination: [1, 2, 3, 4, 5, 6],
                source: [7, 8, 9, 10, 11, 12],
                ether_type: 0x0800,
                payload,
            },
            raw: payload,
            amsdu,
            metadata: MacRxMetadata::unavailable(),
        }
    }

    #[test]
    fn captures_one_ordinary_frame_as_staging_offsets() {
        let raw = [0_u8; 64];
        let mut sink = InPlaceAccessPointRxSink::new(&raw);

        sink.publish(event(&raw[17..43], false));

        let publication = sink.publication.expect("ordinary frame is captured");
        assert_eq!(publication.payload_offset, 17);
        assert_eq!(publication.payload_length, 26);
        assert!(!sink.unsupported);
    }

    #[test]
    fn rejects_amsdu_and_payloads_outside_the_staging_owner() {
        let raw = [0_u8; 64];
        let external = [0_u8; 8];

        let mut amsdu = InPlaceAccessPointRxSink::new(&raw);
        amsdu.publish(event(&raw[16..24], true));
        assert!(amsdu.publication.is_none());
        assert!(amsdu.unsupported);

        let mut outside = InPlaceAccessPointRxSink::new(&raw);
        outside.publish(event(&external, false));
        assert!(outside.publication.is_none());
        assert!(outside.unsupported);
    }

    #[test]
    fn current_frame_joins_an_older_deferred_reorder_release() {
        assert!(can_publish_ap_rx_in_place(
            AccessPointRxPublication::SharedStaging,
            true,
            false,
            0
        ));
        assert!(!can_publish_ap_rx_in_place(
            AccessPointRxPublication::SharedStaging,
            true,
            false,
            64
        ));
        assert!(!can_publish_ap_rx_in_place(
            AccessPointRxPublication::SharedStaging,
            true,
            true,
            0
        ));
        assert!(!can_publish_ap_rx_in_place(
            AccessPointRxPublication::SharedStaging,
            false,
            false,
            0
        ));
        assert!(!can_publish_ap_rx_in_place(
            AccessPointRxPublication::OwnedNetworkPool,
            true,
            false,
            0
        ));
    }
}

fn observe_protected_dispatch(
    dispatch: Esp32s31ApRxDispatch,
    peer: Option<[u8; 6]>,
    #[cfg(any(feature = "diagnostics", test))] report: &mut Esp32s31AccessPointControlObservation,
    activity_peer: &mut Option<[u8; 6]>,
) -> bool {
    match dispatch {
        Esp32s31ApRxDispatch::Data {
            ethernet_frames, ..
        } => {
            if ethernet_frames != 0 {
                *activity_peer = peer;
                true
            } else {
                false
            }
        }
        Esp32s31ApRxDispatch::Duplicate => {
            #[cfg(any(feature = "diagnostics", test))]
            {
                report.protected_data_duplicates =
                    report.protected_data_duplicates.saturating_add(1);
            }
            false
        }
        Esp32s31ApRxDispatch::ForeignPeer => {
            #[cfg(any(feature = "diagnostics", test))]
            {
                report.protected_data_foreign = report.protected_data_foreign.saturating_add(1);
            }
            false
        }
        Esp32s31ApRxDispatch::Unauthorized => {
            #[cfg(any(feature = "diagnostics", test))]
            {
                report.protected_data_unauthorized =
                    report.protected_data_unauthorized.saturating_add(1);
            }
            false
        }
        Esp32s31ApRxDispatch::Rejected(Esp32s31ApRxError::Radio(_)) => {
            #[cfg(any(feature = "diagnostics", test))]
            {
                report.protected_data_radio_rejected =
                    report.protected_data_radio_rejected.saturating_add(1);
            }
            false
        }
        Esp32s31ApRxDispatch::Rejected(Esp32s31ApRxError::Data(_)) => {
            #[cfg(any(feature = "diagnostics", test))]
            {
                report.protected_data_protocol_rejected =
                    report.protected_data_protocol_rejected.saturating_add(1);
            }
            false
        }
    }
}

/// Hot AP data-dispatch leaf shared by direct and retained reorder releases.
/// It owns no hardware capability and reports only value/borrowed protocol
/// outcomes to its caller.
struct AccessPointProtectedFrameDispatch;

impl AccessPointProtectedFrameDispatch {
    #[cfg_attr(
        target_arch = "riscv32",
        unsafe(link_section = ".hot.text.open_radio_ap_rx_dispatch")
    )]
    #[inline(never)]
    fn dispatch(
        data_rx: &mut Esp32s31ApRxDispatcher,
        ordered: open_esp_radio_esp32s31_wifi_mac::rx::RxSegment<'_>,
        mut is_authorized: impl FnMut([u8; 6]) -> bool,
        publication: AccessPointRxPublication,
        current_buffer: usize,
        current_is_amsdu: bool,
        deferred: &mut DeferredAccessPointRxSink<'_>,
        in_place: &mut InPlaceAccessPointRxSink,
        #[cfg(any(feature = "diagnostics", test))]
        report: &mut Esp32s31AccessPointControlObservation,
        activity_peer: &mut Option<[u8; 6]>,
        produced_data: &mut bool,
    ) {
        let peer = data_rx
            .reorder_key(ordered)
            .map(|key| key.peer)
            .or_else(|| {
                view_normalized_rx_frame(
                    &ordered,
                    RxIngressConfig {
                        ring_entry_limit: 1,
                        csi_config: 0,
                        flags: 0,
                    },
                )
                .ok()
                .and_then(|frame| frame.mpdu.get(10..16))
                .and_then(|bytes| <[u8; 6]>::try_from(bytes).ok())
            });
        let current = ordered.buffer.as_ptr() as usize == current_buffer;
        let outcome = if can_publish_ap_rx_in_place(
            publication,
            current,
            current_is_amsdu,
            deferred.used(),
        ) {
            data_rx.dispatch_protected(ordered, &mut is_authorized, in_place)
        } else {
            data_rx.dispatch_protected(ordered, &mut is_authorized, deferred)
        };
        *produced_data |= observe_protected_dispatch(
            outcome,
            peer,
            #[cfg(any(feature = "diagnostics", test))]
            report,
            activity_peer,
        );
    }
}

/// Control-plane owner for one active AP role.
pub struct Esp32s31AccessPointProtocolProcessor<
    'storage,
    'beacon,
    'slot,
    P,
    E,
    T,
    const DMA_BUFFER_SIZE: usize,
    const TX_BUFFER_SIZE: usize,
> {
    mac: Esp32s31ApMac<'beacon, 'slot, P, E, T, TX_BUFFER_SIZE>,
    rx_frame: &'storage mut [u8],
    tx_frame: &'storage mut [u8],
    data_rx: &'storage mut Esp32s31ApRxDispatcher,
    rx_block_ack: &'storage Esp32s31StaApRxBlockAck,
    rx_reorder: &'storage mut Esp32s31AccessPointRxReorder<'storage, DMA_BUFFER_SIZE>,
    rx_reorder_storage:
        &'storage RxReorderFrameStorage<DMA_BUFFER_SIZE, RX_REORDER_BACKING_SLOT_COUNT>,
    rx_addba_in_flight: Option<RxBlockAckActivation>,
    protocol_actions: Esp32s31AccessPointProtocolMailbox<AP_PROTOCOL_ACTION_CAPACITY>,
    rx_batch_used: usize,
    rx_batch_offset: usize,
    serviced_rx_frames: u64,
    serviced_rx_descriptors: u64,
    #[cfg(feature = "diagnostics")]
    observer: &'static mut AccessPointObservationStorage,
    #[cfg(all(test, not(feature = "diagnostics")))]
    observer: AccessPointObservationStorage,
    #[cfg(any(feature = "diagnostics", test))]
    terminal_observer: Option<&'static dyn AccessPointTerminalObserver>,
}

/// AP protocol state with the unique ordinary-TX resource removed.
///
/// This value retains peer, beacon, BlockAck, reorder, mailbox and report
/// state, but it cannot publish hardware until the paired physical owner
/// returns the exact ordinary-TX capability through `resume`.
pub struct Esp32s31AccessPointProtocolProcessorParked<
    'storage,
    'beacon,
    const DMA_BUFFER_SIZE: usize,
> {
    mac: Esp32s31ApMacParked<'beacon>,
    rx_frame: &'storage mut [u8],
    tx_frame: &'storage mut [u8],
    data_rx: &'storage mut Esp32s31ApRxDispatcher,
    rx_block_ack: &'storage Esp32s31StaApRxBlockAck,
    rx_reorder: &'storage mut Esp32s31AccessPointRxReorder<'storage, DMA_BUFFER_SIZE>,
    rx_reorder_storage:
        &'storage RxReorderFrameStorage<DMA_BUFFER_SIZE, RX_REORDER_BACKING_SLOT_COUNT>,
    rx_addba_in_flight: Option<RxBlockAckActivation>,
    protocol_actions: Esp32s31AccessPointProtocolMailbox<AP_PROTOCOL_ACTION_CAPACITY>,
    rx_batch_used: usize,
    rx_batch_offset: usize,
    serviced_rx_frames: u64,
    serviced_rx_descriptors: u64,
    #[cfg(feature = "diagnostics")]
    observer: &'static mut AccessPointObservationStorage,
    #[cfg(all(test, not(feature = "diagnostics")))]
    observer: AccessPointObservationStorage,
    #[cfg(any(feature = "diagnostics", test))]
    terminal_observer: Option<&'static dyn AccessPointTerminalObserver>,
}

impl<'storage, 'beacon, const DMA_BUFFER_SIZE: usize>
    Esp32s31AccessPointProtocolProcessorParked<'storage, 'beacon, DMA_BUFFER_SIZE>
{
    pub const fn rx_batch_pending(&self) -> bool {
        self.rx_batch_offset < self.rx_batch_used
    }

    fn rx_batch_record(
        &self,
    ) -> Result<
        Option<crate::datapath::rx::ethernet::PackedEthernetRecord<'_>>,
        Esp32s31AccessPointControlError,
    > {
        crate::datapath::rx::ethernet::record_at(
            self.rx_frame,
            self.rx_batch_used,
            self.rx_batch_offset,
        )
        .map_err(|_| Esp32s31AccessPointControlError::ReceiveBatchCapacity)
    }

    fn commit_rx_batch_record(&mut self, next_offset: usize) {
        debug_assert!(next_offset > self.rx_batch_offset);
        debug_assert!(next_offset <= self.rx_batch_used);
        self.rx_batch_offset = next_offset;
        if self.rx_batch_offset == self.rx_batch_used {
            self.rx_batch_offset = 0;
            self.rx_batch_used = 0;
        }
    }

    pub const fn beacon_publication_due(&self, now_micros: u32) -> bool {
        self.mac.beacon_publication_due(now_micros)
    }

    fn next_control_delay_millis(
        &self,
        now_micros: u64,
    ) -> Result<u32, Esp32s31AccessPointControlError> {
        let (_, beacon_delay_ms) = self
            .mac
            .next_beacon_delay(now_micros as u32)
            .ok_or(Esp32s31AccessPointControlError::InvalidBeaconSchedule)?;
        let deadline_delay = |deadline: u64| {
            let remaining = deadline.saturating_sub(now_micros);
            u32::try_from(remaining.saturating_add(999) / 1_000)
                .unwrap_or(u32::MAX)
                .max(1)
        };
        Ok(self
            .mac
            .next_control_deadline()
            .into_iter()
            .chain(self.rx_reorder.next_deadline())
            .map(deadline_delay)
            .fold(beacon_delay_ms, u32::min))
    }

    pub fn has_operational_tx_block_ack(&self) -> bool {
        self.mac.has_operational_tx_block_ack()
    }
}

/// Standalone AP composition of a physical RX transport and the
/// queue-independent AP protocol processor.
pub struct Esp32s31AccessPointControl<
    'storage,
    'beacon,
    'slot,
    R,
    C,
    P,
    E,
    T,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
    const TX_BUFFER_SIZE: usize,
> {
    receive: R,
    protocol_rx: C,
    role: AccessPointRoleRuntime<
        Esp32s31AccessPointProtocolProcessor<
            'storage,
            'beacon,
            'slot,
            P,
            E,
            T,
            DMA_BUFFER_SIZE,
            TX_BUFFER_SIZE,
        >,
        (),
        (),
        (),
    >,
}

impl<
    'storage,
    'beacon,
    'slot,
    R,
    C,
    P,
    E,
    T,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
    const TX_BUFFER_SIZE: usize,
> core::ops::Deref
    for Esp32s31AccessPointControl<
        'storage,
        'beacon,
        'slot,
        R,
        C,
        P,
        E,
        T,
        COUNT,
        DMA_BUFFER_SIZE,
        DMA_STORAGE_SIZE,
        TX_BUFFER_SIZE,
    >
{
    type Target = Esp32s31AccessPointProtocolProcessor<
        'storage,
        'beacon,
        'slot,
        P,
        E,
        T,
        DMA_BUFFER_SIZE,
        TX_BUFFER_SIZE,
    >;

    fn deref(&self) -> &Self::Target {
        self.role.protocol()
    }
}

impl<
    'storage,
    'beacon,
    'slot,
    R,
    C,
    P,
    E,
    T,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
    const TX_BUFFER_SIZE: usize,
> core::ops::DerefMut
    for Esp32s31AccessPointControl<
        'storage,
        'beacon,
        'slot,
        R,
        C,
        P,
        E,
        T,
        COUNT,
        DMA_BUFFER_SIZE,
        DMA_STORAGE_SIZE,
        TX_BUFFER_SIZE,
    >
{
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.role.protocol_mut()
    }
}

impl<
    'storage,
    'beacon,
    'slot,
    R,
    C,
    P,
    E,
    T,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
    const TX_BUFFER_SIZE: usize,
>
    Esp32s31AccessPointControl<
        'storage,
        'beacon,
        'slot,
        R,
        C,
        P,
        E,
        T,
        COUNT,
        DMA_BUFFER_SIZE,
        DMA_STORAGE_SIZE,
        TX_BUFFER_SIZE,
    >
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: WifiTxTimer,
{
    pub fn new(
        receive: R,
        protocol_rx: C,
        mac: Esp32s31ApMac<'beacon, 'slot, P, E, T, TX_BUFFER_SIZE>,
        rx_frame: &'storage mut [u8],
        tx_frame: &'storage mut [u8],
        data_rx: &'storage mut Esp32s31ApRxDispatcher,
        rx_block_ack: &'storage Esp32s31StaApRxBlockAck,
        rx_reorder: &'storage mut Esp32s31AccessPointRxReorder<'storage, DMA_BUFFER_SIZE>,
        rx_reorder_storage: &'storage RxReorderFrameStorage<
            DMA_BUFFER_SIZE,
            RX_REORDER_BACKING_SLOT_COUNT,
        >,
        #[cfg(feature = "diagnostics")]
        observation_storage: &'static mut AccessPointObservationStorage,
    ) -> Self {
        Self {
            receive,
            protocol_rx,
            role: AccessPointRoleRuntime::standalone(Esp32s31AccessPointProtocolProcessor::new(
                mac,
                rx_frame,
                tx_frame,
                data_rx,
                rx_block_ack,
                rx_reorder,
                rx_reorder_storage,
                #[cfg(feature = "diagnostics")]
                observation_storage,
            )),
        }
    }

    /// Attach the non-owning terminal observer to the standalone AP role.
    #[cfg(any(feature = "diagnostics", test))]
    pub fn with_terminal_observer(
        mut self,
        observer: &'static dyn AccessPointTerminalObserver,
    ) -> Self {
        self.role.protocol_mut().terminal_observer = Some(observer);
        self
    }

    pub async fn start<H: RxDma>(
        &mut self,
        hardware: &mut H,
    ) -> Result<(), Esp32s31AccessPointControlError>
    where
        R: AccessPointRxProducer<H, COUNT>,
    {
        self.receive.start(hardware).await?;
        Ok(())
    }

    pub fn stop<H: RxDma>(
        &mut self,
        hardware: &mut H,
    ) -> Result<(), Esp32s31AccessPointControlError>
    where
        R: AccessPointRxProducer<H, COUNT>,
    {
        self.receive.stop(hardware)?;
        Ok(())
    }

    /// Observe one RX descriptor without exposing its DMA ownership.
    #[cfg(any(feature = "diagnostics", test))]
    pub fn rx_descriptor_snapshot(&self, index: usize) -> Option<RxDescriptorSnapshot>
    where
        R: AccessPointRxProducerObservation<COUNT>,
    {
        self.receive.descriptor_snapshot(index)
    }

    /// Observe the live RX scheduler frontier without exposing ownership.
    #[cfg(any(feature = "diagnostics", test))]
    pub fn rx_scheduler_snapshot(&self) -> Option<Esp32s31RxFrontierSchedulerSnapshot>
    where
        R: AccessPointRxProducerObservation<COUNT>,
    {
        self.receive.scheduler_snapshot()
    }

    /// Stage a complete finite DMA frontier before processing one AP frame.
    ///
    /// The common producer returns safe descriptors before AP parsing, WPA2,
    /// reorder, or network publication can extend the BUFFER_FULL interval.
    pub async fn service_rx<H, Q>(
        &mut self,
        hardware: &mut H,
        authenticator_nonce: [u8; 32],
        initial_replay_counter: u64,
        now_micros: u64,
        publish_shared_rx: &mut Q,
        #[cfg(feature = "diagnostics")] delivery_observer: Option<&dyn RxNetworkDeliveryObserver>,
    ) -> Result<DatapathRxProgress, Esp32s31AccessPointControlError>
    where
        H: RxDma + TxHardware + Esp32s31ApRuntimeHardware + RxBlockAckHardware,
        R: AccessPointRxProducer<H, COUNT>,
        C: AccessPointRxProtocolConsumer,
        Q: FnMut(u8),
    {
        let tx_pending = self.mac.tx_pending();
        self.apply_protocol_actions(hardware)?;
        let protocol_blocked = self.rx_batch_pending();
        let queued_frames = self.protocol_rx.queued_frames();
        let stage_progress = if should_observe_ap_rx_dma(protocol_blocked, queued_frames) {
            self.service_rx_dma(hardware).await?
        } else {
            DatapathRxProgress::ProbePending
        };

        // Vendor `wDev_ProcessFiq` services the RX-success frontier even when
        // a TX transaction is active. Preserve that ownership edge by moving
        // complete DMA units into independent staging first. Protocol work is
        // Management and EAPOL are deferred while TX owns the sole transmit
        // transaction because they may publish a response. Protected data is
        // protocol-only and must continue releasing the bounded staging pool.
        if protocol_blocked {
            return Ok(stage_progress);
        }
        if self.service_rx_reorder_expiry(now_micros)? {
            return Ok(DatapathRxProgress::ProbePending);
        }

        let staged_frame = if tx_pending {
            self.protocol_rx.try_receive_protected_data()
        } else {
            self.protocol_rx.try_receive()
        };
        let Some(staged_frame) = staged_frame else {
            return Ok(stage_progress);
        };
        self.serviced_rx_frames = self.serviced_rx_frames.saturating_add(1);
        #[cfg(feature = "diagnostics")]
        let protocol_started = Instant::now().as_micros();
        let protocol_class = self.service_staged_rx(
            if rx_protocol_consumer_has_hardware(tx_pending) {
                Some(hardware)
            } else {
                None
            },
            staged_frame,
            AccessPointRxPublication::SharedStaging,
            authenticator_nonce,
            initial_replay_counter,
            now_micros,
            publish_shared_rx,
            #[cfg(feature = "diagnostics")]
            delivery_observer,
        )?;
        // `service_staged_rx` receives no hardware capability while TX is
        // active. Only after that protocol borrow ends does this radio owner
        // translate and execute its value-only mailbox requests.
        self.apply_protocol_actions(hardware)?;
        #[cfg(not(feature = "diagnostics"))]
        let _ = protocol_class;
        #[cfg(feature = "diagnostics")]
        self.observe_rx_protocol_service(
            protocol_class,
            Instant::now().as_micros().saturating_sub(protocol_started),
        );

        Ok(
            if self.protocol_rx.queued_frames() != 0
                || self.rx_batch_pending()
                || self.mac.tx_pending()
            {
                DatapathRxProgress::ProbePending
            } else {
                stage_progress
            },
        )
    }

    /// Drain the hardware RX completion frontier into independently owned
    /// staging slots without parsing a frame or producing a control action.
    /// This is the only AP RX operation allowed to touch DMA hardware while
    /// TX owns the shared MAC transaction domain. The separate protocol
    /// consumer may parse protected data but can only publish typed actions.
    pub async fn service_rx_dma<H>(
        &mut self,
        hardware: &mut H,
    ) -> Result<DatapathRxProgress, Esp32s31AccessPointControlError>
    where
        H: RxDma,
        R: AccessPointRxProducer<H, COUNT>,
    {
        #[cfg(feature = "diagnostics")]
        let started = Instant::now().as_micros();
        let progress = self.receive.stage_completed(hardware).await?;
        #[cfg(feature = "diagnostics")]
        {
            let elapsed = Instant::now().as_micros().saturating_sub(started);
            self.observer.observation.maximum_rx_dma_service_micros = self
                .observer
                .observation
                .maximum_rx_dma_service_micros
                .max(u32::try_from(elapsed).unwrap_or(u32::MAX));
            self.observer.observation.total_rx_dma_service_micros = self
                .observer
                .observation
                .total_rx_dma_service_micros
                .saturating_add(u32::try_from(elapsed).unwrap_or(u32::MAX));
            self.observer.observation.rx_dma_service_calls = self
                .observer
                .observation
                .rx_dma_service_calls
                .saturating_add(1);
        }
        self.serviced_rx_descriptors = self.receive.serviced_descriptors();
        Ok(progress)
    }
}

impl<'storage, 'beacon, 'slot, P, E, T, const DMA_BUFFER_SIZE: usize, const TX_BUFFER_SIZE: usize>
    Esp32s31AccessPointProtocolProcessor<
        'storage,
        'beacon,
        'slot,
        P,
        E,
        T,
        DMA_BUFFER_SIZE,
        TX_BUFFER_SIZE,
    >
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: WifiTxTimer,
{
    /// Construct AP protocol/control state without binding a DMA producer or
    /// private RX queue. Same-channel STA+AP composition owns those physical
    /// resources at its common DATAPATH boundary.
    pub fn new(
        mac: Esp32s31ApMac<'beacon, 'slot, P, E, T, TX_BUFFER_SIZE>,
        rx_frame: &'storage mut [u8],
        tx_frame: &'storage mut [u8],
        data_rx: &'storage mut Esp32s31ApRxDispatcher,
        rx_block_ack: &'storage Esp32s31StaApRxBlockAck,
        rx_reorder: &'storage mut Esp32s31AccessPointRxReorder<'storage, DMA_BUFFER_SIZE>,
        rx_reorder_storage: &'storage RxReorderFrameStorage<
            DMA_BUFFER_SIZE,
            RX_REORDER_BACKING_SLOT_COUNT,
        >,
        #[cfg(feature = "diagnostics")]
        observation_storage: &'static mut AccessPointObservationStorage,
    ) -> Self {
        let access_point = mac.engine().service_address();
        data_rx.reset(Esp32s31ApRxConfig {
            access_point,
            ingress: RxIngressConfig {
                ring_entry_limit: 1,
                csi_config: 0,
                flags: 0,
            },
        });
        rx_block_ack
            .prepare_interface(MacInterface::AccessPoint)
            .expect("AP start requires its previous RX BlockAck epoch to be quiescent");
        let discarded_reorder_frames = rx_reorder.discard_all();
        debug_assert_eq!(discarded_reorder_frames, 0);
        #[cfg(feature = "diagnostics")]
        observation_storage.reset();
        Self {
            mac,
            rx_frame,
            tx_frame,
            data_rx,
            rx_block_ack,
            rx_reorder,
            rx_reorder_storage,
            rx_addba_in_flight: None,
            protocol_actions: Esp32s31AccessPointProtocolMailbox::new(),
            rx_batch_used: 0,
            rx_batch_offset: 0,
            serviced_rx_frames: 0,
            serviced_rx_descriptors: 0,
            #[cfg(feature = "diagnostics")]
            observer: observation_storage,
            #[cfg(all(test, not(feature = "diagnostics")))]
            observer: AccessPointObservationStorage::default(),
            #[cfg(any(feature = "diagnostics", test))]
            terminal_observer: None,
        }
    }

    /// Attach the non-owning terminal observer for this AP role epoch.
    #[cfg(any(feature = "diagnostics", test))]
    pub fn with_terminal_observer(
        mut self,
        observer: &'static dyn AccessPointTerminalObserver,
    ) -> Self {
        self.terminal_observer = Some(observer);
        self
    }

    /// Remove the unique ordinary-TX capability at an idle transaction edge.
    /// A pending publication fails closed and returns the complete processor
    /// unchanged, so the caller cannot lose either hardware or protocol state.
    #[allow(clippy::result_large_err, clippy::type_complexity)]
    pub fn try_park(
        self,
    ) -> Result<
        (
            WifiTxResources<'slot, P, E, T, TX_BUFFER_SIZE>,
            Esp32s31AccessPointProtocolProcessorParked<'storage, 'beacon, DMA_BUFFER_SIZE>,
        ),
        Self,
    > {
        let Self {
            mac,
            rx_frame,
            tx_frame,
            data_rx,
            rx_block_ack,
            rx_reorder,
            rx_reorder_storage,
            rx_addba_in_flight,
            protocol_actions,
            rx_batch_used,
            rx_batch_offset,
            serviced_rx_frames,
            serviced_rx_descriptors,
            #[cfg(any(feature = "diagnostics", test))]
            observer,
            #[cfg(any(feature = "diagnostics", test))]
            terminal_observer,
        } = self;
        match mac.try_park() {
            Ok((resources, mac)) => Ok((
                resources,
                Esp32s31AccessPointProtocolProcessorParked {
                    mac,
                    rx_frame,
                    tx_frame,
                    data_rx,
                    rx_block_ack,
                    rx_reorder,
                    rx_reorder_storage,
                    rx_addba_in_flight,
                    protocol_actions,
                    rx_batch_used,
                    rx_batch_offset,
                    serviced_rx_frames,
                    serviced_rx_descriptors,
                    #[cfg(any(feature = "diagnostics", test))]
                    observer,
                    #[cfg(any(feature = "diagnostics", test))]
                    terminal_observer,
                },
            )),
            Err(mac) => Err(Self {
                mac,
                rx_frame,
                tx_frame,
                data_rx,
                rx_block_ack,
                rx_reorder,
                rx_reorder_storage,
                rx_addba_in_flight,
                protocol_actions,
                rx_batch_used,
                rx_batch_offset,
                serviced_rx_frames,
                serviced_rx_descriptors,
                #[cfg(any(feature = "diagnostics", test))]
                observer,
                #[cfg(any(feature = "diagnostics", test))]
                terminal_observer,
            }),
        }
    }

    /// Reconstitute the AP processor from its exact role state and the sole
    /// ordinary-TX capability owned by the paired physical transaction.
    pub fn resume(
        resources: WifiTxResources<'slot, P, E, T, TX_BUFFER_SIZE>,
        parked: Esp32s31AccessPointProtocolProcessorParked<'storage, 'beacon, DMA_BUFFER_SIZE>,
    ) -> Self {
        let Esp32s31AccessPointProtocolProcessorParked {
            mac,
            rx_frame,
            tx_frame,
            data_rx,
            rx_block_ack,
            rx_reorder,
            rx_reorder_storage,
            rx_addba_in_flight,
            protocol_actions,
            rx_batch_used,
            rx_batch_offset,
            serviced_rx_frames,
            serviced_rx_descriptors,
            #[cfg(any(feature = "diagnostics", test))]
            observer,
            #[cfg(any(feature = "diagnostics", test))]
            terminal_observer,
        } = parked;
        Self {
            mac: Esp32s31ApMac::resume(resources, mac),
            rx_frame,
            tx_frame,
            data_rx,
            rx_block_ack,
            rx_reorder,
            rx_reorder_storage,
            rx_addba_in_flight,
            protocol_actions,
            rx_batch_used,
            rx_batch_offset,
            serviced_rx_frames,
            serviced_rx_descriptors,
            #[cfg(any(feature = "diagnostics", test))]
            observer,
            #[cfg(any(feature = "diagnostics", test))]
            terminal_observer,
        }
    }

    /// Consume one frame already classified for the AP by the common physical
    /// RX dispatcher.
    ///
    /// This path owns no DMA operation and never reads an AP-private queue. It
    /// is the protocol boundary used by same-channel STA+AP composition. If
    /// ordering or an active hardware TX prevents safe processing, the exact
    /// staging lease is returned instead of copied or dropped.
    pub fn service_routed_rx<H, F, Q>(
        &mut self,
        hardware: &mut H,
        frame: F,
        authenticator_nonce: [u8; 32],
        initial_replay_counter: u64,
        now_micros: u64,
        publish_shared_rx: &mut Q,
        #[cfg(feature = "diagnostics")] delivery_observer: Option<&dyn RxNetworkDeliveryObserver>,
    ) -> Result<
        crate::roles::concurrent::Esp32s31RoutedRxDisposition<F>,
        Esp32s31AccessPointControlError,
    >
    where
        H: TxHardware + Esp32s31ApRuntimeHardware + RxBlockAckHardware,
        F: AccessPointStagedRxFrame,
        Q: FnMut(u8),
    {
        let tx_pending = self.mac.tx_pending();
        self.apply_protocol_actions(hardware)?;
        if self.rx_batch_pending() || self.service_rx_reorder_expiry(now_micros)? {
            return Ok(crate::roles::concurrent::Esp32s31RoutedRxDisposition::Deferred(frame));
        }

        if tx_pending && !rx_pipeline::is_protected_data(frame.segment()) {
            return Ok(crate::roles::concurrent::Esp32s31RoutedRxDisposition::Deferred(frame));
        }

        self.serviced_rx_frames = self.serviced_rx_frames.saturating_add(1);
        #[cfg(feature = "diagnostics")]
        let protocol_started = Instant::now().as_micros();
        let protocol_class = self.service_staged_rx(
            rx_protocol_consumer_has_hardware(tx_pending).then_some(hardware),
            frame,
            AccessPointRxPublication::OwnedNetworkPool,
            authenticator_nonce,
            initial_replay_counter,
            now_micros,
            publish_shared_rx,
            #[cfg(feature = "diagnostics")]
            delivery_observer,
        )?;
        self.apply_protocol_actions(hardware)?;

        #[cfg(not(feature = "diagnostics"))]
        let _ = protocol_class;
        #[cfg(feature = "diagnostics")]
        self.observe_rx_protocol_service(
            protocol_class,
            Instant::now().as_micros().saturating_sub(protocol_started),
        );
        Ok(crate::roles::concurrent::Esp32s31RoutedRxDisposition::Processed)
    }

    /// Consume only protected data while another transaction owns the
    /// physical TX domain.
    ///
    /// The frame parser may update role-local reorder/report state and append
    /// value-only mailbox actions. It cannot borrow MMIO or publish a frame;
    /// management and EAPOL owners are returned unchanged for the first idle
    /// transaction boundary.
    pub fn service_routed_rx_during_tx<H, F, Q>(
        &mut self,
        frame: F,
        authenticator_nonce: [u8; 32],
        initial_replay_counter: u64,
        now_micros: u64,
        publish_shared_rx: &mut Q,
        #[cfg(feature = "diagnostics")] delivery_observer: Option<&dyn RxNetworkDeliveryObserver>,
    ) -> Result<
        crate::roles::concurrent::Esp32s31RoutedRxDisposition<F>,
        Esp32s31AccessPointControlError,
    >
    where
        H: TxHardware + Esp32s31ApRuntimeHardware + RxBlockAckHardware,
        F: AccessPointStagedRxFrame,
        Q: FnMut(u8),
    {
        if self.rx_batch_pending() || !rx_pipeline::is_protected_data(frame.segment()) {
            return Ok(crate::roles::concurrent::Esp32s31RoutedRxDisposition::Deferred(frame));
        }
        // Do not consume the affine staging lease unless every value-only
        // action this frame can produce has a slot. A long physical TX may
        // admit several DMA/protocol turns before hardware can drain the
        // mailbox; the exact ordered head remains queued instead of turning
        // bounded backpressure into a role fault.
        if self.protocol_actions.remaining_capacity() < AP_PROTOCOL_ACTIONS_PER_RX_FRAME {
            return Ok(crate::roles::concurrent::Esp32s31RoutedRxDisposition::Deferred(frame));
        }
        if self
            .rx_reorder
            .next_deadline()
            .is_some_and(|deadline| deadline <= now_micros)
        {
            return Ok(crate::roles::concurrent::Esp32s31RoutedRxDisposition::Deferred(frame));
        }

        self.serviced_rx_frames = self.serviced_rx_frames.saturating_add(1);
        #[cfg(feature = "diagnostics")]
        let protocol_started = Instant::now().as_micros();
        let protocol_class = self.service_staged_rx::<H, _, _>(
            None,
            frame,
            AccessPointRxPublication::OwnedNetworkPool,
            authenticator_nonce,
            initial_replay_counter,
            now_micros,
            publish_shared_rx,
            #[cfg(feature = "diagnostics")]
            delivery_observer,
        )?;
        #[cfg(not(feature = "diagnostics"))]
        let _ = protocol_class;
        #[cfg(feature = "diagnostics")]
        self.observe_rx_protocol_service(
            protocol_class,
            Instant::now().as_micros().saturating_sub(protocol_started),
        );
        Ok(crate::roles::concurrent::Esp32s31RoutedRxDisposition::Processed)
    }

    /// Execute value-only RX actions after the physical transaction owner has
    /// returned. This is the sole paired-role mailbox drain edge.
    pub fn apply_pending_protocol_actions<H>(
        &mut self,
        hardware: &mut H,
    ) -> Result<(), Esp32s31AccessPointControlError>
    where
        H: RxBlockAckHardware,
    {
        self.apply_protocol_actions(hardware)
    }

    #[cfg(feature = "diagnostics")]
    fn observe_rx_protocol_service(
        &mut self,
        protocol_class: AccessPointRxProtocolClass,
        elapsed: u64,
    ) {
        self.observer.observation.maximum_rx_protocol_service_micros = self
            .observer
            .observation
            .maximum_rx_protocol_service_micros
            .max(u32::try_from(elapsed).unwrap_or(u32::MAX));
        let elapsed = u32::try_from(elapsed).unwrap_or(u32::MAX);
        let class_maximum = match protocol_class {
            AccessPointRxProtocolClass::ProtectedData => {
                self.observer
                    .observation
                    .total_rx_protected_data_service_micros = self
                    .observer
                    .observation
                    .total_rx_protected_data_service_micros
                    .saturating_add(elapsed);
                Some(
                    &mut self
                        .observer
                        .observation
                        .maximum_rx_protected_data_service_micros,
                )
            }
            AccessPointRxProtocolClass::Management => Some(
                &mut self
                    .observer
                    .observation
                    .maximum_rx_management_service_micros,
            ),
            AccessPointRxProtocolClass::Eapol => {
                Some(&mut self.observer.observation.maximum_rx_eapol_service_micros)
            }
            AccessPointRxProtocolClass::Other | AccessPointRxProtocolClass::Rejected => None,
        };
        if let Some(class_maximum) = class_maximum {
            *class_maximum = (*class_maximum).max(elapsed);
        }
    }

    /// Consume one staged AP RX owner on the protocol hot path.
    ///
    /// Saturated AP RX keeps this routine resident for most of the radio-task
    /// budget.  The S31 PSRAM-code profile therefore places the routine in the
    /// semantic hot-text class; the board linker decides whether that class is
    /// backed by internal executable SRAM.  This does not make the protocol
    /// routine interrupt-safe and does not change its ownership semantics.
    #[cfg_attr(
        target_arch = "riscv32",
        unsafe(link_section = ".hot.text.open_radio_ap_rx")
    )]
    #[inline(never)]
    fn service_staged_rx<H, F, Q>(
        &mut self,
        mut hardware: Option<&mut H>,
        staged_frame: F,
        publication: AccessPointRxPublication,
        authenticator_nonce: [u8; 32],
        initial_replay_counter: u64,
        now_micros: u64,
        publish_shared_rx: &mut Q,
        #[cfg(feature = "diagnostics")] delivery_observer: Option<&dyn RxNetworkDeliveryObserver>,
    ) -> Result<AccessPointRxProtocolClass, Esp32s31AccessPointControlError>
    where
        H: TxHardware + Esp32s31ApRuntimeHardware + RxBlockAckHardware,
        F: AccessPointStagedRxFrame,
        Q: FnMut(u8),
    {
        let mut staged_frame = Some(staged_frame);
        let segment = staged_frame
            .as_ref()
            .expect("current AP staged frame is live")
            .segment();
        let mut activity_peer = None;
        let mut batch_exhausted = false;
        let frame = match view_normalized_rx_frame(
            &segment,
            RxIngressConfig {
                ring_entry_limit: 1,
                csi_config: 0,
                flags: 0,
            },
        ) {
            Ok(frame) => frame,
            Err(_error) => {
                observe_access_point!(self, observation, {
                    match _error {
                        open_esp_radio_esp32s31_wifi_mac::rx::RxError::MicFailure => {
                            observation.rx_mic_failures =
                                observation.rx_mic_failures.saturating_add(1);
                        }
                        open_esp_radio_esp32s31_wifi_mac::rx::RxError::Quarantined => {
                            let duplicate_or_stale = self
                                .data_rx
                                .reorder_key(segment)
                                .is_some_and(|key| self.rx_reorder.is_duplicate_or_stale(key));
                            if duplicate_or_stale {
                                observation.protected_data_duplicates =
                                    observation.protected_data_duplicates.saturating_add(1);
                            } else {
                                observation.rx_quarantined_frames =
                                    observation.rx_quarantined_frames.saturating_add(1);
                            }
                        }
                        _ => {
                            observation.rx_view_rejected =
                                observation.rx_view_rejected.saturating_add(1);
                        }
                    }
                    observation.ignored_rx_frames = observation.ignored_rx_frames.saturating_add(1);
                });
                return Ok(AccessPointRxProtocolClass::Rejected);
            }
        };
        let frame_control = u16::from_le_bytes([frame.mpdu[0], frame.mpdu[1]]);
        let ampdu_contained = matches!(
            frame.metadata.ampdu,
            MacRxEvidence::HardwareObserved(true) | MacRxEvidence::ProtocolValidated(true)
        );
        let ampdu_baseband_format = if ampdu_contained {
            match frame.metadata.rate {
                MacRxEvidence::HardwareObserved(phy) => Some(phy.baseband_format().raw()),
                _ => None,
            }
        } else {
            None
        };
        let protocol_class = if frame_control & 0x000c == 0x0008 && frame_control & 0x4000 != 0 {
            observe_access_point!(self, observation, {
                observation.protected_data_frames =
                    observation.protected_data_frames.saturating_add(1);
                if let MacRxEvidence::HardwareObserved(rssi_dbm) = frame.metadata.rssi_dbm {
                    if observation.rx_rssi_samples == 0 {
                        observation.rx_rssi_min_dbm = rssi_dbm;
                        observation.rx_rssi_max_dbm = rssi_dbm;
                    } else {
                        observation.rx_rssi_min_dbm = observation.rx_rssi_min_dbm.min(rssi_dbm);
                        observation.rx_rssi_max_dbm = observation.rx_rssi_max_dbm.max(rssi_dbm);
                    }
                    observation.rx_rssi_samples = observation.rx_rssi_samples.saturating_add(1);
                    observation.rx_rssi_sum_dbm = observation
                        .rx_rssi_sum_dbm
                        .saturating_add(i32::from(rssi_dbm));
                }
                if let MacRxEvidence::HardwareObserved(phy) = frame.metadata.rate
                    && let Some(signal) = phy.ht_signal()
                {
                    observation.rx_ht_data_frames = observation.rx_ht_data_frames.saturating_add(1);
                    if signal.aggregation {
                        observation.rx_ht_mpdus_with_aggregation_bit = observation
                            .rx_ht_mpdus_with_aggregation_bit
                            .saturating_add(1);
                    }
                    if signal.channel_width_mhz == 40
                        && let Some(count) =
                            observation.rx_ht40_mcs_frames.get_mut(signal.mcs as usize)
                    {
                        *count = count.saturating_add(1);
                    }
                }
            });
            let (
                reorder_progress,
                batch_used,
                current_batch_exhausted,
                in_place_publication,
                produced_data,
            ) = {
                let processor = &mut *self;
                let mac = &processor.mac;
                let data_rx = &mut processor.data_rx;
                #[cfg(any(feature = "diagnostics", test))]
                let report = &mut processor.observer.observation;
                let mut deferred = DeferredAccessPointRxSink::new(processor.rx_frame);
                let mut in_place = InPlaceAccessPointRxSink::new(segment.buffer);
                let mut produced_data = false;
                let key = data_rx.reorder_key(segment);
                let current_buffer = segment.buffer.as_ptr();
                let qos_control_offset = 24
                    + if frame_control & 0x0300 == 0x0300 {
                        6
                    } else {
                        0
                    };
                let current_is_amsdu = frame_control & 0x0080 != 0
                    && frame
                        .mpdu
                        .get(qos_control_offset)
                        .is_some_and(|control| control & 0x80 != 0);
                let reorder_progress = {
                    let mut dispatch =
                        |ordered: open_esp_radio_esp32s31_wifi_mac::rx::RxSegment<'_>| {
                            AccessPointProtectedFrameDispatch::dispatch(
                                data_rx,
                                ordered,
                                |peer| mac.engine().is_authorized_peer(peer),
                                publication,
                                current_buffer as usize,
                                current_is_amsdu,
                                &mut deferred,
                                &mut in_place,
                                #[cfg(any(feature = "diagnostics", test))]
                                report,
                                &mut activity_peer,
                                &mut produced_data,
                            );
                        };
                    if let Some(key) = key {
                        processor.rx_reorder.ingest(
                            processor.rx_reorder_storage,
                            segment,
                            key,
                            ampdu_baseband_format,
                            now_micros,
                            &mut dispatch,
                        )
                    } else {
                        dispatch(segment);
                        Ok(Default::default())
                    }
                }?;
                (
                    reorder_progress,
                    deferred.used(),
                    deferred.exhausted || in_place.unsupported,
                    in_place.publication,
                    produced_data,
                )
            };
            if let Some(reset) = reorder_progress.hardware_window_reset {
                observe_access_point!(self, observation, {
                    observation.rx_reorder_hardware_window_resets = observation
                        .rx_reorder_hardware_window_resets
                        .saturating_add(1);
                });
                let agreement = self.rx_block_ack.snapshots_for(MacInterface::AccessPoint)
                    [usize::from(reset.hardware_index)]
                .expect("AP reorder reset belongs to one live AP BlockAck agreement");
                self.protocol_actions
                    .publisher()
                    .try_publish(Esp32s31AccessPointProtocolAction::Hardware(
                        Esp32s31AccessPointHardwareAction::ResetRxBlockAckWindow {
                            hardware_index: reset.hardware_index,
                            tid: agreement.tid,
                            starting_sequence: reset.starting_sequence,
                            window: RX_BLOCK_ACK_MAX_WINDOW,
                        },
                    ))
                    .map_err(|_| Esp32s31AccessPointControlError::ProtocolActionCapacity)?;
            }
            observe_access_point!(self, observation, {
                if reorder_progress.duplicate {
                    observation.protected_data_duplicates =
                        observation.protected_data_duplicates.saturating_add(1);
                }
                if reorder_progress.buffered {
                    observation.rx_reorder_buffered_mpdus =
                        observation.rx_reorder_buffered_mpdus.saturating_add(1);
                }
                observation.rx_reorder_dispatched_mpdus = observation
                    .rx_reorder_dispatched_mpdus
                    .saturating_add(u32::from(reorder_progress.dispatched));
                if reorder_progress.dropped {
                    observation.protected_data_protocol_rejected = observation
                        .protected_data_protocol_rejected
                        .saturating_add(1);
                }
            });
            batch_exhausted = current_batch_exhausted;
            // Protocol parsing has released all frame and scratch borrows.
            // Only the radio owner now translates the value-only request.
            if let Some(hardware) = hardware.as_deref_mut() {
                self.apply_protocol_actions(hardware)?;
            }
            if batch_used != 0 {
                self.rx_batch_used = batch_used;
                self.rx_batch_offset = 0;
            }
            if let Some(ethernet) = in_place_publication {
                #[cfg(any(feature = "diagnostics", test))]
                let raw = segment.buffer;
                #[cfg(any(feature = "diagnostics", test))]
                let payload = &raw
                    [ethernet.payload_offset..ethernet.payload_offset + ethernet.payload_length];
                #[cfg(any(feature = "diagnostics", test))]
                let ethernet_frame = EthernetFrameParts {
                    destination: ethernet.destination,
                    source: ethernet.source,
                    ether_type: ethernet.ether_type,
                    payload,
                };
                #[cfg(any(feature = "diagnostics", test))]
                let protocol = ethernet_parts_protocol(ethernet_frame);
                #[cfg(feature = "diagnostics")]
                if let Some(observer) = delivery_observer {
                    observer.admitted(RxNetworkDeliveryEvent {
                        frame: ethernet_frame,
                        raw: Some(raw),
                    });
                }
                let current = staged_frame
                    .take()
                    .expect("in-place AP publication owns the current staging frame");
                let index = current
                    .publish_ethernet_in_place(ethernet)
                    .map_err(|_| Esp32s31AccessPointControlError::ReceiveBatchCapacity)?;
                publish_shared_rx(index);
                observe_access_point!(self, observation, {
                    observation.ethernet_frames_staged =
                        observation.ethernet_frames_staged.saturating_add(1);
                    match protocol {
                        Some(EthernetProtocol::ArpRequest) => {
                            observation.ethernet_arp_requests_staged =
                                observation.ethernet_arp_requests_staged.saturating_add(1);
                        }
                        Some(EthernetProtocol::Ipv4Tcp) => {
                            observation.ethernet_tcp_frames_staged =
                                observation.ethernet_tcp_frames_staged.saturating_add(1);
                        }
                        _ => {}
                    }
                });
            }
            if !produced_data {
                observe_access_point!(self, observation, {
                    observation.ignored_rx_frames = observation.ignored_rx_frames.saturating_add(1);
                });
            }
            AccessPointRxProtocolClass::ProtectedData
        } else if frame_control & 0x000c == 0 {
            let hardware = hardware
                .as_deref_mut()
                .ok_or(Esp32s31AccessPointControlError::ProtocolFrameRequiresHardware)?;
            if self.service_management(
                hardware,
                frame.mpdu,
                authenticator_nonce,
                initial_replay_counter,
                now_micros,
            )? {
                observe_access_point!(self, observation, {
                    observation.control_frames_staged =
                        observation.control_frames_staged.saturating_add(1);
                });
            }
            AccessPointRxProtocolClass::Management
        } else if frame_control & 0x000c == 0x0008 {
            let hardware = hardware
                .as_deref_mut()
                .ok_or(Esp32s31AccessPointControlError::ProtocolFrameRequiresHardware)?;
            self.service_eapol(hardware, frame.mpdu, now_micros)?;
            AccessPointRxProtocolClass::Eapol
        } else {
            observe_access_point!(self, observation, {
                observation.ignored_rx_frames = observation.ignored_rx_frames.saturating_add(1);
            });
            AccessPointRxProtocolClass::Other
        };
        if batch_exhausted {
            observe_access_point!(self, observation, {
                observation.protected_data_protocol_rejected = observation
                    .protected_data_protocol_rejected
                    .saturating_add(1);
            });
            return Err(Esp32s31AccessPointControlError::ReceiveBatchCapacity);
        }
        if let Some(peer) = activity_peer {
            self.protocol_actions
                .publisher()
                .try_publish(Esp32s31AccessPointProtocolAction::Control(
                    Esp32s31AccessPointControlAction::ObservePeerActivity {
                        peer,
                        at_micros: now_micros,
                    },
                ))
                .map_err(|_| Esp32s31AccessPointControlError::ProtocolActionCapacity)?;
            if let Some(hardware) = hardware {
                self.apply_protocol_actions(hardware)?;
            }
        }
        Ok(protocol_class)
    }

    fn apply_protocol_actions<H>(
        &mut self,
        hardware: &mut H,
    ) -> Result<(), Esp32s31AccessPointControlError>
    where
        H: RxBlockAckHardware,
    {
        while let Some(action) = self.protocol_actions.receiver().try_receive() {
            match action {
                Esp32s31AccessPointProtocolAction::Hardware(
                    Esp32s31AccessPointHardwareAction::ResetRxBlockAckWindow {
                        hardware_index,
                        tid,
                        starting_sequence,
                        window,
                    },
                ) => hardware.reset_rx_block_ack_window(
                    hardware_index,
                    tid,
                    starting_sequence,
                    window,
                )?,
                Esp32s31AccessPointProtocolAction::Control(
                    Esp32s31AccessPointControlAction::ObservePeerActivity { peer, at_micros },
                ) => self
                    .mac
                    .engine_mut()
                    .observe_peer_activity(peer, at_micros)?,
            }
        }
        Ok(())
    }

    fn service_management<H>(
        &mut self,
        hardware: &mut H,
        mpdu: &[u8],
        authenticator_nonce: [u8; 32],
        initial_replay_counter: u64,
        now_micros: u64,
    ) -> Result<bool, Esp32s31AccessPointControlError>
    where
        H: TxHardware + Esp32s31ApRuntimeHardware + RxBlockAckHardware,
    {
        let request = parse_ap_management_request(mpdu, self.mac.engine().service_address());
        if let Some(ApManagementRequest::BlockAck { peer, action }) = request {
            match action {
                BlockAckAction::AddbaRequest {
                    dialog_token,
                    tid,
                    immediate,
                    window,
                    timeout_tu,
                    starting_sequence,
                    ..
                } if self.mac.engine().is_authorized_peer(peer) => {
                    let offered = self.rx_block_ack.offer(RxBlockAckRequest {
                        interface: MacInterface::AccessPoint,
                        peer,
                        dialog_token,
                        tid,
                        immediate,
                        requested_window: window,
                        timeout_tu,
                        starting_sequence,
                    });
                    if offered.is_err() {
                        self.publish_declined_rx_addba(hardware, peer, dialog_token, tid, window)?;
                        return Ok(true);
                    }
                    let activation = match self.rx_block_ack.begin_pending() {
                        Ok(Some(activation)) => activation,
                        Ok(None) => return Ok(false),
                        Err(RxBlockAckSessionsError::NoFreeHardwareBank) => {
                            let discarded = self.rx_block_ack.discard_pending(
                                MacInterface::AccessPoint,
                                peer,
                                tid,
                            );
                            debug_assert!(discarded);
                            self.publish_declined_rx_addba(
                                hardware,
                                peer,
                                dialog_token,
                                tid,
                                window,
                            )?;
                            return Ok(true);
                        }
                        Err(error) => return Err(error.into()),
                    };
                    self.start_rx_addba_response(hardware, activation, now_micros)?;
                    return Ok(true);
                }
                BlockAckAction::Delba {
                    tid,
                    initiator: true,
                    ..
                } => {
                    if let Some(agreement) =
                        self.rx_block_ack.stop(MacInterface::AccessPoint, peer, tid)
                    {
                        self.release_rx_reorder(agreement.identity(), now_micros)?;
                        hardware.clear_rx_block_ack(agreement.hardware_index)?;
                    }
                    return Ok(false);
                }
                _ => {}
            }
        }

        let processor = &mut *self;
        let outcome = processor.mac.publish_management(
            hardware,
            mpdu,
            authenticator_nonce,
            initial_replay_counter,
            now_micros,
            processor.tx_frame,
        )?;
        if let open_esp_radio_esp32s31_wifi_ap::engine::Esp32s31ApManagementOutcome::PeerRemoved {
            peer,
        } = outcome
        {
            self.discard_rx_peer(hardware, peer)?;
        }
        Ok(matches!(
            outcome,
            open_esp_radio_esp32s31_wifi_ap::engine::Esp32s31ApManagementOutcome::Response { .. }
        ))
    }

    fn publish_declined_rx_addba<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        peer: [u8; 6],
        dialog_token: u8,
        tid: u8,
        requested_window: u16,
    ) -> Result<(), Esp32s31AccessPointControlError> {
        let mut body = [0_u8; 9];
        write_declined_addba_response(&mut body, dialog_token, tid, requested_window)
            .map_err(RxBlockAckSessionsError::Response)?;
        let processor = &mut *self;
        processor
            .mac
            .publish_rx_block_ack_response(hardware, peer, &body, processor.tx_frame)?;
        Ok(())
    }

    fn start_rx_addba_response<H>(
        &mut self,
        hardware: &mut H,
        activation: RxBlockAckActivation,
        now_micros: u64,
    ) -> Result<(), Esp32s31AccessPointControlError>
    where
        H: TxHardware + Esp32s31ApRuntimeHardware + RxBlockAckHardware,
    {
        debug_assert!(self.rx_addba_in_flight.is_none());
        if let Some(replaced) = activation.replaced() {
            if let Err(error) = self.release_rx_reorder(replaced.identity(), now_micros) {
                self.rx_block_ack.cancel(activation)?;
                return Err(error);
            }
            if let Err(error) = hardware.clear_rx_block_ack(replaced.hardware_index) {
                self.rx_block_ack.cancel(activation)?;
                return Err(error.into());
            }
        }
        let negotiated = activation.negotiated();
        // SOURCE: complete vendor `ht_recv_action_ba_addba_request` first
        // enqueues the successful ADDBA response through
        // `ieee80211_send_action`, then publishes the receive agreement via
        // `ic_add_rx_ba`. The direct bank must not become
        // visible before the response publication edge.
        let processor = &mut *self;
        if let Err(error) = processor.mac.publish_rx_block_ack_response(
            hardware,
            negotiated.peer,
            activation.response_body(),
            processor.tx_frame,
        ) {
            self.rx_block_ack.cancel(activation)?;
            return Err(error.into());
        }
        if let Err(error) = hardware.program_rx_block_ack(activation.hardware()) {
            self.rx_block_ack.cancel(activation)?;
            return Err(error.into());
        }
        if let Err(error) = self.rx_reorder.start(negotiated, |_| {}) {
            let clear = hardware.clear_rx_block_ack(negotiated.hardware_index);
            self.rx_block_ack.cancel(activation)?;
            clear?;
            return Err(error.into());
        }
        self.rx_addba_in_flight = Some(activation);
        Ok(())
    }

    fn release_rx_reorder(
        &mut self,
        identity: open_esp_radio_esp32s31_wifi_mac::rx_ampdu::RxBlockAckIdentity,
        now_micros: u64,
    ) -> Result<(), Esp32s31AccessPointControlError> {
        let processor = &mut *self;
        let mac = &processor.mac;
        let data_rx = &mut processor.data_rx;
        #[cfg(any(feature = "diagnostics", test))]
        let report = &mut processor.observer.observation;
        let mut activity_peer = None;
        let mut sink = DeferredAccessPointRxSink::new(processor.rx_frame);
        let _ = processor.rx_reorder.stop(identity, |segment| {
            let peer = data_rx.reorder_key(segment).map(|key| key.peer);
            let outcome = data_rx.dispatch_protected(
                segment,
                |peer| mac.engine().is_authorized_peer(peer),
                &mut sink,
            );
            let _ = observe_protected_dispatch(
                outcome,
                peer,
                #[cfg(any(feature = "diagnostics", test))]
                report,
                &mut activity_peer,
            );
        });
        if sink.exhausted {
            return Err(Esp32s31AccessPointControlError::ReceiveBatchCapacity);
        }
        if let Some(peer) = activity_peer {
            processor
                .mac
                .engine_mut()
                .observe_peer_activity(peer, now_micros)?;
        }
        let used = sink.used();
        if used != 0 {
            processor.rx_batch_used = used;
            processor.rx_batch_offset = 0;
        }
        Ok(())
    }

    fn discard_rx_peer<H: RxBlockAckHardware>(
        &mut self,
        hardware: &mut H,
        peer: [u8; 6],
    ) -> Result<(), Esp32s31AccessPointControlError> {
        for agreement in self
            .rx_block_ack
            .stop_peer(MacInterface::AccessPoint, peer)
            .into_iter()
            .flatten()
        {
            let _ = self.rx_reorder.stop_discard(agreement.identity());
            hardware.clear_rx_block_ack(agreement.hardware_index)?;
        }
        Ok(())
    }

    fn service_rx_reorder_expiry(
        &mut self,
        now_micros: u64,
    ) -> Result<bool, Esp32s31AccessPointControlError> {
        let processor = &mut *self;
        let mac = &processor.mac;
        let data_rx = &mut processor.data_rx;
        #[cfg(any(feature = "diagnostics", test))]
        let report = &mut processor.observer.observation;
        let mut activity_peer = None;
        let mut sink = DeferredAccessPointRxSink::new(processor.rx_frame);
        let pending_dispatched = processor.rx_reorder.dispatch_pending(|segment| {
            let peer = data_rx.reorder_key(segment).map(|key| key.peer);
            let outcome = data_rx.dispatch_protected(
                segment,
                |peer| mac.engine().is_authorized_peer(peer),
                &mut sink,
            );
            let _ = observe_protected_dispatch(
                outcome,
                peer,
                #[cfg(any(feature = "diagnostics", test))]
                report,
                &mut activity_peer,
            );
        });
        let (dispatched, _gap_timeout) = if pending_dispatched {
            (1, false)
        } else {
            let dispatched = processor.rx_reorder.expire_due(now_micros, |segment| {
                let peer = data_rx.reorder_key(segment).map(|key| key.peer);
                let outcome = data_rx.dispatch_protected(
                    segment,
                    |peer| mac.engine().is_authorized_peer(peer),
                    &mut sink,
                );
                let _ = observe_protected_dispatch(
                    outcome,
                    peer,
                    #[cfg(any(feature = "diagnostics", test))]
                    report,
                    &mut activity_peer,
                );
            });
            (dispatched, dispatched != 0)
        };
        observe_access_point!(processor, observation, {
            if _gap_timeout {
                observation.rx_reorder_gap_timeouts =
                    observation.rx_reorder_gap_timeouts.saturating_add(1);
            }
            observation.rx_reorder_dispatched_mpdus = observation
                .rx_reorder_dispatched_mpdus
                .saturating_add(u32::from(dispatched));
        });
        if sink.exhausted {
            return Err(Esp32s31AccessPointControlError::ReceiveBatchCapacity);
        }
        if let Some(peer) = activity_peer {
            processor
                .mac
                .engine_mut()
                .observe_peer_activity(peer, now_micros)?;
        }
        let used = sink.used();
        if used != 0 {
            processor.rx_batch_used = used;
            processor.rx_batch_offset = 0;
            return Ok(true);
        }
        Ok(dispatched != 0)
    }

    pub const fn rx_batch_pending(&self) -> bool {
        self.rx_batch_offset < self.rx_batch_used
    }

    #[cfg(any(feature = "diagnostics", test))]
    fn observe_ht_aggregate(&mut self, rate: HtRate) {
        observe_access_point!(self, observation, {
            observation.tx_ht_aggregates = observation.tx_ht_aggregates.saturating_add(1);
            if rate.channel_width == HtChannelWidth::Mhz40 && rate.mcs == HtMcs::Mcs7 {
                observation.tx_ht40_mcs7_aggregates =
                    observation.tx_ht40_mcs7_aggregates.saturating_add(1);
            }
        });
    }
}

impl<
    'storage,
    'beacon,
    'slot,
    R,
    C,
    P,
    E,
    T,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
    const TX_BUFFER_SIZE: usize,
>
    Esp32s31AccessPointControl<
        'storage,
        'beacon,
        'slot,
        R,
        C,
        P,
        E,
        T,
        COUNT,
        DMA_BUFFER_SIZE,
        DMA_STORAGE_SIZE,
        TX_BUFFER_SIZE,
    >
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: WifiTxTimer,
{
    pub(super) fn rx_work_due(&self, now_micros: u64) -> bool
    where
        C: AccessPointRxProtocolConsumer,
    {
        self.protocol_rx.queued_frames() != 0
            || !self.protocol_actions.is_empty()
            || self.rx_batch_pending()
            || self.rx_reorder.has_pending_release()
            || self
                .rx_reorder
                .next_deadline()
                .is_some_and(|deadline| deadline <= now_micros)
    }
}

impl<'storage, 'beacon, 'slot, P, E, T, const DMA_BUFFER_SIZE: usize, const TX_BUFFER_SIZE: usize>
    Esp32s31AccessPointProtocolProcessor<
        'storage,
        'beacon,
        'slot,
        P,
        E,
        T,
        DMA_BUFFER_SIZE,
        TX_BUFFER_SIZE,
    >
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: WifiTxTimer,
{
    fn rx_batch_record(
        &self,
    ) -> Result<
        Option<crate::datapath::rx::ethernet::PackedEthernetRecord<'_>>,
        Esp32s31AccessPointControlError,
    > {
        crate::datapath::rx::ethernet::record_at(
            self.rx_frame,
            self.rx_batch_used,
            self.rx_batch_offset,
        )
        .map_err(|_| Esp32s31AccessPointControlError::ReceiveBatchCapacity)
    }

    fn commit_rx_batch_record(&mut self, next_offset: usize) {
        debug_assert!(next_offset > self.rx_batch_offset);
        debug_assert!(next_offset <= self.rx_batch_used);
        self.rx_batch_offset = next_offset;
        if self.rx_batch_offset == self.rx_batch_used {
            self.rx_batch_offset = 0;
            self.rx_batch_used = 0;
        }
    }

    fn service_eapol<H>(
        &mut self,
        hardware: &mut H,
        mpdu: &[u8],
        now_micros: u64,
    ) -> Result<(), Esp32s31AccessPointControlError>
    where
        H: TxHardware + Esp32s31ApRuntimeHardware,
    {
        let header_length = if mpdu[0] & 0x80 != 0 {
            IEEE80211_QOS_DATA_HEADER_LEN
        } else {
            IEEE80211_LEGACY_DATA_HEADER_LEN
        };
        let Some(payload_length) = mpdu.len().checked_sub(header_length) else {
            observe_access_point!(self, observation, {
                observation.ignored_rx_frames = observation.ignored_rx_frames.saturating_add(1);
            });
            return Ok(());
        };
        let Ok(plan) = plan_data_decapsulation(
            DataInterfaceRole::AccessPoint,
            mpdu,
            header_length,
            payload_length,
        ) else {
            observe_access_point!(self, observation, {
                observation.ignored_rx_frames = observation.ignored_rx_frames.saturating_add(1);
            });
            return Ok(());
        };
        if plan.ether_type != EAPOL_ETHERTYPE
            || plan.destination != self.mac.engine().service_address()
            || self.mac.engine().peer_status(plan.source).is_none()
        {
            observe_access_point!(self, observation, {
                observation.ignored_rx_frames = observation.ignored_rx_frames.saturating_add(1);
            });
            return Ok(());
        }
        let payload = &mpdu[plan.payload_offset..plan.payload_offset + plan.payload_length];
        let Ok(frame) = OwnedEapolFrame::<EAPOL_CAPACITY>::try_copy(
            Wpa2Interface::AccessPoint,
            plan.source,
            payload,
        ) else {
            observe_access_point!(self, observation, {
                observation.ignored_rx_frames = observation.ignored_rx_frames.saturating_add(1);
            });
            return Ok(());
        };
        match self
            .mac
            .engine_mut()
            .handle_eapol(hardware, plan.source, frame, now_micros)?
        {
            Esp32s31ApWpa2Outcome::Transmit(frame) => {
                let processor = &mut *self;
                processor
                    .mac
                    .publish_eapol(hardware, plan.source, &frame, processor.tx_frame)?;
                observe_access_point!(self, observation, {
                    observation.control_frames_staged =
                        observation.control_frames_staged.saturating_add(1);
                });
            }
            Esp32s31ApWpa2Outcome::DeauthenticatePeer { peer } => {
                let close = self
                    .mac
                    .engine_mut()
                    .begin_wpa2_failure_close(peer)
                    .map_err(Esp32s31ApMacError::Engine)?;
                self.publish_peer_close(hardware, close)?;
            }
            Esp32s31ApWpa2Outcome::PeerAuthorized { peer } => {
                let processor = &mut *self;
                if processor.mac.publish_tx_block_ack_request(
                    hardware,
                    peer,
                    now_micros,
                    processor.tx_frame,
                )? {
                    observe_access_point!(self, observation, {
                        observation.control_frames_staged =
                            observation.control_frames_staged.saturating_add(1);
                    });
                }
            }
            Esp32s31ApWpa2Outcome::None => {}
        }
        Ok(())
    }

    pub async fn service_tx<H>(
        &mut self,
        hardware: &mut H,
        wake: WifiTxWake,
    ) -> Result<WifiTxProgress, Esp32s31AccessPointControlError>
    where
        H: Esp32s31ApRuntimeHardware + TxHardware + RxBlockAckHardware,
    {
        let (progress, action) = self
            .mac
            .service_tx(hardware, wake, Instant::now().as_micros())
            .await?;
        if progress == WifiTxProgress::Complete
            && let Some(activation) = self.rx_addba_in_flight.take()
        {
            let negotiated = activation.negotiated();
            if action == Esp32s31ApTxCompletionAction::PublicationFailed {
                hardware.clear_rx_block_ack(negotiated.hardware_index)?;
                let _ = self.rx_reorder.stop_discard(negotiated.identity());
                self.rx_block_ack.cancel(activation)?;
            } else {
                self.rx_block_ack.commit(activation)?;
            }
        }
        match action {
            Esp32s31ApTxCompletionAction::BeginWpa2 { peer } => {
                let message1 = self.mac.engine().begin_wpa2::<EAPOL_CAPACITY>(peer)?;
                let processor = &mut *self;
                processor
                    .mac
                    .publish_eapol(hardware, peer, &message1, processor.tx_frame)?;
                observe_access_point!(self, observation, {
                    observation.control_frames_staged =
                        observation.control_frames_staged.saturating_add(1);
                });
                return Ok(WifiTxProgress::Pending);
            }
            Esp32s31ApTxCompletionAction::PeerDisconnectTerminal {
                close,
                stage: Esp32s31ApPeerDisconnectStage::Disassociation,
                ..
            } => {
                let processor = &mut *self;
                processor.mac.publish_peer_disconnect(
                    hardware,
                    close,
                    Esp32s31ApPeerDisconnectStage::Deauthentication,
                    processor.tx_frame,
                )?;
                observe_access_point!(self, observation, {
                    observation.control_frames_staged =
                        observation.control_frames_staged.saturating_add(1);
                });
                return Ok(WifiTxProgress::Pending);
            }
            Esp32s31ApTxCompletionAction::PeerDisconnectTerminal {
                close,
                stage: Esp32s31ApPeerDisconnectStage::Deauthentication,
                ..
            } => {
                self.discard_rx_peer(hardware, close.peer)?;
                self.mac.engine_mut().complete_peer_close(hardware, close)?;
            }
            Esp32s31ApTxCompletionAction::None
            | Esp32s31ApTxCompletionAction::PublicationFailed => {}
        }
        Ok(progress)
    }

    fn publish_peer_close<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        close: ApPeerClose,
    ) -> Result<(), Esp32s31AccessPointControlError> {
        let stage = if close.was_associated {
            Esp32s31ApPeerDisconnectStage::Disassociation
        } else {
            Esp32s31ApPeerDisconnectStage::Deauthentication
        };
        let processor = &mut *self;
        processor
            .mac
            .publish_peer_disconnect(hardware, close, stage, processor.tx_frame)?;
        observe_access_point!(self, observation, {
            observation.control_frames_staged = observation.control_frames_staged.saturating_add(1);
        });
        Ok(())
    }

    #[cfg(any(feature = "diagnostics", test))]
    pub fn observation(&self) -> Esp32s31AccessPointControlObservation {
        self.observer.observation
    }

    pub const fn serviced_rx_frames(&self) -> u64 {
        self.serviced_rx_frames
    }

    pub const fn serviced_rx_descriptors(&self) -> u64 {
        self.serviced_rx_descriptors
    }

    #[cfg(any(feature = "diagnostics", test))]
    pub fn mac_observation(&self) -> Esp32s31ApMacObservation {
        self.mac.observation()
    }

    /// Whether the AP owns at least one operational downlink Block Ack
    /// agreement and can therefore profitably collect a network TX batch.
    pub fn has_operational_tx_block_ack(&self) -> bool {
        self.mac.engine().has_operational_tx_block_ack()
    }

    pub const fn tx_pending(&self) -> bool {
        self.mac.tx_pending()
    }

    pub const fn next_beacon_delay(&self, now_micros: u32) -> Option<(u32, u32)> {
        self.mac.next_beacon_delay(now_micros)
    }

    pub const fn beacon_publication_due(&self, now_micros: u32) -> bool {
        self.mac.beacon_publication_due(now_micros)
    }

    pub fn wait_tx_deadline(&mut self) -> impl core::future::Future<Output = ()> + '_ {
        self.mac.wait_tx_deadline()
    }

    pub fn publish_beacon<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        now_micros: u64,
    ) -> Result<(), Esp32s31AccessPointControlError> {
        #[cfg(any(feature = "diagnostics", test))]
        {
            let (missed, lateness) = self.mac.beacon_publication_lateness(now_micros as u32);
            observe_access_point!(self, observation, {
                observation.missed_beacon_intervals =
                    observation.missed_beacon_intervals.saturating_add(missed);
                observation.maximum_beacon_lateness_micros =
                    observation.maximum_beacon_lateness_micros.max(lateness);
            });
        }
        self.mac.publish_beacon(hardware, now_micros)?;
        Ok(())
    }

    /// Copy one network-owned Ethernet frame into the AP's ordinary DMA slot
    /// and begin a pairwise protected publication.
    ///
    /// The caller may release its network lease after this method returns:
    /// the complete plaintext MPDU is then owned by `self` until terminal TX.
    pub fn publish_ethernet<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        peer: [u8; 6],
        ethernet: &[u8],
    ) -> Result<(), Esp32s31AccessPointControlError> {
        let processor = &mut *self;
        processor
            .mac
            .publish_ethernet(hardware, peer, ethernet, processor.tx_frame)?;
        Ok(())
    }

    fn start_network_tx<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        ethernet: &[u8],
    ) -> Result<WifiTxProgress, Esp32s31AccessPointControlError> {
        observe_access_point!(self, observation, {
            observation.network_tx_frames_observed =
                observation.network_tx_frames_observed.saturating_add(1);
            match ethernet_protocol(ethernet) {
                Some(EthernetProtocol::ArpRequest) => {
                    observation.network_tx_arp_requests =
                        observation.network_tx_arp_requests.saturating_add(1);
                }
                Some(EthernetProtocol::ArpReply) => {
                    observation.network_tx_arp_replies =
                        observation.network_tx_arp_replies.saturating_add(1);
                }
                _ => {}
            }
        });
        if self.mac.engine().authorized_peer_count() == 0 {
            observe_access_point!(self, observation, {
                observation.network_tx_rejected_no_peer =
                    observation.network_tx_rejected_no_peer.saturating_add(1);
                observation.network_tx_frames_rejected =
                    observation.network_tx_frames_rejected.saturating_add(1);
            });
            return Ok(WifiTxProgress::Complete);
        }
        let Some(destination) = ethernet
            .get(..6)
            .and_then(|bytes| <[u8; 6]>::try_from(bytes).ok())
        else {
            observe_access_point!(self, observation, {
                observation.network_tx_rejected_destination = observation
                    .network_tx_rejected_destination
                    .saturating_add(1);
                observation.network_tx_frames_rejected =
                    observation.network_tx_frames_rejected.saturating_add(1);
            });
            return Ok(WifiTxProgress::Complete);
        };
        if destination[0] & 1 == 0 && !self.mac.engine().is_authorized_peer(destination) {
            observe_access_point!(self, observation, {
                observation.network_tx_rejected_destination = observation
                    .network_tx_rejected_destination
                    .saturating_add(1);
                observation.network_tx_frames_rejected =
                    observation.network_tx_frames_rejected.saturating_add(1);
            });
            return Ok(WifiTxProgress::Complete);
        }
        self.publish_ethernet(hardware, destination, ethernet)?;
        Ok(WifiTxProgress::Pending)
    }

    fn role_observation(&self) -> (u32, AccessPointServiceStatus, LinkState) {
        (
            self.mac.engine().service_status_revision(),
            self.mac.engine().service_status(),
            if self.mac.engine().authorized_peer_count() == 0 {
                LinkState::Down
            } else {
                LinkState::Up
            },
        )
    }

    /// Advance AP timer and peer policy by one finite DATAPATH control step.
    ///
    /// This method never waits. A published frame returns `TxPending`; the
    /// caller must drive the shared TX owner to a terminal edge before
    /// invoking another control transition.
    pub fn service_control<H>(
        &mut self,
        hardware: &mut H,
        now_micros: u64,
    ) -> Result<DatapathControlProgress<Infallible>, Esp32s31AccessPointControlError>
    where
        H: TxHardware + Esp32s31ApRuntimeHardware,
    {
        if self.tx_pending() {
            return Ok(DatapathControlProgress::TxPending);
        }
        if self.beacon_publication_due(now_micros as u32) {
            self.publish_beacon(hardware, now_micros)?;
            return Ok(DatapathControlProgress::TxPending);
        }
        self.mac.expire_tx_block_ack(now_micros)?;
        match self
            .mac
            .engine_mut()
            .take_due_wpa2_retry::<EAPOL_CAPACITY>(now_micros)?
        {
            ApWpa2RetryProgress::Transmit { peer, frame } => {
                let processor = &mut *self;
                processor
                    .mac
                    .publish_eapol(hardware, peer, &frame, processor.tx_frame)?;
                observe_access_point!(self, observation, {
                    observation.control_frames_staged =
                        observation.control_frames_staged.saturating_add(1);
                });
                return Ok(DatapathControlProgress::TxPending);
            }
            ApWpa2RetryProgress::Close(close) => {
                self.publish_peer_close(hardware, close)?;
                return Ok(DatapathControlProgress::TxPending);
            }
            ApWpa2RetryProgress::None => {}
        }
        if let Some(close) = self.mac.engine_mut().begin_due_peer_close(now_micros) {
            self.publish_peer_close(hardware, close)?;
            return Ok(DatapathControlProgress::TxPending);
        }
        self.next_control_delay_millis(now_micros)?;
        Ok(DatapathControlProgress::Idle)
    }

    fn next_control_delay_millis(
        &self,
        now_micros: u64,
    ) -> Result<u32, Esp32s31AccessPointControlError> {
        let (_, beacon_delay_ms) = self
            .next_beacon_delay(now_micros as u32)
            .ok_or(Esp32s31AccessPointControlError::InvalidBeaconSchedule)?;
        let deadline_delay = |deadline: u64| {
            let remaining = deadline.saturating_sub(now_micros);
            u32::try_from(remaining.saturating_add(999) / 1_000)
                .unwrap_or(u32::MAX)
                .max(1)
        };
        Ok(self
            .mac
            .engine()
            .next_peer_deadline()
            .into_iter()
            .chain(self.mac.engine().next_wpa2_retry_deadline())
            .chain(self.mac.next_tx_block_ack_deadline())
            .chain(self.rx_reorder.next_deadline())
            .map(deadline_delay)
            .fold(beacon_delay_ms, u32::min))
    }

    /// Advance AP shutdown by one finite DATAPATH transition.
    pub fn service_stop<H>(
        &mut self,
        hardware: &mut H,
    ) -> Result<DatapathStopProgress, Esp32s31AccessPointControlError>
    where
        H: TxHardware + RxBlockAckHardware,
    {
        // Shutdown deliberately drops a decoded-but-unpublished network
        // batch. Execute all already-accepted protocol actions before peer
        // teardown so `try_finish_paired` can prove a truly empty mailbox.
        self.rx_batch_used = 0;
        self.rx_batch_offset = 0;
        self.apply_protocol_actions(hardware)?;
        if let Some(close) = self.mac.engine_mut().begin_stop_peer() {
            self.publish_peer_close(hardware, close)?;
            Ok(DatapathStopProgress::TxPending)
        } else {
            Ok(DatapathStopProgress::Stopped)
        }
    }

    /// Consume a quiescent AP protocol role from the paired DATAPATH boundary.
    ///
    /// The common RX producer is intentionally not part of this transaction.
    /// Any pending protocol, reorder, BlockAck, or TX state returns the exact
    /// processor unchanged.
    #[allow(clippy::result_large_err, clippy::type_complexity)]
    pub fn try_finish_paired<H>(
        self,
        hardware: &mut H,
    ) -> Result<
        Esp32s31AccessPointProtocolStopped<
            'storage,
            'beacon,
            'slot,
            P,
            E,
            T,
            DMA_BUFFER_SIZE,
            TX_BUFFER_SIZE,
        >,
        Self,
    >
    where
        H: Esp32s31ApRuntimeHardware,
    {
        if self.rx_batch_pending()
            || self.rx_addba_in_flight.is_some()
            || !self.protocol_actions.is_empty()
            || self.rx_reorder.has_pending_release()
            || self
                .rx_block_ack
                .snapshots_for(MacInterface::AccessPoint)
                .into_iter()
                .any(|entry| entry.is_some())
        {
            return Err(self);
        }
        let Self {
            mac,
            rx_frame,
            tx_frame,
            data_rx,
            rx_block_ack,
            rx_reorder,
            rx_reorder_storage,
            rx_addba_in_flight: _,
            protocol_actions,
            rx_batch_used: _,
            rx_batch_offset: _,
            serviced_rx_frames,
            serviced_rx_descriptors,
            #[cfg(any(feature = "diagnostics", test))]
            observer,
            #[cfg(any(feature = "diagnostics", test))]
            terminal_observer,
        } = self;
        #[cfg(any(feature = "diagnostics", test))]
        let mac_observation = mac.observation();
        #[cfg(any(feature = "diagnostics", test))]
        let engine_observation = mac.engine().observation();
        #[cfg(any(feature = "diagnostics", test))]
        let control_observation = observer.observation;
        let parts = match mac.try_into_parts() {
            Ok(parts) => parts,
            Err(mac) => {
                return Err(Self {
                    mac,
                    rx_frame,
                    tx_frame,
                    data_rx,
                    rx_block_ack,
                    rx_reorder,
                    rx_reorder_storage,
                    rx_addba_in_flight: None,
                    protocol_actions,
                    rx_batch_used: 0,
                    rx_batch_offset: 0,
                    serviced_rx_frames,
                    serviced_rx_descriptors,
                    #[cfg(any(feature = "diagnostics", test))]
                    observer,
                    #[cfg(any(feature = "diagnostics", test))]
                    terminal_observer,
                });
            }
        };
        let open_esp_radio_esp32s31_wifi_ap::mac::Esp32s31ApMacParts { engine, transmit } = parts;
        let engine = engine.stop(hardware);
        #[cfg(any(feature = "diagnostics", test))]
        if let Some(terminal_observer) = terminal_observer {
            terminal_observer.observe(AccessPointTerminalObservation {
                control: control_observation,
                mac: mac_observation,
                engine: engine_observation,
            });
        }
        Ok(Esp32s31AccessPointProtocolStopped {
            transmit,
            rx_frame,
            tx_frame,
            data_rx,
            rx_block_ack,
            rx_reorder,
            rx_reorder_storage,
            #[cfg(feature = "diagnostics")]
            observation_storage: observer,
            engine,
        })
    }
}

impl<
    'storage,
    'beacon,
    'slot,
    R,
    C,
    P,
    E,
    T,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
    const TX_BUFFER_SIZE: usize,
>
    Esp32s31AccessPointControl<
        'storage,
        'beacon,
        'slot,
        R,
        C,
        P,
        E,
        T,
        COUNT,
        DMA_BUFFER_SIZE,
        DMA_STORAGE_SIZE,
        TX_BUFFER_SIZE,
    >
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: WifiTxTimer,
{
    /// Run the AP control plane until the caller publishes stop.
    ///
    /// A pending TX descriptor is always driven to a terminal edge before IRQ
    /// routing is masked. RX is then stopped cooperatively; `Busy` means the
    /// walker has not yet acknowledged the request and is retried without
    /// weakening ownership.
    pub async fn run_until_stopped<
        'resources,
        IR,
        NM,
        H,
        F,
        N,
        NR,
        Q,
        const FRAME_CAPACITY: usize,
        const HEADROOM: usize,
        const TRAILER: usize,
        const RX_QUEUE_DEPTH: usize,
        const TX_QUEUE_DEPTH: usize,
        const AMPDU_SLOTS: usize,
        const AMPDU_BUFFER_SIZE: usize,
    >(
        &mut self,
        hardware: &mut H,
        interrupts: &mut Esp32s31MacInterruptEpoch<'_, IR, NM>,
        platform: &IR::Platform,
        network: &NR,
        aggregate: &mut Esp32s31AccessPointAmpdu<
            'resources,
            PinnedTxFrame<'resources, NM, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
            AMPDU_SLOTS,
            AMPDU_BUFFER_SIZE,
        >,
        aggregate_tx_observer: Option<&dyn AggregateTxObserver>,
        #[cfg(feature = "diagnostics")] delivery_observer: Option<&dyn RxNetworkDeliveryObserver>,
        publish_shared_rx: Q,
        stop: F,
        mut status_observer: impl FnMut(AccessPointServiceStatus),
        security_material: N,
    ) -> Result<Esp32s31AccessPointRunObservation, Esp32s31AccessPointRunError<IR::Error>>
    where
        IR: MacInterruptRoute,
        NM: RawMutex,
        NR: crate::datapath::network::DatapathNetwork<
                'resources,
                NM,
                FRAME_CAPACITY,
                HEADROOM,
                TRAILER,
                RX_QUEUE_DEPTH,
                TX_QUEUE_DEPTH,
            >,
        H: RxDma
            + TxHardware
            + Esp32s31ApRuntimeHardware
            + RxBlockAckHardware
            + open_esp_radio_esp32s31_wifi_mac::tx_ampdu::HtAmpduHardware,
        R: AccessPointRxProducer<H, COUNT>,
        C: AccessPointRxProtocolConsumer,
        F: Future<Output = ()>,
        N: FnMut() -> ([u8; 32], u64),
        Q: FnMut(u8),
    {
        network.set_link_state(
            crate::roles::concurrent::AP_NETWORK_INTERFACE_ID,
            LinkState::Down,
        );
        self.start(hardware)
            .await
            .map_err(Esp32s31AccessPointRunError::Control)?;
        interrupts.mac_runtime().begin_rx_moderation();
        if let Err(error) = interrupts.activate(platform, MAC_COLD_RX_INTERRUPT_MASK) {
            interrupts.mac_runtime().end_rx_moderation();
            return Err(Esp32s31AccessPointRunError::InterruptActivate(error));
        }
        interrupts.mac_runtime().notify_rx_handoff();
        self.publish_beacon(hardware, Instant::now().as_micros())
            .map_err(Esp32s31AccessPointRunError::Control)?;
        let (last_status_revision, status, _) = self.role_observation();
        status_observer(status);
        if let Some(observer) = aggregate_tx_observer {
            observer.observe(AggregateTxObservation::BlockAckOperational {
                tid: 0,
                operational: false,
            });
        }
        let services = Esp32s31AccessPointDatapathServices {
            control: self,
            hardware,
            aggregate,
            network_tx: Esp32s31AccessPointNetworkTx::new(aggregate_tx_observer),
            status_observer,
            security_material,
            set_link_state: |state| {
                network.set_link_state(crate::roles::concurrent::AP_NETWORK_INTERFACE_ID, state)
            },
            publish_shared_rx,
            aggregate_tx_observer,
            #[cfg(feature = "diagnostics")]
            delivery_observer,
            last_status_revision,
            network_link_up: false,
            block_ack_observation: BlockAckObservationState::default(),
            #[cfg(feature = "diagnostics")]
            network_backpressure_since_micros: None,
            #[cfg(feature = "diagnostics")]
            tx_pending_since_micros: Some(Instant::now().as_micros()),
            #[cfg(feature = "diagnostics")]
            network_tx_pending: None,
            next_control_delay_millis: 1,
        };
        let mut runner = DatapathRunner::new(
            interrupts.mac_runtime(),
            network,
            crate::roles::concurrent::AP_NETWORK_INTERFACE_ID,
            services,
        );
        let exit = await_stack_boundary!(runner.run_until(stop)).map_err(|error| match error {
            Esp32s31AccessPointDatapathError::Control(error) => {
                Esp32s31AccessPointRunError::Control(error)
            }
            Esp32s31AccessPointDatapathError::Network(error) => {
                Esp32s31AccessPointRunError::Network(error)
            }
            Esp32s31AccessPointDatapathError::Aggregate(error) => {
                Esp32s31AccessPointRunError::Aggregate(error)
            }
        })?;
        let (_, mut services) = runner.into_parts();
        match exit {
            crate::datapath::DatapathRunnerExit::Stopped => {}
            crate::datapath::DatapathRunnerExit::Role(exit) => match exit {},
        }
        services.clear_block_ack_observation();
        drop(services);
        let _discarded_staged = self.protocol_rx.discard_queued();
        #[cfg(any(feature = "diagnostics", test))]
        let rx_scheduler = self.receive.scheduler_snapshot();
        observe_access_point!(self, observation, {
            observation.ignored_rx_frames = observation
                .ignored_rx_frames
                .saturating_add(u32::try_from(_discarded_staged).unwrap_or(u32::MAX));
            observation.retained_rx_descriptors = rx_scheduler
                .map(|snapshot| snapshot.observed_mask.count_ones())
                .unwrap_or(0);
        });
        let interrupt_drain = interrupts.quiesce(platform);
        interrupts.mac_runtime().end_rx_moderation();
        let _interrupt_drain =
            interrupt_drain.map_err(Esp32s31AccessPointRunError::InterruptQuiesce)?;
        loop {
            match self.stop(hardware) {
                Ok(()) => break,
                Err(Esp32s31AccessPointControlError::Receive(
                    open_esp_radio_esp32s31_wifi_mac::rx_pool::RxStageTransactionError::Ring(
                        open_esp_radio_esp32s31_wifi_mac::rx::RxRingError::Busy,
                    ),
                )) => yield_now().await,
                Err(error) => return Err(Esp32s31AccessPointRunError::Control(error)),
            }
        }
        Ok(Esp32s31AccessPointRunObservation {
            #[cfg(any(feature = "diagnostics", test))]
            interrupt_drain: _interrupt_drain,
            #[cfg(any(feature = "diagnostics", test))]
            rx_scheduler,
        })
    }

    /// Consume a quiescent AP service and return every reusable capability.
    /// Failure returns the exact service; no caller can manufacture stopped
    /// Wi-Fi while RX or TX remains active.
    #[allow(clippy::result_large_err, clippy::type_complexity)]
    pub fn try_finish<H>(
        self,
        hardware: &mut H,
    ) -> Result<
        Esp32s31AccessPointStopped<
            'storage,
            'beacon,
            'slot,
            P,
            E,
            T,
            R,
            C,
            COUNT,
            DMA_BUFFER_SIZE,
            DMA_STORAGE_SIZE,
            TX_BUFFER_SIZE,
        >,
        Self,
    >
    where
        H: Esp32s31ApRuntimeHardware + RxDma,
        R: AccessPointRxProducer<H, COUNT>,
        C: AccessPointRxProtocolConsumer,
    {
        if self.protocol_rx.queued_frames() != 0 {
            return Err(self);
        }
        let Self {
            receive,
            protocol_rx,
            role,
        } = self;
        let (processor, (), (), ()) = role.into_parts();
        let Esp32s31AccessPointProtocolStopped {
            transmit,
            rx_frame,
            tx_frame,
            data_rx,
            rx_block_ack,
            rx_reorder,
            rx_reorder_storage,
            #[cfg(feature = "diagnostics")]
            observation_storage,
            engine,
        } = match processor.try_finish_paired(hardware) {
            Ok(stopped) => stopped,
            Err(processor) => {
                return Err(Self {
                    receive,
                    protocol_rx,
                    role: AccessPointRoleRuntime::standalone(processor),
                });
            }
        };
        Ok(Esp32s31AccessPointStopped {
            receive,
            protocol_rx,
            transmit,
            rx_frame,
            tx_frame,
            data_rx,
            rx_block_ack,
            rx_reorder,
            rx_reorder_storage,
            #[cfg(feature = "diagnostics")]
            observation_storage,
            engine,
        })
    }
}

#[cfg(any(feature = "diagnostics", test))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EthernetProtocol {
    ArpRequest,
    ArpReply,
    Ipv4Tcp,
    Ipv4Other,
    Other,
}

#[cfg(any(feature = "diagnostics", test))]
fn ethernet_protocol(frame: &[u8]) -> Option<EthernetProtocol> {
    let ether_type = u16::from_be_bytes([*frame.get(12)?, *frame.get(13)?]);
    match ether_type {
        0x0800 => Some(if *frame.get(23)? == 6 {
            EthernetProtocol::Ipv4Tcp
        } else {
            EthernetProtocol::Ipv4Other
        }),
        0x0806 => match u16::from_be_bytes([*frame.get(20)?, *frame.get(21)?]) {
            1 => Some(EthernetProtocol::ArpRequest),
            2 => Some(EthernetProtocol::ArpReply),
            _ => Some(EthernetProtocol::Other),
        },
        _ => Some(EthernetProtocol::Other),
    }
}

#[cfg(any(feature = "diagnostics", test))]
#[inline(always)]
fn ethernet_parts_protocol(frame: EthernetFrameParts<'_>) -> Option<EthernetProtocol> {
    match frame.ether_type {
        0x0800 => Some(if *frame.payload.get(9)? == 6 {
            EthernetProtocol::Ipv4Tcp
        } else {
            EthernetProtocol::Ipv4Other
        }),
        0x0806 => match u16::from_be_bytes(frame.payload.get(6..8)?.try_into().ok()?) {
            1 => Some(EthernetProtocol::ArpRequest),
            2 => Some(EthernetProtocol::ArpReply),
            _ => Some(EthernetProtocol::Other),
        },
        _ => Some(EthernetProtocol::Other),
    }
}
