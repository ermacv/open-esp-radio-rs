#![no_std]
#![deny(unsafe_code)]

//! Concrete ESP32-S31 Embassy radio composition.
//!
//! [`new`] returns one application radio root and the sole owner-holding
//! runner. Board firmware owns credentials, IP policy and sockets; it does not
//! assemble PAC, DMA, ISR or role transactions.

#[cfg(feature = "diagnostics")]
macro_rules! diagnostics_event {
    ($($argument:tt)*) => { log::info!($($argument)*) };
}

#[cfg(feature = "diagnostics")]
macro_rules! diagnostics_debug {
    ($($argument:tt)*) => { log::debug!($($argument)*) };
}

#[cfg(not(feature = "diagnostics"))]
macro_rules! diagnostics_event {
    ($($argument:tt)*) => {{
        if false {
            let _ = core::format_args!($($argument)*);
        }
    }};
}

#[cfg(not(feature = "diagnostics"))]
macro_rules! diagnostics_debug {
    ($($argument:tt)*) => {{
        if false {
            let _ = core::format_args!($($argument)*);
        }
    }};
}

mod composition;
#[cfg(feature = "diagnostics")]
mod diagnostics;
mod esp_now;
mod facade;
mod monitor;
mod radio_resources;
mod status;
mod supervisor;
mod wifi_network;

