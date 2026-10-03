//! The target initialization a scenario selects for each session.

use oer_hil_protocol::wifi::{
    WifiDataPlanePlacement, WifiRxChecksumPolicy, WifiRxContinuationPolicy, WifiTxBufferPolicy,
    WifiTxUdpChecksumPolicy,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Settings {
    pub ap_scheduler: oer_hil_protocol::wifi::WifiApScheduler,
    pub data_plane: WifiDataPlanePlacement,
    pub rx_checksum: WifiRxChecksumPolicy,
    pub tx_udp_checksum: WifiTxUdpChecksumPolicy,
    pub tx_buffer: WifiTxBufferPolicy,
    pub rx_continuation: WifiRxContinuationPolicy,
    pub l1_cache_counters: bool,
    /// The power save the station runs once started.
    pub station_power_save: oer_hil_protocol::wifi::WifiStationPowerSave,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            ap_scheduler: Default::default(),
            data_plane: WifiDataPlanePlacement::SplitRadioNetwork,
            rx_checksum: WifiRxChecksumPolicy::Software,
            tx_udp_checksum: WifiTxUdpChecksumPolicy::Software,
            tx_buffer: WifiTxBufferPolicy::OwnedSramPromotion,
            rx_continuation: WifiRxContinuationPolicy::ImmediateSoftwareProbe,
            l1_cache_counters: false,
            station_power_save: oer_hil_protocol::wifi::WifiStationPowerSave::None,
        }
    }
}
