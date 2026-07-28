//! Ownership boundary for the scan-to-associated-STA receive-policy edge.

use open_esp_radio_pac_esp32s31::RadioRegisters;

pub trait StaLinkRxPolicyHardware {
    fn apply_sta_link_policy(&mut self, bssid: [u8; 6]);
}

impl StaLinkRxPolicyHardware for RadioRegisters {
    fn apply_sta_link_policy(&mut self, bssid: [u8; 6]) {
        self.apply_sta_link_receive_policy(bssid);
    }
}

/// Switch RX queue zero from the cold/default scan policy to the associated
/// station-link policy.
///
/// This is the source-owned form of migration's
/// `scan::enable_sta_link_rx_policy`, completed with the preceding
/// `ic_set_bssid`/`hal_mac_set_bssid` transaction recovered from the same
/// vendor STA transition. Unlike migration's policy-five snapshot, the final
/// UBSSID edge follows `wifi_set_rx_policy(6)`, which is the branch observed
/// immediately before the first live vendor Authentication TX. It must run
/// after off-channel scanning and before the station sends Authentication.
pub fn configure_sta_link_receive_policy<H: StaLinkRxPolicyHardware>(
    hardware: &mut H,
    bssid: [u8; 6],
) {
    hardware.apply_sta_link_policy(bssid);
}
