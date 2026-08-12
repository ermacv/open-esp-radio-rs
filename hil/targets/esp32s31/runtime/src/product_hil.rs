//! Product-level HIL composition.
//!
//! This module is deliberately an application of the public driver API. PAC,
//! DMA, ISR and station internals stay in `open-esp-radio-esp32s31-embassy-wifi`.

use core::{
    num::NonZeroU16,
    sync::atomic::{AtomicU32, Ordering},
};

use embassy_executor::{SendSpawner, Spawner};
use embassy_futures::select::{Either, select};
use embassy_net::{
    Config as NetworkConfig, ConfigV4, Ipv4Address, Ipv4Cidr, Stack, StackResources, StaticConfigV4,
};
use embassy_sync::{
    blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel, signal::Signal,
};
use embassy_time::{Duration, Instant, Timer};
use esp_hal::{
    efuse::{self, InterfaceMacAddress},
    rng::Trng,
};
use open_esp_radio::esp32s31::wifi::embassy::await_stack_boundary;
use open_esp_radio::{
    AccessPointRequest, AccessPointSecurity, MonitorCapturePolicy, MonitorRequest, StationRequest,
    StationScanChannels, StationScanPolicy, StationSecurity, WifiMacAddress, WifiMonitorConfig,
    WifiRoleStartFailure, WifiRoleStopFailure, WifiScanRequest as DriverWifiScanRequest, WifiSsid,
    esp32s31::phy::{
        PhyCalibrationIdentity, PhyCalibrationPath, phy_rfpll::phy_get_rf_cal_version,
    },
    esp32s31::wifi::mac::rx::RxBasebandFormat,
    wifi::{
        ieee80211::{channel::WifiChannel, station::StaAssociationPreference},
        softmac::MacRxEvidence,
        sta::station::StaReconnectPolicy,
        wpa2::Pmk,
    },
};
use open_esp_radio_esp32s31_embassy_wifi::{
    ConnectedDisconnectReason, Esp32s31AccessPointObservation, Esp32s31MacIrqObservation,
    Esp32s31MonitorFrame, Esp32s31MonitorFrames, Esp32s31QualificationHooks,
    Esp32s31QualificationSnapshot, Esp32s31RadioConfig, Esp32s31RadioParts, Esp32s31RadioRunner,
    Esp32s31StationLifecycleObservation, Esp32s31WifiControl, Esp32s31WifiDevice,
    Esp32s31WifiParts,
};
use open_esp_radio_esp32s31_wifi_esp_hal::EspHalRadioPeripheral;
#[cfg(feature = "network-scheduler-telemetry")]
use open_esp_radio_hil_esp32s31_telemetry::network_scheduler::NetworkSchedulerCounters;
use open_esp_radio_hil_esp32s31_telemetry::{
    aggregate_tx::AggregateTxCounters, mac_irq::MacIrqClassificationCounters,
    rx_pipeline::RxPipelineCounters, task_poll::TaskPollSet,
};
use open_esp_radio_hil_protocol::{
    Capabilities, Event as HilEvent, FeatureCapabilities, MAX_WIRE_FRAME_BYTES, NetworkCredentials,
    NetworkInfo, NetworkIpv4Configuration, StartupArtifactDisposition, StationAttemptFailureReason,
    StationDisconnectReason, StationEpochEvidence, StationFailureStage, StationLifecycleEvent,
    WIFI_MONITOR_FRAME_CHUNK_MAX_LEN, WifiAccessPointEvidence, WifiDataPlanePlacement,
    WifiMonitorCaptureRequest, WifiMonitorEvidence, WifiMonitorEvidenceSource,
    WifiMonitorFrameChunk, WifiMonitorObserved, WifiMonitorPhyEvidence, WifiMonitorPhyFormat,
    WifiRole, WifiRoleFailureEvidence, WifiRoleFailureReason, WifiRoleOperation,
    WifiRoleTransitionEvidence, WifiScanEvidence,
};
use static_cell::ConstStaticCell;

use crate::console::{
    WifiControlRequest, complete_access_point_start, complete_access_point_stop,
    complete_initialization, complete_monitor_capture, complete_monitor_start,
    complete_monitor_stop, complete_station_epoch_cycle, complete_wifi_role_failure,
    complete_wifi_role_transition, complete_wifi_scan, publish_event_reliably,
    publish_monitor_frame, publish_startup_artifact, publish_station_lifecycle,
    receive_wifi_control_request, runtime_log, set_wifi_role,
};

mod rx_qualification;
mod traffic;

use traffic::{
    observe_open_radio_task_polls, start_connected_traffic, tcp_rx_pattern_worker_task,
    tcp_tx_pattern_worker_task,
};

const NETWORK_SOCKET_COUNT: usize = 5;
const SCAN_DWELL_MS: u16 = 200;
const MAXIMUM_TX_POWER_QUARTER_DBM: i8 = 80;
pub(crate) const OPEN_RADIO_TASK_POLL_TELEMETRY: bool = cfg!(feature = "task-poll-telemetry");
pub(crate) const OPEN_RADIO_NETWORK_SCHEDULER_TELEMETRY: bool =
    cfg!(feature = "network-scheduler-telemetry");
pub(crate) const OPEN_RADIO_RX_DELIVERY_TELEMETRY: bool = cfg!(feature = "rx-delivery-telemetry");
pub(crate) const OPEN_RADIO_TCP_CHUNK_CAPACITY: usize = 32_768;

struct AppNetworkStart {
    device: Esp32s31WifiDevice,
    ipv4: NetworkIpv4Configuration,
    seed: u64,
}

#[derive(Clone, Copy)]
pub(in crate::product_hil) enum QualificationRequester {
    UdpRx,
    UdpTx,
    Tcp,
}

#[derive(Clone, Copy)]
pub(in crate::product_hil) struct QualificationSample {
    pub rx_primary: Option<open_esp_radio::esp32s31::registers::MacRxPrimaryStatistics>,
    pub rx_interrupt_posts: u32,
    pub tx_vector: Option<open_esp_radio_esp32s31_embassy_wifi::Esp32s31QualificationTxVector>,
}

static DIAGNOSTIC_STAGE: AtomicU32 = AtomicU32::new(0);
static NETWORK_RESOURCES: ConstStaticCell<StackResources<NETWORK_SOCKET_COUNT>> =
    ConstStaticCell::new(StackResources::new());
static APP_NETWORK_START: Channel<CriticalSectionRawMutex, AppNetworkStart, 1> = Channel::new();
static PRIMARY_NETWORK_START: Channel<CriticalSectionRawMutex, AppNetworkStart, 1> = Channel::new();
static APP_NETWORK_READY: Signal<CriticalSectionRawMutex, ()> = Signal::new();
static NETWORK_CONFIG_REQUESTS: Channel<CriticalSectionRawMutex, NetworkIpv4Configuration, 1> =
    Channel::new();
static NETWORK_CONFIG_APPLIED: Channel<CriticalSectionRawMutex, (), 1> = Channel::new();
static QUALIFICATION_REQUESTS: Channel<CriticalSectionRawMutex, QualificationRequester, 3> =
    Channel::new();
static UDP_RX_QUALIFICATION: Channel<CriticalSectionRawMutex, QualificationSample, 1> =
    Channel::new();
static UDP_TX_QUALIFICATION: Channel<CriticalSectionRawMutex, QualificationSample, 1> =
    Channel::new();
static TCP_QUALIFICATION: Channel<CriticalSectionRawMutex, QualificationSample, 1> = Channel::new();
static CONNECTED_RX_OBSERVER: ConstStaticCell<rx_qualification::HilConnectedRxObserver> =
    ConstStaticCell::new(rx_qualification::HilConnectedRxObserver::new(4_323));
static PHY_CALIBRATION_ARTIFACT: ConstStaticCell<
    [u8; crate::phy_calibration_artifact::MAX_ENCODED_LEN],
