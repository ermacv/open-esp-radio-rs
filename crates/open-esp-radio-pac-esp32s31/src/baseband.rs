//! Ownership-bound access to recovered ESP32-S31 PHY/baseband registers.
//!
//! Register layout and legal field images come from
//! `svd/esp32s31-radio.svd`. Complete ROM/blob bodies cited there define the
//! finite operation order.

use super::RadioRegisters;

impl RadioRegisters {
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
        let bb = &self.peripherals.phy_baseband_config_oracle;
        bb.front_end_init_0894()
            .modify(|_, w| w.init_enable_unknown().set_bit());
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
        bb.tx_pa_control_0().modify(|_, w| {
            // SAFETY: four is the instruction-exact low-byte value from the
            // complete ROM leaf and fits the recovered eight-bit field.
            unsafe { w.front_end_low_unknown().bits(4) }
        });
        bb.adc_rate_and_front_end_control()
            .modify(|_, w| w.adc_rate_low_or_front_end_control_unknown().set_bit());
        bb.adc_rate_and_front_end_control()
            .modify(|_, w| w.adc_rate_high_or_front_end_control_unknown().set_bit());
        bb.iq_correction_control()
            .modify(|_, w| w.front_end_init_high_unknown().set_bit());
        bb.iq_correction_aux()
            .modify(|_, w| w.front_end_init_high_unknown().set_bit());
        bb.front_end_init_0c20().modify(|_, w| {
            // SAFETY: 0x57 is the complete ROM leaf's final low-byte value
            // and fits the recovered eight-bit field.
            unsafe { w.init_low_unknown().bits(0x57) }
        });
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
