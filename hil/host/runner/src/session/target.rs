//! Laboratory inputs and per-scenario initialization the target protocol needs.

use open_esp_radio_hil_protocol::{
    WifiDataPlanePlacement, WifiRxChecksumPolicy, WifiRxContinuationPolicy, WifiTxBufferPolicy,
    WifiTxUdpChecksumPolicy,
};

use crate::{lab::config::LabConfig, scenario::Scenario};

/// What a session needs from its repetition: the laboratory and the
/// selected scenario's target initialization settings.
#[derive(Clone, Copy)]
pub(crate) struct Target<'a> {
    pub(crate) lab: &'a LabConfig,
    pub(crate) settings: Settings,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Settings {
    pub(crate) ap_scheduler: open_esp_radio_hil_protocol::WifiApScheduler,
    pub(crate) data_plane: WifiDataPlanePlacement,
    pub(crate) rx_checksum: WifiRxChecksumPolicy,
    pub(crate) tx_udp_checksum: WifiTxUdpChecksumPolicy,
    pub(crate) tx_buffer: WifiTxBufferPolicy,
    pub(crate) rx_continuation: WifiRxContinuationPolicy,
    pub(crate) l1_cache_counters: bool,
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
