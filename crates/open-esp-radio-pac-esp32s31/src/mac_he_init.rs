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
        let options = self
            .peripherals
            .wifi_mac_he_init_suffix
            .he_default_control();

        options.modify(|_, w| w.ul_mu_disable().clear_bit());
        options.modify(|_, w| {
            w.ul_mu_disable()
                .clear_bit()
                .ul_mu_data_disable()
                .clear_bit()
        });

        let timing = init.bf_timing_control();
        timing.modify(|_, w| w.non_tb_beam_ru_select().clear_bit());
        timing.modify(|_, w| w.clear_unknown_23().clear_bit());
        timing.modify(|_, w| w.he_beam_hw_sequence_enable().set_bit());
        // SAFETY: all constants fit their complete generated fields.
        timing.modify(|_, w| unsafe { w.he_beam_ndp_time().bits(0x71) });
        timing.modify(|_, w| unsafe {
            w.bf_memory_write_enable()
                .clear_bit()
                .he_beam_bfrp_time()
                .bits(0x10)
        });
        timing.modify(|_, w| unsafe { w.he_beam_hw_sequence_select().bits(5) });
        timing.modify(|_, w| w.enable_unknown_25().set_bit());

        init.bf_enable().modify(|_, w| w.enable_unknown().set_bit());
        init.bf_vector_control()
            .modify(|_, w| unsafe { w.image_unknown().bits(0x801) });
        init.bf_high_image()
            .modify(|_, w| unsafe { w.image_unknown().bits(0x690) });
        init.bf_mode_control()
            .modify(|_, w| w.high_selector_class().clear_bit());
        init.bf_mode_control()
            .modify(|_, w| w.high_selector_parity().set_bit());

        // Complete hal_he_set_bf_report_rate(1, 0x10, 0, 0) derives signal
        // mode one, normalized rate zero, DCM false and ER-SU false, then
        // publishes the 16-QAM, QPSK and BPSK profiles in that order.
        let report = init.bf_report_rate();
        report.modify(|_, w| unsafe {
            w.qam16_rate()
                .bits(0)
                .qam16_signal_mode()
                .bits(1)
                .qam16_dcm()
                .clear_bit()
                .qam16_ersu()
                .clear_bit()
        });
        report.modify(|_, w| unsafe {
            w.qpsk_rate()
                .bits(0)
                .qpsk_signal_mode()
                .bits(1)
                .qpsk_dcm()
                .clear_bit()
                .qpsk_ersu()
                .clear_bit()
        });
        report.modify(|_, w| unsafe {
            w.bpsk_rate()
                .bits(0)
                .bpsk_signal_mode()
                .bits(1)
                .bpsk_dcm()
                .clear_bit()
                .bpsk_ersu()
                .clear_bit()
        });

        // The complete leaf performs this read after all BF writes even though
        // its value is unused. Preserve it because MMIO reads may acknowledge
        // or synchronize hardware state.
        let _ = self
            .peripherals
            .phy_agc_oracle
            .agc_init_high_control()
            .read()
            .bits();

        self.peripherals
            .wifi_mac_rx_bssid_list
            .control()
            .modify(|_, w| unsafe { w.rx_control_9_bssid_position().bits(2) });
        init.rx_field_control()
            .modify(|_, w| unsafe { w.timeout_seconds().bits(0x3c) });
        init.parent_enable()
            .modify(|_, w| w.enable_unknown().set_bit());
        init.tb_tx_control().modify(|_, w| w.clear_15().clear_bit());
        init.tb_tx_control().modify(|_, w| w.clear_14().clear_bit());
    }
}
