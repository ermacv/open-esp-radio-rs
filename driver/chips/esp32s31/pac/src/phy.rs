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
        let registers = &self.peripherals.phy_rx_dco_oracle;
        self.restore_slot.prepare_rx_dco_with(|| {
            let saved = crate::svd::field_read::capture_phy_rx_dco_calibration_control(registers);
            crate::generated::clear_phy_rx_dco_calibration_control(registers);
            saved
        })
    }

    /// Restore the most recently retained RX-DCO control field.
    pub fn restore_rx_dco_control(&mut self) -> Result<(), RxDcoControlRestoreError> {
        let registers = &self.peripherals.phy_rx_dco_oracle;
        self.restore_slot.restore_rx_dco_with(|saved| {
            let saved = crate::generated::PhyRxDcoCalibrationControl::new(u32::from(saved))
                .expect("generated two-bit RX-DCO readback must fit its restore domain");
            crate::generated::restore_phy_rx_dco_calibration_control(registers, saved);
        })
    }

    /// Sample the full-width counter used by the SDM-stability deadline.
    ///
    /// Complete rev0 ROM `phy_wait_i2c_sdm_stable` at `0x2f823e76` proves the
    /// address and wrapping-difference consumer, but not the clock source.
    pub fn sample_sdm_deadline_counter(&mut self) -> u32 {
        crate::svd::field_read::sample_phy_sdm_deadline_counter(
            &self.peripherals.phy_cold_deadline_oracle,
        )
    }
}
