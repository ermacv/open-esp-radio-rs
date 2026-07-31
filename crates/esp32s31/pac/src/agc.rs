//! Ownership-bound AGC control leaves.

use super::RadioRegisters;

const fn agc_parameter_offset(parameter: u8) -> u8 {
    parameter.wrapping_add(0x50)
}

const fn rx_11b_values(enabled: bool) -> (u8, u8, u8, u8, u8) {
    if enabled {
        (0x3f, 0x21, 0x21, 0x03, 0x09)
    } else {
        (0x3e, 0x18, 0x18, 0x04, 0x06)
    }
}

impl RadioRegisters {
    /// Enable or disable the complete three-edge PHY low-rate path.
    ///
    /// SOURCE: complete rev0 ROM `phy_enable_low_rate` at `0x2f82_5210`
    /// and `phy_disable_low_rate` at `0x2f82_5230`, both size `0x20`.
    /// The two primary-word bits remain separate RMWs exactly as in ROM.
    pub fn configure_phy_low_rate(&mut self, enabled: bool) {
        let agc = &self.peripherals.phy_agc_oracle;
        if enabled {
            agc.low_rate_primary_control()
                .modify(|_, w| w.low_rate_enable_first().set_bit());
            agc.low_rate_primary_control()
                .modify(|_, w| w.low_rate_enable_second().set_bit());
            agc.low_rate_secondary_control()
                .modify(|_, w| w.low_rate_enable().set_bit());
        } else {
            agc.low_rate_primary_control()
                .modify(|_, w| w.low_rate_enable_first().clear_bit());
            agc.low_rate_primary_control()
                .modify(|_, w| w.low_rate_enable_second().clear_bit());
            agc.low_rate_secondary_control()
                .modify(|_, w| w.low_rate_enable().clear_bit());
        }
    }

    /// Apply all fourteen internal MMIO edges of `phy_bb_agc_reg_update`.
    pub fn update_agc_baseband_registers(&mut self) {
        let agc = &self.peripherals.phy_agc_oracle;
        // SAFETY: every full-word image below is an instruction-exact store
        // from the complete rev0 ROM body. Write-only words have no useful
        // reset image, while R/W words are intentionally replaced in full.
        unsafe {
            agc.agc_update_8070_opaque()
                .write_with_zero(|w| w.bits(0x0000_08c7));
            agc.agc_update_78a4_opaque()
                .write_with_zero(|w| w.bits(0x0001_721f));
        }
        agc.rx_11b_mode_control()
            .modify(|_, w| w.bb_agc_update_clear_unknown().clear_bit());
        unsafe {
            agc.agc_update_8010_opaque()
                .write_with_zero(|w| w.bits(0x0008_52a1));
            agc.agc_update_8018_opaque()
                .write_with_zero(|w| w.bits(0x0060_0030));
            agc.agc_update_801c_opaque()
                .write_with_zero(|w| w.bits(0x0100_00a0));
            agc.agc_update_8020_opaque()
                .write_with_zero(|w| w.bits(0x0000_0180));
            agc.agc_update_8028_opaque()
                .write_with_zero(|w| w.bits(0xc040_3020));
            agc.agc_update_802c_opaque()
                .write_with_zero(|w| w.bits(0x0100_0080));
        }
        // SAFETY: seven is the complete image of the generated three-bit
        // field.
        agc.agc_update_8078_control()
            .modify(|_, w| unsafe { w.bb_agc_update_set_unknown().bits(7) });
        unsafe {
            agc.rx_11b_path_control_0()
                .write_with_zero(|w| w.bits(0xfe3f_e1fe));
            agc.agc_update_7048_opaque()
                .write_with_zero(|w| w.bits(0xff7d_a4f3));
            agc.rx_11b_window_control()
                .write_with_zero(|w| w.bits(0x06ac_c7c8));
            agc.rx_11b_path_control_1()
                .write_with_zero(|w| w.bits(0xb220_8553));
        }
    }

    /// Select the complete rev0 ROM AGC enable or disable sequence.
    pub fn set_agc_enabled(&mut self, enabled: bool) {
        let agc = &self.peripherals.phy_agc_oracle;
        if !enabled {
            agc.agc_antenna_control()
                .modify(|_, w| w.agc_disable_unknown().set_bit());
            return;
        }

        agc.agc_antenna_control()
            .modify(|_, w| w.agc_disable_unknown().clear_bit());
        agc.agc_shared_control()
            .modify(|_, w| w.pulse_unknown().set_bit());
        agc.agc_shared_control()
            .modify(|_, w| w.pulse_unknown().clear_bit());
    }

