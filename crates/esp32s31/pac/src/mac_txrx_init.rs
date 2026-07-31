//! Generated-PAC ownership for the direct `mac_txrx_init` prefix.

use super::RadioRegisters;

impl RadioRegisters {
    /// Apply all eighteen direct RMW edges before the first HE callback.
    ///
    /// SOURCE: complete pinned `_oracles/libpp.a[hal_mac.o]::mac_txrx_init`,
    /// offsets `0x08..0xd0`. Callback effects and the direct suffix remain
    /// outside this deliberately bounded operation.
    pub fn initialize_mac_txrx_prefix(&mut self) {
        let init = &self.peripherals.wifi_mac_txrx_prefix;

        init.feature_edges().modify(|_, w| {
            w.first_group_bit_31_unknown()
                .set_bit()
                .first_group_bit_23_unknown()
                .set_bit()
                .first_group_bit_15_unknown()
                .set_bit()
                .first_group_bit_13_unknown()
                .set_bit()
        });
        init.feature_edges()
            .modify(|_, w| w.second_enable_unknown().set_bit());
        init.feature_edges()
            .modify(|_, w| w.third_enable_unknown().set_bit());
        init.mode_control()
            .modify(|_, w| w.init_clear_unknown().clear_bit());

        for queue in 0..4 {
            // SAFETY: preserving the sampled low half and clearing the high
            // half is the complete single RMW performed for each queue.
            init.rx_queue_default(queue)
                .modify(|r, w| unsafe { w.bits(r.bits() & 0x0000_ffff) });
        }
        init.rx_queue_default(0)
            .modify(|_, w| w.queue_bit_24_unknown().set_bit());
        init.rx_queue_default(1)
            .modify(|_, w| w.queue_bit_24_unknown().set_bit());
        init.rx_queue_default(0)
            .modify(|_, w| w.queue_bit_26_unknown().set_bit());
        init.rx_queue_default(1)
            .modify(|_, w| w.queue_bit_26_unknown().set_bit());

        init.feature_edges()
            .modify(|_, w| w.late_enable_unknown().set_bit());
        init.control_edges()
            .modify(|_, w| w.first_enable_unknown().set_bit());
        init.control_edges()
            .modify(|_, w| w.second_enable_unknown().set_bit());
        init.timing_control()
            .modify(|_, w| w.enable_unknown().set_bit());
        // SAFETY: 0x1b is the complete eight-bit field image from the prefix.
        init.timing_control()
            .modify(|_, w| unsafe { w.timing_image_unknown().bits(0x1b) });
        // SAFETY: three is the complete two-bit set image.
        init.shared_enable_control()
            .modify(|_, w| unsafe { w.enable_group_unknown().bits(3) });
    }

    /// Apply the on-chip paths of all three HE callbacks in `mac_txrx_init`.
    ///
    /// SOURCE: complete pinned `_oracles/libpp.a[hal_mac_ctl.o]`
    /// `hal_he_set_mac_delay`, `hal_he_set_ack_rate(0)` and
    /// `hal_he_set_bbrxhung_time(0)`. `delay_slot` is `_random() % 11`.
    ///
    /// Returns false without touching hardware when the slot is outside the
    /// complete vendor range `0..=10`.
    pub fn initialize_mac_txrx_callbacks(&mut self, delay_slot: u8) -> bool {
        if delay_slot > 10 {
            return false;
        }

        let callbacks = &self.peripherals.wifi_mac_txrx_callbacks;
        // SAFETY: all images fit their complete generated fields. Arithmetic
        // bounds follow from the checked vendor slot range above.
        callbacks
            .delay_primary()
            .modify(|_, w| unsafe { w.rx_cck_delay().bits(0x3b9) });
        callbacks
            .delay_primary()
            .modify(|_, w| unsafe { w.low_delay_unknown().bits(0xf5 + u16::from(delay_slot)) });
        callbacks
            .delay_primary()
            .modify(|_, w| unsafe { w.high_delay_unknown().bits(0x5e) });
        callbacks
            .delay_secondary()
            .modify(|_, w| unsafe { w.high_delay_unknown().bits(0xfa + u16::from(delay_slot)) });
        callbacks
            .delay_secondary()
            .modify(|_, w| unsafe { w.tx_cck_delay().bits(0x276) });

        // SAFETY: each callback performs a complete full-word store; no reset
        // value or unread field is used to construct these exact blob images.
        unsafe {
            callbacks
                .ack_rate_table()
                .write_with_zero(|w| w.bits(0x0009_0a0b));
            callbacks
                .cts_rate_table()
                .write_with_zero(|w| w.bits(0x0009_0a0b));
            callbacks
                .ack_cck_rate_table()
                .write_with_zero(|w| w.bits(0x0005_0100));
            callbacks
                .cts_cck_rate_table()
                .write_with_zero(|w| w.bits(0x0005_0100));
        }
        callbacks
            .bb_rx_hang_control()
            .modify(|_, w| unsafe { w.timeout_unknown().bits(0x00f) });
        true
    }

    /// Apply all nine direct RMW edges after the three HE callbacks.
    ///
    /// SOURCE: complete pinned `_oracles/libpp.a[hal_mac.o]::mac_txrx_init`,
    /// offsets `0xee..0x16e`.
    pub fn initialize_mac_txrx_suffix(&mut self) {
        let callbacks = &self.peripherals.wifi_mac_txrx_callbacks;
        callbacks
            .bb_rx_hang_control()
            .modify(|_, w| w.txrx_suffix_first_enable_unknown().set_bit());
        callbacks
            .bb_rx_hang_control()
            .modify(|_, w| w.txrx_suffix_second_enable_unknown().set_bit());
        let init = &self.peripherals.wifi_mac_txrx_suffix;
        // SAFETY: both values fit their complete generated fields.
        init.default_image_a()
            .modify(|_, w| unsafe { w.low_image_unknown().bits(0x0f0) });
        init.default_image_b()
            .modify(|_, w| unsafe { w.low_image_unknown().bits(0x0f0) });
        init.field_control()
            .modify(|_, w| unsafe { w.field_unknown().bits(4) });
        init.gate_control()
            .modify(|_, w| unsafe { w.low_gate_group_unknown().bits(0x7fff) });
        init.gate_control()
            .modify(|_, w| w.high_gate_unknown().set_bit());
        init.aux_enable()
            .modify(|_, w| w.enable_unknown().set_bit());
        self.peripherals
            .wifi_mac_rx_dma
            .rx_control()
            .modify(|_, w| w.walker_enable().clear_bit());
    }
}
