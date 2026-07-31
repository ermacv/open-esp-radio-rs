//! Ownership-bound DC/IQ estimator operations.

use super::RadioRegisters;

const fn truncate_control_window(control: u16) -> u16 {
    control & 0x7fff
}

impl RadioRegisters {
    /// Publish the complete rev0 ROM estimator configuration prefix.
    ///
    /// Complete `phy_iq_est_enable` performs three separate fresh-read RMW
    /// operations in this order. Bit 15 of `control` is discarded by the ROM
    /// shift/mask sequence and therefore is deliberately truncated here.
    pub fn configure_iq_estimator(&mut self, control: u16) {
        // SAFETY: one is representable by the SVD-described two-bit field.
        self.peripherals
            .phy_iq_estimator_oracle
            .estimator_config()
            .modify(|_, w| unsafe { w.config_mode_unknown().bits(1) });
        // SAFETY: two is representable by the SVD-described two-bit field.
        self.peripherals
            .phy_iq_estimator_oracle
            .estimator_control()
            .modify(|_, w| unsafe { w.mode_unknown().bits(2) });
        // SAFETY: truncation proves the value fits the fifteen-bit field.
        self.peripherals
            .phy_iq_estimator_oracle
            .estimator_control()
            .modify(|_, w| unsafe {
                w.control_window_unknown()
                    .bits(truncate_control_window(control))
            });
    }

    /// Set or clear the first estimator-enable phase through one fresh RMW.
    pub fn set_iq_estimator_start_enabled(&mut self, enabled: bool) {
        self.peripherals
            .phy_iq_estimator_oracle
            .estimator_control()
            .modify(|_, w| w.start_enable().bit(enabled));
    }

    /// Set or clear the second estimator-enable phase through one fresh RMW.
    pub fn set_iq_estimator_measurement_enabled(&mut self, enabled: bool) {
        self.peripherals
            .phy_iq_estimator_oracle
            .estimator_control()
            .modify(|_, w| w.measurement_enable().bit(enabled));
    }

    /// Sample the ready word and shared activity word exactly once each.
    pub fn sample_iq_estimator_readiness(&mut self) -> (bool, bool) {
        let ready = self
            .peripherals
            .phy_iq_estimator_oracle
            .estimator_ready_status()
            .read()
            .ready()
            .bit_is_set();
        let activity = self
            .peripherals
            .phy_iq_estimator_oracle
            .estimator_activity_status()
            .read()
            .activity_unknown()
            .bits()
            != 0;
        (ready, activity)
    }

    /// Read I, Q and power accumulators in complete `phy_dc_iq_est` order.
    pub fn read_iq_estimator_dc_accumulators(&mut self) -> [i32; 3] {
        let i = self
            .peripherals
            .phy_iq_estimator_oracle
            .dc_i_accumulator()
            .read()
            .bits() as i32;
        let q = self
            .peripherals
            .phy_iq_estimator_oracle
            .dc_q_accumulator()
            .read()
            .bits() as i32;
        let power = self
            .peripherals
            .phy_iq_estimator_oracle
            .power_accumulator()
            .read()
            .bits() as i32;
        [i, q, power]
    }

    /// Read the signed total-power accumulator exactly once.
    pub fn read_iq_estimator_total_power(&mut self) -> i32 {
        self.peripherals
            .phy_iq_estimator_oracle
            .power_accumulator()
            .read()
            .bits() as i32
    }

    /// Read signal words in complete `phy_rxiq_get_mis` physical order.
    ///
    /// The returned semantic order is sum-I, difference-I, difference-Q,
    /// sum-Q. The hardware reads remain sum-I, sum-Q, difference-Q,
    /// difference-I as proved by the complete rev0 ROM body.
    pub fn read_iq_estimator_rxiq_mismatch(&mut self) -> [i32; 4] {
        let sum_i = self
            .peripherals
            .phy_iq_estimator_oracle
            .signal_power_sum_i()
            .read()
            .bits() as i32;
        let sum_q = self
            .peripherals
            .phy_iq_estimator_oracle
            .signal_power_sum_q()
            .read()
            .bits() as i32;
        let difference_q = self
            .peripherals
            .phy_iq_estimator_oracle
            .signal_power_difference_q()
            .read()
            .bits() as i32;
        let difference_i = self
            .peripherals
            .phy_iq_estimator_oracle
            .signal_power_difference_i()
            .read()
            .bits() as i32;
        [sum_i, difference_i, difference_q, sum_q]
    }

    /// Read signal words in complete `phy_get_rx_sig_pwr` physical order.
    ///
    /// Hardware reads remain sum-I, sum-Q, difference-I, difference-Q. The
    /// returned common semantic order is sum-I, difference-I, difference-Q,
    /// sum-Q.
    pub fn read_iq_estimator_signal_power(&mut self) -> [i32; 4] {
        let sum_i = self
            .peripherals
            .phy_iq_estimator_oracle
            .signal_power_sum_i()
            .read()
            .bits() as i32;
        let sum_q = self
            .peripherals
            .phy_iq_estimator_oracle
            .signal_power_sum_q()
            .read()
            .bits() as i32;
        let difference_i = self
            .peripherals
            .phy_iq_estimator_oracle
            .signal_power_difference_i()
            .read()
            .bits() as i32;
        let difference_q = self
            .peripherals
            .phy_iq_estimator_oracle
            .signal_power_difference_q()
            .read()
            .bits() as i32;
        [sum_i, difference_i, difference_q, sum_q]
    }

    /// Read the complete shared estimator-activity register image once.
    ///
    /// `phy_check_rx_sat` consumes the raw image after its own mask/shift;
    /// the bounded 100-sample policy remains in the upper PHY transition.
    pub fn read_iq_estimator_activity_image(&mut self) -> u32 {
        self.peripherals
            .phy_iq_estimator_oracle
            .estimator_activity_status()
            .read()
            .bits()
    }
}

#[cfg(test)]
mod tests {
    use super::truncate_control_window;

    #[test]
    fn control_window_matches_the_rom_u16_shift_and_mask() {
        assert_eq!(truncate_control_window(0x8fa0), 0x0fa0);
        assert_eq!(truncate_control_window(u16::MAX), 0x7fff);
    }
}