> = ConstStaticCell::new([0; crate::phy_calibration_artifact::MAX_ENCODED_LEN]);
static STATION_LIFECYCLE: Channel<CriticalSectionRawMutex, StationLinkEdge, 16> = Channel::new();
static AP_CHANNEL: AtomicU32 = AtomicU32::new(0);
static AP_BEACONS: AtomicU32 = AtomicU32::new(0);
static AP_MISSED_BEACON_INTERVALS: AtomicU32 = AtomicU32::new(0);
static AP_MAXIMUM_BEACON_LATENESS_MICROS: AtomicU32 = AtomicU32::new(0);
static AP_TX_INTERRUPT_WAKES: AtomicU32 = AtomicU32::new(0);
static AP_TX_DEADLINE_WAKES: AtomicU32 = AtomicU32::new(0);
static AP_MAXIMUM_TX_PENDING_MICROS: AtomicU32 = AtomicU32::new(0);
static AP_MAXIMUM_RX_SERVICE_MICROS: AtomicU32 = AtomicU32::new(0);
static AP_MAXIMUM_NETWORK_BACKPRESSURE_MICROS: AtomicU32 = AtomicU32::new(0);
static AP_AUTHENTICATIONS: AtomicU32 = AtomicU32::new(0);
static AP_ASSOCIATIONS: AtomicU32 = AtomicU32::new(0);
static AP_AUTHORIZATIONS: AtomicU32 = AtomicU32::new(0);
static AP_REMOVALS: AtomicU32 = AtomicU32::new(0);
static AP_COMPLETED_RX_DESCRIPTORS: AtomicU32 = AtomicU32::new(0);
static AP_IGNORED_RX_FRAMES: AtomicU32 = AtomicU32::new(0);
static AP_RX_MIC_FAILURES: AtomicU32 = AtomicU32::new(0);
static AP_RX_QUARANTINED_FRAMES: AtomicU32 = AtomicU32::new(0);
static AP_RX_VIEW_REJECTED: AtomicU32 = AtomicU32::new(0);
static AP_CONTROL_FRAMES_STAGED: AtomicU32 = AtomicU32::new(0);
static AP_CONTROL_FRAMES_DROPPED_WHILE_BUSY: AtomicU32 = AtomicU32::new(0);
static AP_ETHERNET_FRAMES_STAGED: AtomicU32 = AtomicU32::new(0);
static AP_ETHERNET_ARP_REQUESTS_STAGED: AtomicU32 = AtomicU32::new(0);
static AP_ETHERNET_TCP_FRAMES_STAGED: AtomicU32 = AtomicU32::new(0);
static AP_NETWORK_TX_FRAMES_OBSERVED: AtomicU32 = AtomicU32::new(0);
static AP_NETWORK_TX_ARP_REQUESTS: AtomicU32 = AtomicU32::new(0);
static AP_NETWORK_TX_ARP_REPLIES: AtomicU32 = AtomicU32::new(0);
static AP_NETWORK_TX_REJECTED_NO_PEER: AtomicU32 = AtomicU32::new(0);
static AP_NETWORK_TX_REJECTED_DESTINATION: AtomicU32 = AtomicU32::new(0);
static AP_NETWORK_TX_FRAMES_REJECTED: AtomicU32 = AtomicU32::new(0);
static AP_DATA_FRAMES_TRANSMITTED: AtomicU32 = AtomicU32::new(0);
static AP_TX_FAILURES: AtomicU32 = AtomicU32::new(0);
static AP_PROTECTED_DATA_FRAMES: AtomicU32 = AtomicU32::new(0);
static AP_PROTECTED_DATA_UNAUTHORIZED: AtomicU32 = AtomicU32::new(0);
static AP_PROTECTED_DATA_FOREIGN: AtomicU32 = AtomicU32::new(0);
static AP_PROTECTED_DATA_DUPLICATES: AtomicU32 = AtomicU32::new(0);
static AP_PROTECTED_DATA_RADIO_REJECTED: AtomicU32 = AtomicU32::new(0);
static AP_PROTECTED_DATA_PROTOCOL_REJECTED: AtomicU32 = AtomicU32::new(0);

fn observe_access_point(observation: Esp32s31AccessPointObservation) {
    AP_CHANNEL.store(u32::from(observation.channel), Ordering::Release);
    AP_BEACONS.store(observation.beacons_transmitted, Ordering::Release);
    AP_MISSED_BEACON_INTERVALS.store(observation.missed_beacon_intervals, Ordering::Release);
    AP_MAXIMUM_BEACON_LATENESS_MICROS.store(
        observation.maximum_beacon_lateness_micros,
        Ordering::Release,
    );
    AP_TX_INTERRUPT_WAKES.store(observation.tx_interrupt_wakes, Ordering::Release);
    AP_TX_DEADLINE_WAKES.store(observation.tx_deadline_wakes, Ordering::Release);
    AP_MAXIMUM_TX_PENDING_MICROS.store(observation.maximum_tx_pending_micros, Ordering::Release);
    AP_MAXIMUM_RX_SERVICE_MICROS.store(observation.maximum_rx_service_micros, Ordering::Release);
    AP_MAXIMUM_NETWORK_BACKPRESSURE_MICROS.store(
        observation.maximum_network_backpressure_micros,
        Ordering::Release,
    );
    AP_AUTHENTICATIONS.store(observation.authentication_responses, Ordering::Release);
    AP_ASSOCIATIONS.store(observation.association_responses, Ordering::Release);
    AP_AUTHORIZATIONS.store(observation.authorized_peers, Ordering::Release);
    AP_REMOVALS.store(observation.peer_removals, Ordering::Release);
    AP_COMPLETED_RX_DESCRIPTORS.store(observation.completed_rx_descriptors, Ordering::Release);
    AP_IGNORED_RX_FRAMES.store(observation.ignored_rx_frames, Ordering::Release);
    AP_RX_MIC_FAILURES.store(observation.rx_mic_failures, Ordering::Release);
    AP_RX_QUARANTINED_FRAMES.store(observation.rx_quarantined_frames, Ordering::Release);
    AP_RX_VIEW_REJECTED.store(observation.rx_view_rejected, Ordering::Release);
    AP_CONTROL_FRAMES_STAGED.store(observation.control_frames_staged, Ordering::Release);
    AP_CONTROL_FRAMES_DROPPED_WHILE_BUSY.store(
        observation.control_frames_dropped_while_busy,
        Ordering::Release,
    );
    AP_ETHERNET_FRAMES_STAGED.store(observation.ethernet_frames_staged, Ordering::Release);
    AP_ETHERNET_ARP_REQUESTS_STAGED
        .store(observation.ethernet_arp_requests_staged, Ordering::Release);
    AP_ETHERNET_TCP_FRAMES_STAGED.store(observation.ethernet_tcp_frames_staged, Ordering::Release);
    AP_NETWORK_TX_FRAMES_OBSERVED.store(observation.network_tx_frames_observed, Ordering::Release);
    AP_NETWORK_TX_ARP_REQUESTS.store(observation.network_tx_arp_requests, Ordering::Release);
    AP_NETWORK_TX_ARP_REPLIES.store(observation.network_tx_arp_replies, Ordering::Release);
    AP_NETWORK_TX_REJECTED_NO_PEER
        .store(observation.network_tx_rejected_no_peer, Ordering::Release);
    AP_NETWORK_TX_REJECTED_DESTINATION.store(
        observation.network_tx_rejected_destination,
        Ordering::Release,
    );
    AP_NETWORK_TX_FRAMES_REJECTED.store(observation.network_tx_frames_rejected, Ordering::Release);
    AP_DATA_FRAMES_TRANSMITTED.store(observation.data_frames_transmitted, Ordering::Release);
    AP_TX_FAILURES.store(
        u32::from(observation.tx_hardware_failures)
            | (u32::from(observation.tx_hardware_timeouts) << 8)
            | (u32::from(observation.tx_collision_limits) << 16)
            | (u32::from(observation.tx_last_hardware_status) << 24),
        Ordering::Release,
    );
    AP_PROTECTED_DATA_FRAMES.store(observation.protected_data_frames, Ordering::Release);
    AP_PROTECTED_DATA_UNAUTHORIZED
        .store(observation.protected_data_unauthorized, Ordering::Release);
    AP_PROTECTED_DATA_FOREIGN.store(observation.protected_data_foreign, Ordering::Release);
    AP_PROTECTED_DATA_DUPLICATES.store(observation.protected_data_duplicates, Ordering::Release);
    AP_PROTECTED_DATA_RADIO_REJECTED
        .store(observation.protected_data_radio_rejected, Ordering::Release);
    AP_PROTECTED_DATA_PROTOCOL_REJECTED.store(
        observation.protected_data_protocol_rejected,
        Ordering::Release,
    );
}

