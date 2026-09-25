//! Laboratory inputs and per-scenario initialization the target protocol needs.

use oer_hil_protocol::{
    WifiDataPlanePlacement, WifiRxChecksumPolicy, WifiRxContinuationPolicy, WifiTxBufferPolicy,
    WifiTxUdpChecksumPolicy,
};

use crate::{lab::config::LabConfig, scenario::Scenario};

/// What a session needs from its repetition: the laboratory and the
/// selected scenario's target initialization settings.
#[derive(Clone, Copy)]
pub struct Target<'a> {
    pub lab: &'a LabConfig,
    pub settings: Settings,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Settings {
    pub ap_scheduler: oer_hil_protocol::WifiApScheduler,
    pub data_plane: WifiDataPlanePlacement,
    pub rx_checksum: WifiRxChecksumPolicy,
    pub tx_udp_checksum: WifiTxUdpChecksumPolicy,
    pub tx_buffer: WifiTxBufferPolicy,
    pub rx_continuation: WifiRxContinuationPolicy,
    pub l1_cache_counters: bool,
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
        }
    }
}

impl From<&Scenario> for Settings {
    fn from(scenario: &Scenario) -> Self {
        Self {
            ap_scheduler: scenario.ap_scheduler,
            data_plane: scenario.data_plane,
            rx_checksum: scenario.rx_checksum,
            tx_udp_checksum: scenario.tx_udp_checksum,
            tx_buffer: scenario.tx_buffer,
            rx_continuation: scenario.rx_continuation,
            l1_cache_counters: scenario.l1_cache_counters,
        }
    }
}