#[cfg(feature = "diagnostics")]
pub use diagnostics::{
    Esp32s31ConnectedRxObservation, Esp32s31ConnectedRxObserver, Esp32s31DecodedRxPhyObservation,
    Esp32s31HeSuRxObservation, Esp32s31HtRxObservation, Esp32s31RxEvidence,
};
#[cfg(target_arch = "riscv32")]
pub use esp_now::Esp32s31StandaloneEspNowPhyChannelControl;
pub use esp_now::{
    ESP_NOW_CCMP_HEADER_LEN, ESP_NOW_CCMP_MIC_LEN, ESP_NOW_DEFAULT_ENCRYPTED_PEER_CAPACITY,
    ESP_NOW_DEFAULT_PEER_CAPACITY, ESP_NOW_KEY_LEN, ESP_NOW_RX_REPLAY_WINDOW_BITS,
    ESP_NOW_V1_MAX_PAYLOAD_LEN, ESP_NOW_V1_MAX_PROTECTED_MPDU_LEN,
    ESP_NOW_V1_MIN_PROTECTED_MPDU_LEN, ESP_NOW_V2_ACTION_PREFIX_LEN, ESP_NOW_V2_MAX_ACTION_LEN,
    ESP_NOW_V2_MAX_ELEMENT_COUNT, ESP_NOW_V2_MAX_ELEMENT_PAYLOAD_LEN, ESP_NOW_V2_MAX_MPDU_LEN,
    ESP_NOW_V2_MAX_PAYLOAD_LEN, ESP_NOW_V2_MAX_VENDOR_CONTENT_LEN, ESP_NOW_V2_VERSION,
    ESP32S31_DEFAULT_ESP_NOW_RX_QUEUE_DEPTH, ESP32S31_DEFAULT_ESP_NOW_TX_QUEUE_DEPTH,
    Esp32s31EspNowConnectedControl, Esp32s31EspNowConnectedControlConfigError,
    Esp32s31EspNowConnectedControlError, Esp32s31EspNowConnectedControlShutdown,
    Esp32s31EspNowCryptoDiagnostics, Esp32s31EspNowCryptoError, Esp32s31EspNowKeyOwner,
    Esp32s31EspNowKeySlot, Esp32s31EspNowLongRangeMissing, Esp32s31EspNowLongRangeRate,
    Esp32s31EspNowLongRangeReached, Esp32s31EspNowLongRangeUnsupported, Esp32s31EspNowPhySupport,
    Esp32s31EspNowRxMetadata, Esp32s31EspNowRxPublisher, Esp32s31EspNowRxRateNormalization,
    Esp32s31EspNowRxReceiver, Esp32s31EspNowRxResources, Esp32s31EspNowTxBinding,
    Esp32s31EspNowTxConfig, Esp32s31EspNowTxConfigError, Esp32s31EspNowTxError,
    Esp32s31EspNowTxHandle, Esp32s31EspNowTxMailboxOwner, Esp32s31EspNowTxResources,
    Esp32s31StandaloneEspNowBinding, Esp32s31StandaloneEspNowBindingError,
    Esp32s31StandaloneEspNowChannelControl, Esp32s31StandaloneEspNowOffChannelRunError,
    Esp32s31StandaloneEspNowOffChannelRunFailure, Esp32s31StandaloneEspNowPrepareFailure,
    Esp32s31StandaloneEspNowReceive, Esp32s31StandaloneEspNowRunError,
    Esp32s31StandaloneEspNowRunFailure, Esp32s31StandaloneEspNowRunReport,
    Esp32s31StandaloneEspNowRx, Esp32s31StandaloneEspNowRxProgress,
    Esp32s31StandaloneEspNowService, Esp32s31StandaloneEspNowStopError,
    Esp32s31StandaloneEspNowStopped, EspNowCcmpPacketNumber, EspNowCcmpPacketNumberError,
    EspNowConfig, EspNowConfigError, EspNowDestination, EspNowEncryptedPeerConfig,
    EspNowEncryptedPeerDiagnostics, EspNowEncryptedPeerError, EspNowEncryptedPeerId,
    EspNowEncryptedPeerMutationFailure, EspNowEncryptedPeerReplacement,
    EspNowEncryptedPeerRestoreFailure, EspNowEncryptedPeerTable, EspNowEncryptedPeerView,
    EspNowEncryptedProtocol, EspNowEncryptedReceiveError, EspNowEncryptedRxCandidate,
    EspNowEncryptedSendError, EspNowEncryptedV1Unavailable, EspNowLmk,
    EspNowOffChannelFailureStage, EspNowOwnedV1Tx, EspNowPeerCapability, EspNowPeerChannelPolicy,
    EspNowPeerConfig, EspNowPeerId, EspNowPeerSecurity, EspNowPeerTableError, EspNowPhyMode,
    EspNowPmk, EspNowPmkError, EspNowPmkId, EspNowPmkMutationFailure, EspNowPmkOwner,
    EspNowPreparedEncryptedV1Tx, EspNowPreparedV2Tx, EspNowProtectedV1Envelope,
    EspNowProtectedV1WireError, EspNowProtocol, EspNowRandomValue, EspNowReceivedV2,
    EspNowRemovedEncryptedPeer, EspNowRxMailboxEpochError, EspNowRxMailboxResources,
    EspNowRxMailboxShutdown, EspNowRxPublishOutcome, EspNowRxPublisher, EspNowRxReceiver,
    EspNowRxReplayCandidate, EspNowTxBackpressure, EspNowTxCancelReason, EspNowTxCompletion,
    EspNowTxMailboxEpochError, EspNowTxMailboxInvariantError, EspNowTxMailboxShutdown,
    EspNowTxRuntimeFailure, EspNowTxTerminal, EspNowTxTicket, EspNowTxTrySendError,
    EspNowUnicastAddress, EspNowV1WireError, EspNowV2Action, EspNowV2Element, EspNowV2Elements,
    EspNowV2Frame, EspNowV2Payload, EspNowV2Reassembly, EspNowV2ReceiveError, EspNowV2RxEvent,
    EspNowV2RxMailboxError, EspNowV2RxOutcome, EspNowV2SendError, EspNowV2TxTrySendError,
    EspNowV2WireError, EspNowVersionError, EspNowWireVersion, StandaloneEspNowPeerError,
    StandaloneEspNowRequest, WifiStandaloneEspNowPlan, attach_esp_now_tx,
    encrypted_peer_destination, esp_now_encrypted_v1_codec_status, esp_now_wire_version,
    esp32s31_esp_now_phy_support, normalize_esp_now_rx_metadata,
    prepare_esp32s31_standalone_esp_now,
};
pub use facade::{
    Esp32s31NewError, Esp32s31Radio, Esp32s31RadioError, Esp32s31RadioInitialization,
    Esp32s31RadioParts, Esp32s31Wifi, Esp32s31WifiControl, Esp32s31WifiParts,
};
pub use monitor::{
    ESP32S31_MONITOR_CAPTURE_CAPACITY, Esp32s31MonitorBasebandFormat,
    Esp32s31MonitorCaptureStatistics, Esp32s31MonitorFrame, Esp32s31MonitorFrames,
    Esp32s31MonitorPhyInfo, MONITOR_CHANNEL_SEQUENCE_CAPACITY, MonitorCapturePolicy,
    MonitorChannelPolicy, MonitorChannelSequence, MonitorChannelSequenceError, MonitorRequest,
};
#[cfg(feature = "core0-rx-coarse-telemetry")]
pub use open_esp_radio_esp32s31_wifi_embassy::datapath::configure_adaptive_recycled_rx_probe_for_diagnostics;
#[cfg(feature = "core0-rx-coarse-telemetry")]
pub use open_esp_radio_esp32s31_wifi_embassy::datapath::configure_recycled_rx_probe_delay_for_diagnostics;
#[cfg(feature = "core0-rx-coarse-telemetry")]
pub use open_esp_radio_esp32s31_wifi_embassy::datapath::rx::dma::configure_interrupt_driven_recycled_append_for_diagnostics;
#[cfg(feature = "task-poll-telemetry")]
pub use open_esp_radio_esp32s31_wifi_embassy::diagnostics::core0_ap_rx_cycles::{
    CORE0_AP_RX_CYCLES, Core0ApRxCycleSnapshot,
};
#[cfg(feature = "task-poll-telemetry")]
pub use open_esp_radio_esp32s31_wifi_embassy::diagnostics::core0_rx_cycles::{
    CORE0_RX_CYCLES, Core0RxCycleSnapshot, cycle_count,
};
#[cfg(any(feature = "task-poll-telemetry", feature = "core0-rx-coarse-telemetry"))]
pub use open_esp_radio_esp32s31_wifi_embassy::diagnostics::core0_rx_performance::{
    CORE0_PERFORMANCE, Core0PerformanceSample, Core0PerformanceSnapshot,
};
#[cfg(feature = "core0-rx-coarse-telemetry")]
pub use open_esp_radio_esp32s31_wifi_embassy::diagnostics::core0_rx_performance::configure_ap_terminal_identity_diagnostics;
#[cfg(feature = "core0-rx-coarse-telemetry")]
pub use open_esp_radio_esp32s31_wifi_embassy::diagnostics::egress::{
    EgressPolicyShadowSnapshot, egress_policy_shadow_snapshot,
};
#[cfg(feature = "task-poll-telemetry")]
pub use open_esp_radio_esp32s31_wifi_embassy::diagnostics::core0_rx_reorder_cycles::{
    CORE0_REORDER_CYCLES, Core0ReorderSnapshot,
};
#[cfg(feature = "task-poll-telemetry")]
pub use open_esp_radio_esp32s31_wifi_embassy::diagnostics::core0_rx_service_histogram::{
    CORE0_RX_SERVICE_HISTOGRAM, CORE0_RX_SERVICE_HISTOGRAM_BINS, Core0RxServiceBinSnapshot,
    Core0RxServiceHistogramSnapshot,
};
#[cfg(feature = "diagnostics")]
pub use open_esp_radio_esp32s31_wifi_embassy::diagnostics::network::{
    RxNetworkDeliveryEvent, RxNetworkDeliveryObserver, RxObservedEthernetFrame,
    RxQosSequenceObservation,
};
#[cfg(feature = "core0-rx-coarse-telemetry")]
pub use open_esp_radio_esp32s31_wifi_embassy::roles::station::rx_protocol::configure_direct_immediate_rx_dispatch_for_diagnostics;
#[cfg(feature = "core0-rx-coarse-telemetry")]
pub use radio_resources::{access_point_egress_control_snapshot, station_egress_control_snapshot};
pub use open_esp_radio_esp32s31_wifi_sta::connected_control::ConnectedDisconnectReason;
#[cfg(feature = "tx-psram-dma-probe")]
pub use radio_resources::configure_direct_psram_tx_dma_probe;
#[cfg(feature = "tx-psram-dma-probe")]
pub use radio_resources::{
    DirectPsramTxDmaProbeObservation, direct_psram_tx_dma_probe_observation,
};
pub use radio_resources::{Esp32s31WifiDevice, Esp32s31WifiDevices};
pub use status::{
    Esp32s31AccessPointStatus, Esp32s31AccessPointStatusSnapshot, Esp32s31StationLinkState,
    Esp32s31StationStatus, Esp32s31StationStatusSnapshot,
};
#[cfg(feature = "mac-irq-diagnostics")]
pub use supervisor::station::Esp32s31MacIrqObservation;
#[cfg(feature = "diagnostics")]
pub use supervisor::station::{
    Esp32s31DiagnosticRxStatistics, Esp32s31DiagnosticSnapshot, Esp32s31DiagnosticTxVector,
};
pub use supervisor::{Esp32s31RadioRunner, Esp32s31RadioRunners, Esp32s31RadioSystem, new};
pub use wifi_network::Esp32s31WifiNetworkRunner;

