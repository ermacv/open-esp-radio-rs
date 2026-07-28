//! Ownership boundary for the scan-to-associated-STA receive-policy edge.

use open_esp_radio_pac_esp32s31::RadioRegisters;

pub trait StaLinkRxPolicyHardware {
    fn apply_sta_link_policy(&mut self);
}

impl StaLinkRxPolicyHardware for RadioRegisters {
    fn apply_sta_link_policy(&mut self) {
        self.apply_sta_link_receive_policy();
    }
}

/// Switch RX queue zero from the cold/default scan policy to the associated
/// station-link policy.
///
/// This is the source-owned form of migration's
/// `scan::enable_sta_link_rx_policy`, recovered from the policy-five branch of
/// `wifi_set_rx_policy`. It must run after off-channel scanning and before the
/// station sends Authentication.
pub fn configure_sta_link_receive_policy<H: StaLinkRxPolicyHardware>(hardware: &mut H) {
    hardware.apply_sta_link_policy();
}