    /// Publish both complete pinned RX-compensation fields in order.
    pub fn configure_rx_compensation(&mut self) {
        let agc = &self.peripherals.phy_agc_oracle;
        // SAFETY: 0xed fits both generated eight-bit fields.
        agc.agc_shared_control()
            .modify(|_, w| unsafe { w.rx_compensation_low_unknown().bits(0xed) });
        agc.rx_compensation_high_control()
            .modify(|_, w| unsafe { w.rx_compensation_high_unknown().bits(0xed) });
    }

    /// Pulse the generated DC-memory clear field through two fresh RMWs.
    pub fn clear_agc_dc_memory(&mut self) {
        let control = self.peripherals.phy_agc_oracle.dc_memory_control();
        control.modify(|_, w| w.clear_pulse_unknown().set_bit());
        control.modify(|_, w| w.clear_pulse_unknown().clear_bit());
    }

    /// Apply the two MMIO edges after the PBus work-mode 1 µs delay.
    pub fn configure_pbus_work_mode_pulse(&mut self) {
        let control = self.peripherals.phy_agc_oracle.agc_shared_control();
        // SAFETY: 0x32 fits the generated eight-bit field.
        control.modify(|_, w| unsafe { w.control_high_unknown().bits(0x32) });
        control.modify(|_, w| w.pulse_unknown().set_bit());
    }

    /// Clear the PBus work-mode pulse after the caller-owned 2 µs delay.
    pub fn clear_pbus_work_mode_pulse(&mut self) {
        self.peripherals
            .phy_agc_oracle
            .agc_shared_control()
            .modify(|_, w| w.pulse_unknown().clear_bit());
    }

    /// Apply all ten fresh-read updates of complete `phy_agc_reg_init`.
    pub fn initialize_agc_registers(&mut self, parameter_121: u8, parameter_120: u8) {
        let agc = &self.peripherals.phy_agc_oracle;
        let gain_minus_one = parameter_121.wrapping_sub(1) & 0x7f;
        // SAFETY: all dynamic values are explicitly bounded to their
        // generated seven- or eight-bit field widths; fixed literals fit the
        // SVD-described fields.
        agc.rx_gain_limit_control()
            .modify(|_, w| unsafe { w.rx_gain_limit_unknown().bits(gain_minus_one) });
        agc.agc_gain_limit_low()
            .modify(|_, w| unsafe { w.parameter_minus_one_unknown().bits(gain_minus_one) });
        agc.agc_shared_control()
            .modify(|_, w| unsafe { w.rx_gain_index_unknown().bits(parameter_121 & 0x7f) });
        agc.agc_saturation_control()
            .modify(|_, w| unsafe { w.low_unknown().bits(0x0bb8) });
        agc.agc_parameter_control()
            .modify(|_, w| unsafe { w.parameter_121_unknown().bits(parameter_121) });
        agc.agc_parameter_control().modify(|_, w| unsafe {
            w.parameter_120_offset_unknown()
                .bits(agc_parameter_offset(parameter_120))
        });
        agc.agc_shared_control()
            .modify(|_, w| unsafe { w.control_high_unknown().bits(0x32) });
        agc.agc_shared_control()
            .modify(|_, w| w.pulse_unknown().set_bit());
        agc.agc_shared_control()
            .modify(|_, w| w.pulse_unknown().clear_bit());
        agc.agc_init_high_control()
            .modify(|_, w| unsafe { w.init_high_unknown().bits(0xd2) });
    }

    /// Apply all three fresh-read antenna initialization updates.
    pub fn configure_agc_antenna(&mut self) {
        let agc = &self.peripherals.phy_agc_oracle;
        // SAFETY: zero fits the generated eleven-bit clear field.
        agc.antenna_control_0().modify(|_, w| unsafe {
            w.low_clear_unknown()
                .bits(0)
                .bit_12_clear_unknown()
                .clear_bit()
        });
        // SAFETY: all three 0x34/0x1e values fit their generated fields.
        agc.agc_antenna_control()
            .modify(|_, w| unsafe { w.antenna_init_unknown().bits(0x34) });
        agc.antenna_control_2()
            .modify(|_, w| unsafe { w.low_unknown().bits(0x1e).high_unknown().bits(0x1e) });
    }

    /// Apply either complete branch of rev0 ROM `phy_rfrx_sat_rst`.
    pub fn configure_rf_rx_saturation(&mut self, enabled: bool) {
        let agc = &self.peripherals.phy_agc_oracle;
        // SAFETY: this is the complete full-word store from the ROM body.
        unsafe {
            agc.rf_rx_saturation_config()
                .write_with_zero(|w| w.bits(0x0000_0404));
        }
        // SAFETY: the high field receives either its all-zero or all-one
        // two-bit image; the other members are generated single-bit fields.
        agc.agc_saturation_control().modify(|_, w| unsafe {
            w.rf_rx_saturation_bit_19_unknown()
                .bit(enabled)
                .rf_rx_saturation_bit_24_unknown()
                .bit(enabled)
                .rf_rx_saturation_bit_28_unknown()
                .bit(enabled)
                .rf_rx_saturation_high_unknown()
                .bits(if enabled { 3 } else { 0 })
        });
        // SAFETY: both complete branch values fit the nineteen-bit field.
        agc.agc_saturation_control()
            .modify(|_, w| unsafe { w.low_unknown().bits(if enabled { 0x0800 } else { 0x0400 }) });
    }

