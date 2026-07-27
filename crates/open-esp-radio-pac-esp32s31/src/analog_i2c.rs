//! Ownership-bound PMU controls for the analog PHY-I²C power domain.
//!
//! Field identities come from the pinned S31 PMU SVD and headers. The HAL
//! retains the edge ordering recovered from
//! `libphy.a[phy_reg.o]::phy_open_i2c_xpd_new`.

use super::RadioRegisters;

impl RadioRegisters {
    /// Power all recovered RF circuit subdomains as one oracle-defined group.
    pub fn set_rf_circuit_power(&mut self, powered: bool) {
        let value = if powered { u16::MAX } else { 0 };
        // SAFETY: `value` exactly fits the recovered 16-bit
        // `XPD_RF_CIRCUIT` field.
        unsafe {
            self.peripherals
                .pmu
                .rf_pwc()
                .modify(|_, w| w.xpd_rf_circuit().bits(value));
        }
    }

    /// Drive or release the immediate BB-I²C power tie.
    pub fn set_bb_i2c_power_tie(&mut self, powered: bool) {
        self.peripherals
            .pmu
            .imm_hp_ck_power_0()
            .modify(|_, w| w.tie_high_xpd_bb_i2c().bit(powered));
    }

    /// Whether the analog peripheral-I²C power bit is currently asserted.
    pub fn analog_i2c_is_powered(&mut self) -> bool {
        self.peripherals
            .pmu
            .ana_peri_pwr_ctrl()
            .read()
            .xpd_perif_i2c()
            .bit_is_set()
    }

    /// Assert or clear analog peripheral-I²C power.
    pub fn set_analog_i2c_power(&mut self, powered: bool) {
        self.peripherals
            .pmu
            .ana_peri_pwr_ctrl()
            .modify(|_, w| w.xpd_perif_i2c().bit(powered));
    }

    /// Whether the active-low analog peripheral-I²C reset is released.
    pub fn analog_i2c_reset_is_released(&mut self) -> bool {
        self.peripherals
            .pmu
            .ana_peri_pwr_ctrl()
            .read()
            .rstb_perif_i2c()
            .bit_is_set()
    }

    /// Assert or release the active-low analog peripheral-I²C reset.
    pub fn set_analog_i2c_reset_released(&mut self, released: bool) {
        self.peripherals
            .pmu
            .ana_peri_pwr_ctrl()
            .modify(|_, w| w.rstb_perif_i2c().bit(released));
    }
}
