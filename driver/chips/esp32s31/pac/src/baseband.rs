//! Ownership-bound access to recovered ESP32-S31 PHY/baseband registers.
//!
//! Register layout and legal field images come from
//! `svd/esp32s31-radio.svd`. Complete ROM/blob bodies cited there define the
//! finite operation order.

#![forbid(unsafe_code)]

use super::RadioPhyRegisters;

const fn tone_path_image(previous: u32, enabled: bool, selector: u16, step: u8) -> u32 {
    let encoded =
        ((enabled as u32) << 18) | ((selector as u32) >> 2) | ((step.wrapping_neg() as u32) << 10);
    (previous & 0xf000_0000) | (encoded & 0x0fff_ffff)
}

const fn txiq_first_mismatch_image(
    previous: u32,
    polarity: bool,
    attenuation: u8,
    selector: u16,
) -> u32 {
    let encoded = ((attenuation.wrapping_neg() as u32) << 10)
        | ((selector as u32) >> 2)
        | ((polarity as u32) << 26);
    (previous & 0xf000_0000) | (encoded & 0x0fff_ffff) | 0x002c_0000
}

const fn txiq_second_mismatch_image(previous: u32, polarity: bool) -> u32 {
    let polarity = polarity as u32;
    (previous & 0xf0ff_ffff) | ((((!polarity) & 1) | ((polarity & 1) << 3)) << 24)
}

const fn clear_power_detector_enable_field(field: u8, bit: u8) -> u8 {
    field & !bit
}

const fn low_bit(input: u32) -> bool {
    input & 1 != 0
}

const fn txdc_power_detector_images(table: u32, control: u32) -> (u8, u32, u32, u32) {
    let saved_table_low = table as u8;
    let saved_control_field = control & 0x0000_0ff0;
    let next_table = (table & !0x0000_00ff) | 0x0000_00f0;
    let next_control = (control & !0x0000_0ff0) | 0x0000_0780;
    (
        saved_table_low,
        saved_control_field,
        next_table,
        next_control,
    )
}

const fn clear_baseband_tail_low(value: u8) -> u8 {
    value & !3
}

const fn clear_baseband_tail_high(value: u8) -> u8 {
    value & !4
}

const fn decode_noise_floor_quarter_db(raw_low_twelve: u16) -> i32 {
    // Complete ROM `phy_read_hw_noisefloor` subtracts 0x1000 from the
    // masked field unconditionally, sign-extends and shifts by two.
    let signed_sixteenth_db = (raw_low_twelve & 0x0fff) as i32 - 0x1000;
    signed_sixteenth_db >> 2
}

const fn quarter_db_to_dbm(quarter_db: i32) -> i8 {
    // Complete blob `wDev_GetNoiseFloor` consumes the quarter-dB ROM result,
    // adds two, shifts by two and stores a byte.
    ((quarter_db + 2) >> 2) as i8
}

const fn tx_iq_gain_field(coefficient: i8) -> u8 {
    // Complete ROM `phy_txiq_set_reg` deliberately excludes the most
    // negative two's-complement endpoint before publishing the six-bit
    // field. This differs from merely retaining the low bits of an `i8`.
    let saturated = if coefficient < -31 {
        -31
    } else if coefficient > 31 {
        31
    } else {
        coefficient
    };
    saturated as u8 & 0x3f
}

const fn tx_iq_phase_field(coefficient: i8) -> u8 {
    // The seven-bit phase field has the analogous symmetric ROM range.
    let saturated = if coefficient < -63 {
        -63
    } else if coefficient > 63 {
        63
    } else {
        coefficient
    };
    saturated as u8 & 0x7f
}

impl RadioPhyRegisters {
    /// Enable both RX- and TX-IQ correction modes through two fresh RMWs.
    ///
    /// Complete rev0 ROM `phy_iq_corr_enable` at `0x2f82_7d8c` sets both
    /// recovered mode bits in each word while preserving all coefficients.
    pub fn enable_iq_correction_modes(&mut self) {
        let bb = &self.peripherals.phy_baseband_config_oracle;
        bb.iq_correction_control().modify(|_, w| {
            w.rx_iq_correction_mode_low()
                .set_bit()
                .rx_iq_correction_mode_high()
                .set_bit()
        });
        bb.iq_correction_aux().modify(|_, w| {
            w.tx_iq_correction_mode_low()
                .set_bit()
                .tx_iq_correction_mode_high()
                .set_bit()
        });
    }

    /// Publish both RXIQ root status bits through independent fresh RMWs.
    pub fn configure_rxiq_root_status(&mut self) {
        let status = self.peripherals.phy_pbus.status_clock_force();
        status.modify(|_, w| w.rx_clock_low_or_rxiq_status_first_unknown().set_bit());
        status.modify(|_, w| w.rx_clock_high_or_rxiq_status_second_unknown().set_bit());
    }

    /// Apply the complete four-edge RXIQ correction prefix or suffix.
    pub fn configure_rxiq_root_correction(&mut self, begin: bool) {
        let bb = &self.peripherals.phy_baseband_config_oracle;
        if begin {
            bb.iq_correction_control()
                .modify(|_, w| w.rx_iq_correction_mode_low().set_bit());
            bb.iq_correction_aux()
                .modify(|_, w| w.tx_iq_correction_mode_low().set_bit());
            bb.iq_correction_control()
                .modify(|_, w| w.rx_iq_correction_mode_high().clear_bit());
            bb.iq_correction_aux()
                .modify(|_, w| w.tx_iq_correction_mode_high().clear_bit());
        } else {
            bb.iq_correction_control()
                .modify(|_, w| w.rx_iq_correction_mode_high().set_bit());
            bb.iq_correction_aux()
                .modify(|_, w| w.tx_iq_correction_mode_high().set_bit());
            bb.iq_correction_control()
                .modify(|_, w| w.rx_iq_correction_mode_low().clear_bit());
            self.peripherals
                .phy_pbus
                .status_clock_force()
                .modify(|_, w| w.rx_clock_high_or_rxiq_status_second_unknown().clear_bit());
        }
    }

