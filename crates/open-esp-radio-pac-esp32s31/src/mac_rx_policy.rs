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
    /// `libnet80211.a::wifi_set_rx_policy` policy-six branch used immediately
    /// before the first live vendor Authentication TX.
    pub fn apply_sta_link_receive_policy(&mut self, bssid_address: [u8; 6]) {
        let filter = self.peripherals.wifi_mac_rx_filter.policy(0);
        let bssid_low = self.peripherals.wifi_mac_bssid_policy.bssid_low(0);
        let bssid = self.peripherals.wifi_mac_bssid_policy.bssid_high(0);
        let interface = self.peripherals.wifi_mac_interface_address.address_high(0);

        // `ic_set_bssid(0, bssid)` delegates to the complete
        // `hal_mac_set_bssid` leaf. Keep its four hardware edges ordered:
        // disable matching, publish the low word, replace the two high bytes,
        // then make the new address valid.
        //
        // SOURCE: complete pinned `_oracles/libpp.a[hal_mac.o]`
        // `hal_mac_set_bssid`, size 0x5a.
        bssid.modify(|_, w| w.address_check_enable().clear_bit());
        // SAFETY: the full low word and low sixteen-bit high-address field are
        // exactly the values assembled by the recovered blob leaf.
        unsafe {
            bssid_low.write_with_zero(|w| {
                w.bytes_0_3().bits(u32::from_le_bytes([
                    bssid_address[0],
                    bssid_address[1],
                    bssid_address[2],
                    bssid_address[3],
                ]))
            });
            bssid.modify(|_, w| {
                w.bssid_high()
                    .bits(u16::from_le_bytes([bssid_address[4], bssid_address[5]]))
            });
        }
        bssid.modify(|_, w| w.address_check_enable().set_bit());

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

        // `wifi_set_rx_policy(6)`, used by the live vendor STA immediately
        // before Open Authentication, finishes with
        // `ic_set_rx_policy_ubssid_check(0, true)`. Its complete libpp leaf
        // performs these two ordered RMWs. The earlier open transcription used
        // policy five/false from migration and produced filter 0x0001_c285;
        // the working vendor authentication capture on ESP32-S31 rev0 shows
        // 0x0001_c387, differing by exactly these bits.
        //
        // SOURCE: `_oracles/libnet80211.a[ieee80211_supplicant.o]`
        // `wifi_set_rx_policy`, jump-table case six at `.L255` through
        // `.L262`; `_oracles/libpp.a[if_hwctrl.o]`
        // `ic_set_rx_policy_ubssid_check`; live `wifi-sta-auth-probe` HIL on
        // 2026-07-28.
        filter.modify(|_, w| w.ubssid_check_high_unknown().set_bit());
        filter.modify(|_, w| w.ubssid_check_low_unknown().set_bit());
        device_fence();
    }
}
