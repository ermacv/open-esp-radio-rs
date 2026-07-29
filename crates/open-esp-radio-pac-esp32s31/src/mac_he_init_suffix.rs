//! Generated-PAC ownership for the complete post-power `hal_he_init` suffix.

use super::RadioRegisters;

/// One hardware TX MPDU-length link-table entry.
///
/// SOURCE: complete `_oracles/libpp.a[hal_debug.o]::dbg_read_tx_mplen`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacHeTxMpduLengthLink {
    pub mpdu_length: u16,
    pub next_link: u8,
    /// Bits 31:21 are read by the blob but not assigned a field name.
    pub high_unknown: u16,
}

impl RadioRegisters {
    /// Sample one of the 120 linked TX MPDU-length entries.
    pub fn he_tx_mpdu_length_link(&self, index: u8) -> Option<MacHeTxMpduLengthLink> {
        if index >= 120 {
            return None;
        }
        let entry = self
            .peripherals
            .wifi_mac_he_init_suffix
            .he_scratch(usize::from(index));
        let value = entry.read();
        Some(MacHeTxMpduLengthLink {
            mpdu_length: value.mpdu_length().bits(),
            next_link: value.next_link().bits(),
            high_unknown: value.high_unknown().bits(),
        })
    }

    /// Apply the complete HE Buffer Status Report initialization leaf.
    ///
    /// SOURCE: complete pinned `_oracles/libpp.a[hal_mac_ctl.o]`
    /// `hal_he_bsr_init`, size `0x50`. All eight fresh-read RMW edges and
    /// their order are preserved. This leaf is reached later by the vendor
    /// STA-node setup, not by `hal_he_init`; omitting that second lifecycle
    /// phase produced `0x2010_4c7c = 0x1082_3c00` instead of the vendor auth
    /// image whose BSR portion is `0x0000_81c1`.
    pub fn initialize_he_buffer_status_report(&mut self) {
        let init = &self.peripherals.wifi_mac_he_init_suffix;
        let control = init.ersu_and_vht_control();
        control.modify(|_, w| w.bsr_enable().set_bit());
        init.he_default_control()
            .modify(|_, w| w.bsr_update_enable().set_bit());
        control.modify(|_, w| w.ignore_valid_edca().set_bit());
        control.modify(|_, w| w.ignore_valid_tb().set_bit());
        control.modify(|_, w| w.qos_data_update_bsr().set_bit());
        control.modify(|_, w| w.preac_no_resource_enable_bsr().clear_bit());
        control.modify(|_, w| w.ac_empty_single_bsr().clear_bit());
        control.modify(|_, w| unsafe { w.bsr_aci_high_comparison_method().bits(1) });
    }

    /// Apply the ER-SU state parsed from the associated BSS's HE Operation.
    ///
    /// SOURCE: complete pinned `_oracles/libpp.a[hal_mac_ctl.o]`
    /// `hal_he_set_ersu` and
    /// `_oracles/libnet80211.a[ieee80211_he.o]`
    /// `ieee80211_parse_heopr`. A true argument clears the blob-named
    /// `AUTO_ACK_ALLOW_ERSU` bit; false sets it. The counterintuitive polarity
    /// is instruction-exact. The false leaf additionally rewrites ACK-rate bytes,
    /// already initialized by the complete cold HE suffix.
    pub fn set_he_extended_range_single_user(&mut self, enabled: bool) {
        self.peripherals
            .wifi_mac_he_init_suffix
            .ersu_and_vht_control()
            .modify(|_, w| w.auto_ack_allow_ersu().bit(!enabled));
    }

    /// Publish the associated BSS's HE Default PE Duration.
    ///
    /// SOURCE: complete pinned `_oracles/libpp.a[hal_mac_ctl.o]`
    /// `hal_he_set_default_pe` and
    /// `_oracles/libnet80211.a[ieee80211_he.o]`
    /// `ieee80211_parse_heopr`. The first leaf replaces
    /// `0x2010_4c80[2:0]`; the second passes HE Operation Parameters byte
    /// zero bits 2:0. Vendor auth HIL for the target BSS consequently has
    /// low bits `4`, while the old open path incorrectly left the reset
    /// value `0`.
    pub fn set_he_default_packet_extension_duration(&mut self, duration: u8) {
        debug_assert!(duration < 8);
        self.peripherals
            .wifi_mac_he_init_suffix
            .he_default_control()
            .modify(|_, w| unsafe { w.default_pe_duration().bits(duration & 0x07) });
    }

