#![no_std]
#![deny(unsafe_code)]
#![cfg_attr(not(feature = "qualification"), allow(unused_variables))]

//! Concrete ESP32-S31 Embassy radio composition.
//!
//! [`new`] returns one application radio root and the sole owner-holding
//! runner. Board firmware owns credentials, IP policy and sockets; it does not
//! assemble PAC, DMA, ISR or role transactions.

#[cfg(feature = "qualification")]
macro_rules! qualification_event {
    ($($argument:tt)*) => { log::info!($($argument)*) };
}

#[cfg(feature = "qualification")]
macro_rules! qualification_debug {
    ($($argument:tt)*) => { log::debug!($($argument)*) };
}

#[cfg(not(feature = "qualification"))]
macro_rules! qualification_event {
    ($($argument:tt)*) => {{}};
}

#[cfg(not(feature = "qualification"))]
macro_rules! qualification_debug {
    ($($argument:tt)*) => {{}};
}

mod access_point_status;
mod connected;
mod facade;
mod monitor;
mod radio_resources;
mod runtime;
mod wifi_network;

pub use access_point_status::{Esp32s31AccessPointStatus, Esp32s31AccessPointStatusSnapshot};
#[cfg(feature = "qualification")]
pub use connected::{
    Esp32s31ConnectedRxObserver, Esp32s31MacIrqObservation, Esp32s31QualificationSnapshot,
    Esp32s31QualificationTxVector,
};
pub use connected::Esp32s31WifiProtocolRunner;
pub use radio_resources::Esp32s31WifiDevice;
pub use facade::{
    Esp32s31NewError, Esp32s31Radio, Esp32s31RadioError, Esp32s31RadioInitialization,
    Esp32s31RadioParts, Esp32s31Wifi, Esp32s31WifiControl, Esp32s31WifiParts,
};
pub use monitor::{
    ESP32S31_MONITOR_CAPTURE_CAPACITY, Esp32s31MonitorCaptureStatistics, Esp32s31MonitorFrame,
    Esp32s31MonitorFrames,
};
#[cfg(feature = "qualification")]
pub use open_esp_radio::esp32s31::wifi::sta::connected_control::ConnectedDisconnectReason;
#[cfg(feature = "qualification")]
pub use open_esp_radio_esp32s31_wifi_embassy::network_rx::{
    RxNetworkDeliveryEvent, RxNetworkDeliveryObserver,
};
pub use runtime::{Esp32s31RadioRunner, Esp32s31RadioRunners, Esp32s31RadioSystem, new};
pub use wifi_network::{Esp32s31WifiNetworkRunner, new_wifi_network};

/// Board-derived radio identity. Reading eFuse remains an application
/// responsibility; credentials are supplied separately to `start_station`.
pub struct Esp32s31RadioConfig {
    pub(crate) station_mac: open_esp_radio::WifiMacAddress,
    pub(crate) access_point_mac: open_esp_radio::WifiMacAddress,
    pub(crate) calibration: open_esp_radio::esp32s31::phy::PhyCalibrationIdentity,
    pub(crate) initial_channel: open_esp_radio::wifi::ieee80211::channel::WifiChannel,
    pub(crate) calibration_cache: Option<open_esp_radio::esp32s31::phy::PhyCalibrationCache>,
    pub(crate) maximum_tx_power_quarter_dbm: Option<i8>,
    #[cfg(feature = "qualification")]
    pub(crate) qualification: Option<Esp32s31QualificationHooks>,
}

impl Esp32s31RadioConfig {
    pub const fn new(
        station_mac: open_esp_radio::WifiMacAddress,
        access_point_mac: open_esp_radio::WifiMacAddress,
        calibration: open_esp_radio::esp32s31::phy::PhyCalibrationIdentity,
        initial_channel: open_esp_radio::wifi::ieee80211::channel::WifiChannel,
    ) -> Self {
        Self {
            station_mac,
            access_point_mac,
            calibration,
            initial_channel,
            calibration_cache: None,
            maximum_tx_power_quarter_dbm: None,
            #[cfg(feature = "qualification")]
            qualification: None,
        }
    }

