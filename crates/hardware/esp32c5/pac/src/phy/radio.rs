//! PMU analog I2C power and the PHY baseband configuration words.
//!
//! SOURCE: reviewed evidence `C5_BLOB_LIBPHY_RF_INIT_LEAVES` (ESP32-C5
//! `libphy.a[phy_reg.o]::phy_open_i2c_xpd`,
//! `libphy.a[phy_i2c.o]::{phy_dac_rate_set, phy_adc_rate_set}`) with the
//! PMU fields of `ESP_IDF_4D59230D_C5_PMU`.

#![forbid(unsafe_code)]

use crate::{
    generated::{self, PhyRateBit},
    svd,
};

/// Unique owner of the PHY's PMU fields and baseband configuration words.
#[must_use = "dropping the PHY radio owner loses its register authority"]
pub struct PhyRadioRegisters {
    peripherals: svd::peripheral_ownership::PhyRadioPeripherals,
}

impl PhyRadioRegisters {
    pub(crate) const fn new(peripherals: svd::peripheral_ownership::PhyRadioPeripherals) -> Self {
        Self { peripherals }
    }

    fn pmu(&self) -> &svd::PmuRadio {
        &self.peripherals.pmu_radio
    }

    fn baseband(&self) -> &svd::PhyBasebandConfig {
        &self.peripherals.phy_baseband_config
    }

    /// Power the RF analog I2C blocks as `phy_open_i2c_xpd` does: power the
    /// 5 GHz transceiver, TX RF, RX PBus, clock generator and PLL I2C, tie
    /// the BB I2C power high, power and reset-pulse the peripheral I2C when
    /// it was off, and release its reset when it is still held.
    pub fn power_analog_i2c(&mut self) {
        generated::power_rf_analog_i2c(self.pmu());
        generated::tie_high_bb_analog_i2c_power(self.pmu());
        if !svd::field_read::observe_peripheral_analog_i2c_power(self.pmu()) {
            generated::power_peripheral_analog_i2c(self.pmu());
            generated::hold_peripheral_analog_i2c_reset(self.pmu());
            generated::release_peripheral_analog_i2c_reset(self.pmu());
        }
        if !svd::field_read::observe_peripheral_analog_i2c_released(self.pmu()) {
            generated::release_peripheral_analog_i2c_reset(self.pmu());
        }
    }

    /// Power the 5 GHz clock generator, as `phy_rf_init` does after
    /// `phy_open_i2c_xpd`.
    pub fn power_5g_clock_generator(&mut self) {
        generated::power_5g_clock_generator(self.pmu());
    }

    /// The baseband half of `phy_adc_rate_set(rate)`: bit 1 and then bit 0
    /// of the configuration word take the rate bit.
    pub fn set_adc_rate_bits(&mut self, rate: bool) {
        let bit = if rate {
            PhyRateBit::Set
        } else {
            PhyRateBit::Clear
        };
        generated::set_phy_adc_rate_high(self.baseband(), bit);
        generated::set_phy_adc_rate_low(self.baseband(), bit);
    }

    /// The baseband half of `phy_dac_rate_set`: clear bit 3 and then bit 2.
    pub fn clear_dac_rate_bits(&mut self) {
        generated::clear_phy_dac_rate_high(self.baseband());
        generated::clear_phy_dac_rate_low(self.baseband());
    }
}