    /// Configure all fourteen ordered TX-power tracking RMW edges.
    pub fn configure_tx_power_tracking(&mut self, enabled: bool) {
        let bb = &self.peripherals.phy_baseband_config_oracle;
        bb.tx_power_track_control_0()
            .modify(|_, w| w.track_enable().bit(enabled));
        bb.tx_power_track_control_0()
            .modify(|_, w| w.init_clear_unknown().set(0));
        bb.tx_power_track_control_0()
            .modify(|_, w| w.init_set_unknown().set(0x1f));

        // The complete body clears the adjacent bits through separate reads.
        bb.tx_power_track_control_1().modify(|r, w| {
            w.init_clear_unknown()
                .set(r.init_clear_unknown().bits() & !1)
        });
        bb.tx_power_track_control_1().modify(|r, w| {
            w.init_clear_unknown()
                .set(r.init_clear_unknown().bits() & !2)
        });

        bb.tx_power_track_control_3()
            .modify(|_, w| w.track_value_1_unknown().set(0x79));
        bb.tx_power_track_control_3()
            .modify(|_, w| w.track_value_0_unknown().set(0x83));
        bb.tx_power_track_control_2()
            .modify(|_, w| w.track_value_3_unknown().set(0x8d));
        bb.tx_power_track_control_2()
            .modify(|_, w| w.track_value_2_unknown().set(0x96));
        bb.tx_power_track_control_2()
            .modify(|_, w| w.track_value_1_unknown().set(0xa0));
        bb.tx_power_track_control_2()
            .modify(|_, w| w.track_value_0_unknown().set(0xb1));
        bb.tx_power_track_control_1()
            .modify(|_, w| w.track_value_2_unknown().set(0xbe));
        bb.tx_power_track_control_1()
            .modify(|_, w| w.track_value_1_unknown().set(0xd2));
        bb.tx_power_track_control_1()
            .modify(|_, w| w.track_value_0_unknown().set(0xe6));
    }

    /// Apply complete rev0 ROM `phy_btbb_wifi_bb_cfg2`.
    pub fn configure_bt_wifi_baseband(&mut self) {
        self.peripherals
            .phy_baseband_config_oracle
            .baseband_init_7cd0()
            .modify(|r, w| {
                w.init_low_unknown()
                    .set(r.init_low_unknown().bits() | 0x0b)
                    .init_high_unknown()
                    .set(0x0f)
            });
    }

    /// Apply complete rev0 ROM `phy_chan_dump_cfg`.
    pub fn configure_channel_dump(&mut self, value: u32, enabled: u32, mode: u32) {
        let bb = &self.peripherals.phy_baseband_config_oracle;
        bb.baseband_init_790c()
            .modify(|_, w| w.channel_dump_value_unknown().set(value as u8 & 0x0f));
        bb.baseband_init_790c()
            .modify(|_, w| w.init_clear_unknown().bit(mode & 1 != 0));
        bb.baseband_tx_pa_control()
            .modify(|_, w| w.channel_dump_enable_unknown().bit(enabled & 1 != 0));
    }

    /// Apply complete rev0 ROM `phy_dac_rate_set`.
    pub fn configure_dac_rate(&mut self, rate: u32) {
        self.configure_adc_rate(rate);
    }

    /// Configure both I²C TX-rate fields and the four gain-compensation bytes.
    pub fn configure_i2c_tx_rate(&mut self) {
        let rate = self
            .peripherals
            .phy_baseband_config_oracle
            .i2c_tx_rate_control();
        rate.modify(|_, w| w.tx_rate_high_unknown().set(0x55));
        rate.modify(|_, w| w.tx_rate_low_unknown().set(2));
        self.restore_tx_gain_compensation();
    }

    /// Configure the complete baseband watchdog leaf.
    pub fn configure_baseband_watchdog(&mut self) {
        let bb = &self.peripherals.phy_baseband_config_oracle;
        bb.baseband_watchdog_control().modify(|_, w| {
            w.watchdog_config_unknown()
                .set(0x00aa)
                .watchdog_control_unknown()
                .set_bit()
        });
        bb.baseband_watchdog_enable()
            .modify(|_, w| w.watchdog_enable().set_bit());
    }

    /// Replace the standalone PHY VHT-support bit through one fresh RMW.
    pub fn set_vht_support(&mut self, input: u32) {
        self.peripherals
            .phy_frequency_channel_oracle
            .channel_cbw_control_1()
            .modify(|_, w| w.vht_support().bit(low_bit(input)));
    }

    /// Replace the PHY CSI-dump force-LLTF bit through one fresh RMW.
    pub fn set_csi_dump_force_lltf(&mut self, input: u32) {
        self.peripherals
            .phy_agc_oracle
            .csi_dump_force_control()
            .modify(|_, w| w.force_lltf().bit(low_bit(input)));
    }