/// One low-overhead batch of Core0 connected-DATAPATH poll residence.
///
/// The diagnostic image supplies the CPU frequency used to convert `mcycle`
/// deltas. Batching keeps atomic/reporting work out of the per-poll hot path.
#[cfg(feature = "connected-datapath-cycle-telemetry")]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Esp32s31ConnectedDatapathPollBatch {
    pub polls: u32,
    pub poll_micros: u32,
    pub maximum_poll_micros: u32,
    pub over_100_micros: u32,
    pub over_500_micros: u32,
    pub over_1_000_micros: u32,
    pub over_5_000_micros: u32,
}

/// Value-only sink for connected-DATAPATH executor residence.
///
/// This hook exists only in the dedicated diagnostic build. It cannot access
/// the runner or alter wake/ownership semantics.
#[cfg(feature = "connected-datapath-cycle-telemetry")]
#[derive(Clone, Copy)]
pub struct Esp32s31ConnectedDatapathPollObserver {
    cycles_per_micro: u32,
    record: fn(Esp32s31ConnectedDatapathPollBatch),
}

#[cfg(feature = "connected-datapath-cycle-telemetry")]
impl Esp32s31ConnectedDatapathPollObserver {
    pub const fn new(
        cycles_per_micro: u32,
        record: fn(Esp32s31ConnectedDatapathPollBatch),
    ) -> Self {
        assert!(cycles_per_micro != 0, "CPU clock must be non-zero");
        Self {
            cycles_per_micro,
            record,
        }
    }

