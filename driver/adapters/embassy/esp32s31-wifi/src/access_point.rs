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
    FrameLengthError, LinkState, PinnedTxConsumer, PinnedTxFrame, RxEnqueueError,
    SplitPinnedRadioRunner,
};

use open_esp_radio_esp32s31_wifi::{
    ampdu_tx::HtAmpduTxRolePolicy,
    ordinary_tx::{WifiTxEntropy, WifiTxPowerProfile, WifiTxResources, WifiTxTimer},
    tx::{WifiTxProgress, WifiTxWake},
};
use open_esp_radio_esp32s31_wifi_ap::protocol::{
    AP_MAX_CLIENTS, AccessPointServiceStatus, ApPeerClose, ApWpa2RetryProgress,
};
use open_esp_radio_esp32s31_wifi_ap::{
    ampdu::{
        Esp32s31ApAggregateAdmission, Esp32s31ApAmpduCompletion, Esp32s31ApAmpduError,
        Esp32s31ApAmpduProgress,
    },
    engine::{Esp32s31ApRuntimeHardware, Esp32s31ApWpa2Outcome},
    mac::{
        Esp32s31ApMac, Esp32s31ApMacError, Esp32s31ApMacReport, Esp32s31ApPeerDisconnectStage,
        Esp32s31ApTxCompletionAction,
    },
    rx::{
        Esp32s31ApRxConfig, Esp32s31ApRxDispatch, Esp32s31ApRxDispatcher, Esp32s31ApRxError,
        Esp32s31ApRxEvent, Esp32s31ApRxSink,
    },
};
use open_esp_radio_esp32s31_wifi_mac::{
    init::MAC_COLD_RX_INTERRUPT_MASK,
    irq::MacInterruptRoute,
    rx::{RxDescriptorSnapshot, RxDma, RxIngressConfig, view_normalized_rx_frame},
    rx_ampdu::{
        RX_BLOCK_ACK_MAX_WINDOW, RxBlockAckActivation, RxBlockAckRequest, RxBlockAckSessions,
        RxBlockAckSessionsError, write_declined_addba_response,
    },
    rx_ampdu_hw::{RxBlockAckHardware, S31RxBlockAckAgreementError},
    tx::{HtChannelWidth, HtMcs, HtRate, TxHardware},
};
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