fn access_point_evidence(generation: u32, requested_channel: u8) -> WifiAccessPointEvidence {
    let observed_channel = AP_CHANNEL.load(Ordering::Acquire) as u8;
    let tx_failures = AP_TX_FAILURES.load(Ordering::Acquire);
    WifiAccessPointEvidence {
        generation,
        channel: if observed_channel == 0 {
            requested_channel
        } else {
            observed_channel
        },
        beacons_transmitted: AP_BEACONS.load(Ordering::Acquire),
        missed_beacon_intervals: AP_MISSED_BEACON_INTERVALS.load(Ordering::Acquire),
        maximum_beacon_lateness_micros: AP_MAXIMUM_BEACON_LATENESS_MICROS.load(Ordering::Acquire),
        tx_interrupt_wakes: AP_TX_INTERRUPT_WAKES.load(Ordering::Acquire),
        tx_deadline_wakes: AP_TX_DEADLINE_WAKES.load(Ordering::Acquire),
        maximum_tx_pending_micros: AP_MAXIMUM_TX_PENDING_MICROS.load(Ordering::Acquire),
        maximum_rx_service_micros: AP_MAXIMUM_RX_SERVICE_MICROS.load(Ordering::Acquire),
        maximum_network_backpressure_micros: AP_MAXIMUM_NETWORK_BACKPRESSURE_MICROS
            .load(Ordering::Acquire),
        authentication_responses: AP_AUTHENTICATIONS.load(Ordering::Acquire),
        association_responses: AP_ASSOCIATIONS.load(Ordering::Acquire),
        authorized_peers: AP_AUTHORIZATIONS.load(Ordering::Acquire),
        peer_removals: AP_REMOVALS.load(Ordering::Acquire),
        completed_rx_descriptors: AP_COMPLETED_RX_DESCRIPTORS.load(Ordering::Acquire),
        ignored_rx_frames: AP_IGNORED_RX_FRAMES.load(Ordering::Acquire),
        rx_mic_failures: AP_RX_MIC_FAILURES.load(Ordering::Acquire),
        rx_quarantined_frames: AP_RX_QUARANTINED_FRAMES.load(Ordering::Acquire),
        rx_view_rejected: AP_RX_VIEW_REJECTED.load(Ordering::Acquire),
        control_frames_staged: AP_CONTROL_FRAMES_STAGED.load(Ordering::Acquire),
        control_frames_dropped_while_busy: AP_CONTROL_FRAMES_DROPPED_WHILE_BUSY
            .load(Ordering::Acquire),
        ethernet_frames_staged: AP_ETHERNET_FRAMES_STAGED.load(Ordering::Acquire),
        ethernet_arp_requests_staged: AP_ETHERNET_ARP_REQUESTS_STAGED.load(Ordering::Acquire),
        ethernet_tcp_frames_staged: AP_ETHERNET_TCP_FRAMES_STAGED.load(Ordering::Acquire),
        network_tx_frames_observed: AP_NETWORK_TX_FRAMES_OBSERVED.load(Ordering::Acquire),
        network_tx_arp_requests: AP_NETWORK_TX_ARP_REQUESTS.load(Ordering::Acquire),
        network_tx_arp_replies: AP_NETWORK_TX_ARP_REPLIES.load(Ordering::Acquire),
        network_tx_rejected_no_peer: AP_NETWORK_TX_REJECTED_NO_PEER.load(Ordering::Acquire),
        network_tx_rejected_destination: AP_NETWORK_TX_REJECTED_DESTINATION.load(Ordering::Acquire),
        network_tx_frames_rejected: AP_NETWORK_TX_FRAMES_REJECTED.load(Ordering::Acquire),
        data_frames_transmitted: AP_DATA_FRAMES_TRANSMITTED.load(Ordering::Acquire),
        tx_hardware_failures: tx_failures as u8,
        tx_hardware_timeouts: (tx_failures >> 8) as u8,
        tx_collision_limits: (tx_failures >> 16) as u8,
        tx_last_hardware_status: (tx_failures >> 24) as u8,
        protected_data_frames: AP_PROTECTED_DATA_FRAMES.load(Ordering::Acquire),
        protected_data_unauthorized: AP_PROTECTED_DATA_UNAUTHORIZED.load(Ordering::Acquire),
        protected_data_foreign: AP_PROTECTED_DATA_FOREIGN.load(Ordering::Acquire),
        protected_data_duplicates: AP_PROTECTED_DATA_DUPLICATES.load(Ordering::Acquire),
        protected_data_radio_rejected: AP_PROTECTED_DATA_RADIO_REJECTED.load(Ordering::Acquire),
        protected_data_protocol_rejected: AP_PROTECTED_DATA_PROTOCOL_REJECTED
            .load(Ordering::Acquire),
    }
}

#[derive(Clone, Copy, Debug)]
enum StationLinkEdge {
    Connected,
    Disconnected(StationDisconnectReason),
    AttemptFailed {
        attempt: u16,
        stage: open_esp_radio::wifi::sta::station::StaLifecycleStage,
    },
    RetryExhausted {
        attempts: u16,
        stage: open_esp_radio::wifi::sta::station::StaLifecycleStage,
    },
}

fn observe_station_lifecycle(observation: Esp32s31StationLifecycleObservation) {
    log_station_rx_frontier(observation);
    let edge = match observation {
        Esp32s31StationLifecycleObservation::Connected => StationLinkEdge::Connected,
        Esp32s31StationLifecycleObservation::Disconnected(reason) => {
            StationLinkEdge::Disconnected(match reason {
                ConnectedDisconnectReason::BeaconLoss => StationDisconnectReason::BeaconLoss,
                ConnectedDisconnectReason::PeerDeauthentication { reason_code } => {
                    StationDisconnectReason::PeerDeauthentication { reason_code }
                }
                ConnectedDisconnectReason::PeerDisassociation { reason_code } => {
                    StationDisconnectReason::PeerDisassociation { reason_code }
                }
                ConnectedDisconnectReason::ActiveStateRestoreFailed => {
                    StationDisconnectReason::ActiveStateRestoreFailed
                }
                ConnectedDisconnectReason::GroupKeyHandshakeFailed => {
                    StationDisconnectReason::GroupKeyHandshakeFailed
                }
            })
        }
        Esp32s31StationLifecycleObservation::AttemptFailed { attempt, stage } => {
            StationLinkEdge::AttemptFailed { attempt, stage }
        }
        Esp32s31StationLifecycleObservation::RetryExhausted { attempts, stage } => {
            StationLinkEdge::RetryExhausted { attempts, stage }
        }
    };
    STATION_LIFECYCLE
        .try_send(edge)
        .expect("qualification station lifecycle queue must not overflow");
}

fn log_station_rx_frontier(observation: Esp32s31StationLifecycleObservation) {
    let pipeline = RX_PIPELINE.snapshot();
    let irq = MAC_IRQ.snapshot();
    runtime_log(format_args!(
        "ORLC event={observation:?} irq_epochs={} irq_rx_only={} irq_rx_mixed={} \
         service_calls={} frontier={} admitted={} protocol_frames={} data_frames={} \
         network_enqueued={} network_dropped={} buffer_full={}",
        pipeline.rx_irq_epochs,
        irq.rx_only_entries,
        irq.rx_mixed_entries,
        pipeline.service_calls,
        pipeline.completion_frontier_frames,
        pipeline.admitted_frames,
        pipeline.protocol_frames,
        pipeline.protocol_data_frames,
        pipeline.network_enqueued,
        pipeline.network_dropped,
        pipeline.dma_buffer_full_increments,
    ));
}

// Qualification observers execute on the RX/TX hot paths. Their atomics stay
// in internal SRAM so measuring a production image does not introduce PSRAM
// cache traffic into the path being measured.
#[unsafe(link_section = ".critical.data.open_radio_rx_telemetry")]
pub(crate) static RX_PIPELINE: RxPipelineCounters = RxPipelineCounters::new(now_micros);
#[unsafe(link_section = ".critical.bss.open_radio_tx_telemetry")]
pub(crate) static AGGREGATE_TX: AggregateTxCounters = AggregateTxCounters::new();
#[unsafe(link_section = ".critical.bss.open_radio_rx_telemetry")]
pub(crate) static MAC_IRQ: MacIrqClassificationCounters = MacIrqClassificationCounters::new();
#[unsafe(link_section = ".critical.bss.open_radio_task_poll_telemetry")]
pub(crate) static TASK_POLLS: TaskPollSet = TaskPollSet::new();
#[cfg(feature = "network-scheduler-telemetry")]
#[unsafe(link_section = ".critical.bss.open_radio_network_scheduler_telemetry")]
pub(crate) static NETWORK_SCHEDULER: NetworkSchedulerCounters = NetworkSchedulerCounters::new();