    /// Apply complete ROM `phy_hemu_ru26_good_res`.
    pub fn configure_he_ru26_good_response(&mut self) {
        let control = self
            .peripherals
            .phy_baseband_config_oracle
            .baseband_init_7890();
        control.modify(|_, w| w.he_ru26_good_response_enable().set_bit());
        control.modify(|_, w| w.he_ru26_good_response_disable().clear_bit());
    }

    /// Apply complete ROM `phy_freq_band_reg_set` and its VHT tail.
    pub fn set_frequency_band(&mut self, input: u32) {
        let selected = low_bit(input);
        self.peripherals
            .phy_agc_oracle
            .agc_antenna_control()
            .modify(|_, w| w.frequency_band_inverse().bit(!selected));
        self.set_vht_support(input);
    }

    /// Apply the three fresh RMWs of complete ROM `phy_bbtx_outfilter`.
    pub fn configure_tx_output_filter(&mut self, input_0: u32, input_1: u32, input_2: u32) {
        let control = self
            .peripherals
            .phy_baseband_config_oracle
            .tx_output_filter_control();
        control.modify(|_, w| w.filter_input_0().bit(low_bit(input_0)));
        control.modify(|_, w| w.filter_input_1().bit(low_bit(input_1)));
        control.modify(|_, w| w.filter_input_2().bit(low_bit(input_2)));
    }

    /// Replace the baseband watchdog reset-enable bit.
    pub fn set_baseband_watchdog_reset_enabled(&mut self, input: u32) {
        self.peripherals
            .phy_baseband_config_oracle
            .baseband_watchdog_enable()
            .modify(|_, w| w.watchdog_enable().bit(low_bit(input)));
    }

    /// Replace the baseband watchdog interrupt-enable bit.
    pub fn set_baseband_watchdog_interrupt_enabled(&mut self, input: u32) {
        self.peripherals
            .phy_baseband_config_oracle
            .baseband_watchdog_enable()
            .modify(|_, w| w.watchdog_interrupt_enable().bit(low_bit(input)));
    }

    /// Set the baseband watchdog timeout-clear bit through one fresh RMW.
    pub fn clear_baseband_watchdog_timeout(&mut self) {
        self.peripherals
            .phy_baseband_config_oracle
            .baseband_watchdog_enable()
            .modify(|_, w| w.watchdog_timeout_clear().set_bit());
    }

    /// Return the complete standalone baseband watchdog status word.
    pub fn baseband_watchdog_status(&mut self) -> u32 {
        self.peripherals
            .phy_baseband_config_oracle
            .baseband_watchdog_status()
            .read()
            .bits()
    }

    /// Apply both fresh RMWs of complete ROM `phy_lltf_mask_en`.
    pub fn configure_lltf_mask(&mut self, input_0: u32, input_1: u32) {
        let control = self
            .peripherals
            .phy_baseband_config_oracle
            .baseband_init_790c();
        control.modify(|_, w| w.lltf_mask_input_0().bit(low_bit(input_0)));
        control.modify(|_, w| w.lltf_mask_input_1().bit(low_bit(input_1)));
    }

    /// Enable all four recovered automatic noise-floor controls.
    pub fn configure_noise_floor_auto(&mut self) {
        let bb = &self.peripherals.phy_baseband_config_oracle;
        bb.noise_floor_control()
            .modify(|_, w| w.auto_control_low_unknown().set_bit());
        bb.noise_floor_control()
            .modify(|_, w| w.auto_control_high_unknown().set_bit());
        bb.noise_floor_enable_0()
            .modify(|_, w| w.auto_enable_unknown().set_bit());
        bb.noise_floor_enable_1()
            .modify(|_, w| w.auto_enable_unknown().set_bit());
    }

    /// Read the current hardware noise floor as the signed byte used by MAC
    /// rate control.
    ///
    /// SOURCE: complete rev0 ROM `phy_read_hw_noisefloor` at
    /// `0x2f82_7d72`, size `0x1a`, reads `0x2010_708c[11:0]` and performs the
    /// first arithmetic divide by four. Complete
    /// `libpp.a[wdev.o]::wDev_GetNoiseFloor`, size `0x36`, applies
    /// `(quarter_db + 2) >> 2` and retains the result as a signed byte.
    pub fn read_noise_floor_dbm(&self) -> i8 {
        quarter_db_to_dbm(self.read_noise_floor_quarter_db())
    }

    /// Read the exact signed quarter-dB result returned by complete rev0 ROM
    /// `phy_read_hw_noisefloor`.
    pub fn read_noise_floor_quarter_db(&self) -> i32 {
        let raw = self
            .peripherals
            .phy_baseband_config_oracle
            .noise_floor_measurement()
            .read()
            .signed_sixteenth_db_code()
            .bits();
        decode_noise_floor_quarter_db(raw)
    }

    /// Apply all six ordered PA-on configuration operations.
    pub fn configure_tx_pa_on(&mut self) {
        let bb = &self.peripherals.phy_baseband_config_oracle;
        bb.baseband_tx_pa_control()
            .modify(|_, w| w.pa_on_field_unknown().set(0x14));
        bb.tx_pa_control_0()
            .modify(|_, w| w.pa_on_high_unknown().set(0x78));
        super::svd::fixed_register_image::initialize_tx_pa_table(bb);
        bb.baseband_tx_pa_timing()
            .modify(|_, w| w.pa_on_timing_unknown().set(0x1e));
        bb.tx_pa_control_1()
            .modify(|_, w| w.pa_on_high_unknown().set(0x0a0e));
        bb.tx_pa_control_1()
            .modify(|_, w| w.pa_on_bt_delay().set(0xc8));
    }