    pub(crate) const fn cycles_per_micro(self) -> u32 {
        self.cycles_per_micro
    }

    pub(crate) fn record(self, batch: Esp32s31ConnectedDatapathPollBatch) {
        (self.record)(batch);
    }
}

/// Board-derived radio identity. Reading eFuse remains an application
/// responsibility; credentials are supplied separately to `start_station`.
pub struct Esp32s31RadioConfig {
    pub(crate) station_mac: open_esp_radio::WifiMacAddress,
    pub(crate) access_point_mac: open_esp_radio::WifiMacAddress,
    pub(crate) calibration: open_esp_radio_esp32s31_phy::PhyCalibrationIdentity,
    pub(crate) initial_channel: open_esp_radio_ieee80211::channel::WifiChannel,
    pub(crate) calibration_cache: Option<open_esp_radio_esp32s31_phy::PhyCalibrationCache>,
    pub(crate) maximum_tx_power_quarter_dbm: Option<i8>,
    #[cfg(feature = "connected-datapath-cycle-telemetry")]
    pub(crate) connected_datapath_poll_observer: Option<Esp32s31ConnectedDatapathPollObserver>,
    #[cfg(feature = "diagnostics")]
    pub(crate) diagnostics: Option<Esp32s31DiagnosticObservers>,
}

