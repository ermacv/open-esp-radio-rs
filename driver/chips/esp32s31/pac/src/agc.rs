//! Ownership-bound AGC control leaves.

#![forbid(unsafe_code)]

use super::generated::{PhyAgcParameterByte, PhyLowRateState, PhyRx11bLowRateArgument};
use super::{PhyFtmEnableVendorArgument, RadioPhyRegisters};

const fn phy_low_rate_state(enabled: bool) -> PhyLowRateState {
    if enabled {
        PhyLowRateState::Enabled
    } else {
        PhyLowRateState::Disabled
    }
}

const fn rx_11b_values(enabled: bool) -> (u8, u8, u8, u8, u8) {
    if enabled {
        (0x3f, 0x21, 0x21, 0x03, 0x09)
    } else {
        (0x3e, 0x18, 0x18, 0x04, 0x06)
    }
}

impl RadioPhyRegisters {
    /// Enable or disable the complete three-edge PHY low-rate path.
    ///
    /// SOURCE: complete rev0 ROM `phy_enable_low_rate` at `0x2f82_5210`
    /// and `phy_disable_low_rate` at `0x2f82_5230`, both size `0x20`.
    /// The two primary-word bits remain separate RMWs exactly as in ROM.
    pub fn configure_phy_low_rate(&mut self, enabled: bool) {
        let agc = &self.peripherals.phy_agc_oracle;
        let state = phy_low_rate_state(enabled);
        super::generated::configure_phy_low_rate_first_state(agc, state);
        super::generated::configure_phy_low_rate_second_state(agc, state);
        super::generated::configure_phy_low_rate_secondary_state(agc, state);
    }

    /// Read the complete ROM `phy_is_low_rate_enabled` status bit.
    pub fn phy_low_rate_enabled(&self) -> bool {
        super::svd::field_read::observe_phy_low_rate_enabled(&self.peripherals.phy_agc_oracle)
    }

    /// Apply all fourteen internal MMIO edges of `phy_bb_agc_reg_update`.
    pub fn update_agc_baseband_registers(&mut self) {
        let agc = &self.peripherals.phy_agc_oracle;
        super::svd::fixed_register_image::initialize_agc_update_8070(agc);
        super::svd::fixed_register_image::initialize_agc_update_78a4(agc);
        super::generated::clear_agc_baseband_update_mode(agc);
        super::svd::fixed_register_image::initialize_agc_update_8010(agc);
        super::svd::fixed_register_image::initialize_agc_update_8018(agc);
        super::svd::fixed_register_image::initialize_agc_update_801c(agc);
        super::svd::fixed_register_image::initialize_agc_update_8020(agc);
        super::svd::fixed_register_image::initialize_agc_update_8028(agc);
        super::svd::fixed_register_image::initialize_agc_update_802c(agc);
        super::generated::complete_agc_baseband_update_mode(agc);
        super::svd::fixed_register_image::initialize_rx_11b_path_control_0(agc);
        super::svd::fixed_register_image::initialize_agc_update_7048(agc);
        super::svd::fixed_register_image::initialize_rx_11b_window_control(agc);
        super::svd::fixed_register_image::initialize_rx_11b_path_control_1(agc);
    }

    /// Select the complete rev0 ROM AGC enable or disable sequence.
    pub fn set_agc_enabled(&mut self, enabled: bool) {
        let agc = &self.peripherals.phy_agc_oracle;
        if !enabled {
            super::generated::disable_phy_agc(agc);
            return;
        }

        super::generated::enable_phy_agc(agc);
        super::generated::raise_phy_agc_enable_pulse(agc);
        super::generated::lower_phy_agc_enable_pulse(agc);
    }

    /// Publish both complete pinned RX-compensation fields in order.
    pub fn configure_rx_compensation(&mut self) {
        let agc = &self.peripherals.phy_agc_oracle;
        super::generated::configure_phy_rx_compensation_low(agc);
        super::generated::configure_phy_rx_compensation_high(agc);
    }

