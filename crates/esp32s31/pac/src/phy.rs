//! Ownership-bound leaves shared by the cold PHY prelude.

use super::RadioRegisters;

impl RadioRegisters {
    /// Capture and clear the two RX-DCO control bits through two fresh reads.
    pub fn capture_and_clear_rx_dco_control(&mut self) -> u8 {
        let control = self.peripherals.phy_rx_dco_oracle.control();
        let saved = control.read().calibration_control_unknown().bits();
        control.modify(|_, w| unsafe { w.calibration_control_unknown().bits(0) });
        saved
    }

    /// Restore only the captured RX-DCO control field through one fresh RMW.
    pub fn restore_rx_dco_control(&mut self, saved_field: u8) {
        debug_assert!(saved_field <= 3);
        self.peripherals
            .phy_rx_dco_oracle
            .control()
            .modify(|_, w| unsafe { w.calibration_control_unknown().bits(saved_field & 3) });
    }

    /// Sample the full-width counter used by the SDM-stability deadline.
    ///
    /// Complete rev0 ROM `phy_wait_i2c_sdm_stable` at `0x2f823e76` proves the
    /// address and wrapping-difference consumer, but not the clock source.
    pub fn sample_sdm_deadline_counter(&mut self) -> u32 {
        self.peripherals
            .phy_cold_deadline_oracle
            .deadline_counter_unknown()
            .read()
            .bits()
    }
}
