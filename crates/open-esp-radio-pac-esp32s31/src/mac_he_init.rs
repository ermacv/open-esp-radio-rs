//! Generated-PAC ownership for bounded parts of complete `hal_he_init`.

use super::RadioRegisters;

impl RadioRegisters {
    /// Apply the complete prefix through `hal_init_tb_tx`.
    ///
    /// SOURCE: complete pinned `_oracles/libpp.a[hal_mac_ctl.o]` parent and
    /// leaves recorded as `BLOB_LIBPP_HAL_HE_INIT_PREFIX`. The transaction is
    /// twenty-two fresh-read RMW edges plus one final volatile status/sync read.
    /// It stops immediately before the parent calls `hal_init_tx_pwr`.
    pub fn initialize_mac_he_prefix(&mut self) {
        let init = &self.peripherals.wifi_mac_he_init_prefix;

        init.parent_control_edges()
            .modify(|_, w| w.clear_31().clear_bit());
        init.parent_control_edges()
            .modify(|_, w| w.clear_31().clear_bit().clear_30().clear_bit());

        let timing = init.bf_timing_control();
        timing.modify(|_, w| w.clear_21().clear_bit());
        timing.modify(|_, w| w.clear_23().clear_bit());
        timing.modify(|_, w| w.enable_19().set_bit());
        // SAFETY: all constants fit their complete generated fields.
        timing.modify(|_, w| unsafe { w.byte_one().bits(0x71) });
        timing.modify(|_, w| unsafe { w.low_byte().bits(0x20) });
        timing.modify(|_, w| unsafe { w.mode().bits(5) });
        timing.modify(|_, w| w.enable_25().set_bit());

        init.bf_enable().modify(|_, w| w.enable_unknown().set_bit());
        init.bf_vector_control()
            .modify(|_, w| unsafe { w.image_unknown().bits(0x801) });
        init.bf_high_image()
            .modify(|_, w| unsafe { w.image_unknown().bits(0x690) });
        init.bf_mode_control()
            .modify(|_, w| w.clear_unknown().clear_bit());
        init.bf_mode_control()
            .modify(|_, w| w.enable_unknown().set_bit());

        // Complete hal_he_set_bf_report_rate(1, 0x10, 0, 0) derives 0x20
        // and publishes high, middle and low fields in that order.
        let report = init.bf_report_rate();
        report.modify(|_, w| unsafe { w.high_rate().bits(0x20) });
        report.modify(|_, w| unsafe { w.middle_rate().bits(0x20) });
        report.modify(|_, w| unsafe { w.low_rate().bits(0x20) });

        // The complete leaf performs this read after all BF writes even though
        // its value is unused. Preserve it because MMIO reads may acknowledge
        // or synchronize hardware state.
        let _ = init.bf_sync_status_unknown().read().bits();

        init.he_queue_mode()
            .modify(|_, w| unsafe { w.mode_unknown().bits(2) });
        init.rx_field_control()
            .modify(|_, w| unsafe { w.image_unknown().bits(0x3c) });
        init.parent_enable()
            .modify(|_, w| w.enable_unknown().set_bit());
        init.tb_tx_control().modify(|_, w| w.clear_15().clear_bit());
        init.tb_tx_control().modify(|_, w| w.clear_14().clear_bit());
    }
}