    /// Supply a caller-owned retained PHY calibration cache. The driver
    /// validates its embedded identity before deciding whether it is reusable.
    pub fn with_calibration_cache(
        mut self,
        cache: open_esp_radio::esp32s31::phy::PhyCalibrationCache,
    ) -> Self {
        self.calibration_cache = Some(cache);
        self
    }

    /// Apply the board/regulatory TX ceiling to the calibrated power profile.
    pub const fn with_maximum_tx_power_quarter_dbm(mut self, maximum: i8) -> Self {
        self.maximum_tx_power_quarter_dbm = Some(maximum);
        self
    }

    /// Attach value-only, non-blocking qualification observers. This API does
    /// not exist in production builds and grants no register or owner access.
    #[cfg(feature = "qualification")]
    pub const fn with_qualification_hooks(mut self, hooks: Esp32s31QualificationHooks) -> Self {
        self.qualification = Some(hooks);
        self
    }
}

/// Optional HIL observers compiled only into qualification firmware.
#[cfg(feature = "qualification")]
#[derive(Clone, Copy)]
pub struct Esp32s31QualificationHooks {
    pub rx_pipeline: &'static dyn open_esp_radio_esp32s31_wifi_embassy::rx_pipeline_observer::RxPipelineObserver,
    pub aggregate_tx: &'static dyn open_esp_radio_esp32s31_wifi_embassy::aggregate_tx_observer::AggregateTxObserver,
    pub connected_rx: &'static dyn Esp32s31ConnectedRxObserver,
    pub rx_delivery: Option<
        &'static dyn open_esp_radio_esp32s31_wifi_embassy::network_rx::RxNetworkDeliveryObserver,
    >,
    pub mac_irq: fn(Esp32s31MacIrqObservation),
    /// Continuous residence of each RX protocol future poll, in microseconds.
    pub protocol_task_poll: fn(u64),
    pub station_lifecycle: fn(Esp32s31StationLifecycleObservation),
    pub access_point: fn(Esp32s31AccessPointObservation),
}

/// Value-only terminal AP epoch evidence emitted after TX, RX and IRQ have
/// quiesced but before their typed owners return to role-neutral Wi-Fi.
#[cfg(feature = "qualification")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
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
    pub completed_rx_units: u32,
    pub completed_rx_descriptors: u32,
    pub recycled_rx_descriptors: u32,
    /// Hardware MAC RX BUFFER_FULL increments across the complete AP epoch.
    pub rx_hardware_buffer_full: u16,
    /// Hardware MAC RX FIFO overflow increments across the complete AP epoch.
    pub rx_hardware_fifo_overflow: u16,
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
    pub rx_ht_data_frames: u32,
    pub rx_ht_ampdu_data_frames: u32,
    pub rx_rssi_samples: u32,
    pub rx_rssi_sum_dbm: i32,
    pub rx_rssi_min_dbm: i8,
    pub rx_rssi_max_dbm: i8,
    pub rx_ht40_mcs_frames: [u32; 8],
    pub tx_ht_aggregates: u32,
    pub tx_ht40_mcs7_aggregates: u32,
    pub data_frames_transmitted: u32,
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
    pub protected_data_radio_rejected: u32,
    pub protected_data_protocol_rejected: u32,
}

/// Value-only connected-link edge emitted to qualification firmware.
#[cfg(feature = "qualification")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31StationLifecycleObservation {
    Connected,
    Disconnected(ConnectedDisconnectReason),
    AttemptFailed {
        attempt: u16,
        stage: open_esp_radio::wifi::sta::station::StaLifecycleStage,
    },
    RetryExhausted {
        attempts: u16,
        stage: open_esp_radio::wifi::sta::station::StaLifecycleStage,
    },
}