    /// Apply the complete hardware-visible `hal_he_init` suffix.
    ///
    /// SOURCE: complete pinned `_oracles/libpp.a[hal_mac_ctl.o]` parent and
    /// reached leaves recorded as `BLOB_LIBPP_HAL_HE_INIT_SUFFIX`. The order
    /// below is 163 writes/RMWs plus both conditional multi-BSSID guard reads.
    /// It starts after the separate `dbg_read_tx_power` traversal.
    pub fn initialize_mac_he_suffix(&mut self) {
        let init = &self.peripherals.wifi_mac_he_init_suffix;

        // Complete hal_he_set_ersu(0), followed by its complete
        // hal_he_set_ersu_ack_rate(0) child. The four bytes are deliberately
        // four fresh-read RMWs rather than one full-word write.
        init.ersu_and_vht_control()
            .modify(|_, w| w.auto_ack_allow_ersu().set_bit());
        let ack = init.ersu_ack_rate();
        ack.modify(|_, w| unsafe { w.rate_0().bits(0x80) });
        ack.modify(|_, w| unsafe { w.rate_1().bits(0x80) });
        ack.modify(|_, w| unsafe { w.rate_2().bits(0x80) });
        ack.modify(|_, w| unsafe { w.rate_3().bits(0x80) });

        // Complete hal_set_tx_min_pwr(-11), then the parent field update.
        init.common_power_control()
            .modify(|_, w| unsafe { w.minimum_power_index().bits(0x35) });
        init.he_default_control()
            .modify(|_, w| unsafe { w.mpdu_length_offset().bits(0x17c) });

        // The parent clears this complete 120-word aperture in ascending order.
        for word in 0..120 {
            // SAFETY: the complete parent proves zero as the full 32-bit image
            // of every TX MPDU-length link-table word. This deliberately
            // clears both named fields and the still-unknown high bits.
            unsafe {
                init.he_scratch(word).write_with_zero(|w| w.bits(0));
            }
        }

        // Physical protection words are traversed high-to-low.
        for physical in (0..4).rev() {
            init.protection(physical)
                .modify(|_, w| unsafe { w.mode_unknown().bits(0) });
        }

        init.mode_control()
            .modify(|_, w| w.enable_unknown().set_bit());
        init.shared_enable_control()
            .modify(|_, w| w.enable_unknown().set_bit());
        init.feature_edges().modify(|_, w| w.enable_1().set_bit());
        init.feature_edges().modify(|_, w| w.enable_0().set_bit());
        init.tx_mode_control()
            .modify(|_, w| unsafe { w.mode_unknown().bits(1) });

        // Complete hal_he_set_bcast_ru(0x7fd, 0, 0): six independent RMWs.
        let broadcast_low = init.broadcast_ru_low();
        broadcast_low.modify(|_, w| w.enable().set_bit());
        broadcast_low.modify(|_, w| unsafe { w.value().bits(0x7fd) });
        let broadcast_high = init.broadcast_ru_high();
        broadcast_high.modify(|_, w| w.low_enable().set_bit());
        broadcast_high.modify(|_, w| unsafe { w.low_value().bits(0) });
        broadcast_high.modify(|_, w| w.high_enable().set_bit());
        broadcast_high.modify(|_, w| unsafe { w.high_value().bits(0) });

        // Complete hal_he_set_uora_parameter with packed argument byte 0x2b.
        let uora = init.uora_control();
        uora.modify(|_, w| unsafe { w.low_window().bits(7) });
        uora.modify(|_, w| unsafe { w.high_window().bits(31) });

        init.common_power_control()
            .modify(|_, w| w.parent_enable_unknown().set_bit());
        init.dump_complete_hesigb()
            .modify(|_, w| w.enable().clear_bit());
        init.ersu_and_vht_control()
            .modify(|_, w| w.vht_bw_signaling_rts().clear_bit());

        // Two parent RMWs precede the complete clear leaf.
        let multi = init.multi_bssid_control();
        multi.modify(|_, w| w.multi_bssid_enable().clear_bit());
        multi.modify(|_, w| w.co_hosted_enable().clear_bit());

        // Complete hal_he_clr_multi_bssid. Preserve its explicit guard read
        // and its repeated ENABLE update even though the parent just cleared
        // the same bit.
        if multi.read().co_hosted_enable().bit_is_clear() {
            multi.modify(|_, w| w.multi_bssid_enable().clear_bit());
            multi.modify(|r, w| unsafe {
                w.multi_bssid_mask()
                    .bits(r.multi_bssid_mask().bits() | 0xff)
            });
            multi.modify(|_, w| unsafe { w.bssid_byte_5().bits(0) });
            init.multi_bssid_high()
                .modify(|_, w| unsafe { w.high_address_unknown().bits(0) });
            for physical in (0..8).rev() {
                init.queue_control(physical)
                    .modify(|_, w| w.qos_null_to_translated_bss().clear_bit());
            }
        }

        // Complete hal_he_set_co_hosted_bss(0, 0), including its independent
        // guard read and the repeated hosted-mask OR.
        if multi.read().multi_bssid_enable().bit_is_clear() {
            multi.modify(|_, w| w.co_hosted_enable().clear_bit());
            multi.modify(|r, w| unsafe {
                w.multi_bssid_mask()
                    .bits(r.multi_bssid_mask().bits() | 0xff)
            });
        }
    }
}
