//! Exact bounded Bluetooth baseband initialization transaction.
//!
//! This module is deliberately crate-private. The transaction is the observed
//! MMIO portion of `bt_bb_v2_init_cmplx(1)`, but it is not a controller-enable
//! API: the vendor lifecycle first completes common PHY initialization. A
//! higher Bluetooth crate may expose this transaction only after it owns an
//! equivalent PHY-initialized typestate.

#![deny(unsafe_code)]

use super::{BluetoothTaskRegisters, device_fence};

const fn gain_force_low(parameter: u8) -> u8 {
    parameter & 0x7f
}

const fn gain_image(parameter: u8) -> u8 {
    parameter.wrapping_sub(5) & 0x7f
}

impl BluetoothTaskRegisters {
    /// Execute only the exact MMIO path of vendor `bt_bb_v2_init_cmplx(1)`.
    ///
    /// The caller-provided byte is the positional value read by the vendor
    /// body from the real linked `phy_param` object at offset `0x120`. Its
    /// higher-level meaning remains unassigned. The vendor diagnostic print
    /// selected by argument one is intentionally outside this hardware
    /// transaction and is omitted from the comparison effect contract.
    ///
    /// This hidden SPI is public only because Rust has no cross-crate friend
    /// visibility. It is unsafe so safe downstream code cannot bypass the
    /// post-common-PHY lifecycle owner.
    ///
    /// # Safety
    ///
    /// The caller must prove that controller clocks/resets are active, common
    /// PHY initialization completed for this same hardware owner, and
    /// `gain_parameter` came from that terminal PHY state. Every physical
    /// owner must remain retained until a verified last-owner PHY teardown;
    /// the task partition must not be reunited into cold ownership after this
    /// transaction without that teardown.
    #[allow(
        unsafe_code,
        reason = "the unsafe signature encodes the cross-crate common-PHY hardware prerequisite"
    )]
    #[doc(hidden)]
    pub unsafe fn initialize_baseband_v2_arg_one(&mut self, gain_parameter: u8) {
        self.initialize_baseband_v2_tx();
        self.initialize_baseband_v2_rx(gain_parameter);
        self.initialize_gaussian_1m_coefficients();
        self.initialize_gaussian_2m_coefficients();
        self.initialize_baseband_tx_timing();
        self.initialize_baseband_coexistence_defaults();
        self.initialize_baseband_cca_defaults();
        self.initialize_shared_receive_image();

        // The vendor function ends after the final shared receive image. The
        // Rust owner adds one reviewed device-ordering boundary before a later
        // lifecycle state may consume the initialized hardware.
        device_fence();
    }

    fn initialize_baseband_v2_tx(&mut self) {
        let baseband = &self.bluetooth.bt_v3_2_baseband;
        baseband
            .rx_setup_argument()
            .modify(|_, w| w.tx_argument_0().set(0));
        baseband
            .rx_setup_image_0()
            .modify(|_, w| w.tx_setup_image().set(0));
        baseband
            .gaussian_2m_coefficient_and_tx_config()
            .modify(|_, w| w.tx_set_low_byte().set(0xf5));
    }

    fn initialize_baseband_v2_rx(&mut self, gain_parameter: u8) {
        self.initialize_baseband_rx_setup();
        self.initialize_receive_compensation();
        self.initialize_receive_gain_offsets();
        self.initialize_receive_gain(gain_parameter);
        self.initialize_receive_rssi_thresholds();
        self.initialize_receive_targets();
        self.initialize_receive_restart();
        self.initialize_receive_recorrection();
        self.initialize_receive_detection();

        let baseband = &self.bluetooth.bt_v3_2_baseband;
        baseband
            .rx_correlator_control()
            .modify(|_, w| w.config_value().set(8));
        baseband
            .rx_dpo_control()
            .modify(|_, w| w.config_force_zero_19().clear_bit());
        baseband
            .rx_dpo_control()
            .modify(|_, w| w.config_value().set(2));
        baseband
            .rx_filter_select()
            .modify(|_, w| w.select_1().set(2));
        baseband
            .rx_filter_select()
            .modify(|_, w| w.select_5().set(1));
        baseband
            .rx_filter_select()
            .modify(|_, w| w.select_4().set(2));
        baseband
            .rx_filter_select()
            .modify(|_, w| w.select_3().set(0));
        baseband
            .rx_filter_select()
            .modify(|_, w| w.select_2().set(0));
        baseband
            .rx_filter_select()
            .modify(|_, w| w.select_0().set(2));

        self.initialize_shared_receive_prefix();
        self.bluetooth
            .bt_v3_2_baseband
            .rx_correlator_control()
            .modify(|_, w| w.final_force_zero_13().clear_bit());
    }

    fn initialize_baseband_rx_setup(&mut self) {
        self.bluetooth
            .bt_v3_2_baseband
            .rx_setup_argument()
            .modify(|_, w| w.argument_0().set(4));
        self.bluetooth
            .bt_v3_2_baseband
            .rx_setup_image_0()
            .modify(|_, w| w.config_image().set(0x0199a));
        self.shared_radio
            .shared_radio_init_control
            .control()
            .modify(|_, w| w.bt_rx_setup_bit_31_unknown().clear_bit());
        self.bluetooth
            .bt_v3_2_baseband
            .rx_setup_control_1()
            .modify(|_, w| w.enable().set_bit());

        let btagc = &self.radio_phy.peripherals.phy_btagc_recovered;
        btagc
            .agc_config_00d0()
            .modify(|_, w| w.config_value_high().set(20));
        btagc
            .agc_config_00d0()
            .modify(|_, w| w.config_value_low().set(20));
        btagc
            .agc_config_00d4()
            .modify(|_, w| w.config_value().set(0x3c0));
        btagc
            .rx_config_008c()
            .modify(|_, w| w.config_force_zero_29().clear_bit());
        btagc
            .rx_config_0088()
            .modify(|_, w| w.config_force_zero_29().clear_bit());
        btagc
            .rx_config_0088()
            .modify(|_, w| w.config_force_one_18().set_bit());

        let baseband = &self.bluetooth.bt_v3_2_baseband;
        baseband
            .tx_cca_control_1()
            .modify(|_, w| w.rx_setup_enable().set_bit());
        baseband
            .tx_cca_control_2()
            .modify(|_, w| w.rx_setup_disable().clear_bit());
        btagc
            .cte_dc_shift()
            .modify(|_, w| w.rx_config_value().set(7));
        btagc.cte_dc_shift().modify(|_, w| w.dc_shift_max().set(7));
        baseband
            .tx_cca_control_2()
            .modify(|_, w| w.rx_setup_enable().set_bit());
        btagc
            .rx_config_004c()
            .modify(|_, w| w.config_force_zero_26().clear_bit());
        btagc
            .rx_config_008c()
            .modify(|_, w| w.config_force_zero_29().clear_bit());
        baseband
            .rx_setup_control_0()
            .modify(|_, w| w.enable().set_bit());

        let shared = &self.shared_radio.zbbb_radio_control;
        shared
            .rx_setup_control()
            .modify(|_, w| w.config_force_zero_8().clear_bit());
        shared
            .rx_setup_image()
            .modify(|_, w| w.config_image().set(0x001cb));
        shared
            .rx_setup_control()
            .modify(|_, w| w.config_force_zero_low().set(0));
    }

    fn initialize_receive_compensation(&mut self) {
        let control = self
            .radio_phy
            .peripherals
            .phy_btagc_recovered
            .rx_comp_control();
        control.modify(|_, w| w.dynamic_bits_7_13().set(6));
        control.modify(|_, w| w.dynamic_bits_0_6().set(6));
        control.modify(|_, w| w.finite_bits_24_28().set(6));
        control.modify(|_, w| w.finite_bits_19_23().set(0));
        control.modify(|_, w| w.finite_bits_14_18().set(8));
    }

    fn initialize_receive_gain_offsets(&mut self) {
        let btagc = &self.radio_phy.peripherals.phy_btagc_recovered;
        let first = btagc.gain_offset_word_0_opaque();
        first.modify(|_, w| w.positional_bits_5_9().set(4));
        first.modify(|_, w| w.positional_bits_0_4().set(4));
        first.modify(|_, w| w.positional_bits_16_23().set(0x50));
        first.modify(|_, w| w.positional_bits_24_31().set(0x50));

        let second = btagc.gain_offset_word_1_opaque();
        second.modify(|_, w| w.positional_bits_24_31().set(0x50));
        second.modify(|_, w| w.positional_bits_16_23().set(0x50));
    }

    fn initialize_receive_gain(&mut self, parameter: u8) {
        self.radio_phy
            .peripherals
            .phy_baseband_config_oracle
            .baseband_init_7cd0()
            .modify(|_, w| w.init_low_unknown().set(0x0f).init_high_unknown().set(0x0f));

        let btagc = &self.radio_phy.peripherals.phy_btagc_recovered;
        btagc
            .rx_gain_force_opaque()
            .modify(|_, w| w.dynamic_bits_0_6().set(gain_force_low(parameter)));
        btagc
            .agc_gain_image()
            .modify(|_, w| w.dynamic_bits_2_8().set(gain_image(parameter)));
    }

    fn initialize_receive_rssi_thresholds(&mut self) {
        let btagc = &self.radio_phy.peripherals.phy_btagc_recovered;
        btagc
            .shared_rx_sense_and_detect_00a0()
            .modify(|_, w| w.positional_bits_16_23().set(0x9c));
        btagc
            .shared_rx_sense_and_detect_00a8()
            .modify(|_, w| w.positional_bits_17_24().set(0x9c));
        btagc
            .shared_rx_sense_and_detect_00a8()
            .modify(|_, w| w.positional_bits_4_11().set(0x88));
        btagc
            .shared_rx_sense_and_detect_00b8()
            .modify(|_, w| w.positional_bits_12_19().set(0x88));
    }

    fn initialize_receive_targets(&mut self) {
        let btagc = &self.radio_phy.peripherals.phy_btagc_recovered;
        btagc
            .cte_agc_target()
            .modify(|_, w| w.baseband_target_value().set(0x1d4));
        btagc
            .agc_recorrect_and_target_00b0()
            .modify(|_, w| w.target_bits_23_31().set(0x1d4));
        btagc
            .rx_gain_force_opaque()
            .modify(|_, w| w.target_bits_13_21().set(0x1d4));
        btagc
            .rx_config_008c()
            .modify(|_, w| w.target_bits_0_8().set(0x1d4));
        btagc
            .rx_config_008c()
            .modify(|_, w| w.target_bits_9_17().set(0x1dc));
        btagc
            .cca_config()
            .modify(|_, w| w.target_bits_14_22().set(0x1ce));
        btagc
            .agc_recorrect_and_target_00b4()
            .modify(|_, w| w.target_bits_23_31().set(0x1ce));
    }

    fn initialize_receive_restart(&mut self) {
        let btagc = &self.radio_phy.peripherals.phy_btagc_recovered;
        btagc
            .rx_config_0088()
            .modify(|_, w| w.config_force_zero_30().clear_bit());
        btagc
            .agc_recorrect_and_restart_00bc()
            .modify(|_, w| w.restart_bit_31().set_bit());
        btagc
            .agc_restart_config_0084()
            .modify(|_, w| w.finite_bits_24_31().set(0xf4));

        let e0 = btagc.agc_restart_image_00e0();
        e0.modify(|_, w| w.bits_24_31().set(0x14));
        e0.modify(|_, w| w.bits_16_23().set(0x0e));
        e0.modify(|_, w| w.bits_8_15().set(0x0e));
        let e4 = btagc.agc_restart_image_00e4();
        e4.modify(|_, w| w.bits_16_23().set(0xec));
        e4.modify(|_, w| w.bits_8_15().set(0xf2));
        e4.modify(|_, w| w.bits_0_7().set(0xf2));
        e0.modify(|_, w| w.bits_0_7().set(0x0f));
        let e8 = btagc.agc_restart_image_00e8();
        e8.modify(|_, w| w.bits_24_31().set(0xf1));
        let ec = btagc.agc_restart_image_00ec();
        ec.modify(|_, w| w.bits_24_31().set(0x2d));
        ec.modify(|_, w| w.bits_8_15().set(0xd3));
        let f0 = btagc.agc_restart_image_00f0();
        f0.modify(|_, w| w.bits_24_31().set(0xa6));
        e4.modify(|_, w| w.bits_24_31().set(0x1e));
        e8.modify(|_, w| w.bits_16_23().set(0xe2));
        ec.modify(|_, w| w.bits_16_23().set(0x28));
        ec.modify(|_, w| w.bits_0_7().set(0xd8));
        f0.modify(|_, w| w.bits_16_23().set(0xa6));

        let image_100 = btagc.agc_restart_image_0100();
        image_100.modify(|_, w| w.bits_20_23().set(6));
        image_100.modify(|_, w| w.bits_24_27().set(6));
        image_100.modify(|_, w| w.bits_16_19().set(8));
        image_100.modify(|_, w| w.bits_12_15().set(0x0a));
        image_100.modify(|_, w| w.bits_28_31().set(6));
        e8.modify(|_, w| w.bits_10_15().set(3));

        let baseband = &self.bluetooth.bt_v3_2_baseband;
        baseband
            .rx_correlator_control()
            .modify(|_, w| w.rx_setup_enable().set_bit());
        let restart = baseband.agc_restart_control();
        restart.modify(|_, w| w.restart_bit_21_unknown().set_bit());
        restart.modify(|_, w| w.restart_bit_20_unknown().set_bit());
        restart.modify(|_, w| w.restart_bit_19_unknown().set_bit());
        restart.modify(|_, w| w.restart_bit_10_unknown().set_bit());
        restart.modify(|_, w| w.restart_bit_9_unknown().set_bit());
        restart.modify(|_, w| w.restart_bit_8_unknown().set_bit());
        restart.modify(|_, w| w.restart_bit_7_unknown().set_bit());
        restart.modify(|_, w| w.restart_bit_6_unknown().set_bit());

        let bits = btagc.agc_restart_bits_00dc();
        bits.modify(|_, w| w.restart_bit_31().set_bit());
        bits.modify(|_, w| w.restart_bit_30().set_bit());
        bits.modify(|_, w| w.restart_bit_29().set_bit());
        bits.modify(|_, w| w.restart_bit_27().set_bit());
        bits.modify(|_, w| w.restart_bit_28().set_bit());

        let f8 = btagc.agc_restart_image_00f8();
        f8.modify(|_, w| w.bits_2_7().set(0x0c));
        f8.modify(|_, w| w.bits_8_13().set(0x0c));
        f8.modify(|_, w| w.bits_14_19().set(0x20));
        f8.modify(|_, w| w.bits_26_31().set(0x0c));
        f8.modify(|_, w| w.bits_20_25().set(5));
    }

    fn initialize_receive_recorrection(&mut self) {
        let btagc = &self.radio_phy.peripherals.phy_btagc_recovered;
        btagc
            .agc_recorrect_and_target_00b4()
            .modify(|_, w| w.recorrect_bit_8().set_bit());
        btagc
            .agc_recorrect_and_target_00b4()
            .modify(|_, w| w.recorrect_bits_9_13().set(0x0a));
        btagc
            .agc_recorrect_and_restart_00bc()
            .modify(|_, w| w.recorrect_bit_5().set_bit().recorrect_bit_7().set_bit());
        btagc
            .gain_offset_word_1_opaque()
            .modify(|_, w| w.positional_bits_13().set_bit());
        btagc.agc_recorrect_and_restart_00bc().modify(|_, w| {
            w.recorrect_bit_6()
                .clear_bit()
                .recorrect_bit_8()
                .clear_bit()
        });
        btagc
            .gain_offset_word_1_opaque()
            .modify(|_, w| w.positional_bits_5().set_bit());
        btagc
            .gain_offset_word_1_opaque()
            .modify(|_, w| w.positional_bits_11_12().set(3));
        btagc
            .gain_offset_word_1_opaque()
            .modify(|_, w| w.positional_bits_6_10().set(20));
        btagc
            .agc_recorrect_and_target_00b0()
            .modify(|_, w| w.recorrect_bits_18_22().set(0x18));
        btagc
            .agc_recorrect_and_target_00b0()
            .modify(|_, w| w.recorrect_bits_13_17().set(0x18));
        btagc
            .gain_offset_word_1_opaque()
            .modify(|_, w| w.positional_bits_0_4().set(20));
        btagc
            .agc_recorrect_control_006c()
            .modify(|_, w| w.finite_bits_24_31().set(0x0f));
    }

    fn initialize_receive_detection(&mut self) {
        let btagc = &self.radio_phy.peripherals.phy_btagc_recovered;
        btagc
            .shared_rx_sense_and_detect_00a0()
            .modify(|_, w| w.positional_bits_24_27().set(4));
        let c0 = btagc.agc_detect_config_00c0();
        c0.modify(|_, w| w.bits_5_9().set(0x0a));
        btagc
            .shared_rx_sense_and_detect_00a8()
            .modify(|_, w| w.positional_bits_12_16().set(4));
        btagc
            .shared_rx_sense_and_detect_00a8()
            .modify(|_, w| w.positional_bits_0_3().set(7));
        c0.modify(|_, w| w.bits_10_14().set(0x0a));
        btagc
            .shared_rx_sense_and_detect_00b8()
            .modify(|_, w| w.positional_bits_8_11().set(7));
        let c4 = btagc.agc_detect_config_00c4();
        c4.modify(|_, w| w.bits_10_14().set(0x0f));
        c0.modify(|_, w| w.bits_24_31().set(0x9c));
        c0.modify(|_, w| w.bits_20_23().set(7));
        c0.modify(|_, w| w.bits_15_19().set(0x0a));
        c4.modify(|_, w| w.bits_24_31().set(0x9c));
        c4.modify(|_, w| w.bits_20_23().set(0x0a));
        c4.modify(|_, w| w.bits_15_19().set(0x0f));
    }

    fn initialize_gaussian_1m_coefficients(&mut self) {
        let baseband = &self.bluetooth.bt_v3_2_baseband;
        let c0 = baseband.gaussian_1m_coefficient_0();
        c0.modify(|_, w| w.bits_28_31().set(0));
        c0.modify(|_, w| w.bits_23_27().set(0));
        c0.modify(|_, w| w.bits_17_22().set(0));
        c0.modify(|_, w| w.bits_10_16().set(3));
        c0.modify(|_, w| w.bits_2_9().set(0x13));
        let c1 = baseband.gaussian_1m_coefficient_1();
        c1.modify(|_, w| w.bits_23_31().set(0x5f));
        c1.modify(|_, w| w.bits_13_22().set(0x140));
        c1.modify(|_, w| w.bits_2_12().set(0x2f2));
        let c2 = baseband.gaussian_1m_coefficient_2();
        c2.modify(|_, w| w.bits_21_31().set(0x50d));
        c2.modify(|_, w| w.bits_10_20().set(0x6bf));
        let c3 = baseband.gaussian_1m_coefficient_3();
        c3.modify(|_, w| w.bits_21_31().set(0x7a0));
        c3.modify(|_, w| w.bits_10_20().set(0x7e9));
    }

    fn initialize_gaussian_2m_coefficients(&mut self) {
        let baseband = &self.bluetooth.bt_v3_2_baseband;
        let c0 = baseband.gaussian_2m_coefficient_and_tx_config();
        c0.modify(|_, w| w.gaussian_bits_25_31().set(0));
        c0.modify(|_, w| w.gaussian_bits_17_24().set(7));
        c0.modify(|_, w| w.gaussian_bits_8_16().set(0x69));
        let c1 = baseband.gaussian_2m_coefficient_1();
        c1.modify(|_, w| w.bits_22_31().set(0x258));
        c1.modify(|_, w| w.bits_11_21().set(0x5a7));
        c1.modify(|_, w| w.bits_0_10().set(0x78f));
    }

    fn initialize_baseband_tx_timing(&mut self) {
        self.radio_phy
            .peripherals
            .phy_baseband_config_oracle
            .tx_pa_control_1()
            .modify(|_, w| w.pa_on_bt_delay().set(0x96));
        self.bluetooth
            .bt_v3_2_baseband
            .le_tx_on_delay()
            .modify(|_, w| {
                w.force_zero_bits_16_18()
                    .set(0)
                    .encoded_value_minus_10()
                    .set(50)
            });
        let cca = self.bluetooth.bt_v3_2_baseband.tx_cca_control_0();
        cca.modify(|_, w| {
            w.period_force_zero_20_22()
                .set(0)
                .period_argument_0_minus_argument_1_image()
                .set(0x2c)
        });
        cca.modify(|_, w| w.period_argument_0_image().set(0x1ff));
        self.shared_radio
            .shared_baseband_tx_timing
            .auxiliary_tx_on_delay()
            .modify(|_, w| w.encoded_image().set(0x190));
    }

    fn initialize_baseband_coexistence_defaults(&mut self) {
        let coex = self.bluetooth.bt_v3_2_baseband.coex_config();
        coex.modify(|_, w| w.config_force_one_18().set_bit());
        coex.modify(|_, w| w.config_force_one_20().set_bit());
    }

    fn initialize_baseband_cca_defaults(&mut self) {
        let btagc = &self.radio_phy.peripherals.phy_btagc_recovered;
        let cca = btagc.cca_config();
        cca.modify(|_, w| w.config_value_0().set(0x1ce));
        cca.modify(|_, w| w.config_value_1().set(0x1e));
        cca.modify(|_, w| w.config_force_one_23().set_bit());
        self.bluetooth
            .bt_v3_2_baseband
            .tx_cca_control_1()
            .modify(|_, w| w.default_config_value().set(5));
    }

    fn initialize_shared_receive_prefix(&mut self) {
        let shared = &self.shared_radio.zbbb_radio_control;
        shared
            .shared_rx_init_image_0()
            .modify(|_, w| w.positional_image_21_31().set(0x792));
        shared
            .rx_setup_control()
            .modify(|_, w| w.shared_init_image_10_20().set(0x79c));
        shared
            .shared_rx_init_image_1()
            .modify(|_, w| w.positional_image_0_10().set(0x7a6));
        shared
            .shared_rx_init_image_0()
            .modify(|_, w| w.positional_image_13_20().set(0xa6));
        shared
            .shared_rx_init_image_4()
            .modify(|_, w| w.positional_image_20_30().set(0x7e1));
        shared
            .shared_rx_init_image_4()
            .modify(|_, w| w.positional_image_9_19().set(0x7ed));
        shared
            .shared_rx_init_image_2()
            .modify(|_, w| w.positional_image_8_31().set(0x07a120));
        shared
            .shared_rx_init_image_3()
            .modify(|_, w| w.positional_image_8_31().set(0xf85edf));
        shared
            .shared_rx_init_control_0()
            .modify(|_, w| w.positional_force_zero_1().clear_bit());
        shared
            .shared_rx_init_control_1()
            .modify(|_, w| w.positional_force_one_30().set_bit());
        shared
            .shared_rx_init_image_2()
            .modify(|_, w| w.positional_image_0_7().set(0xf1));
        shared
            .shared_rx_init_image_4()
            .modify(|_, w| w.positional_force_one_0().set_bit());
    }

    fn initialize_shared_receive_image(&mut self) {
        self.initialize_shared_receive_prefix();
        self.shared_radio
            .zbbb_radio_control
            .rx_setup_image()
            .modify(|_, w| w.config_image().set(0x0019f));
    }
}

#[cfg(test)]
mod tests {
    use super::{gain_force_low, gain_image};

    #[test]
    fn linked_parameter_byte_uses_the_exact_vendor_transforms() {
        assert_eq!(gain_force_low(0), 0);
        assert_eq!(gain_image(0), 0x7b);
        assert_eq!(gain_force_low(0x7f), 0x7f);
        assert_eq!(gain_image(0x7f), 0x7a);
        assert_eq!(gain_force_low(0xff), 0x7f);
        assert_eq!(gain_image(0xff), 0x7a);
    }
}