fn now_micros() -> u64 {
    Instant::now().as_micros()
}

fn protocol_observed<T>(evidence: MacRxEvidence<T>) -> Option<WifiMonitorObserved<T>> {
    match evidence {
        MacRxEvidence::HardwareObserved(value) => Some(WifiMonitorObserved {
            source: WifiMonitorEvidenceSource::Hardware,
            value,
        }),
        MacRxEvidence::ProtocolValidated(value) => Some(WifiMonitorObserved {
            source: WifiMonitorEvidenceSource::Protocol,
            value,
        }),
        MacRxEvidence::Unavailable => None,
    }
}

fn protocol_rate(
    evidence: MacRxEvidence<open_esp_radio::esp32s31::wifi::mac::rx::RxPhyInfo>,
) -> Option<WifiMonitorObserved<WifiMonitorPhyEvidence>> {
    protocol_observed(evidence).map(|observed| WifiMonitorObserved {
        source: observed.source,
        value: WifiMonitorPhyEvidence {
            format: match observed.value.baseband_format() {
                RxBasebandFormat::Dot11b => WifiMonitorPhyFormat::Dot11b,
                RxBasebandFormat::Ofdm => WifiMonitorPhyFormat::Ofdm,
                RxBasebandFormat::Ht => WifiMonitorPhyFormat::Ht,
                RxBasebandFormat::Vht => WifiMonitorPhyFormat::Vht,
                RxBasebandFormat::HeSu => WifiMonitorPhyFormat::HeSu,
                RxBasebandFormat::HeMu => WifiMonitorPhyFormat::HeMu,
                RxBasebandFormat::HeExtendedRangeSu => WifiMonitorPhyFormat::HeExtendedRangeSu,
                RxBasebandFormat::HeTriggerBased => WifiMonitorPhyFormat::HeTriggerBased,
                RxBasebandFormat::VhtMu => WifiMonitorPhyFormat::VhtMu,
                RxBasebandFormat::Unknown(raw) => WifiMonitorPhyFormat::Unknown(raw),
            },
            hardware_rate_code: observed.value.rate,
            he_siga1: observed.value.he_siga1,
            he_siga2: observed.value.he_siga2,
        },
    })
}

struct ExportedMonitorFrame {
    captured_bytes: u64,
    generation_mismatch: bool,
    channel_mismatch: bool,
    channel_unavailable: bool,
    last_observed_channel: u8,
}

async fn export_monitor_frame(
    request_id: u32,
    generation: u32,
    frame_sequence: u32,
    requested_channel: u8,
    frame: &Esp32s31MonitorFrame,
) -> ExportedMonitorFrame {
    let channel = protocol_observed(frame.metadata().rx.channel);
    let (channel_mismatch, channel_unavailable, last_observed_channel) = match channel {
        Some(observed) => (observed.value != requested_channel, false, observed.value),
        None => (false, true, 0),
    };
    let rssi_dbm = protocol_observed(frame.metadata().rx.rssi_dbm);
    let rate = protocol_rate(frame.metadata().rx.rate);
    let captured_length = u16::try_from(frame.captured_length())
        .expect("monitor frame fits the configured capture slot");
    let logical_length = u16::try_from(frame.metadata().logical_length.min(u16::MAX as usize))
        .expect("bounded logical length fits u16");
    let dequeued_at_micros = now_micros();
    for (index, bytes) in frame
        .bytes()
        .chunks(WIFI_MONITOR_FRAME_CHUNK_MAX_LEN)
        .enumerate()
    {
        let offset = u16::try_from(index * WIFI_MONITOR_FRAME_CHUNK_MAX_LEN)
            .expect("capture offset fits u16");
        let chunk = WifiMonitorFrameChunk::try_new(
            generation,
            frame_sequence,
            dequeued_at_micros,
            captured_length,
            logical_length,
            offset,
            channel,
            rssi_dbm,
            rate,
            bytes,
        )
        .expect("bounded monitor chunk fits the HIL protocol");
        publish_monitor_frame(request_id, chunk).await;
    }
    ExportedMonitorFrame {
        captured_bytes: u64::from(captured_length),
        generation_mismatch: frame.metadata().generation != generation,
        channel_mismatch,
        channel_unavailable,
        last_observed_channel,
    }
}

async fn run_finite_monitor_capture(
    idle: Esp32s31WifiControl,
    monitor_frames: &Esp32s31MonitorFrames,
    request_id: u32,
    request: WifiMonitorCaptureRequest,
) -> Esp32s31WifiControl {
    let mut monitor_request = MonitorRequest::new(
        WifiChannel::mhz20(request.channel).expect("console validates the monitor channel"),
        WifiMonitorConfig::normalized(),
    );
    if let Some(snapshot_length) = NonZeroU16::new(request.snapshot_length) {
        monitor_request =
            monitor_request.with_capture_policy(MonitorCapturePolicy::truncate_at(snapshot_length));
    }
    let owner = idle
        .start_monitor(monitor_request)
        .await
        .unwrap_or_else(|error| panic!("production finite monitor start failed: {error:?}"));
    let generation = owner.generation().value();
    let capture_started = Instant::now();
    set_wifi_role(WifiRole::Monitor);
    complete_monitor_start(
        request_id,
        WifiRoleTransitionEvidence {
            previous: WifiRole::Idle,
            current: WifiRole::Monitor,
            generation,
        },
    )
    .await;

    let deadline = Timer::after_millis(u64::from(request.duration_millis));
    let mut deadline = core::pin::pin!(deadline);
    let mut captured_frames = 0_u32;
    let mut captured_bytes = 0_u64;
    let mut generation_mismatches = 0_u32;
    let mut channel_mismatches = 0_u32;
    let mut channel_unavailable = 0_u32;
    let mut last_observed_channel = 0_u8;
    loop {
        match select(deadline.as_mut(), monitor_frames.receive()).await {
            Either::First(()) => break,
            Either::Second(frame) => {
                let observation = export_monitor_frame(
                    request_id,
                    generation,
                    captured_frames,
                    request.channel,
                    &frame,
                )
                .await;
                captured_frames = captured_frames.saturating_add(1);
                captured_bytes = captured_bytes.saturating_add(observation.captured_bytes);
                generation_mismatches = generation_mismatches
                    .saturating_add(u32::from(observation.generation_mismatch));
                channel_mismatches =
                    channel_mismatches.saturating_add(u32::from(observation.channel_mismatch));
                channel_unavailable =
                    channel_unavailable.saturating_add(u32::from(observation.channel_unavailable));
                if !observation.channel_unavailable {
                    last_observed_channel = observation.last_observed_channel;
                }
            }
        }
    }
    let elapsed_micros = capture_started.elapsed().as_micros();
    let idle = owner
        .stop()
        .await
        .unwrap_or_else(|error| panic!("production finite monitor stop failed: {error:?}"));
    let statistics = monitor_frames.statistics();
    set_wifi_role(WifiRole::Idle);
    complete_monitor_capture(
        request_id,
        WifiMonitorEvidence {
            generation,
            elapsed_micros,
            channel: request.channel,
            captured_frames,
            captured_bytes,
            generation_mismatches,
            channel_mismatches,
            channel_unavailable,
            last_observed_channel,
            published_frames: statistics.published_frames,
            full_drops: statistics.full_drops,
            oversized_drops: statistics.oversized_drops,
            discarded_frames: statistics.discarded_frames,
            exported_frames: captured_frames,
        },
    )
    .await;
    idle
}

fn observe_mac_irq(observation: Esp32s31MacIrqObservation) {
    match observation {
        Esp32s31MacIrqObservation::RxEpoch => RX_PIPELINE.record_rx_irq_epoch(),
        Esp32s31MacIrqObservation::TxEpoch => AGGREGATE_TX.record_tx_irq_epoch(now_micros),
        Esp32s31MacIrqObservation::Entry {
            first_status,
            observed_status,
            nonzero_snapshots,
        } => MAC_IRQ.record(first_status, observed_status, u32::from(nonzero_snapshots)),
    }
}

