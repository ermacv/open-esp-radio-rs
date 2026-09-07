//! Generated-PAC ownership for the direct `mac_txrx_init` prefix.

#![forbid(unsafe_code)]

use crate::WifiRadioRegisters;

impl WifiRadioRegisters {
    /// Apply all three ordered fresh-read updates of complete rev0 ROM
    /// `phy_sifs_reg_init`.
    pub fn initialize_phy_wifi_sifs(&mut self) {
        let callbacks = &self.peripherals.wifi_mac.wifi_mac_txrx_callbacks;
        callbacks
            .delay_secondary()
            .modify(|_, w| w.high_delay_unknown().set(0xea));
        callbacks
            .delay_primary()
            .modify(|_, w| w.rx_cck_delay().set(0x3b8));
        callbacks
            .delay_primary()
            .modify(|_, w| w.low_delay_unknown().set(0xf0));
    }

    /// Apply all eighteen direct RMW edges before the first HE callback.
    ///
    /// SOURCE: complete pinned `libpp.a[hal_mac.o]::mac_txrx_init`,
    /// offsets `0x08..0xd0`. Callback effects and the direct suffix remain
    /// outside this deliberately bounded operation.
    pub fn initialize_mac_txrx_prefix(&mut self) {
        let init = &self.peripherals.wifi_mac.wifi_mac_txrx_prefix;

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
            init.rx_queue_default(queue).modify(|_, w| {
                w.high_23_16_unknown()
                    .set(0)
                    .queue_bit_24_unknown()
                    .clear_bit()
                    .queue_bit_25_unknown()
                    .clear_bit()
                    .queue_bit_26_unknown()
                    .clear_bit()
                    .high_31_27_unknown()
                    .set(0)
            });
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
        init.timing_control()
            .modify(|_, w| w.timing_image_unknown().set(0x1b));
        init.shared_enable_control()
            .modify(|_, w| w.enable_group_unknown().set(3));
    }

    /// Apply the on-chip paths of all three HE callbacks in `mac_txrx_init`.
    ///
    /// SOURCE: complete pinned `libpp.a[hal_mac_ctl.o]`
    /// `hal_he_set_mac_delay`, `hal_he_set_ack_rate(0)` and
    /// `hal_he_set_bbrxhung_time(0)`. `delay_slot` is `_random() % 11`.
    ///
    /// Returns false without touching hardware when the slot is outside the
    /// complete vendor range `0..=10`.
    pub fn initialize_mac_txrx_callbacks(&mut self, delay_slot: u8) -> bool {
        if delay_slot > 10 {
            return false;
        }

        let callbacks = &self.peripherals.wifi_mac.wifi_mac_txrx_callbacks;
        callbacks
            .delay_primary()
            .modify(|_, w| w.rx_cck_delay().set(0x3b9));
        callbacks
            .delay_primary()
            .modify(|_, w| w.low_delay_unknown().set(0xf5 + u16::from(delay_slot)));
        callbacks
            .delay_primary()
            .modify(|_, w| w.high_delay_unknown().set(0x5e));
        callbacks
            .delay_secondary()
            .modify(|_, w| w.high_delay_unknown().set(0xfa + u16::from(delay_slot)));
        callbacks
            .delay_secondary()
            .modify(|_, w| w.tx_cck_delay().set(0x276));

        crate::svd::zero_based_field_write::mac_ack_rate_table(callbacks, 0x0b, 0x0a, 0x09, 0);
        crate::svd::zero_based_field_write::mac_cts_rate_table(callbacks, 0x0b, 0x0a, 0x09, 0);
        crate::svd::zero_based_field_write::mac_ack_cck_rate_table(callbacks, 0, 1, 5, 0);
        crate::svd::zero_based_field_write::mac_cts_cck_rate_table(callbacks, 0, 1, 5, 0);
        callbacks
            .bb_rx_hang_control()
            .modify(|_, w| w.timeout_unknown().set(0x00f));
        true
    }

    /// Apply all nine direct RMW edges after the three HE callbacks.
    ///
    /// SOURCE: complete pinned `libpp.a[hal_mac.o]::mac_txrx_init`,
    /// offsets `0xee..0x16e`.
    pub fn initialize_mac_txrx_suffix(&mut self) {
        let callbacks = &self.peripherals.wifi_mac.wifi_mac_txrx_callbacks;
        callbacks
            .bb_rx_hang_control()
            .modify(|_, w| w.txrx_suffix_first_enable_unknown().set_bit());
        callbacks
            .bb_rx_hang_control()
            .modify(|_, w| w.txrx_suffix_second_enable_unknown().set_bit());
        let init = &self.peripherals.wifi_mac.wifi_mac_txrx_suffix;
        init.default_image_a()
            .modify(|_, w| w.low_image_unknown().set(0x0f0));
        self.peripherals
            .shared_radio
            .shared_radio_init_control
            .control()
            .modify(|_, w| w.wifi_init_low_image_unknown().set(0x0f0));
        init.field_control().modify(|_, w| w.field_unknown().set(4));
        init.gate_control()
            .modify(|_, w| w.low_gate_group_unknown().set(0x7fff));
        init.gate_control()
            .modify(|_, w| w.high_gate_unknown().set_bit());
        init.aux_enable()
            .modify(|_, w| w.enable_unknown().set_bit());
        self.peripherals
            .wifi_mac
            .wifi_mac_rx_dma
            .rx_control()
            .modify(|_, w| w.walker_enable().clear_bit());
    }
}