    /// Pulse the generated DC-memory clear field through two fresh RMWs.
    pub fn clear_agc_dc_memory(&mut self) {
        let agc = &self.peripherals.phy_agc_oracle;
        super::generated::raise_phy_agc_dc_memory_clear_pulse(agc);
        super::generated::lower_phy_agc_dc_memory_clear_pulse(agc);
    }

    /// Apply the two MMIO edges after the PBus work-mode 1 µs delay.
    pub fn configure_pbus_work_mode_pulse(&mut self) {
        let agc = &self.peripherals.phy_agc_oracle;
        super::generated::configure_phy_pbus_work_mode_control(agc);
        super::generated::raise_phy_pbus_work_mode_pulse(agc);
    }

    /// Clear the PBus work-mode pulse after the caller-owned 2 µs delay.
    pub fn clear_pbus_work_mode_pulse(&mut self) {
        super::generated::lower_phy_pbus_work_mode_pulse(&self.peripherals.phy_agc_oracle);
    }

    /// Apply all ten fresh-read updates of complete `phy_agc_reg_init`.
    pub fn initialize_agc_registers(&mut self, parameter_121: u8, parameter_120: u8) {
        let agc = &self.peripherals.phy_agc_oracle;
        let parameter_121 = PhyAgcParameterByte::new(u32::from(parameter_121))
            .expect("every u8 is a complete PHY AGC parameter byte");
        let parameter_120 = PhyAgcParameterByte::new(u32::from(parameter_120))
            .expect("every u8 is a complete PHY AGC parameter byte");
        super::generated::configure_phy_agc_initial_rx_gain_limit(agc, parameter_121);
        super::generated::configure_phy_agc_initial_gain_limit_low(agc, parameter_121);
        super::generated::configure_phy_agc_initial_rx_gain_index(agc, parameter_121);
        super::generated::configure_phy_agc_initial_saturation_low(agc);
        super::generated::configure_phy_agc_parameter_121(agc, parameter_121);
        super::generated::configure_phy_agc_parameter_120_offset(agc, parameter_120);
        super::generated::configure_phy_agc_initial_control_high(agc);
        super::generated::raise_phy_agc_initial_pulse(agc);
        super::generated::lower_phy_agc_initial_pulse(agc);
        super::generated::configure_phy_agc_initial_high(agc);
    }

    /// Apply all three fresh-read antenna initialization updates.
    pub fn configure_agc_antenna(&mut self) {
        let agc = &self.peripherals.phy_agc_oracle;
        super::generated::clear_phy_agc_initial_antenna_fields(agc);
        super::generated::configure_phy_agc_initial_antenna_control(agc);
        super::generated::configure_phy_agc_initial_antenna_paths(agc);
    }

    /// Apply complete rev0 ROM `phy_rx11blr_cfg` without widening the caller
    /// low-bit contract into a boolean ABI.
    pub fn configure_rx_11b_low_rate(&mut self, input: u32) {
        let input = PhyRx11bLowRateArgument::new(input)
            .expect("every u32 is a complete phy_rx11blr_cfg argument");
        let agc = &self.peripherals.phy_agc_oracle;
        super::generated::configure_phy_rx11b_first_low_rate_state(agc, input);
        super::generated::configure_phy_rx11b_second_low_rate_state(agc, input);
        super::generated::configure_phy_rx11b_secondary_low_rate_state(agc, input);
    }

    /// Apply either complete branch of rev0 ROM `phy_rfrx_sat_rst`.
    pub fn configure_rf_rx_saturation(&mut self, enabled: bool) {
        let agc = &self.peripherals.phy_agc_oracle;
        super::svd::fixed_register_image::initialize_rf_rx_saturation_config(agc);
        agc.agc_saturation_control().modify(|_, w| {
            w.rf_rx_saturation_bit_19_unknown()
                .bit(enabled)
                .rf_rx_saturation_bit_24_unknown()
                .bit(enabled)
                .rf_rx_saturation_bit_28_unknown()
                .bit(enabled)
                .rf_rx_saturation_high_unknown()
                .set(if enabled { 3 } else { 0 })
        });
        agc.agc_saturation_control()
            .modify(|_, w| w.low_unknown().set(if enabled { 0x0800 } else { 0x0400 }));
    }