impl Esp32s31RadioConfig {
    pub const fn new(
        station_mac: open_esp_radio::WifiMacAddress,
        access_point_mac: open_esp_radio::WifiMacAddress,
        calibration: open_esp_radio_esp32s31_phy::PhyCalibrationIdentity,
        initial_channel: open_esp_radio_ieee80211::channel::WifiChannel,
    ) -> Self {
        Self {
            station_mac,
            access_point_mac,
            calibration,
            initial_channel,
            calibration_cache: None,
            maximum_tx_power_quarter_dbm: None,
            #[cfg(feature = "connected-datapath-cycle-telemetry")]
            connected_datapath_poll_observer: None,
            #[cfg(feature = "diagnostics")]
            diagnostics: None,
        }
    }

    /// Supply a caller-owned retained PHY calibration cache. The driver
    /// validates its embedded identity before deciding whether it is reusable.
    pub fn with_calibration_cache(
        mut self,
        cache: open_esp_radio_esp32s31_phy::PhyCalibrationCache,
    ) -> Self {
        self.calibration_cache = Some(cache);
        self
    }

    /// Apply the board/regulatory TX ceiling to the calibrated power profile.
    pub const fn with_maximum_tx_power_quarter_dbm(mut self, maximum: i8) -> Self {
        self.maximum_tx_power_quarter_dbm = Some(maximum);
        self
    }

    /// Attach the dedicated Core0 connected-DATAPATH residence sink.
    #[cfg(feature = "connected-datapath-cycle-telemetry")]
    pub const fn with_connected_datapath_poll_observer(
        mut self,
        observer: Esp32s31ConnectedDatapathPollObserver,
    ) -> Self {
        self.connected_datapath_poll_observer = Some(observer);
        self
    }

    /// Attach value-only, non-blocking diagnostics observers. This API does
    /// not exist in production builds and grants no register or owner access.
    #[cfg(feature = "diagnostics")]
    pub const fn with_diagnostic_observers(mut self, hooks: Esp32s31DiagnosticObservers) -> Self {
        self.diagnostics = Some(hooks);
        self
    }
}

/// Optional value-only observers compiled only into diagnostics firmware.
#[cfg(feature = "diagnostics")]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Esp32s31DiagnosticRxAdmission {
    /// Publish an ordinary shared staging owner without an async capacity edge.
    #[default]
    SynchronousShared,
    /// Retain the former immediately-ready async edge for same-image HIL A/B.
    DeferredReady,
}

/// Optional value-only observers compiled only into diagnostics firmware.
#[cfg(feature = "diagnostics")]
#[derive(Clone, Copy)]
pub struct Esp32s31DiagnosticObservers {
    /// Same-image selector for the ordinary shared RX admission experiment.
    pub rx_admission: Esp32s31DiagnosticRxAdmission,
    /// Intrusive per-stage timing/counter observer. Correctness images leave
    /// this unset; only dedicated pipeline diagnostics may charge the RX hot
    /// path for these observations.
    pub rx_pipeline: Option<
        &'static dyn open_esp_radio_esp32s31_wifi_embassy::diagnostics::rx_pipeline::RxPipelineObserver,
    >,
    /// Low-frequency typed BlockAck agreement observer used by correctness
    /// images without attaching the per-frame pipeline profiler.
    pub rx_reorder: Option<
        &'static dyn open_esp_radio_esp32s31_wifi_embassy::diagnostics::rx_pipeline::RxReorderAgreementObserver,
    >,
    pub aggregate_tx: &'static dyn open_esp_radio_esp32s31_wifi_embassy::diagnostics::aggregate_tx::AggregateTxObserver,
    pub connected_rx: &'static dyn Esp32s31ConnectedRxObserver,
    pub rx_delivery: Option<
        &'static dyn open_esp_radio_esp32s31_wifi_embassy::diagnostics::network::RxNetworkDeliveryObserver,
    >,
    #[cfg(feature = "mac-irq-diagnostics")]
    pub mac_irq: fn(Esp32s31MacIrqObservation),
    pub station_attempt: fn(Esp32s31StationAttemptObservation),
    pub access_point: fn(Esp32s31AccessPointObservation),
}