    /// Apply the local prefix of complete rev0 ROM `phy_bb_reg_init`.
    pub fn initialize_baseband_prefix(&mut self) {
        self.peripherals
            .phy_baseband_config_oracle
            .baseband_init_7400()
            .modify(|_, w| w.init_unknown().set(3));
    }

    /// Apply the twelve local middle edges of complete `phy_bb_reg_init`.
    pub fn initialize_baseband_middle(&mut self) {
        let bb = &self.peripherals.phy_baseband_config_oracle;
        bb.baseband_init_7808()
            .modify(|_, w| w.init_value_unknown().set(0x60));
        bb.baseband_init_78dc()
            .modify(|_, w| w.init_value_unknown().set(2));
        bb.baseband_init_78e4()
            .modify(|_, w| w.init_clear_unknown().clear_bit());
        bb.baseband_tx_pa_timing()
            .modify(|_, w| w.baseband_init_clear_unknown().set(0));
        bb.baseband_init_790c()
            .modify(|_, w| w.init_clear_unknown().clear_bit());
        bb.baseband_init_7ca8()
            .modify(|_, w| w.init_enable_unknown().set_bit());
        bb.baseband_init_7980()
            .modify(|_, w| w.init_clear_unknown().clear_bit());

        // Complete ROM updates the adjacent mode bits through separate reads.
        bb.baseband_init_7890()
            .modify(|_, w| w.he_ru26_good_response_disable().clear_bit());
        bb.baseband_init_7890()
            .modify(|_, w| w.he_ru26_good_response_enable().set_bit());
        bb.baseband_init_7a28()
            .modify(|_, w| w.init_clear_unknown().clear_bit());
        bb.baseband_init_7cd0()
            .modify(|_, w| w.init_low_unknown().set(0x0f).init_high_unknown().set(0x0f));
        bb.baseband_tx_pa_control()
            .modify(|_, w| w.baseband_init_enable_unknown().set_bit());
    }

    /// Apply the five local tail edges of complete `phy_bb_reg_init`.
    pub fn initialize_baseband_tail(&mut self) {
        let bb = &self.peripherals.phy_baseband_config_oracle;
        // Complete ROM clears bits 7:6 and bit 8 through separate reads.
        bb.baseband_init_743c().modify(|r, w| {
            w.init_clear_unknown()
                .set(clear_baseband_tail_low(r.init_clear_unknown().bits()))
        });
        bb.baseband_init_743c().modify(|r, w| {
            w.init_clear_unknown()
                .set(clear_baseband_tail_high(r.init_clear_unknown().bits()))
        });
        bb.baseband_init_7428()
            .modify(|_, w| w.init_enable_unknown().set_bit());
        bb.baseband_init_7428()
            .modify(|_, w| w.init_value_unknown().set(0x15));
        bb.baseband_init_7cd0().modify(|r, w| {
            w.init_low_unknown()
                .set(r.init_low_unknown().bits() | 0x0b)
                .init_high_unknown()
                .set(r.init_high_unknown().bits() | 0x0f)
        });
    }

    /// Apply the five internal-MMIO stores of complete ROM `phy_pwdet_reg_init`.
    pub fn initialize_power_detector_registers(&mut self) {
        let bb = &self.peripherals.phy_baseband_config_oracle;
        super::svd::fixed_register_image::initialize_power_detector_table_0(bb);
        super::svd::fixed_register_image::initialize_power_detector_table_1(bb);
        bb.power_detector_control()
            .modify(|_, w| w.calibration_field_unknown().set(0x50));
        super::svd::zero_based_field_write::power_detector_reference(bb, 0xaaaa);
        bb.power_detector_control()
            .modify(|_, w| w.initialization_mode_unknown().set(2));
    }

    /// Apply the internal-MMIO portion of complete ROM `phy_en_pwdet`.
    pub fn configure_power_detector_enabled(&mut self) {
        let bb = &self.peripherals.phy_baseband_config_oracle;
        let control = bb.power_detector_control();
        for bit in [2_u8, 1, 4] {
            control.modify(|r, w| {
                let field = clear_power_detector_enable_field(r.enable_clear_unknown().bits(), bit);
                w.enable_clear_unknown().set(field)
            });
        }
        bb.power_detector_sar_control_status()
            .modify(|_, w| w.sar_mode_unknown().set(3));
        bb.power_detector_sar_control_status()
            .modify(|_, w| w.sar_config_clear_unknown().clear_bit());
        super::svd::zero_based_field_write::power_detector_reference(bb, 0x016a);
    }

    /// Set the final background-control bit after PWDET enable.
    pub fn enable_power_detector_background_control(&mut self) {
        self.peripherals
            .phy_baseband_config_oracle
            .power_detector_control()
            .modify(|_, w| w.background_control_enable_unknown().set_bit());
    }

    /// Capture and replace the two fields owned by TX-DC PWDET calibration.
    pub fn capture_txdc_power_detector_fields(&mut self) -> (u8, u32) {
        let bb = &self.peripherals.phy_baseband_config_oracle;
        let table = bb.power_detector_table_1().read().bits();
        let control = bb.power_detector_control().read().bits();
        let (saved_table, saved_control, next_table, next_control) =
            txdc_power_detector_images(table, control);
        super::generated::publish_power_detector_table_1_image(
            bb,
            super::generated::PowerDetectorTable1Image::new(next_table),
        );
        super::generated::publish_power_detector_control_image(
            bb,
            super::generated::PowerDetectorControlImage::new(next_control),
        );
        (saved_table, saved_control)
    }