fn observe_protocol_task_poll(elapsed_micros: u64) {
    if OPEN_RADIO_TASK_POLL_TELEMETRY {
        TASK_POLLS.protocol().record(elapsed_micros);
    }
}

#[cfg(feature = "network-scheduler-telemetry")]
fn observe_network_scheduler(report: embassy_net::CooperativePollReport) {
    NETWORK_SCHEDULER.record(report);
}

pub fn diagnostic_snapshot() -> (u32, u32) {
    (DIAGNOSTIC_STAGE.load(Ordering::Acquire), 0)
}

pub const fn hil_capabilities() -> Capabilities {
    Capabilities {
        features: FeatureCapabilities {
            udp: true,
            tcp: true,
            rx: true,
            tx: true,
            bidirectional: true,
            runtime_initialization: true,
            runtime_configuration: true,
            structured_evidence: true,
            startup_artifact: true,
            station_epoch_control: true,
            wifi_role_control: true,
            wifi_access_point: true,
            wifi_monitor_capture: true,
            station_lifecycle_events: true,
            rx_delivery_evidence: OPEN_RADIO_RX_DELIVERY_TELEMETRY,
            task_poll_evidence: OPEN_RADIO_TASK_POLL_TELEMETRY,
            network_scheduler_evidence: OPEN_RADIO_NETWORK_SCHEDULER_TELEMETRY,
            data_plane_placement: true,
            timebase_probe: true,
        },
        maximum_payload_bytes: OPEN_RADIO_TCP_CHUNK_CAPACITY as u16,
        maximum_wire_frame_bytes: MAX_WIRE_FRAME_BYTES as u16,
    }
}

#[embassy_executor::task]
async fn radio_runner_task(runner: Esp32s31RadioRunner) {
    observe_open_radio_task_polls(
        runner.run(),
        TASK_POLLS.radio(),
        OPEN_RADIO_TASK_POLL_TELEMETRY,
    )
    .await
}

#[embassy_executor::task]
async fn qualification_snapshot_task(snapshot: Esp32s31QualificationSnapshot) {
    loop {
        let requester = QUALIFICATION_REQUESTS.receive().await;
        let sample = QualificationSample {
            rx_primary: snapshot
                .rx_statistics()
                .map(|statistics| statistics.primary),
            rx_interrupt_posts: snapshot.rx_interrupt_posts(),
            tx_vector: snapshot.tx_vector(),
        };
        match requester {
            QualificationRequester::UdpRx => UDP_RX_QUALIFICATION.send(sample).await,
            QualificationRequester::UdpTx => UDP_TX_QUALIFICATION.send(sample).await,
            QualificationRequester::Tcp => TCP_QUALIFICATION.send(sample).await,
        }
    }
}

pub(in crate::product_hil) async fn qualification_sample(
    requester: QualificationRequester,
) -> QualificationSample {
    QUALIFICATION_REQUESTS.send(requester).await;
    match requester {
        QualificationRequester::UdpRx => UDP_RX_QUALIFICATION.receive().await,
        QualificationRequester::UdpTx => UDP_TX_QUALIFICATION.receive().await,
        QualificationRequester::Tcp => TCP_QUALIFICATION.receive().await,
    }
}

type ProductNetworkRunner = embassy_net::Runner<'static, Esp32s31WifiDevice>;

#[embassy_executor::task]
async fn network_runner_task(mut runner: ProductNetworkRunner) {
    let policy = embassy_net::CooperativeConfig::new(Duration::from_micros(750));
    #[cfg(feature = "network-scheduler-telemetry")]
    let policy = policy.with_observer(observe_network_scheduler);
    observe_open_radio_task_polls(
        runner.run_cooperative(policy),
        TASK_POLLS.network(),
        OPEN_RADIO_TASK_POLL_TELEMETRY,
    )
    .await
}

#[embassy_executor::task]
async fn network_config_task(stack: Stack<'static>) {
    loop {
        let configuration = NETWORK_CONFIG_REQUESTS.receive().await;
        stack.set_config_v4(network_config_v4(configuration));
        NETWORK_CONFIG_APPLIED.send(()).await;
    }
}

/// CPU1-local network composition used only by the explicit split topology.
#[embassy_executor::task]
pub(crate) async fn secondary_network_task(spawner: Spawner) {
    let AppNetworkStart { device, ipv4, seed } = APP_NETWORK_START.receive().await;
    run_network_composition(spawner, device, ipv4, seed).await
}

/// CPU0-local network composition used by the default single-core topology.
#[embassy_executor::task]
async fn primary_network_task(spawner: Spawner) {
    let AppNetworkStart { device, ipv4, seed } = PRIMARY_NETWORK_START.receive().await;
    run_network_composition(spawner, device, ipv4, seed).await
}

async fn run_network_composition(
    spawner: Spawner,
    device: Esp32s31WifiDevice,
    ipv4: NetworkIpv4Configuration,
    seed: u64,
) -> ! {
    let (stack, network_runner) =
        embassy_net::new(device, network_config(ipv4), NETWORK_RESOURCES.take(), seed);
    spawner.spawn(
        network_runner_task(network_runner).expect("network runner task must allocate once"),
    );
    spawner
        .spawn(network_config_task(stack).expect("network configuration task must allocate once"));
    spawner.spawn(network_report_task(stack).expect("network report task must allocate once"));
    start_connected_traffic(spawner, stack);
    APP_NETWORK_READY.signal(());
    core::future::pending().await
}

async fn apply_network_config(configuration: NetworkIpv4Configuration) {
    NETWORK_CONFIG_REQUESTS.send(configuration).await;
    NETWORK_CONFIG_APPLIED.receive().await;
}

#[embassy_executor::task]
async fn network_report_task(stack: Stack<'static>) {
    report_network(stack).await
}

#[embassy_executor::task]
async fn station_lifecycle_task() {
    let mut generation = 0_u32;
    let mut connected = false;
    loop {
        match STATION_LIFECYCLE.receive().await {
            StationLinkEdge::Connected => {
                if !connected {
                    publish_station_lifecycle(StationLifecycleEvent::Connected { generation })
                        .await;
                    connected = true;
                }
            }
            StationLinkEdge::Disconnected(reason) => {
                if connected {
                    publish_station_lifecycle(StationLifecycleEvent::Disconnected {
                        generation,
                        reason,
                    })
                    .await;
                    generation = generation.wrapping_add(1);
                    connected = false;
                }
            }
            StationLinkEdge::AttemptFailed { attempt, stage } => {
                publish_station_lifecycle(StationLifecycleEvent::AttemptFailed {
                    generation,
                    attempt,
                    stage: hil_failure_stage(stage),
                    reason: hil_failure_reason(stage),
                })
                .await;
            }
            StationLinkEdge::RetryExhausted { attempts, stage } => {
                publish_station_lifecycle(StationLifecycleEvent::RetryExhausted {
                    generation,
                    attempts,
                    stage: hil_failure_stage(stage),
                    reason: hil_failure_reason(stage),
                })
                .await;
            }
        }
    }
}

const fn hil_failure_stage(
    stage: open_esp_radio::wifi::sta::station::StaLifecycleStage,
) -> StationFailureStage {
    use open_esp_radio::wifi::sta::station::StaLifecycleStage as DriverStage;
    match stage {
        DriverStage::CandidateSelection => StationFailureStage::CandidateSelection,
        DriverStage::Authentication => StationFailureStage::Authentication,
        DriverStage::Association => StationFailureStage::Association,
        DriverStage::Security => StationFailureStage::Security,
        DriverStage::Connected => StationFailureStage::Connected,
        DriverStage::Hardware => StationFailureStage::Hardware,
    }
}

const fn hil_failure_reason(
    stage: open_esp_radio::wifi::sta::station::StaLifecycleStage,
) -> StationAttemptFailureReason {
    use open_esp_radio::wifi::sta::station::StaLifecycleStage as DriverStage;
    match stage {
        DriverStage::CandidateSelection => StationAttemptFailureReason::NoCandidate,
        DriverStage::Authentication | DriverStage::Association | DriverStage::Security => {
            StationAttemptFailureReason::PeerProtocol
        }
        DriverStage::Connected | DriverStage::Hardware => StationAttemptFailureReason::Hardware,
    }
}

fn network_config(configuration: NetworkIpv4Configuration) -> NetworkConfig {
    match network_config_v4(configuration) {
        ConfigV4::Dhcp(config) => NetworkConfig::dhcpv4(config),
        ConfigV4::Static(config) => NetworkConfig::ipv4_static(config),
        ConfigV4::None => NetworkConfig::default(),
    }
}

