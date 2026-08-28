//! Ownership-bound leaves shared by the cold PHY prelude.

#![forbid(unsafe_code)]

use super::{RadioPhyRegisters, RxDcoControlPrepareError, RxDcoControlRestoreError};

impl RadioPhyRegisters {
    /// Retain and clear the RX-DCO control field through two fresh reads.
    ///
    /// The private PAC restore slot is a two-entry LIFO because the crystal
    /// duty operation masks this field around a nested RX-DCO calibration
    /// which independently performs the same save/clear/restore sequence.
    pub fn prepare_rx_dco_control_restore(&mut self) -> Result<(), RxDcoControlPrepareError> {
        let control = self.peripherals.phy_rx_dco_oracle.control();
        self.restore_slot.prepare_rx_dco_with(|| {
            let saved = control.read().calibration_control_unknown().bits();
            control.modify(|_, w| w.calibration_control_unknown().set(0));
            saved
        })
    }

    /// Restore the most recently retained RX-DCO control field.
    pub fn restore_rx_dco_control(&mut self) -> Result<(), RxDcoControlRestoreError> {
        let control = self.peripherals.phy_rx_dco_oracle.control();
        self.restore_slot.restore_rx_dco_with(|saved| {
            control.modify(|_, w| w.calibration_control_unknown().set(saved));
        })
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
            .value()
            .bits()
    }
}
