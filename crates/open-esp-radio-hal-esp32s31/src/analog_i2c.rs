//! ESP32-S31 PMU operations used by the open PHY.
//!
//! Operation order comes from complete ROM/blob bodies. The documented PMU
//! register ownership and field decoding stay in the integration layer's
//! official PAC implementation.

/// Platform PMU capability required by the PHY cold-start sequence.
pub trait PhyPmuControl {
    fn set_rf_circuit_power(&mut self, enabled: bool);
    fn set_bb_i2c_power_tie(&mut self, enabled: bool);
    fn analog_i2c_is_powered(&self) -> bool;
    fn set_analog_i2c_power(&mut self, enabled: bool);
    fn analog_i2c_reset_is_released(&self) -> bool;
    fn set_analog_i2c_reset_released(&mut self, released: bool);
    fn enable_frontend_baseband_power(&mut self);
}

/// Apply the two PMU updates before the 100 us analog-I2C power delay.
///
/// SOURCE[`libphy.a[phy_reg.o]::phy_open_i2c_xpd_new`, offsets
/// `0x2e..0x4e`]. It clears `PMU.RF_PWC.XPD_RF_CIRCUIT`, then clears
/// `PMU.IMM_HP_CK_POWER_0.TIE_HIGH_XPD_BB_I2C`.
pub fn prepare_open_i2c_pre_delay(platform: &mut impl PhyPmuControl) {
    platform.set_rf_circuit_power(false);
    platform.set_bb_i2c_power_tie(false);
}

/// Power the RF/analog-I2C circuits and release the peripheral-I2C reset.
///
/// SOURCE[complete `libphy.a[phy_reg.o]::phy_open_i2c_xpd_new`]. When
/// analog-I2C was powered down, reset is explicitly asserted before release;
/// this edge is deliberately not collapsed into one final write.
pub fn complete_open_i2c_power_and_reset(platform: &mut impl PhyPmuControl) {
    platform.set_rf_circuit_power(true);
    platform.set_bb_i2c_power_tie(true);

    if !platform.analog_i2c_is_powered() {
        platform.set_analog_i2c_power(true);
        platform.set_analog_i2c_reset_released(false);
        platform.set_analog_i2c_reset_released(true);
    }
    if !platform.analog_i2c_reset_is_released() {
        platform.set_analog_i2c_reset_released(true);
    }
}

/// Complete the ROM frontend/baseband clock leaf after internal radio gates.
///
/// SOURCE[complete rev0 ROM `phy_open_fe_bb_clk`]. The PMU update is the
/// fourth and final operation; the first three undocumented radio gates are
/// executed by the recovered radio PAC before this call.
pub fn enable_frontend_baseband_power(platform: &mut impl PhyPmuControl) {
    platform.enable_frontend_baseband_power();
}

#[cfg(test)]
mod tests {
    use std::vec::Vec;

    use super::{complete_open_i2c_power_and_reset, prepare_open_i2c_pre_delay, PhyPmuControl};

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum Operation {
        SetRfCircuitPower(bool),
        SetBbI2cPowerTie(bool),
        SetAnalogI2cPower(bool),
        SetAnalogI2cReset(bool),
    }

    struct FakePmu {
        powered: bool,
        reset_released: bool,
        operations: Vec<Operation>,
    }

    impl FakePmu {
        fn new(powered: bool, reset_released: bool) -> Self {
            Self {
                powered,
                reset_released,
                operations: Vec::new(),
            }
        }
    }

    impl PhyPmuControl for FakePmu {
        fn set_rf_circuit_power(&mut self, enabled: bool) {
            self.operations.push(Operation::SetRfCircuitPower(enabled));
        }

        fn set_bb_i2c_power_tie(&mut self, enabled: bool) {
            self.operations.push(Operation::SetBbI2cPowerTie(enabled));
        }

        fn analog_i2c_is_powered(&self) -> bool {
            self.powered
        }

        fn set_analog_i2c_power(&mut self, enabled: bool) {
            self.operations.push(Operation::SetAnalogI2cPower(enabled));
            self.powered = enabled;
        }

        fn analog_i2c_reset_is_released(&self) -> bool {
            self.reset_released
        }

        fn set_analog_i2c_reset_released(&mut self, released: bool) {
            self.operations.push(Operation::SetAnalogI2cReset(released));
            self.reset_released = released;
        }

        fn enable_frontend_baseband_power(&mut self) {}
    }

    #[test]
    fn pre_delay_retains_both_blob_operations() {
        let mut pmu = FakePmu::new(true, true);
        prepare_open_i2c_pre_delay(&mut pmu);
        assert_eq!(
            pmu.operations,
            [
                Operation::SetRfCircuitPower(false),
                Operation::SetBbI2cPowerTie(false),
            ]
        );
    }

    #[test]
    fn powered_down_i2c_gets_the_complete_assert_release_edge() {
        let mut pmu = FakePmu::new(false, false);
        complete_open_i2c_power_and_reset(&mut pmu);
        assert_eq!(
            pmu.operations,
            [
                Operation::SetRfCircuitPower(true),
                Operation::SetBbI2cPowerTie(true),
                Operation::SetAnalogI2cPower(true),
                Operation::SetAnalogI2cReset(false),
                Operation::SetAnalogI2cReset(true),
            ]
        );
        assert!(pmu.powered);
        assert!(pmu.reset_released);
    }

    #[test]
    fn already_powered_i2c_only_releases_reset_when_needed() {
        let mut pmu = FakePmu::new(true, false);
        complete_open_i2c_power_and_reset(&mut pmu);
        assert_eq!(
            pmu.operations,
            [
                Operation::SetRfCircuitPower(true),
                Operation::SetBbI2cPowerTie(true),
                Operation::SetAnalogI2cReset(true),
            ]
        );
    }
}