    /// Select TX-DC SAR mode one after the initial PBus setup.
    pub fn configure_txdc_power_detector_sar(&mut self) {
        self.peripherals
            .phy_baseband_config_oracle
            .power_detector_sar_control_status()
            .modify(|_, w| w.sar_mode_unknown().set(1));
    }

    /// Restore the captured TX-DC fields and select final SAR mode three.
    pub fn restore_txdc_power_detector_fields(
        &mut self,
        power_table_low: u8,
        shifted_power_control_field: u32,
    ) {
        let bb = &self.peripherals.phy_baseband_config_oracle;
        bb.power_detector_table_1()
            .modify(|_, w| w.tx_dc_temporary_low_unknown().set(power_table_low));
        bb.power_detector_control().modify(|_, w| {
            w.calibration_field_unknown()
                .set((shifted_power_control_field >> 4) as u8)
        });
        bb.power_detector_sar_control_status()
            .modify(|_, w| w.sar_mode_unknown().set(3));
    }

    /// Publish one zero-extended power-detector reference word.
    pub fn write_power_detector_reference(&mut self, value: u16) {
        super::svd::zero_based_field_write::power_detector_reference(
            &self.peripherals.phy_baseband_config_oracle,
            value,
        );
    }

    /// Pulse the power-detector SAR trigger through two fresh RMW edges.
    pub fn trigger_power_detector_sar(&mut self) {
        let control = self
            .peripherals
            .phy_baseband_config_oracle
            .power_detector_control();
        control.modify(|_, w| w.sar_trigger().clear_bit());
        control.modify(|_, w| w.sar_trigger().set_bit());
    }

    /// Read the SVD-described power-detector readiness field.
    pub fn power_detector_ready(&mut self) -> bool {
        self.peripherals
            .phy_baseband_config_oracle
            .power_detector_sar_control_status()
            .read()
            .sar_ready()
            .bits()
            == 0b111
    }

    /// Read the SVD-described power-detector SAR sample field.
    pub fn power_detector_sar_sample(&mut self) -> u16 {
        self.peripherals
            .phy_baseband_config_oracle
            .power_detector_sar_result()
            .read()
            .sar_sample()
            .bits()
    }

    fn clear_tx_gain_compensation(&mut self) {
        let bb = &self.peripherals.phy_baseband_config_oracle;
        super::svd::zero_register_write::clear_tx_gain_compensation(bb);
        super::svd::zero_register_write::clear_tx_gain_compensation_aux(bb);
    }

    /// Apply complete pinned `phy_txgain_comp_pacfg_new(1)` as four ordered
    /// fresh-read byte updates.
    pub fn restore_tx_gain_compensation(&mut self) {
        let compensation = self
            .peripherals
            .phy_baseband_config_oracle
            .tx_gain_compensation();
        compensation.modify(|_, w| w.compensation_byte_0_unknown().set(0));
        compensation.modify(|_, w| w.compensation_byte_1_unknown().set(0xfa));
        compensation.modify(|_, w| w.compensation_byte_2_unknown().set(0xff));
        compensation.modify(|_, w| w.compensation_byte_3_unknown().set(0));
    }

    fn configure_tone_selectors(&mut self, path_0: u16, path_1: u16) {
        debug_assert!(path_0 <= 0x03ff);
        debug_assert!(path_1 <= 0x03ff);
        let selectors = self
            .peripherals
            .phy_baseband_config_oracle
            .tone_selector_control();
        selectors.modify(|_, w| w.path_0_selector_low().set((path_0 & 3) as u8));
        selectors.modify(|_, w| w.path_1_selector_low().set((path_1 & 3) as u8));
    }

    fn configure_tone_paths(&mut self, enabled: bool, path_0_selector: u16, path_0_step: u8) {
        debug_assert!(path_0_selector <= 0x03ff);
        let bb = &self.peripherals.phy_baseband_config_oracle;
        super::generated::publish_tone_path_0_image(
            bb,
            super::generated::TonePath0MaskedInput::new(tone_path_image(
                0,
                enabled,
                path_0_selector,
                path_0_step,
            )),
        );
        super::generated::publish_tone_path_1_image(
            bb,
            super::generated::TonePath1MaskedInput::new(tone_path_image(0, false, 0, 0)),
        );
    }

    /// Program the complete archive calibration-tone leaf and restore TX gain.
    ///
    /// This preserves every fresh-read/write edge in
    /// `libphy.a[phy_reg.o]::phy_start_tx_tone_step_new` and its
    /// `phy_txgain_comp_pacfg_new` child.
    pub fn configure_calibration_tone(&mut self, enabled: bool, selector: u16, step: u8) {
        self.clear_tx_gain_compensation();
        self.configure_tone_selectors(selector, 0);
        self.configure_tone_paths(enabled, selector, step);
        self.restore_tx_gain_compensation();
    }

    /// Program the ROM power-control tone with DAC scale and TX gain disabled.
    pub fn configure_power_control_tone(&mut self, selector: u16, step: u8) {
        let bb = &self.peripherals.phy_baseband_config_oracle;
        bb.front_end_and_tone_stop_control()
            .modify(|_, w| w.tone_stop_control_unknown().set(0));
        bb.dac_scale_control()
            .modify(|_, w| w.dac_scale_high_unknown().set(0));
        bb.dac_scale_control()
            .modify(|_, w| w.dac_scale_low_unknown().set(0));
        self.clear_tx_gain_compensation();
        self.configure_tone_selectors(selector, 0);
        self.configure_tone_paths(true, selector, step);
    }

