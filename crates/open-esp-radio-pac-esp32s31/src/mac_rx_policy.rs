//! Generated-PAC ownership for finite receive-policy transitions.

use super::{device_fence, RadioRegisters};

impl RadioRegisters {
    /// Switch receive queue zero from scan policy to associated-STA policy.
    ///
    /// SOURCE: complete pinned `libpp.a::hal_mac_rx_set_policy` and
    /// `hal_mac_set_rxq_policy`, reached from the complete
    /// `libnet80211.a::wifi_set_rx_policy` policy-five branch.
    pub fn apply_sta_link_receive_policy(&mut self) {
        let filter = self.peripherals.wifi_mac_rx_filter.policy(0);
        let bssid = self.peripherals.wifi_mac_bssid_policy.bssid_high(0);
        let interface = self.peripherals.wifi_mac_interface_address.address_high(0);

        // `ic_set_rx_policy(0, mode=0, control=1, management=1)`.
        // Keep every complete-leaf fresh-read RMW edge separate.
        filter.modify(|_, w| {
            w.mode_policy_unknown()
                .clear_bit()
                .management_policy_unknown()
                .clear_bit()
        });
        bssid.modify(|_, w| w.policy_bit_30_unknown().clear_bit());
        filter.modify(|_, w| w.control_policy_unknown().clear_bit());
        bssid.modify(|_, w| w.address_check_enable().set_bit());
        interface.modify(|_, w| w.rx_policy_enable().set_bit());

        // `ic_set_rx_policy_ubssid_check(0, false)` performs two ordered RMWs.
        filter.modify(|_, w| w.ubssid_check_high_unknown().clear_bit());
        filter.modify(|_, w| w.ubssid_check_low_unknown().clear_bit());
        device_fence();
    }
}
