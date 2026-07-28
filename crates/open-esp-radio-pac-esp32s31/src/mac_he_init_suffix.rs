//! Generated-PAC ownership for the complete post-power `hal_he_init` suffix.

use super::RadioRegisters;

impl RadioRegisters {
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
            .modify(|_, w| w.ersu_disabled().set_bit());
        let ack = init.ersu_ack_rate();
        ack.modify(|_, w| unsafe { w.rate_0().bits(0x80) });
        ack.modify(|_, w| unsafe { w.rate_1().bits(0x80) });
        ack.modify(|_, w| unsafe { w.rate_2().bits(0x80) });
        ack.modify(|_, w| unsafe { w.rate_3().bits(0x80) });

        // Complete hal_set_tx_min_pwr(-11), then the parent field update.
        init.common_power_control()
            .modify(|_, w| unsafe { w.minimum_power_index().bits(0x35) });
        init.he_default_control()
            .modify(|_, w| unsafe { w.image_unknown().bits(0x17c) });

        // The parent clears this complete 120-word aperture in ascending order.
        for word in 0..120 {
            // SAFETY: this write-only register is a complete 32-bit scratch
            // word, and zero is the instruction-exact full image.
            unsafe {
                init.he_scratch(word)
                    .write_with_zero(|w| w.image_unknown().bits(0));
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
        multi.modify(|_, w| w.enable().clear_bit());
        multi.modify(|_, w| w.abort_translated_bss().clear_bit());

        // Complete hal_he_clr_multi_bssid. Preserve its explicit guard read
        // and its repeated ENABLE update even though the parent just cleared
        // the same bit.
        if multi.read().abort_translated_bss().bit_is_clear() {
            multi.modify(|_, w| w.enable().clear_bit());
            multi.modify(|r, w| unsafe { w.hosted_mask().bits(r.hosted_mask().bits() | 0xff) });
            multi.modify(|_, w| unsafe { w.low_address_unknown().bits(0) });
            init.multi_bssid_high()
                .modify(|_, w| unsafe { w.high_address_unknown().bits(0) });
            for physical in (0..8).rev() {
                init.queue_control(physical)
                    .modify(|_, w| w.qos_null_to_translated_bss().clear_bit());
            }
        }

        // Complete hal_he_set_co_hosted_bss(0, 0), including its independent
        // guard read and the repeated hosted-mask OR.
        if multi.read().enable().bit_is_clear() {
            multi.modify(|_, w| w.abort_translated_bss().clear_bit());
            multi.modify(|r, w| unsafe { w.hosted_mask().bits(r.hosted_mask().bits() | 0xff) });
        }
    }
}