    /// Publish both final RX-gain limits through separate fresh RMWs.
    pub fn configure_rx_gain_limits(&mut self, wifi_last_index: u8) {
        let agc = &self.peripherals.phy_agc_oracle;
        // SAFETY: the first value is masked to seven bits and the capped
        // second value cannot exceed the generated seven-bit field.
        agc.agc_shared_control()
            .modify(|_, w| unsafe { w.rx_gain_index_unknown().bits(wifi_last_index & 0x7f) });
        agc.rx_gain_limit_control()
            .modify(|_, w| unsafe { w.rx_gain_limit_unknown().bits(wifi_last_index.min(0x4c)) });
    }

    /// Publish one saturation-gain word to both recovered destinations.
    pub fn set_agc_saturation_gain(&mut self, value: u32) {
        let agc = &self.peripherals.phy_agc_oracle;
        // SAFETY: complete ROM `phy_wifi_agc_sat_gain` performs two
        // unrestricted full-word stores in this order.
        unsafe {
            agc.saturation_gain_low().write_with_zero(|w| w.bits(value));
            agc.saturation_gain_high()
                .write_with_zero(|w| w.bits(value));
        }
    }

    /// Apply complete pinned `phy_reg_update_new` and both finite children.
    pub fn update_agc_post_initialization(&mut self) {
        self.peripherals
            .phy_agc_oracle
            .agc_saturation_control()
            .modify(|_, w| w.post_init_set_unknown().set_bit());
        self.set_agc_saturation_gain(0x0818_212d);
        let agc = &self.peripherals.phy_agc_oracle;
        // SAFETY: 0x1c0 fits the generated nine-bit field and 0x17 fits both
        // generated seven-bit fields.
        agc.rx_11b_window_control()
            .modify(|_, w| unsafe { w.window_unknown().bits(0x1c0) });
        agc.post_init_rx_control()
            .modify(|_, w| unsafe { w.low_unknown().bits(0x17) });
        agc.post_init_rx_control()
            .modify(|_, w| unsafe { w.high_unknown().bits(0x17) });
        agc.ftm_control().modify(|_, w| w.enable().set_bit());
    }

    /// Apply either complete branch of rev0 ROM `phy_rx_11b_opt`.
    pub fn configure_rx_11b_optimization(&mut self, enabled: bool) {
        let agc = &self.peripherals.phy_agc_oracle;
        let (path0_high, path0_low, path1_high, path1_low, mode) = rx_11b_values(enabled);
        // SAFETY: rx_11b_values contains only instruction-exact values
        // bounded to the generated six-, four- and four-bit fields.
        agc.rx_11b_path_control_0()
            .modify(|_, w| unsafe { w.rx_11b_high_unknown().bits(path0_high) });
        agc.rx_11b_path_control_0()
            .modify(|_, w| unsafe { w.rx_11b_low_unknown().bits(path0_low) });
        agc.rx_11b_path_control_1()
            .modify(|_, w| unsafe { w.rx_11b_high_unknown().bits(path1_high) });
        agc.rx_11b_path_control_1()
            .modify(|_, w| unsafe { w.rx_11b_low_unknown().bits(path1_low) });
        agc.rx_11b_mode_control()
            .modify(|_, w| unsafe { w.rx_11b_mode_unknown().bits(mode) });
        // SAFETY: 0x1c8 fits the generated nine-bit field.
        agc.rx_11b_window_control()
            .modify(|_, w| unsafe { w.window_unknown().bits(0x1c8) });
    }
}

#[cfg(test)]
mod tests {
    use super::{agc_parameter_offset, rx_11b_values};

    #[test]
    fn parameter_offset_retains_rom_u8_wrapping() {
        assert_eq!(agc_parameter_offset(0x20), 0x70);
        assert_eq!(agc_parameter_offset(0xe0), 0x30);
    }

    #[test]
    fn both_rx_11b_branches_keep_complete_rom_values() {
        assert_eq!(rx_11b_values(true), (0x3f, 0x21, 0x21, 3, 9));
        assert_eq!(rx_11b_values(false), (0x3e, 0x18, 0x18, 4, 6));
    }
}