#[cfg(feature = "rx-delivery-observation")]
use crate::network_rx::{RxNetworkDeliveryEvent, RxNetworkDeliveryObserver};
use crate::{
    aggregate_tx_common::AggregateTxServiceEvent,
    aggregate_tx_observer::{AggregateBuildStop, AggregateTxObservation, AggregateTxObserver},
    connected_rx_protocol::StagedEthernetPublication,
    embassy_irq::{
        Esp32s31MacInterruptEpoch, Esp32s31MacInterruptEpochActivateError,
        Esp32s31MacInterruptEpochDrain, Esp32s31MacInterruptEpochQuiesceError,
    },
    rx_frontier::Esp32s31RxFrontierSchedulerSnapshot,
    rx_reorder::{RX_REORDER_BACKING_SLOT_COUNT, RxReorderFrameStorage},
    wdev::{
        WdevControlContext, WdevControlProgress, WdevNetworkRx, WdevRunner, WdevRxProgress,
        WdevRxServiceContext, WdevServices, WdevStopProgress,
    },
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
/// The WDEV owner explicitly services DMA again at each protocol-quantum
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
const fn can_publish_ap_rx_in_place(
    current_staging_owner: bool,
    current_is_amsdu: bool,
    deferred_bytes: usize,
) -> bool {
    current_staging_owner && !current_is_amsdu && deferred_bytes == 0
}

mod ampdu;
mod network_tx;
mod protocol_mailbox;
mod rx_pipeline;
mod rx_reorder;
mod wdev;

pub use ampdu::Esp32s31AccessPointAmpdu;
use network_tx::Esp32s31AccessPointNetworkTx;
pub use protocol_mailbox::{
    Esp32s31AccessPointControlAction, Esp32s31AccessPointHardwareAction,
    Esp32s31AccessPointProtocolAction, Esp32s31AccessPointProtocolMailbox,
    Esp32s31AccessPointProtocolPublisher, Esp32s31AccessPointProtocolReceiver,
};
#[doc(hidden)]
pub use rx_pipeline::{
    AccessPointRxProducer, AccessPointRxProducerObservation, AccessPointRxProtocolConsumer,
    AccessPointStagedRxFrame, Esp32s31AccessPointRxConsumer, Esp32s31AccessPointRxProducer,
};
pub use rx_reorder::{Esp32s31AccessPointRxReorder, Esp32s31AccessPointRxReorderError};
use wdev::{BlockAckObservationState, Esp32s31AccessPointWdevServices};

const AP_PROTOCOL_ACTION_CAPACITY: usize = 2;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Esp32s31AccessPointControlReport {
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
    pub completed_rx_units: u32,
    pub completed_rx_descriptors: u32,
    /// Staged owners actually consumed by the AP protocol frontier.
    ///
    /// This is deliberately distinct from `completed_rx_descriptors`: one
    /// DMA observation can stage a complete burst while the protocol owner
    /// has advanced by only one frame.
    pub serviced_staged_rx_frames: u32,
    pub recycled_rx_descriptors: u32,
    pub retained_rx_descriptors: u32,
    pub discarded_rx_units: u32,
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
    pub rx_ht_ampdu_data_frames: u32,
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
    pub protected_data_radio_rejected: u32,
    pub protected_data_protocol_rejected: u32,
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
enum Esp32s31AccessPointWdevError {
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
pub struct Esp32s31AccessPointRunReport {
    pub control: Esp32s31AccessPointControlReport,
    pub mac: Esp32s31ApMacReport,
    pub engine: open_esp_radio_esp32s31_wifi_ap::engine::Esp32s31ApEngineReport,
    pub interrupt_drain: Esp32s31MacInterruptEpochDrain,
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
    pub rx_block_ack: &'storage mut RxBlockAckSessions<{ AP_MAX_CLIENTS }>,
    pub rx_reorder: &'storage mut Esp32s31AccessPointRxReorder<'storage, DMA_BUFFER_SIZE>,
    pub rx_reorder_storage:
        &'storage RxReorderFrameStorage<DMA_BUFFER_SIZE, RX_REORDER_BACKING_SLOT_COUNT>,
    pub engine: open_esp_radio_esp32s31_wifi_ap::engine::Esp32s31ApEngineStop<'beacon>,
    pub control_report: Esp32s31AccessPointControlReport,
    pub mac_report: Esp32s31ApMacReport,
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
    frames: crate::ethernet_rx::PackedEthernetWriter<'storage>,
    exhausted: bool,
}

impl<'storage> DeferredAccessPointRxSink<'storage> {
    fn new(storage: &'storage mut [u8]) -> Self {
        Self {
            frames: crate::ethernet_rx::PackedEthernetWriter::new(storage),
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
        assert!(can_publish_ap_rx_in_place(true, false, 0));
        assert!(!can_publish_ap_rx_in_place(true, false, 64));
        assert!(!can_publish_ap_rx_in_place(true, true, 0));
        assert!(!can_publish_ap_rx_in_place(false, false, 0));
    }
}

fn observe_protected_dispatch(
    dispatch: Esp32s31ApRxDispatch,
    peer: Option<[u8; 6]>,
    report: &mut Esp32s31AccessPointControlReport,
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
            report.protected_data_duplicates = report.protected_data_duplicates.saturating_add(1);
            false
        }
        Esp32s31ApRxDispatch::ForeignPeer => {
            report.protected_data_foreign = report.protected_data_foreign.saturating_add(1);
            false
        }
        Esp32s31ApRxDispatch::Unauthorized => {
            report.protected_data_unauthorized =
                report.protected_data_unauthorized.saturating_add(1);
            false
        }
        Esp32s31ApRxDispatch::Rejected(Esp32s31ApRxError::Radio(_)) => {
            report.protected_data_radio_rejected =
                report.protected_data_radio_rejected.saturating_add(1);
            false
        }
        Esp32s31ApRxDispatch::Rejected(Esp32s31ApRxError::Data(_)) => {
            report.protected_data_protocol_rejected =
                report.protected_data_protocol_rejected.saturating_add(1);
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
        current_buffer: usize,
        current_is_amsdu: bool,
        deferred: &mut DeferredAccessPointRxSink<'_>,
        in_place: &mut InPlaceAccessPointRxSink,
        report: &mut Esp32s31AccessPointControlReport,
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
        let outcome = if can_publish_ap_rx_in_place(current, current_is_amsdu, deferred.used()) {
            data_rx.dispatch_protected(ordered, &mut is_authorized, in_place)
        } else {
            data_rx.dispatch_protected(ordered, &mut is_authorized, deferred)
        };
        *produced_data |= observe_protected_dispatch(outcome, peer, report, activity_peer);
    }
}

/// Control-plane owner for one active AP role.
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
    mac: Esp32s31ApMac<'beacon, 'slot, P, E, T, TX_BUFFER_SIZE>,
    rx_frame: &'storage mut [u8],
    tx_frame: &'storage mut [u8],
    data_rx: &'storage mut Esp32s31ApRxDispatcher,
    rx_block_ack: &'storage mut RxBlockAckSessions<{ AP_MAX_CLIENTS }>,
    rx_reorder: &'storage mut Esp32s31AccessPointRxReorder<'storage, DMA_BUFFER_SIZE>,
    rx_reorder_storage:
        &'storage RxReorderFrameStorage<DMA_BUFFER_SIZE, RX_REORDER_BACKING_SLOT_COUNT>,
    rx_addba_in_flight: Option<RxBlockAckActivation>,
    protocol_actions: Esp32s31AccessPointProtocolMailbox<AP_PROTOCOL_ACTION_CAPACITY>,
    rx_batch_used: usize,
    rx_batch_offset: usize,
    report: Esp32s31AccessPointControlReport,
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
        rx_block_ack: &'storage mut RxBlockAckSessions<{ AP_MAX_CLIENTS }>,
        rx_reorder: &'storage mut Esp32s31AccessPointRxReorder<'storage, DMA_BUFFER_SIZE>,
        rx_reorder_storage: &'storage RxReorderFrameStorage<
            DMA_BUFFER_SIZE,
            RX_REORDER_BACKING_SLOT_COUNT,
        >,
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
        // The composition layer owns the negotiated RX window because it
        // also owns descriptor, staging and reorder capacity. Clear stale AP
        // epoch state without widening that integration contract.
        rx_block_ack.reset();
        let discarded_reorder_frames = rx_reorder.discard_all();
        debug_assert_eq!(discarded_reorder_frames, 0);
        Self {
            receive,
            protocol_rx,
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
            report: Esp32s31AccessPointControlReport::default(),
        }
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
    pub fn rx_descriptor_snapshot(&self, index: usize) -> Option<RxDescriptorSnapshot>
    where
        R: AccessPointRxProducerObservation<COUNT>,
    {
        self.receive.descriptor_snapshot(index)
    }

    /// Observe the live RX scheduler frontier without exposing ownership.
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
        #[cfg(feature = "rx-delivery-observation")] delivery_observer: Option<
            &dyn RxNetworkDeliveryObserver,
        >,
    ) -> Result<WdevRxProgress, Esp32s31AccessPointControlError>
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
            WdevRxProgress::ProbePending
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
            return Ok(WdevRxProgress::ProbePending);
        }

        let staged_frame = if tx_pending {
            self.protocol_rx.try_receive_protected_data()
        } else {
            self.protocol_rx.try_receive()
        };
        let Some(staged_frame) = staged_frame else {
            return Ok(stage_progress);
        };
        self.report.serviced_staged_rx_frames =
            self.report.serviced_staged_rx_frames.saturating_add(1);
        #[cfg(feature = "rx-delivery-observation")]
        let protocol_started = Instant::now().as_micros();
        let protocol_class = self.service_staged_rx(
            if rx_protocol_consumer_has_hardware(tx_pending) {
                Some(hardware)
            } else {
                None
            },
            staged_frame,
            authenticator_nonce,
            initial_replay_counter,
            now_micros,
            publish_shared_rx,
            #[cfg(feature = "rx-delivery-observation")]
            delivery_observer,
        )?;
        // `service_staged_rx` receives no hardware capability while TX is
        // active. Only after that protocol borrow ends does this radio owner
        // translate and execute its value-only mailbox requests.
        self.apply_protocol_actions(hardware)?;
        #[cfg(not(feature = "rx-delivery-observation"))]
        let _ = protocol_class;
        #[cfg(feature = "rx-delivery-observation")]
        {
            let elapsed = Instant::now().as_micros().saturating_sub(protocol_started);
            self.report.maximum_rx_protocol_service_micros = self
                .report
                .maximum_rx_protocol_service_micros
                .max(u32::try_from(elapsed).unwrap_or(u32::MAX));
            let elapsed = u32::try_from(elapsed).unwrap_or(u32::MAX);
            let class_maximum = match protocol_class {
                AccessPointRxProtocolClass::ProtectedData => {
                    self.report.total_rx_protected_data_service_micros = self
                        .report
                        .total_rx_protected_data_service_micros
                        .saturating_add(elapsed);
                    Some(&mut self.report.maximum_rx_protected_data_service_micros)
                }
                AccessPointRxProtocolClass::Management => {
                    Some(&mut self.report.maximum_rx_management_service_micros)
                }
                AccessPointRxProtocolClass::Eapol => {
                    Some(&mut self.report.maximum_rx_eapol_service_micros)
                }
                AccessPointRxProtocolClass::Other | AccessPointRxProtocolClass::Rejected => None,
            };
            if let Some(class_maximum) = class_maximum {
                *class_maximum = (*class_maximum).max(elapsed);
            }
        }

        Ok(
            if self.protocol_rx.queued_frames() != 0
                || self.rx_batch_pending()
                || self.mac.tx_pending()
            {
                WdevRxProgress::ProbePending
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
    ) -> Result<WdevRxProgress, Esp32s31AccessPointControlError>
    where
        H: RxDma,
        R: AccessPointRxProducer<H, COUNT>,
    {
        #[cfg(feature = "rx-delivery-observation")]
        let started = Instant::now().as_micros();
        let progress = self.receive.stage_completed(hardware).await?;
        #[cfg(feature = "rx-delivery-observation")]
        {
            let elapsed = Instant::now().as_micros().saturating_sub(started);
            self.report.maximum_rx_dma_service_micros = self
                .report
                .maximum_rx_dma_service_micros
                .max(u32::try_from(elapsed).unwrap_or(u32::MAX));
            self.report.total_rx_dma_service_micros = self
                .report
                .total_rx_dma_service_micros
                .saturating_add(u32::try_from(elapsed).unwrap_or(u32::MAX));
            self.report.rx_dma_service_calls = self.report.rx_dma_service_calls.saturating_add(1);
        }
        let producer = self.receive.report();
        self.report.completed_rx_units = producer.completed_units;
        self.report.completed_rx_descriptors = producer.completed_descriptors;
        self.report.recycled_rx_descriptors = producer.recycled_descriptors;
        self.report.discarded_rx_units = producer.discarded_units;
        Ok(progress)
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
        authenticator_nonce: [u8; 32],
        initial_replay_counter: u64,
        now_micros: u64,
        publish_shared_rx: &mut Q,
        #[cfg(feature = "rx-delivery-observation")] delivery_observer: Option<
            &dyn RxNetworkDeliveryObserver,
        >,
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
            Err(error) => {
                match error {
                    open_esp_radio_esp32s31_wifi_mac::rx::RxError::MicFailure => {
                        self.report.rx_mic_failures = self.report.rx_mic_failures.saturating_add(1);
                    }
                    open_esp_radio_esp32s31_wifi_mac::rx::RxError::Quarantined => {
                        let duplicate_or_stale = self
                            .data_rx
                            .reorder_key(segment)
                            .is_some_and(|key| self.rx_reorder.is_duplicate_or_stale(key));
                        if duplicate_or_stale {
                            self.report.protected_data_duplicates =
                                self.report.protected_data_duplicates.saturating_add(1);
                        } else {
                            self.report.rx_quarantined_frames =
                                self.report.rx_quarantined_frames.saturating_add(1);
                        }
                    }
                    _ => {
                        self.report.rx_view_rejected =
                            self.report.rx_view_rejected.saturating_add(1);
                    }
                }
                self.report.ignored_rx_frames = self.report.ignored_rx_frames.saturating_add(1);
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
            self.report.protected_data_frames = self.report.protected_data_frames.saturating_add(1);
            if let MacRxEvidence::HardwareObserved(rssi_dbm) = frame.metadata.rssi_dbm {
                if self.report.rx_rssi_samples == 0 {
                    self.report.rx_rssi_min_dbm = rssi_dbm;
                    self.report.rx_rssi_max_dbm = rssi_dbm;
                } else {
                    self.report.rx_rssi_min_dbm = self.report.rx_rssi_min_dbm.min(rssi_dbm);
                    self.report.rx_rssi_max_dbm = self.report.rx_rssi_max_dbm.max(rssi_dbm);
                }
                self.report.rx_rssi_samples = self.report.rx_rssi_samples.saturating_add(1);
                self.report.rx_rssi_sum_dbm = self
                    .report
                    .rx_rssi_sum_dbm
                    .saturating_add(i32::from(rssi_dbm));
            }
            if let MacRxEvidence::HardwareObserved(phy) = frame.metadata.rate
                && let Some(signal) = phy.ht_signal()
            {
                self.report.rx_ht_data_frames = self.report.rx_ht_data_frames.saturating_add(1);
                if signal.aggregation {
                    self.report.rx_ht_ampdu_data_frames =
                        self.report.rx_ht_ampdu_data_frames.saturating_add(1);
                }
                if signal.channel_width_mhz == 40
                    && let Some(count) = self.report.rx_ht40_mcs_frames.get_mut(signal.mcs as usize)
                {
                    *count = count.saturating_add(1);
                }
            }
            let (
                reorder_progress,
                batch_used,
                current_batch_exhausted,
                in_place_publication,
                produced_data,
            ) = {
                let mac = &self.mac;
                let data_rx = &mut self.data_rx;
                let report = &mut self.report;
                let mut deferred = DeferredAccessPointRxSink::new(self.rx_frame);
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
                                current_buffer as usize,
                                current_is_amsdu,
                                &mut deferred,
                                &mut in_place,
                                report,
                                &mut activity_peer,
                                &mut produced_data,
                            );
                        };
                    if let Some(key) = key {
                        self.rx_reorder.ingest(
                            self.rx_reorder_storage,
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
                let agreement = self.rx_block_ack.snapshots()[usize::from(reset.hardware_index)]
                    .expect("reorder reset belongs to one live receive BlockAck agreement");
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
            if reorder_progress.duplicate {
                self.report.protected_data_duplicates =
                    self.report.protected_data_duplicates.saturating_add(1);
            }
            if reorder_progress.dropped {
                self.report.protected_data_protocol_rejected = self
                    .report
                    .protected_data_protocol_rejected
                    .saturating_add(1);
            }
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
                let raw = segment.buffer;
                let payload = &raw
                    [ethernet.payload_offset..ethernet.payload_offset + ethernet.payload_length];
                let ethernet_frame = EthernetFrameParts {
                    destination: ethernet.destination,
                    source: ethernet.source,
                    ether_type: ethernet.ether_type,
                    payload,
                };
                let protocol = ethernet_parts_protocol(ethernet_frame);
                #[cfg(feature = "rx-delivery-observation")]
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
                self.report.ethernet_frames_staged =
                    self.report.ethernet_frames_staged.saturating_add(1);
                match protocol {
                    Some(EthernetProtocol::ArpRequest) => {
                        self.report.ethernet_arp_requests_staged =
                            self.report.ethernet_arp_requests_staged.saturating_add(1);
                    }
                    Some(EthernetProtocol::Ipv4Tcp) => {
                        self.report.ethernet_tcp_frames_staged =
                            self.report.ethernet_tcp_frames_staged.saturating_add(1);
                    }
                    _ => {}
                }
            }
            if !produced_data {
                self.report.ignored_rx_frames = self.report.ignored_rx_frames.saturating_add(1);
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
                self.report.control_frames_staged =
                    self.report.control_frames_staged.saturating_add(1);
            }
            AccessPointRxProtocolClass::Management
        } else if frame_control & 0x000c == 0x0008 {
            let hardware = hardware
                .as_deref_mut()
                .ok_or(Esp32s31AccessPointControlError::ProtocolFrameRequiresHardware)?;
            self.service_eapol(hardware, frame.mpdu, now_micros)?;
            AccessPointRxProtocolClass::Eapol
        } else {
            self.report.ignored_rx_frames = self.report.ignored_rx_frames.saturating_add(1);
            AccessPointRxProtocolClass::Other
        };
        if batch_exhausted {
            self.report.protected_data_protocol_rejected = self
                .report
                .protected_data_protocol_rejected
                .saturating_add(1);
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
                    let activation = match self.rx_block_ack.begin_pending(
                        open_esp_radio_esp32s31_hal::types::MacInterface::AccessPoint,
                    ) {
                        Ok(Some(activation)) => activation,
                        Ok(None) => return Ok(false),
                        Err(RxBlockAckSessionsError::NoFreeHardwareBank) => {
                            let discarded = self.rx_block_ack.discard_pending(peer, tid);
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
                    if let Some(agreement) = self.rx_block_ack.stop(peer, tid) {
                        self.release_rx_reorder(agreement.identity(), now_micros)?;
                        hardware.clear_rx_block_ack(agreement.hardware_index)?;
                    }
                    return Ok(false);
                }
                _ => {}
            }
        }

        let outcome = self.mac.publish_management(
            hardware,
            mpdu,
            authenticator_nonce,
            initial_replay_counter,
            now_micros,
            self.tx_frame,
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
        self.mac
            .publish_rx_block_ack_response(hardware, peer, &body, self.tx_frame)?;
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
        if let Err(error) = self.mac.publish_rx_block_ack_response(
            hardware,
            negotiated.peer,
            activation.response_body(),
            self.tx_frame,
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
        let mac = &self.mac;
        let data_rx = &mut self.data_rx;
        let report = &mut self.report;
        let mut activity_peer = None;
        let mut sink = DeferredAccessPointRxSink::new(self.rx_frame);
        let _ = self.rx_reorder.stop(identity, |segment| {
            let peer = data_rx.reorder_key(segment).map(|key| key.peer);
            let outcome = data_rx.dispatch_protected(
                segment,
                |peer| mac.engine().is_authorized_peer(peer),
                &mut sink,
            );
            let _ = observe_protected_dispatch(outcome, peer, report, &mut activity_peer);
        });
        if sink.exhausted {
            return Err(Esp32s31AccessPointControlError::ReceiveBatchCapacity);
        }
        if let Some(peer) = activity_peer {
            self.mac
                .engine_mut()
                .observe_peer_activity(peer, now_micros)?;
        }
        let used = sink.used();
        if used != 0 {
            self.rx_batch_used = used;
            self.rx_batch_offset = 0;
        }
        Ok(())
    }

    fn discard_rx_peer<H: RxBlockAckHardware>(
        &mut self,
        hardware: &mut H,
        peer: [u8; 6],
    ) -> Result<(), Esp32s31AccessPointControlError> {
        for agreement in self.rx_block_ack.stop_peer(peer).into_iter().flatten() {
            let _ = self.rx_reorder.stop_discard(agreement.identity());
            hardware.clear_rx_block_ack(agreement.hardware_index)?;
        }
        Ok(())
    }

    fn service_rx_reorder_expiry(
        &mut self,
        now_micros: u64,
    ) -> Result<bool, Esp32s31AccessPointControlError> {
        let mac = &self.mac;
        let data_rx = &mut self.data_rx;
        let report = &mut self.report;
        let mut activity_peer = None;
        let mut sink = DeferredAccessPointRxSink::new(self.rx_frame);
        let dispatched = if self.rx_reorder.dispatch_pending(|segment| {
            let peer = data_rx.reorder_key(segment).map(|key| key.peer);
            let outcome = data_rx.dispatch_protected(
                segment,
                |peer| mac.engine().is_authorized_peer(peer),
                &mut sink,
            );
            let _ = observe_protected_dispatch(outcome, peer, report, &mut activity_peer);
        }) {
            1
        } else {
            self.rx_reorder.expire_due(now_micros, |segment| {
                let peer = data_rx.reorder_key(segment).map(|key| key.peer);
                let outcome = data_rx.dispatch_protected(
                    segment,
                    |peer| mac.engine().is_authorized_peer(peer),
                    &mut sink,
                );
                let _ = observe_protected_dispatch(outcome, peer, report, &mut activity_peer);
            })
        };
        if sink.exhausted {
            return Err(Esp32s31AccessPointControlError::ReceiveBatchCapacity);
        }
        if let Some(peer) = activity_peer {
            self.mac
                .engine_mut()
                .observe_peer_activity(peer, now_micros)?;
        }
        let used = sink.used();
        if used != 0 {
            self.rx_batch_used = used;
            self.rx_batch_offset = 0;
            return Ok(true);
        }
        Ok(dispatched != 0)
    }

    pub const fn rx_batch_pending(&self) -> bool {
        self.rx_batch_offset < self.rx_batch_used
    }

    fn observe_ht_aggregate(&mut self, rate: HtRate) {
        self.report.tx_ht_aggregates = self.report.tx_ht_aggregates.saturating_add(1);
        if rate.channel_width == HtChannelWidth::Mhz40 && rate.mcs == HtMcs::Mcs7 {
            self.report.tx_ht40_mcs7_aggregates =
                self.report.tx_ht40_mcs7_aggregates.saturating_add(1);
        }
    }

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

    fn rx_batch_record(
        &self,
    ) -> Result<Option<crate::ethernet_rx::PackedEthernetRecord<'_>>, Esp32s31AccessPointControlError>
    {
        crate::ethernet_rx::record_at(self.rx_frame, self.rx_batch_used, self.rx_batch_offset)
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
            self.report.ignored_rx_frames = self.report.ignored_rx_frames.saturating_add(1);
            return Ok(());
        };
        let Ok(plan) = plan_data_decapsulation(
            DataInterfaceRole::AccessPoint,
            mpdu,
            header_length,
            payload_length,
        ) else {
            self.report.ignored_rx_frames = self.report.ignored_rx_frames.saturating_add(1);
            return Ok(());
        };
        if plan.ether_type != EAPOL_ETHERTYPE
            || plan.destination != self.mac.engine().service_address()
            || self.mac.engine().peer_status(plan.source).is_none()
        {
            self.report.ignored_rx_frames = self.report.ignored_rx_frames.saturating_add(1);
            return Ok(());
        }
        let payload = &mpdu[plan.payload_offset..plan.payload_offset + plan.payload_length];
        let Ok(frame) = OwnedEapolFrame::<EAPOL_CAPACITY>::try_copy(
            Wpa2Interface::AccessPoint,
            plan.source,
            payload,
        ) else {
            self.report.ignored_rx_frames = self.report.ignored_rx_frames.saturating_add(1);
            return Ok(());
        };
        match self
            .mac
            .engine_mut()
            .handle_eapol(hardware, plan.source, frame, now_micros)?
        {
            Esp32s31ApWpa2Outcome::Transmit(frame) => {
                self.mac
                    .publish_eapol(hardware, plan.source, &frame, self.tx_frame)?;
                self.report.control_frames_staged =
                    self.report.control_frames_staged.saturating_add(1);
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
                if self.mac.publish_tx_block_ack_request(
                    hardware,
                    peer,
                    now_micros,
                    self.tx_frame,
                )? {
                    self.report.control_frames_staged =
                        self.report.control_frames_staged.saturating_add(1);
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
                self.mac
                    .publish_eapol(hardware, peer, &message1, self.tx_frame)?;
                self.report.control_frames_staged =
                    self.report.control_frames_staged.saturating_add(1);
                return Ok(WifiTxProgress::Pending);
            }
            Esp32s31ApTxCompletionAction::PeerDisconnectTerminal {
                close,
                stage: Esp32s31ApPeerDisconnectStage::Disassociation,
                ..
            } => {
                self.mac.publish_peer_disconnect(
                    hardware,
                    close,
                    Esp32s31ApPeerDisconnectStage::Deauthentication,
                    self.tx_frame,
                )?;
                self.report.control_frames_staged =
                    self.report.control_frames_staged.saturating_add(1);
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
        self.mac
            .publish_peer_disconnect(hardware, close, stage, self.tx_frame)?;
        self.report.control_frames_staged = self.report.control_frames_staged.saturating_add(1);
        Ok(())
    }

    pub const fn report(&self) -> Esp32s31AccessPointControlReport {
        self.report
    }

    pub const fn mac_report(&self) -> Esp32s31ApMacReport {
        self.mac.report()
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
        let (missed, lateness) = self.mac.beacon_publication_lateness(now_micros as u32);
        self.report.missed_beacon_intervals =
            self.report.missed_beacon_intervals.saturating_add(missed);
        self.report.maximum_beacon_lateness_micros =
            self.report.maximum_beacon_lateness_micros.max(lateness);
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
        self.mac
            .publish_ethernet(hardware, peer, ethernet, self.tx_frame)?;
        Ok(())
    }

    fn start_network_tx<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        ethernet: &[u8],
    ) -> Result<WifiTxProgress, Esp32s31AccessPointControlError> {
        self.report.network_tx_frames_observed =
            self.report.network_tx_frames_observed.saturating_add(1);
        match ethernet_protocol(ethernet) {
            Some(EthernetProtocol::ArpRequest) => {
                self.report.network_tx_arp_requests =
                    self.report.network_tx_arp_requests.saturating_add(1);
            }
            Some(EthernetProtocol::ArpReply) => {
                self.report.network_tx_arp_replies =
                    self.report.network_tx_arp_replies.saturating_add(1);
            }
            _ => {}
        }
        if self.mac.engine().authorized_peer_count() == 0 {
            self.report.network_tx_rejected_no_peer =
                self.report.network_tx_rejected_no_peer.saturating_add(1);
            self.report.network_tx_frames_rejected =
                self.report.network_tx_frames_rejected.saturating_add(1);
            return Ok(WifiTxProgress::Complete);
        }
        let Some(destination) = ethernet
            .get(..6)
            .and_then(|bytes| <[u8; 6]>::try_from(bytes).ok())
        else {
            self.report.network_tx_rejected_destination = self
                .report
                .network_tx_rejected_destination
                .saturating_add(1);
            self.report.network_tx_frames_rejected =
                self.report.network_tx_frames_rejected.saturating_add(1);
            return Ok(WifiTxProgress::Complete);
        };
        if destination[0] & 1 == 0 && !self.mac.engine().is_authorized_peer(destination) {
            self.report.network_tx_rejected_destination = self
                .report
                .network_tx_rejected_destination
                .saturating_add(1);
            self.report.network_tx_frames_rejected =
                self.report.network_tx_frames_rejected.saturating_add(1);
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

    /// Advance AP timer and peer policy by one finite WDEV control step.
    ///
    /// This method never waits. A published frame returns `TxPending`; the
    /// caller must drive the shared TX owner to a terminal edge before
    /// invoking another control transition.
    pub fn service_control<H>(
        &mut self,
        hardware: &mut H,
        now_micros: u64,
    ) -> Result<WdevControlProgress<Infallible>, Esp32s31AccessPointControlError>
    where
        H: TxHardware + Esp32s31ApRuntimeHardware,
    {
        if self.tx_pending() {
            return Ok(WdevControlProgress::TxPending);
        }
        if self.beacon_publication_due(now_micros as u32) {
            self.publish_beacon(hardware, now_micros)?;
            return Ok(WdevControlProgress::TxPending);
        }
        self.mac.expire_tx_block_ack(now_micros)?;
        match self
            .mac
            .engine_mut()
            .take_due_wpa2_retry::<EAPOL_CAPACITY>(now_micros)?
        {
            ApWpa2RetryProgress::Transmit { peer, frame } => {
                self.mac
                    .publish_eapol(hardware, peer, &frame, self.tx_frame)?;
                self.report.control_frames_staged =
                    self.report.control_frames_staged.saturating_add(1);
                return Ok(WdevControlProgress::TxPending);
            }
            ApWpa2RetryProgress::Close(close) => {
                self.publish_peer_close(hardware, close)?;
                return Ok(WdevControlProgress::TxPending);
            }
            ApWpa2RetryProgress::None => {}
        }
        if let Some(close) = self.mac.engine_mut().begin_due_peer_close(now_micros) {
            self.publish_peer_close(hardware, close)?;
            return Ok(WdevControlProgress::TxPending);
        }
        self.next_control_delay_millis(now_micros)?;
        Ok(WdevControlProgress::Idle)
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

    /// Advance AP shutdown by one finite WDEV transition.
    pub fn service_stop<H>(
        &mut self,
        hardware: &mut H,
    ) -> Result<WdevStopProgress, Esp32s31AccessPointControlError>
    where
        H: TxHardware,
    {
        if let Some(close) = self.mac.engine_mut().begin_stop_peer() {
            self.publish_peer_close(hardware, close)?;
            Ok(WdevStopProgress::TxPending)
        } else {
            Ok(WdevStopProgress::Stopped)
        }
    }

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
        network: &SplitPinnedRadioRunner<
            'resources,
            NM,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            RX_QUEUE_DEPTH,
            TX_QUEUE_DEPTH,
        >,
        aggregate: &mut Esp32s31AccessPointAmpdu<
            'resources,
            PinnedTxFrame<'resources, NM, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
            AMPDU_SLOTS,
            AMPDU_BUFFER_SIZE,
        >,
        aggregate_tx_observer: Option<&dyn AggregateTxObserver>,
        #[cfg(feature = "rx-delivery-observation")] delivery_observer: Option<
            &dyn RxNetworkDeliveryObserver,
        >,
        publish_shared_rx: Q,
        stop: F,
        mut status_observer: impl FnMut(AccessPointServiceStatus),
        security_material: N,
    ) -> Result<Esp32s31AccessPointRunReport, Esp32s31AccessPointRunError<IR::Error>>
    where
        IR: MacInterruptRoute,
        NM: RawMutex,
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
        network.set_link_state(LinkState::Down);
        network.set_hardware_address(self.mac.engine().service_address());
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
        let services = Esp32s31AccessPointWdevServices {
            control: self,
            hardware,
            network_tx: Esp32s31AccessPointNetworkTx::new(aggregate, aggregate_tx_observer),
            status_observer,
            security_material,
            set_link_state: |state| network.set_link_state(state),
            publish_shared_rx,
            aggregate_tx_observer,
            #[cfg(feature = "rx-delivery-observation")]
            delivery_observer,
            last_status_revision,
            network_link_up: false,
            block_ack_observation: BlockAckObservationState::default(),
            network_backpressure_since_micros: None,
            tx_pending_since_micros: Some(Instant::now().as_micros()),
            network_tx_pending: None,
            next_control_delay_millis: 1,
        };
        let mut runner = WdevRunner::new(interrupts.mac_runtime(), network, services);
        let exit = await_stack_boundary!(runner.run_until(stop)).map_err(|error| match error {
            Esp32s31AccessPointWdevError::Control(error) => {
                Esp32s31AccessPointRunError::Control(error)
            }
            Esp32s31AccessPointWdevError::Network(error) => {
                Esp32s31AccessPointRunError::Network(error)
            }
            Esp32s31AccessPointWdevError::Aggregate(error) => {
                Esp32s31AccessPointRunError::Aggregate(error)
            }
        })?;
        let (_, mut services) = runner.into_parts();
        match exit {
            crate::wdev::WdevRunnerExit::Stopped => {}
            crate::wdev::WdevRunnerExit::Role(exit) => match exit {},
        }
        services.clear_block_ack_observation();
        drop(services);
        let discarded_staged = self.protocol_rx.discard_queued();
        self.report.ignored_rx_frames = self
            .report
            .ignored_rx_frames
            .saturating_add(u32::try_from(discarded_staged).unwrap_or(u32::MAX));
        let rx_scheduler = self.receive.scheduler_snapshot();
        self.report.retained_rx_descriptors = rx_scheduler
            .map(|snapshot| snapshot.observed_mask.count_ones())
            .unwrap_or(0);
        let interrupt_drain = interrupts.quiesce(platform);
        interrupts.mac_runtime().end_rx_moderation();
        let interrupt_drain =
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
        Ok(Esp32s31AccessPointRunReport {
            control: self.report(),
            mac: self.mac_report(),
            engine: self.mac.engine().report(),
            interrupt_drain,
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
        if self.rx_batch_pending()
            || self.protocol_rx.queued_frames() != 0
            || self.rx_addba_in_flight.is_some()
            || !self.protocol_actions.is_empty()
            || self.rx_reorder.has_pending_release()
            || self
                .rx_block_ack
                .snapshots()
                .into_iter()
                .any(|entry| entry.is_some())
        {
            return Err(self);
        }
        let Self {
            receive,
            protocol_rx,
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
            report,
        } = self;
        let (engine, transmit, mac_report) = match mac.try_into_parts() {
            Ok(parts) => parts,
            Err(mac) => {
                return Err(Self {
                    receive,
                    protocol_rx,
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
                    report,
                });
            }
        };
        let engine = engine.stop(hardware);
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
            engine,
            control_report: report,
            mac_report,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EthernetProtocol {
    ArpRequest,
    ArpReply,
    Ipv4Tcp,
    Ipv4Other,
    Other,
}

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