fn network_config_v4(configuration: NetworkIpv4Configuration) -> ConfigV4 {
    match configuration {
        NetworkIpv4Configuration::Dhcp => ConfigV4::Dhcp(Default::default()),
        NetworkIpv4Configuration::Static {
            address,
            prefix_length,
            gateway,
        } => ConfigV4::Static(StaticConfigV4 {
            address: Ipv4Cidr::new(Ipv4Address::from_octets(address), prefix_length),
            gateway: gateway.map(Ipv4Address::from_octets),
            dns_servers: Default::default(),
        }),
    }
}

fn station_request(ssid: &[u8], passphrase: &[u8]) -> StationRequest {
    let ssid = WifiSsid::new(ssid).expect("validated HIL SSID must fit the driver request");
    let pmk = Pmk::derive(passphrase, ssid.as_bytes())
        .expect("validated HIL WPA2 credentials must derive a PMK");
    StationRequest::new(
        ssid,
        StationSecurity::wpa2_personal(pmk),
        StaReconnectPolicy::new(3, 100, 1_000, 100).expect("fixed HIL reconnect policy is valid"),
        StationScanPolicy::new(
            StationScanChannels::CHANNELS_1_TO_13,
            NonZeroU16::new(SCAN_DWELL_MS).expect("scan dwell is nonzero"),
            StaAssociationPreference::PreferHe20,
        ),
    )
}

fn access_point_request(
    request: &open_esp_radio_hil_protocol::WifiAccessPointRequest,
) -> AccessPointRequest {
    let ssid = WifiSsid::new(request.credentials.ssid())
        .expect("validated HIL AP SSID must fit the driver request");
    let pmk = Pmk::derive(request.credentials.passphrase(), ssid.as_bytes())
        .expect("validated HIL AP credentials must derive a PMK");
    AccessPointRequest::new(
        ssid,
        AccessPointSecurity::wpa2_personal(pmk),
        WifiChannel::mhz20(request.channel).expect("console validates the AP channel"),
    )
    .expect("validated HIL AP request satisfies the production AP contract")
}

async fn report_network(stack: Stack<'static>) -> ! {
    stack.wait_config_up().await;
    loop {
        if let Some(config) = stack.config_v4() {
            publish_event_reliably(
                0,
                0,
                HilEvent::NetworkReady(NetworkInfo {
                    address: config.address.address().octets(),
                    prefix_length: config.address.prefix_len(),
                    gateway: config.gateway.map(|address| address.octets()),
                }),
            )
            .await;
            runtime_log(format_args!(
                "OPEN_RADIO_HIL result=PASS stage=network-ready address={} gateway={:?}",
                config.address, config.gateway,
            ));
            loop {
                Timer::after_secs(60).await;
            }
        }
        Timer::after_millis(10).await;
    }
}

pub async fn run(
    spawner: Spawner,
    secondary_core_spawner: SendSpawner,
    platform: EspHalRadioPeripheral,
    trng: Trng,
) {
    DIAGNOSTIC_STAGE.store(10, Ordering::Release);
    spawner.spawn(station_lifecycle_task().expect("station lifecycle task must allocate once"));
    let crate::console::StartupConfiguration {
        request_id: initialization_request_id,
        ipv4: startup_ipv4,
        data_plane,
        phy_calibration_artifact,
    } = crate::console::receive_startup_configuration().await;
    let primary_core_spawner = spawner.make_send();
    let network_worker_spawner = match data_plane {
        WifiDataPlanePlacement::SingleCore => primary_core_spawner,
        WifiDataPlanePlacement::SplitRadioNetwork => secondary_core_spawner,
    };
    network_worker_spawner
        .spawn(tcp_rx_pattern_worker_task().expect("TCP RX pattern task must allocate once"));
    network_worker_spawner
        .spawn(tcp_tx_pattern_worker_task().expect("TCP TX pattern task must allocate once"));
    let efuse_registers = esp_hal::peripherals::EFUSE::regs();
    let mut station_address = [0; 6];
    station_address
        .copy_from_slice(efuse::interface_mac_address(InterfaceMacAddress::Station).as_bytes());
    let mut access_point_address = [0; 6];
    access_point_address
        .copy_from_slice(efuse::interface_mac_address(InterfaceMacAddress::AccessPoint).as_bytes());
    let station_mac = WifiMacAddress::new(station_address)
        .expect("ESP32-S31 station eFuse address must be unicast");
    let access_point_mac = WifiMacAddress::new(access_point_address)
        .expect("ESP32-S31 AP eFuse address must be unicast");
    let connected_rx_observer = CONNECTED_RX_OBSERVER.take();
    let config = Esp32s31RadioConfig::new(
        station_mac,
        access_point_mac,
        PhyCalibrationIdentity {
            rf_cal_version: phy_get_rf_cal_version(),
            mac_sys0: efuse_registers.rd_mac_sys0().read().bits(),
            mac_sys1: efuse_registers.rd_mac_sys1().read().bits(),
        },
        WifiChannel::mhz20(1).expect("initial channel is valid"),
    )
    .with_maximum_tx_power_quarter_dbm(MAXIMUM_TX_POWER_QUARTER_DBM)
    .with_qualification_hooks(Esp32s31QualificationHooks {
        rx_pipeline: &RX_PIPELINE,
        aggregate_tx: &AGGREGATE_TX,
        connected_rx: connected_rx_observer,
        rx_delivery: {
            #[cfg(feature = "rx-delivery-telemetry")]
            {
                Some(connected_rx_observer)
            }
            #[cfg(not(feature = "rx-delivery-telemetry"))]
            {
                None
            }
        },
        mac_irq: observe_mac_irq,
        protocol_task_poll: observe_protocol_task_poll,
        station_lifecycle: observe_station_lifecycle,
        access_point: observe_access_point,
    });
    let artifact_was_supplied = phy_calibration_artifact.is_some();
    let calibration_cache = phy_calibration_artifact
        .as_ref()
        .and_then(|artifact| crate::phy_calibration_artifact::decode(artifact.bytes()));
    let config = match calibration_cache {
        Some(cache) => config.with_calibration_cache(cache),
        None => config,
    };

    let started_at = Instant::now();
    let (radio, runner) =
        open_esp_radio_esp32s31_embassy_wifi::new(primary_core_spawner, platform, trng, config)
            .await
            .unwrap_or_else(|error| panic!("production radio initialization failed: {error:?}"));
    let Esp32s31RadioParts {
        wifi,
        initialization,
    } = radio.into_parts();
    // The runner is an idle supervisor until a role command arrives. Spawn it
    // before the asynchronous artifact publication so its large unique owner
    // graph is moved into the task arena instead of retained in this future.
    spawner
        .spawn(radio_runner_task(runner).expect("production radio runner task must allocate once"));
    if let Some(cache) = initialization.calibration_cache {
        let disposition = match initialization.start.wifi.registration.calibration_path {
            PhyCalibrationPath::FullAfterRejectedCache => StartupArtifactDisposition::Replaced,
            PhyCalibrationPath::FullForCache if artifact_was_supplied => {
                // Transport-valid bytes can still carry an unknown HIL wire
                // schema. They are untrusted input, so a decode rejection
                // falls back to full calibration and replaces the artifact.
                StartupArtifactDisposition::Replaced
            }
            PhyCalibrationPath::FullForCache | PhyCalibrationPath::FullUncached => {
                StartupArtifactDisposition::Created
            }
        };
        let encoded =
            crate::phy_calibration_artifact::encode(&cache, PHY_CALIBRATION_ARTIFACT.take())
                .expect("typed PHY calibration artifact exceeds its explicit HIL storage budget");
        publish_startup_artifact(disposition, started_at.elapsed().as_micros(), encoded).await;
    }
    let Esp32s31WifiParts {
        control,
        device,
        monitor_frames,
        qualification,
    } = wifi.into_parts();
    spawner.spawn(
        qualification_snapshot_task(qualification)
            .expect("qualification snapshot task must allocate once"),
    );
    let seed = u64::from_le_bytes([
        station_address[0],
        station_address[1],
        station_address[2],
        station_address[3],
        station_address[4],
        station_address[5],
        0xa5,
        0x31,
    ]);
    let network_start = AppNetworkStart {
        device,
        ipv4: startup_ipv4,
        seed,
    };
    match data_plane {
        WifiDataPlanePlacement::SingleCore => {
            spawner.spawn(
                primary_network_task(spawner)
                    .expect("primary-core network task must allocate once"),
            );
            PRIMARY_NETWORK_START.send(network_start).await;
        }
        WifiDataPlanePlacement::SplitRadioNetwork => {
            APP_NETWORK_START.send(network_start).await;
        }
    }
    APP_NETWORK_READY.wait().await;
    runtime_log(format_args!(
        "OPEN_RADIO_HIL data_plane={data_plane:?} radio_core=0 protocol_core=0 network_core={}",
        match data_plane {
            WifiDataPlanePlacement::SingleCore => 0,
            WifiDataPlanePlacement::SplitRadioNetwork => 1,
        }
    ));

    spawner.spawn(
        wifi_role_task(
            control,
            monitor_frames,
            startup_ipv4,
            initialization_request_id,
        )
        .expect("Wi-Fi role owner task must allocate once"),
    );
}