    /// Capture the full first-path word saved by complete ROM `phy_rfcal_txiq`.
    pub fn txiq_tone_control(&mut self) -> u32 {
        self.peripherals
            .phy_baseband_config_oracle
            .tone_path_0_control()
            .read()
            .bits()
    }

    /// Restore the complete first-path word after TX-IQ cleanup.
    pub fn restore_txiq_tone_control(&mut self, saved: u32) {
        super::generated::restore_txiq_tone_control(
            &self.peripherals.phy_baseband_config_oracle,
            super::generated::TxiqToneControlImage::new(saved),
        );
    }

    /// Configure one of the two complete TX-IQ mismatch-power polarity edges.
    pub fn configure_txiq_mismatch_power(
        &mut self,
        first: bool,
        polarity: bool,
        attenuation: u8,
        selector: u16,
    ) {
        debug_assert!(selector <= 0x03ff);
        let bb = &self.peripherals.phy_baseband_config_oracle;
        if first {
            super::generated::publish_txiq_first_mismatch_image(
                bb,
                super::generated::TxiqFirstMismatchInput::new(txiq_first_mismatch_image(
                    0,
                    polarity,
                    attenuation,
                    selector,
                )),
            );
            bb.tone_selector_control()
                .modify(|_, w| w.path_0_selector_low().set((selector & 3) as u8));
        } else {
            super::generated::publish_txiq_second_mismatch_image(
                bb,
                super::generated::TxiqSecondMismatchInput::new(txiq_second_mismatch_image(
                    0, polarity,
                )),
            );
        }
    }

    /// Set or clear the shared first-path arm bit for one PWDET sample.
    pub fn set_power_detector_tone_armed(&mut self, armed: bool) {
        self.peripherals
            .phy_baseband_config_oracle
            .tone_path_0_control()
            .modify(|_, w| w.tone_enable_or_arm().bit(armed));
    }

    /// Stop both tone paths and restore the two DAC-scale fields.
    pub fn stop_power_detector_tone(&mut self) {
        self.stop_calibration_tone_paths();
        let bb = &self.peripherals.phy_baseband_config_oracle;
        bb.dac_scale_control()
            .modify(|_, w| w.dac_scale_high_unknown().set(0xff));
        bb.dac_scale_control()
            .modify(|_, w| w.dac_scale_low_unknown().set(0xff));
    }

    /// Stop both tone paths without changing their DAC-scale fields.
    ///
    /// This is the complete pinned `libphy.a` `phy_stop_tx_tone_new` leaf.
    /// The longer ROM `phy_stop_tx_tone(1)` composes this exact prefix with
    /// two additional DAC-scale restores in [`Self::stop_power_detector_tone`].
    pub fn stop_calibration_tone_paths(&mut self) {
        let bb = &self.peripherals.phy_baseband_config_oracle;
        bb.tone_path_0_control()
            .modify(|_, w| w.tone_enable_or_arm().clear_bit());
        bb.tone_path_1_control()
            .modify(|_, w| w.tone_enable_or_arm().clear_bit());
        bb.front_end_and_tone_stop_control()
            .modify(|_, w| w.tone_stop_control_unknown().set(3));
    }

    /// Enter or complete the TX-IQ correction phase with one fresh RMW.
    ///
    /// Complete ROM `phy_rfcal_txiq` clears the high mode bit while setting
    /// the low bit on entry. Its completion edge sets only the high bit.
    pub fn configure_tx_iq_correction(&mut self, begin: bool) {
        let control = self
            .peripherals
            .phy_baseband_config_oracle
            .iq_correction_aux();
        if begin {
            control.modify(|_, w| {
                w.tx_iq_correction_mode_high()
                    .clear_bit()
                    .tx_iq_correction_mode_low()
                    .set_bit()
            });
        } else {
            control.modify(|_, w| w.tx_iq_correction_mode_high().set_bit());
        }
    }

    /// Select the RX-IQ calibration mode with one fresh RMW.
    pub fn configure_rx_iq_calibration_mode(&mut self) {
        self.peripherals
            .phy_baseband_config_oracle
            .iq_correction_control()
            .modify(|_, w| {
                w.rx_iq_correction_mode_high()
                    .clear_bit()
                    .rx_iq_correction_mode_low()
                    .set_bit()
            });
    }

    /// Publish one signed TX-IQ gain coefficient using the ROM saturation.
    pub fn set_tx_iq_gain_coefficient(&mut self, coefficient: i8) {
        self.peripherals
            .phy_baseband_config_oracle
            .iq_correction_aux()
            .modify(|_, w| {
                w.tx_iq_gain_coefficient()
                    .set(tx_iq_gain_field(coefficient))
            });
    }

    /// Publish one signed TX-IQ phase coefficient using the ROM saturation.
    pub fn set_tx_iq_phase_coefficient(&mut self, coefficient: i8) {
        self.peripherals
            .phy_baseband_config_oracle
            .iq_correction_aux()
            .modify(|_, w| {
                w.tx_iq_phase_coefficient()
                    .set(tx_iq_phase_field(coefficient))
            });
    }

    /// Publish one signed RX-IQ gain coefficient using the ROM truncation.
    pub fn set_rx_iq_gain_coefficient(&mut self, coefficient: i8) {
        self.peripherals
            .phy_baseband_config_oracle
            .iq_correction_control()
            .modify(|_, w| w.rx_iq_gain_coefficient().set(coefficient as u8 & 0x3f));
    }