    /// Publish both final RX-gain limits through separate fresh RMWs.
    pub fn configure_rx_gain_limits(&mut self, wifi_last_index: u8) {
        let agc = &self.peripherals.phy_agc_oracle;
        agc.agc_shared_control()
            .modify(|_, w| w.rx_gain_index_unknown().set(wifi_last_index & 0x7f));
        agc.rx_gain_limit_control()
            .modify(|_, w| w.rx_gain_limit_unknown().set(wifi_last_index.min(0x4c)));
    }

    /// Publish one saturation-gain word to both recovered destinations.
    pub fn set_agc_saturation_gain(&mut self, value: u32) {
        let agc = &self.peripherals.phy_agc_oracle;
        super::generated::agc_saturation_gain_low(
            agc,
            super::generated::AgcSaturationGainLow::new(value),
        );
        super::generated::agc_saturation_gain_high(
            agc,
            super::generated::AgcSaturationGainHigh::new(value),
        );
    }

    /// Select the complete pinned `phy_set_ftm_en` one-bit image.
    pub fn set_ftm_enabled(&mut self, enabled: bool) {
        self.peripherals
            .phy_agc_oracle
            .ftm_control()
            .modify(|_, w| w.enable().bit(enabled));
    }

    /// Apply the complete vendor ABI projection through generated PAC masks.
    pub fn set_ftm_enabled_from_vendor_argument(&mut self, input: PhyFtmEnableVendorArgument) {
        super::generated::set_ftm_enabled_from_vendor_argument(
            &self.peripherals.phy_agc_oracle,
            input,
        );
    }

    /// Read back the single field owned by `phy_set_ftm_en`.
    pub fn ftm_enabled(&self) -> bool {
        self.peripherals
            .phy_agc_oracle
            .ftm_control()
            .read()
            .enable()
            .bit_is_set()
    }

    /// Apply complete pinned `phy_reg_update_new` and both finite children.
    pub fn update_agc_post_initialization(&mut self) {
        self.peripherals
            .phy_agc_oracle
            .agc_saturation_control()
            .modify(|_, w| w.post_init_set_unknown().set_bit());
        self.set_agc_saturation_gain(0x0818_212d);
        let agc = &self.peripherals.phy_agc_oracle;
        agc.rx_11b_window_control()
            .modify(|_, w| w.window_unknown().set(0x1c0));
        agc.post_init_rx_control()
            .modify(|_, w| w.low_unknown().set(0x17));
        agc.post_init_rx_control()
            .modify(|_, w| w.high_unknown().set(0x17));
        self.set_ftm_enabled(true);
    }

    /// Apply either complete branch of rev0 ROM `phy_rx_11b_opt`.
    pub fn configure_rx_11b_optimization(&mut self, enabled: bool) {
        let agc = &self.peripherals.phy_agc_oracle;
        let (path0_high, path0_low, path1_high, path1_low, mode) = rx_11b_values(enabled);
        agc.rx_11b_path_control_0()
            .modify(|_, w| w.rx_11b_high_unknown().set(path0_high));
        agc.rx_11b_path_control_0()
            .modify(|_, w| w.rx_11b_low_unknown().set(path0_low));
        agc.rx_11b_path_control_1()
            .modify(|_, w| w.rx_11b_high_unknown().set(path1_high));
        agc.rx_11b_path_control_1()
            .modify(|_, w| w.rx_11b_low_unknown().set(path1_low));
        agc.rx_11b_mode_control()
            .modify(|_, w| w.rx_11b_mode_unknown().set(mode));
        agc.rx_11b_window_control()
            .modify(|_, w| w.window_unknown().set(0x1c8));
    }
}

#[cfg(test)]
mod tests {
    use super::rx_11b_values;

    #[test]
    fn both_rx_11b_branches_keep_complete_rom_values() {
        assert_eq!(rx_11b_values(true), (0x3f, 0x21, 0x21, 3, 9));
        assert_eq!(rx_11b_values(false), (0x3e, 0x18, 0x18, 4, 6));
    }
}