#[embassy_executor::task]
async fn wifi_role_task(
    control: Esp32s31WifiControl,
    monitor_frames: Esp32s31MonitorFrames,
    station_ipv4: NetworkIpv4Configuration,
    initialization_request_id: u32,
) -> ! {
    enum ProductWifiRole<P> {
        Idle(open_esp_radio::WifiIdle<P>),
        Station(open_esp_radio::WifiStation<P>),
        AccessPoint {
            owner: open_esp_radio::WifiAccessPoint<P>,
            channel: u8,
        },
        Monitor {
            owner: open_esp_radio::WifiMonitor<P>,
            channel: u8,
            started_at_micros: u64,
            captured_frames: u32,
            captured_bytes: u64,
            generation_mismatches: u32,
            channel_mismatches: u32,
            channel_unavailable: u32,
            last_observed_channel: u8,
        },
    }

    DIAGNOSTIC_STAGE.store(20, Ordering::Release);
    set_wifi_role(WifiRole::Idle);
    complete_initialization(initialization_request_id).await;
    let mut credentials = None::<NetworkCredentials>;
    let mut role = ProductWifiRole::Idle(control);
    loop {
        role = match role {
            ProductWifiRole::AccessPoint { owner, channel } => {
                let request_id = match receive_wifi_control_request().await {
                    WifiControlRequest::StopAccessPoint { request_id } => request_id,
                    _ => unreachable!(
                        "console admits only AP stop while the access point owns Wi-Fi"
                    ),
                };
                let generation = owner.generation().value();
                match await_stack_boundary!(owner.stop()) {
                    Ok(idle) => {
                        apply_network_config(station_ipv4).await;
                        set_wifi_role(WifiRole::Idle);
                        complete_access_point_stop(
                            request_id,
                            access_point_evidence(generation, channel),
                        )
                        .await;
                        ProductWifiRole::Idle(idle)
                    }
                    Err(error) => {
                        let reason = match error {
                            WifiRoleStopFailure::GenerationMismatch => {
                                WifiRoleFailureReason::GenerationMismatch
                            }
                            WifiRoleStopFailure::Faulted(_) => WifiRoleFailureReason::HardwareFault,
                        };
                        complete_wifi_role_failure(
                            request_id,
                            WifiRoleFailureEvidence {
                                role: WifiRole::AccessPoint,
                                operation: WifiRoleOperation::Stop,
                                reason,
                            },
                        )
                        .await;
                        core::future::pending().await
                    }
                }
            }
            ProductWifiRole::Station(station) => match receive_wifi_control_request().await {
                WifiControlRequest::Cycle { request_id } => {
                    let idle = await_stack_boundary!(station.stop()).unwrap_or_else(|error| {
                        panic!("production station stop failed: {error:?}")
                    });
                    STATION_LIFECYCLE
                        .send(StationLinkEdge::Disconnected(
                            StationDisconnectReason::ReconnectRequested,
                        ))
                        .await;
                    let station = await_stack_boundary!(
                        idle.start_station(station_request(
                            credentials
                                .as_ref()
                                .expect("station role retains its credentials")
                                .ssid(),
                            credentials
                                .as_ref()
                                .expect("station role retains its credentials")
                                .passphrase(),
                        ))
                    )
                    .unwrap_or_else(|error| panic!("production station restart failed: {error:?}"));
                    complete_station_epoch_cycle(request_id, StationEpochEvidence::COMPLETE).await;
                    ProductWifiRole::Station(station)
                }
                WifiControlRequest::StopStation { request_id } => {
                    let stopped_generation = station.generation().value();
                    let idle = await_stack_boundary!(station.stop()).unwrap_or_else(|error| {
                        panic!("production station stop failed: {error:?}")
                    });
                    STATION_LIFECYCLE
                        .send(StationLinkEdge::Disconnected(
                            StationDisconnectReason::LinkPolicy,
                        ))
                        .await;
                    set_wifi_role(WifiRole::Idle);
                    complete_wifi_role_transition(
                        request_id,
                        WifiRoleTransitionEvidence {
                            previous: WifiRole::Station,
                            current: WifiRole::Idle,
                            generation: stopped_generation,
                        },
                    )
                    .await;
                    ProductWifiRole::Idle(idle)
                }
                _ => unreachable!("console admits only station commands while station owns Wi-Fi"),
            },
            ProductWifiRole::Idle(idle) => match receive_wifi_control_request().await {
                WifiControlRequest::StartAccessPoint {
                    request_id,
                    request,
                } => {
                    AP_CHANNEL.store(0, Ordering::Release);
                    AP_BEACONS.store(0, Ordering::Release);
                    AP_MISSED_BEACON_INTERVALS.store(0, Ordering::Release);
                    AP_MAXIMUM_BEACON_LATENESS_MICROS.store(0, Ordering::Release);
                    AP_TX_INTERRUPT_WAKES.store(0, Ordering::Release);
                    AP_TX_DEADLINE_WAKES.store(0, Ordering::Release);
                    AP_MAXIMUM_TX_PENDING_MICROS.store(0, Ordering::Release);
                    AP_AUTHENTICATIONS.store(0, Ordering::Release);
                    AP_ASSOCIATIONS.store(0, Ordering::Release);
                    AP_AUTHORIZATIONS.store(0, Ordering::Release);
                    AP_REMOVALS.store(0, Ordering::Release);
                    AP_COMPLETED_RX_DESCRIPTORS.store(0, Ordering::Release);
                    AP_IGNORED_RX_FRAMES.store(0, Ordering::Release);
                    AP_RX_MIC_FAILURES.store(0, Ordering::Release);
                    AP_RX_QUARANTINED_FRAMES.store(0, Ordering::Release);
                    AP_RX_VIEW_REJECTED.store(0, Ordering::Release);
                    AP_CONTROL_FRAMES_STAGED.store(0, Ordering::Release);
                    AP_CONTROL_FRAMES_DROPPED_WHILE_BUSY.store(0, Ordering::Release);
                    AP_ETHERNET_FRAMES_STAGED.store(0, Ordering::Release);
                    AP_ETHERNET_ARP_REQUESTS_STAGED.store(0, Ordering::Release);
                    AP_ETHERNET_TCP_FRAMES_STAGED.store(0, Ordering::Release);
                    AP_NETWORK_TX_FRAMES_OBSERVED.store(0, Ordering::Release);
                    AP_NETWORK_TX_ARP_REQUESTS.store(0, Ordering::Release);
                    AP_NETWORK_TX_ARP_REPLIES.store(0, Ordering::Release);
                    AP_NETWORK_TX_REJECTED_NO_PEER.store(0, Ordering::Release);
                    AP_NETWORK_TX_REJECTED_DESTINATION.store(0, Ordering::Release);
                    AP_NETWORK_TX_FRAMES_REJECTED.store(0, Ordering::Release);
                    AP_DATA_FRAMES_TRANSMITTED.store(0, Ordering::Release);
                    AP_TX_FAILURES.store(0, Ordering::Release);
                    AP_PROTECTED_DATA_FRAMES.store(0, Ordering::Release);
                    AP_PROTECTED_DATA_UNAUTHORIZED.store(0, Ordering::Release);
                    AP_PROTECTED_DATA_FOREIGN.store(0, Ordering::Release);
                    AP_PROTECTED_DATA_DUPLICATES.store(0, Ordering::Release);
                    AP_PROTECTED_DATA_RADIO_REJECTED.store(0, Ordering::Release);
                    AP_PROTECTED_DATA_PROTOCOL_REJECTED.store(0, Ordering::Release);
                    let channel = request.channel;
                    match await_stack_boundary!(
                        idle.start_access_point(access_point_request(&request))
                    ) {
                        Ok(owner) => {
                            apply_network_config(request.ipv4).await;
                            set_wifi_role(WifiRole::AccessPoint);
                            complete_access_point_start(
                                request_id,
                                WifiRoleTransitionEvidence {
                                    previous: WifiRole::Idle,
                                    current: WifiRole::AccessPoint,
                                    generation: owner.generation().value(),
                                },
                            )
                            .await;
                            ProductWifiRole::AccessPoint { owner, channel }
                        }
                        Err(WifiRoleStartFailure::Rejected {
                            wifi,
                            request: _,
                            error: _,
                        }) => {
                            complete_wifi_role_failure(
                                request_id,
                                WifiRoleFailureEvidence {
                                    role: WifiRole::AccessPoint,
                                    operation: WifiRoleOperation::Start,
                                    reason: WifiRoleFailureReason::Rejected,
                                },
                            )
                            .await;
                            ProductWifiRole::Idle(wifi)
                        }
                        Err(WifiRoleStartFailure::Faulted { error: _ }) => {
                            complete_wifi_role_failure(
                                request_id,
                                WifiRoleFailureEvidence {
                                    role: WifiRole::AccessPoint,
                                    operation: WifiRoleOperation::Start,
                                    reason: WifiRoleFailureReason::HardwareFault,
                                },
                            )
                            .await;
                            core::future::pending().await
                        }
                    }
                }
                WifiControlRequest::StartStation {
                    request_id,
                    credentials: requested_credentials,
                } => {
                    let station = await_stack_boundary!(idle.start_station(station_request(
                        requested_credentials.ssid(),
                        requested_credentials.passphrase(),
                    )))
                    .unwrap_or_else(|error| panic!("production station start failed: {error:?}"));
                    credentials = Some(requested_credentials);
                    DIAGNOSTIC_STAGE.store(30, Ordering::Release);
                    set_wifi_role(WifiRole::Station);
                    complete_wifi_role_transition(
                        request_id,
                        WifiRoleTransitionEvidence {
                            previous: WifiRole::Idle,
                            current: WifiRole::Station,
                            generation: station.generation().value(),
                        },
                    )
                    .await;
                    ProductWifiRole::Station(station)
                }
                WifiControlRequest::Scan {
                    request_id,
                    request,
                } => {
                    let mut channels = [0_u8; 13];
                    let mut channel_count = 0_usize;
                    for channel in 1_u8..=13 {
                        if request.channel_mask_2_4_ghz & (1_u16 << (channel - 1)) != 0 {
                            channels[channel_count] = channel;
                            channel_count += 1;
                        }
                    }
                    let scan_channels =
                        StationScanChannels::from_primary_channels(&channels[..channel_count])
                            .expect("console validates the scan channel mask");
                    let started_at = Instant::now();
                    let completed = idle
                        .scan(DriverWifiScanRequest::new(
                            scan_channels,
                            NonZeroU16::new(request.dwell_millis)
                                .expect("console validates nonzero scan dwell"),
                        ))
                        .await
                        .unwrap_or_else(|error| {
                            panic!("production standalone scan failed: {error:?}")
                        });
                    let (idle, report) = completed.into_parts();
                    let configured = credentials.as_ref().and_then(|credentials| {
                        report
                            .results()
                            .iter()
                            .find(|result| result.ssid() == credentials.ssid())
                    });
                    let evidence = WifiScanEvidence {
                        generation: report.generation().value(),
                        elapsed_micros: started_at.elapsed().as_micros(),
                        observed_frames: report.observed_frames,
                        unique_bss: report.results().len() as u8,
                        dropped_unique_bss: report.dropped_unique_bss,
                        configured_ssid_found: configured.is_some(),
                        configured_ssid_channel: configured.map_or(0, |result| result.channel),
                        configured_ssid_rssi_dbm: configured
                            .map_or(i8::MIN, |result| result.rssi_dbm),
                    };
                    set_wifi_role(WifiRole::Idle);
                    complete_wifi_scan(request_id, evidence).await;
                    ProductWifiRole::Idle(idle)
                }
                WifiControlRequest::StartMonitor {
                    request_id,
                    request,
                } => {
                    let mut monitor_request = MonitorRequest::new(
                        WifiChannel::mhz20(request.channel)
                            .expect("console validates the monitor channel"),
                        WifiMonitorConfig::normalized(),
                    );
                    if let Some(snapshot_length) = NonZeroU16::new(request.snapshot_length) {
                        monitor_request = monitor_request.with_capture_policy(
                            MonitorCapturePolicy::truncate_at(snapshot_length),
                        );
                    }
                    let monitor =
                        idle.start_monitor(monitor_request)
                            .await
                            .unwrap_or_else(|error| {
                                panic!("production monitor start failed: {error:?}")
                            });
                    set_wifi_role(WifiRole::Monitor);
                    complete_monitor_start(
                        request_id,
                        WifiRoleTransitionEvidence {
                            previous: WifiRole::Idle,
                            current: WifiRole::Monitor,
                            generation: monitor.generation().value(),
                        },
                    )
                    .await;
                    ProductWifiRole::Monitor {
                        owner: monitor,
                        channel: request.channel,
                        started_at_micros: Instant::now().as_micros(),
                        captured_frames: 0,
                        captured_bytes: 0,
                        generation_mismatches: 0,
                        channel_mismatches: 0,
                        channel_unavailable: 0,
                        last_observed_channel: 0,
                    }
                }
                WifiControlRequest::CaptureMonitor {
                    request_id,
                    request,
                } => ProductWifiRole::Idle(
                    run_finite_monitor_capture(idle, &monitor_frames, request_id, request).await,
                ),
                _ => unreachable!("console admits only idle commands while Wi-Fi is idle"),
            },
            ProductWifiRole::Monitor {
                owner,
                channel,
                started_at_micros,
                mut captured_frames,
                mut captured_bytes,
                mut generation_mismatches,
                mut channel_mismatches,
                mut channel_unavailable,
                mut last_observed_channel,
            } => {
                let request_id = loop {
                    match select(receive_wifi_control_request(), monitor_frames.receive()).await {
                        Either::First(WifiControlRequest::StopMonitor { request_id }) => {
                            break request_id;
                        }
                        Either::First(_) => unreachable!(
                            "console admits only monitor stop while monitor owns Wi-Fi"
                        ),
                        Either::Second(frame) => {
                            captured_frames = captured_frames.saturating_add(1);
                            captured_bytes =
                                captured_bytes.saturating_add(frame.captured_length() as u64);
                            if frame.metadata().generation != owner.generation().value() {
                                generation_mismatches = generation_mismatches.saturating_add(1);
                            }
                            match frame.metadata().rx.channel {
                                MacRxEvidence::HardwareObserved(observed)
                                | MacRxEvidence::ProtocolValidated(observed) => {
                                    last_observed_channel = observed;
                                    if observed != channel {
                                        channel_mismatches = channel_mismatches.saturating_add(1);
                                    }
                                }
                                MacRxEvidence::Unavailable => {
                                    channel_unavailable = channel_unavailable.saturating_add(1);
                                }
                            }
                        }
                    }
                };
                let generation = owner.generation().value();
                let idle = owner
                    .stop()
                    .await
                    .unwrap_or_else(|error| panic!("production monitor stop failed: {error:?}"));
                let statistics = monitor_frames.statistics();
                set_wifi_role(WifiRole::Idle);
                complete_monitor_stop(
                    request_id,
                    WifiMonitorEvidence {
                        generation,
                        elapsed_micros: Instant::now()
                            .as_micros()
                            .saturating_sub(started_at_micros),
                        channel,
                        captured_frames,
                        captured_bytes,
                        generation_mismatches,
                        channel_mismatches,
                        channel_unavailable,
                        last_observed_channel,
                        published_frames: statistics.published_frames,
                        full_drops: statistics.full_drops,
                        oversized_drops: statistics.oversized_drops,
                        discarded_frames: statistics.discarded_frames,
                        exported_frames: captured_frames,
                    },
                )
                .await;
                ProductWifiRole::Idle(idle)
            }
        };
    }
}