    /// Publish one signed RX-IQ phase coefficient using the ROM truncation.
    pub fn set_rx_iq_phase_coefficient(&mut self, coefficient: i8) {
        self.peripherals
            .phy_baseband_config_oracle
            .iq_correction_control()
            .modify(|_, w| w.rx_iq_phase_coefficient().set(coefficient as u8 & 0x7f));
    }

    /// Trigger one TX-DC comparator measurement using three fresh RMW edges.
    pub fn trigger_tx_dc_measurement(&mut self) {
        let control = self
            .peripherals
            .phy_baseband_config_oracle
            .tx_dc_measurement_control_status();
        control.modify(|_, w| w.measurement_enable().set_bit());
        control.modify(|_, w| w.measurement_start().clear_bit());
        control.modify(|_, w| w.measurement_start().set_bit());
    }

    /// Sample the TX-DC ready bit exactly once.
    pub fn tx_dc_measurement_is_ready(&mut self) -> bool {
        self.peripherals
            .phy_baseband_config_oracle
            .tx_dc_measurement_control_status()
            .read()
            .measurement_ready()
            .bit_is_set()
    }

    /// Preserve the complete ROM's independent I and Q comparator reads.
    pub fn sample_tx_dc_comparators(&mut self) -> [bool; 2] {
        let control = self
            .peripherals
            .phy_baseband_config_oracle
            .tx_dc_measurement_control_status();
        [
            control.read().i_comparator_high().bit_is_set(),
            control.read().q_comparator_high().bit_is_set(),
        ]
    }

    /// Clear TX-DC enable and start through two fresh RMW edges.
    pub fn clear_tx_dc_measurement(&mut self) {
        let control = self
            .peripherals
            .phy_baseband_config_oracle
            .tx_dc_measurement_control_status();
        control.modify(|_, w| w.measurement_enable().clear_bit());
        control.modify(|_, w| w.measurement_start().clear_bit());
    }

    /// Publish the two-register suffix of complete ROM `phy_adc_rate_set`.
    ///
    /// The ROM body at `0x2f82_a6d2`, size `0x4a`, uses two fresh reads to
    /// copy `rate` bit zero into physical bit one and then physical bit zero.
    pub fn configure_adc_rate(&mut self, rate: u32) {
        let enabled = rate & 1 != 0;
        let control = self
            .peripherals
            .phy_baseband_config_oracle
            .adc_rate_and_front_end_control();
        control.modify(|_, w| w.adc_rate_high_or_front_end_control_unknown().bit(enabled));
        control.modify(|_, w| w.adc_rate_low_or_front_end_control_unknown().bit(enabled));
    }

    /// Apply the four front-end initialization edges before table-memory setup.
    ///
    /// This is the exact prefix of complete rev0 ROM `phy_fe_reg_init` at
    /// `0x2f82_7740`, size `0xf6`. The table-memory edge remains between this
    /// method and [`Self::initialize_front_end_suffix`].
    pub fn initialize_front_end_prefix(&mut self) {
        self.peripherals
            .phy_pbus
            .read_result_0()
            .modify(|_, w| w.fe_init_enable_unknown().set_bit());
        let bb = &self.peripherals.phy_baseband_config_oracle;
        bb.front_end_init_0c08()
            .modify(|_, w| w.init_first_unknown().set_bit());
        bb.front_end_init_0c08()
            .modify(|_, w| w.init_second_unknown().set_bit());
        bb.front_end_clear_control()
            .modify(|_, w| w.init_clear_first_unknown().clear_bit());
    }

    /// Apply the twelve front-end initialization edges after table-memory setup.
    ///
    /// Complete rev0 ROM `phy_fe_reg_init` performs every update below using
    /// a fresh read. Repeated sets are retained because intermediate device
    /// states are observable hardware behavior.
    pub fn initialize_front_end_suffix(&mut self) {
        let bb = &self.peripherals.phy_baseband_config_oracle;
        bb.front_end_and_tone_stop_control()
            .modify(|_, w| w.front_end_init_enable_unknown().set_bit());
        bb.iq_correction_control().modify(|_, w| {
            w.rx_iq_correction_mode_low()
                .set_bit()
                .rx_iq_correction_mode_high()
                .set_bit()
        });
        bb.iq_correction_aux().modify(|_, w| {
            w.tx_iq_correction_mode_low()
                .set_bit()
                .tx_iq_correction_mode_high()
                .set_bit()
        });
        bb.front_end_clear_control()
            .modify(|_, w| w.init_clear_second_unknown().clear_bit());
        bb.adc_rate_and_front_end_control()
            .modify(|_, w| w.adc_rate_high_or_front_end_control_unknown().set_bit());
        bb.adc_rate_and_front_end_control()
            .modify(|_, w| w.adc_rate_low_or_front_end_control_unknown().set_bit());
        bb.tx_pa_control_0()
            .modify(|_, w| w.front_end_low_unknown().set(4));
        bb.adc_rate_and_front_end_control()
            .modify(|_, w| w.adc_rate_low_or_front_end_control_unknown().set_bit());
        bb.adc_rate_and_front_end_control()
            .modify(|_, w| w.adc_rate_high_or_front_end_control_unknown().set_bit());
        bb.iq_correction_control()
            .modify(|_, w| w.front_end_init_high_unknown().set_bit());
        bb.iq_correction_aux()
            .modify(|_, w| w.front_end_init_high_unknown().set_bit());
        bb.front_end_init_0c20()
            .modify(|_, w| w.init_low_unknown().set(0x57));
    }

