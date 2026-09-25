//! Ownership-bound leaves shared by the cold PHY prelude.

#![forbid(unsafe_code)]

use super::{RadioPhyRegisters, RxDcoControlField};

impl RadioPhyRegisters {
    /// Capture the RX-DCO control field, then clear it with a fresh RMW.
    pub fn capture_and_clear_rx_dco_control(&mut self) -> RxDcoControlField {
        let registers = &self.peripherals.phy_rx_dco_oracle;
        let saved = crate::svd::field_read::capture_phy_rx_dco_calibration_control(registers);
        crate::generated::clear_phy_rx_dco_calibration_control(registers);
        RxDcoControlField::from_capture(saved)
    }

    /// Restore one captured RX-DCO control field.
    pub fn restore_rx_dco_control(&mut self, field: RxDcoControlField) {
        let saved = crate::generated::PhyRxDcoCalibrationControl::new(u32::from(field.bits()))
            .expect("generated two-bit RX-DCO readback must fit its restore domain");
        crate::generated::restore_phy_rx_dco_calibration_control(
            &self.peripherals.phy_rx_dco_oracle,
            saved,
        );
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

pub(crate) mod agc;

pub(crate) mod baseband;

pub(crate) mod cfr;

pub mod clock;

pub(crate) mod frequency;

pub mod i2c;

pub(crate) mod iq_estimator;

pub(crate) mod low_power;

pub mod pbus;

pub(crate) mod table_memory;
