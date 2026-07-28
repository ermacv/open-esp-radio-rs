//! Generated-PAC ownership for finite receive-policy transitions.

use super::{device_fence, RadioRegisters};

impl RadioRegisters {
    /// Apply the complete four-queue cold policy transaction from `hal_init`.
    ///
    /// SOURCE: complete pinned `_oracles/libpp.a[hal_mac.o]::hal_init`,
    /// offsets `0x56..0x90`, and complete `hal_mac_rx_set_policy`.
    /// All 31 fresh-read RMW edges remain separate and in blob order.
    pub fn initialize_cold_receive_policy(&mut self) {
        let filters = &self.peripherals.wifi_mac_rx_filter;
        let bssids = &self.peripherals.wifi_mac_bssid_policy;
        let interfaces = &self.peripherals.wifi_mac_interface_address;

        for queue in 0..4 {
            let filter = filters.policy(queue);

            filter.modify(|_, w| {
                w.policy_bit_7_unknown()
                    .set_bit()
                    .policy_bit_9_unknown()
                    .set_bit()
            });
            filter.modify(|_, w| w.mode_policy_unknown().clear_bit());
            filter.modify(|_, w| w.low_unknown().set_bit().policy_bit_2_unknown().set_bit());
            filter.modify(|_, w| {
                w.cold_clear_bit_13_unknown()
                    .clear_bit()
                    .control_policy_unknown()
                    .clear_bit()
            });

            if queue < 3 {
                let bssid = bssids.bssid_high(queue);
                filter.modify(|_, w| {
                    w.mode_policy_unknown()
                        .clear_bit()
                        .management_policy_unknown()
                        .clear_bit()
                });
                if queue == 1 {
                    bssid.modify(|_, w| w.policy_bit_30_unknown().set_bit());
                } else {
                    bssid.modify(|_, w| {
                        w.policy_bit_30_unknown()
                            .clear_bit()
                            .address_check_enable()
                            .clear_bit()
                    });
                }
                filter.modify(|_, w| w.control_policy_unknown().clear_bit());
                bssid.modify(|_, w| w.address_check_enable().clear_bit());
                // SAFETY: zero is the complete low-half replacement performed
                // by the mode-zero cold leaf; high policy bits are preserved.
                interfaces
                    .address_high(queue)
                    .modify(|_, w| unsafe { w.bytes_4_5().bits(0) });
            }
        }
    }

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