    /// Apply complete pinned `libphy.a[phy_reg.o]::phy_fe_reg_update`.
    ///
    /// The `0x32`-byte body performs exactly three fresh-read RMW edges and
    /// has no ROM-only DAC-scale tail.
    pub fn update_front_end(&mut self) {
        let bb = &self.peripherals.phy_baseband_config_oracle;
        bb.front_end_init_0c08()
            .modify(|_, w| w.init_first_unknown().set_bit());
        bb.front_end_init_0c08()
            .modify(|_, w| w.init_second_unknown().set_bit());
        bb.adc_rate_and_front_end_control().modify(|_, w| {
            w.adc_rate_low_or_front_end_control_unknown()
                .set_bit()
                .adc_rate_high_or_front_end_control_unknown()
                .set_bit()
        });
    }

    /// Select the direct-register prefix or cleanup state of RX-gain DC calibration.
    ///
    /// Complete rev0 ROM `phy_set_rx_gain_cal_dc` at `0x2f82_9858`, size
    /// `0x206`, sets bits 6:5 to `0b11` before entering the bounded
    /// calibration graph and clears them to `0b00` in its common cleanup.
    /// The field's narrower electrical meaning is not independently proved.
    pub fn set_rx_gain_dc_calibration(&mut self, enabled: bool) {
        self.peripherals
            .phy_baseband_config_oracle
            .rx_gain_dc_control()
            .modify(|_, w| {
                if enabled {
                    w.calibration_enable_unknown().enabled()
                } else {
                    w.calibration_enable_unknown().disabled()
                }
            });
    }
}

#[cfg(test)]
mod tests {
    use super::{
        clear_baseband_tail_high, clear_baseband_tail_low, clear_power_detector_enable_field,
        decode_noise_floor_quarter_db, low_bit, quarter_db_to_dbm, tone_path_image,
        tx_iq_gain_field, tx_iq_phase_field, txdc_power_detector_images, txiq_first_mismatch_image,
        txiq_second_mismatch_image,
    };

    #[test]
    fn noise_floor_decode_reproduces_both_complete_arithmetic_shifts() {
        // -96 dBm is encoded as -1536 sixteenth-dB, or low twelve bits 0xa00.
        assert_eq!(decode_noise_floor_quarter_db(0x0a00), -384);
        assert_eq!(decode_noise_floor_quarter_db(0x0fff), -1);
        assert_eq!(decode_noise_floor_quarter_db(0x0000), -1024);
        assert_eq!(quarter_db_to_dbm(-384), -96);
        assert_eq!(quarter_db_to_dbm(-1), 0);
        assert_eq!(quarter_db_to_dbm(-1024), 0);
    }

    #[test]
    fn power_detector_enable_clears_remain_three_distinct_images() {
        let first = clear_power_detector_enable_field(0b111, 0b010);
        let second = clear_power_detector_enable_field(first, 0b001);
        let third = clear_power_detector_enable_field(second, 0b100);
        assert_eq!([first, second, third], [0b101, 0b100, 0]);
    }

    #[test]
    fn standalone_feature_inputs_retain_only_the_vendor_low_bit() {
        assert!(!low_bit(0));
        assert!(low_bit(1));
        assert!(!low_bit(2));
        assert!(low_bit(u32::MAX));
    }

    #[test]
    fn baseband_partial_field_edges_preserve_unselected_bits() {
        assert_eq!(clear_baseband_tail_low(7), 4);
        assert_eq!(clear_baseband_tail_high(7), 3);
    }

    #[test]
    fn txdc_capture_images_replace_only_owned_fields() {
        assert_eq!(
            txdc_power_detector_images(0xa5a5_5a34, 0x5a5a_0ab5),
            (0x34, 0x0000_0ab0, 0xa5a5_5af0, 0x5a5a_0785)
        );
    }

    #[test]
    fn calibration_tone_images_match_both_complete_archive_calls() {
        assert_eq!(tone_path_image(0xa000_0000, true, 0x80, 0), 0xa004_0020);
        assert_eq!(tone_path_image(0xa000_0000, false, 0x80, 0x28), 0xa003_6020);
        assert_eq!(tone_path_image(0xbfff_ffff, false, 0, 0), 0xb000_0000);
    }

    #[test]
    fn txiq_mismatch_images_match_complete_rom_leaves() {
        assert_eq!(
            txiq_first_mismatch_image(0xa000_0000, true, 0x50, 0x80),
            0xa42e_c020
        );
        assert_eq!(txiq_second_mismatch_image(0xa6ae_c020, true), 0xa8ae_c020);
        assert_eq!(txiq_second_mismatch_image(0xa6ae_c020, false), 0xa1ae_c020);
    }

    #[test]
    fn txiq_coefficient_fields_apply_complete_rom_saturation() {
        assert_eq!(tx_iq_gain_field(-128), 0x21);
        assert_eq!(tx_iq_gain_field(-32), 0x21);
        assert_eq!(tx_iq_gain_field(-31), 0x21);
        assert_eq!(tx_iq_gain_field(0), 0x00);
        assert_eq!(tx_iq_gain_field(31), 0x1f);
        assert_eq!(tx_iq_gain_field(127), 0x1f);

        assert_eq!(tx_iq_phase_field(-128), 0x41);
        assert_eq!(tx_iq_phase_field(-64), 0x41);
        assert_eq!(tx_iq_phase_field(-63), 0x41);
        assert_eq!(tx_iq_phase_field(0), 0x00);
        assert_eq!(tx_iq_phase_field(63), 0x3f);
        assert_eq!(tx_iq_phase_field(127), 0x3f);
    }
}