/// Value-only terminal AP epoch evidence emitted after TX, RX and IRQ have
/// quiesced but before their typed owners return to role-neutral Wi-Fi.
#[cfg(feature = "diagnostics")]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Esp32s31AccessPointObservation {
    pub channel: u8,
    pub bandwidth_mhz: u16,
    pub beacons_transmitted: u32,
    pub missed_beacon_intervals: u32,
    pub maximum_beacon_lateness_micros: u32,
    pub tx_interrupt_wakes: u32,
    pub tx_deadline_wakes: u32,
    pub maximum_tx_pending_micros: u32,
    pub maximum_network_tx_pending_micros: u32,
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
    pub authentication_responses: u32,
    pub association_responses: u32,
    /// Successful controlled-port openings, including re-authorizations.
    pub authorized_peers: u32,
    pub maximum_associated_peers: u8,
    pub maximum_authorized_peers: u8,
    pub peer_removals: u32,
    pub authentication_timeouts: u32,
    pub wpa2_response_windows: u32,
    pub wpa2_pending_on_stop: u32,
    pub wpa2_retransmissions: u32,
    pub wpa2_handshake_failures: u32,
    pub wpa2_handshake_timeouts: u32,
    pub inactivity_timeouts: u32,
    pub disassociations_prepared: u32,
    pub disassociations_published: u32,
    pub disassociations_acknowledged: u32,
    pub deauthentications_prepared: u32,
    pub deauthentications_published: u32,
    pub deauthentications_acknowledged: u32,
    pub tx_block_ack_requests_prepared: u32,
    pub tx_block_ack_responses_observed: u32,
    pub tx_block_ack_agreements_operational: u32,
    pub tx_block_ack_responses_rejected: u32,
    pub tx_block_ack_negotiation_timeouts: u32,
    /// Peer-originated RX ADDBA responses that reached terminal TX success.
    pub rx_block_ack_responses_transmitted: u32,
    /// Hardware MAC/baseband counter increments across the complete AP epoch.
    pub rx_hardware: Esp32s31DiagnosticRxStatistics,
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
    pub rx_ht_data_frames: u32,
    /// MPDUs whose copied HT-SIG metadata has the Aggregation bit set.
    /// This is not an aggregate-PPDU count and does not imply aggregate depth.
    pub rx_ht_mpdus_with_aggregation_bit: u32,
    pub rx_rssi_samples: u32,
    pub rx_rssi_sum_dbm: i32,
    pub rx_rssi_min_dbm: i8,
    pub rx_rssi_max_dbm: i8,
    pub rx_ht40_mcs_frames: [u32; 8],
    pub rx_ht40_long_gi_frames: u32,
    pub rx_ht40_short_gi_frames: u32,
    pub rx_ht40_mcs32_frames: u32,
    pub rx_ht_mcs32_width_mismatches: u32,
    pub tx_ht_aggregates: u32,
    pub tx_ht40_mcs7_aggregates: u32,
    pub data_frames_transmitted: u32,
    pub ht_duplicate_tx_requests: u32,
    pub ht_duplicate_tx_selection: open_esp_radio_esp32s31_wifi_mac::tx::HtDuplicateTxSelection,
    pub data_tx_attempts: u32,
    pub data_tx_retried_frames: u32,
    pub data_tx_maximum_attempts: u8,
    pub data_tx_minimum_final_rate_kbps: u32,
    pub data_tx_ack_snr_samples: u32,
    pub data_tx_minimum_ack_snr_db: i8,
    pub data_tx_maximum_ack_snr_db: i8,
    pub tx_ack_timeout_retries: u32,
    pub tx_cts_timeout_retries: u32,
    pub tx_collision_retries: u32,
    pub tx_hardware_failures: u8,
    pub tx_hardware_timeouts: u8,
    pub tx_collision_limits: u8,
    pub tx_last_hardware_status: u8,
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
    /// Data MPDUs whose Protected bit contradicted the requested AP mode.
    pub security_mode_mismatches: u32,
}

/// Value-only failed-attempt detail emitted to diagnostics firmware.
#[cfg(feature = "diagnostics")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31StationAttemptObservation {
    AttemptFailed {
        attempt: u16,
        stage: open_esp_radio_wifi_sta::station::StaLifecycleStage,
    },
    RetryExhausted {
        attempts: u16,
        stage: open_esp_radio_wifi_sta::station::StaLifecycleStage,
    },
}
