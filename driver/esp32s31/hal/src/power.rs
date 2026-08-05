//! Finite ESP32-S31 modem/PHY prerequisite sequence.
//!
//! The order is the merged cold-boot path immediately preceding
//! `register_chipv7_phy` in the ESP32-S31 `esp-radio` and `esp-phy` oracle.
//! Wi-Fi MAC clocks are intentionally excluded: they belong to the later MAC
//! start transition.

/// Official chip-platform capability needed by the open radio power sequence.
///
/// The open driver owns the order of operations, while the integration layer
/// owns the chip PAC handles and implements each operation with that PAC. This
/// keeps documented system peripherals out of the recovered radio PAC and
/// makes the required ownership transfer explicit in the `Radio<P>` token.
pub trait PowerClockControl {
    fn set_wifi_baseband_and_mac_reset(&mut self, asserted: bool);
    fn select_hp_active_modem_icg(&mut self);
    fn apply_modem_icg_selection(&mut self);
    fn apply_sleep_icg_selection(&mut self);
    fn enable_modem_register_bus_clock(&mut self);
    fn configure_hp_active_modem_clock_map(&mut self);
    fn configure_shared_modem_clock_map(&mut self);
    fn configure_modem_source_clocks(&mut self);
    fn set_wifi_baseband_reset(&mut self, asserted: bool);
    fn enable_phy_calibration_clocks(&mut self);
    fn select_phy_i2c_160mhz_source(&mut self);
    fn enable_phy_i2c_master_clock(&mut self);
    fn power_clock_images(&self) -> PowerClockImages;
}

/// Semantic read-back state captured after the cold clock/reset sequence.
///
/// Field decoding stays beside the official PAC in the integration layer.
/// Neither register handles, addresses nor system-register masks cross into
/// the open driver.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PowerClockImages {
    pub reset_released: bool,
    pub hp_active_icg_selected: bool,
    pub modem_bus_clock_enabled: bool,
    pub hp_active_clock_map_configured: bool,
    pub shared_clock_map_configured: bool,
    pub modem_source_clocks_configured: bool,
    pub phy_calibration_clocks_enabled: bool,
    pub phy_i2c_160mhz_selected: bool,
    pub phy_i2c_master_clock_enabled: bool,
}

/// Read-back checkpoint following the finite prerequisite sequence.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PowerCheckpoint {
    /// Both Wi-Fi reset lines were released.
    ResetReleased,
    /// PMU selects the HP-active modem ICG code.
    HpActiveIcg,
    /// The modem register bus clock is enabled.
    ModemBusClock,
    /// HP-active modem domain clocks are ungated.
    HpActiveClockMap,
    /// Shared low-power modem clocks are ungated.
    SharedClockMap,
    /// The modem PLL/XTAL source configuration is active.
    ModemClockSource,
    /// All PHY frontend and calibration clocks are enabled.
    PhyClocks,
    /// PHY-I²C uses the 160 MHz source.
    I2cSource,
    /// The PHY-I²C master clock is enabled.
    I2cClock,
}

/// A prerequisite failed its bounded semantic read-back checkpoint.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PowerError {
    /// Semantic checkpoint that failed.
    pub checkpoint: PowerCheckpoint,
    /// Expected semantic state.
    pub expected: bool,
    /// Observed semantic state.
    pub observed: bool,
}

pub(crate) fn execute_owned(platform: &mut impl PowerClockControl) -> Result<(), PowerError> {
    // Keep the operation order here: it is a lifecycle property recovered
    // from the qualified S31 esp-hal clock implementation, not a
    // property of the register layout.
    platform.set_wifi_baseband_and_mac_reset(true);
    platform.set_wifi_baseband_and_mac_reset(false);
    platform.select_hp_active_modem_icg();
    platform.apply_modem_icg_selection();
    platform.apply_sleep_icg_selection();
    platform.enable_modem_register_bus_clock();
    platform.configure_hp_active_modem_clock_map();
    platform.configure_shared_modem_clock_map();
    platform.configure_modem_source_clocks();
    platform.set_wifi_baseband_reset(true);
    platform.set_wifi_baseband_reset(false);
    platform.enable_phy_calibration_clocks();
    platform.select_phy_i2c_160mhz_source();
    platform.enable_phy_i2c_master_clock();

    let images = platform.power_clock_images();
    verify_state(PowerCheckpoint::ResetReleased, images.reset_released)?;
    verify_state(PowerCheckpoint::HpActiveIcg, images.hp_active_icg_selected)?;
    verify_state(
        PowerCheckpoint::ModemBusClock,
        images.modem_bus_clock_enabled,
    )?;
    verify_state(
        PowerCheckpoint::HpActiveClockMap,
        images.hp_active_clock_map_configured,
    )?;
    verify_state(
        PowerCheckpoint::SharedClockMap,
        images.shared_clock_map_configured,
    )?;
    verify_state(
        PowerCheckpoint::ModemClockSource,
        images.modem_source_clocks_configured,
    )?;
    verify_state(
        PowerCheckpoint::PhyClocks,
        images.phy_calibration_clocks_enabled,
    )?;
    verify_state(PowerCheckpoint::I2cSource, images.phy_i2c_160mhz_selected)?;
    verify_state(
        PowerCheckpoint::I2cClock,
        images.phy_i2c_master_clock_enabled,
    )
}

fn verify_state(checkpoint: PowerCheckpoint, observed: bool) -> Result<(), PowerError> {
    if observed {
        Ok(())
    } else {
        Err(PowerError {
            checkpoint,
            expected: true,
            observed,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::vec::Vec;

    use super::{PowerCheckpoint, PowerClockControl, PowerClockImages, PowerError, execute_owned};

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum Operation {
        ResetWifi(bool),
        SelectHpActiveIcg,
        ApplyModemIcg,
        ApplySleepIcg,
        EnableModemBus,
        ConfigureHpActiveMap,
        ConfigureSharedMap,
        ConfigureModemSource,
        ResetBaseband(bool),
        EnablePhyClocks,
        SelectI2c160Mhz,
        EnableI2cClock,
    }

    struct FakePlatform {
        operations: Vec<Operation>,
        images: PowerClockImages,
    }

    impl FakePlatform {
        fn ready() -> Self {
            Self {
                operations: Vec::new(),
                images: PowerClockImages {
                    reset_released: true,
                    hp_active_icg_selected: true,
                    modem_bus_clock_enabled: true,
                    hp_active_clock_map_configured: true,
                    shared_clock_map_configured: true,
                    modem_source_clocks_configured: true,
                    phy_calibration_clocks_enabled: true,
                    phy_i2c_160mhz_selected: true,
                    phy_i2c_master_clock_enabled: true,
                },
            }
        }
    }

    impl PowerClockControl for FakePlatform {
        fn set_wifi_baseband_and_mac_reset(&mut self, asserted: bool) {
            self.operations.push(Operation::ResetWifi(asserted));
        }

        fn select_hp_active_modem_icg(&mut self) {
            self.operations.push(Operation::SelectHpActiveIcg);
        }

        fn apply_modem_icg_selection(&mut self) {
            self.operations.push(Operation::ApplyModemIcg);
        }

        fn apply_sleep_icg_selection(&mut self) {
            self.operations.push(Operation::ApplySleepIcg);
        }

        fn enable_modem_register_bus_clock(&mut self) {
            self.operations.push(Operation::EnableModemBus);
        }

        fn configure_hp_active_modem_clock_map(&mut self) {
            self.operations.push(Operation::ConfigureHpActiveMap);
        }

        fn configure_shared_modem_clock_map(&mut self) {
            self.operations.push(Operation::ConfigureSharedMap);
        }

        fn configure_modem_source_clocks(&mut self) {
            self.operations.push(Operation::ConfigureModemSource);
        }

        fn set_wifi_baseband_reset(&mut self, asserted: bool) {
            self.operations.push(Operation::ResetBaseband(asserted));
        }

        fn enable_phy_calibration_clocks(&mut self) {
            self.operations.push(Operation::EnablePhyClocks);
        }

        fn select_phy_i2c_160mhz_source(&mut self) {
            self.operations.push(Operation::SelectI2c160Mhz);
        }

        fn enable_phy_i2c_master_clock(&mut self) {
            self.operations.push(Operation::EnableI2cClock);
        }

        fn power_clock_images(&self) -> PowerClockImages {
            self.images
        }
    }

    #[test]
    fn exact_semantic_sequence_is_finite_and_ordered() {
        let mut platform = FakePlatform::ready();
        assert_eq!(execute_owned(&mut platform), Ok(()));
        assert_eq!(
            platform.operations,
            [
                Operation::ResetWifi(true),
                Operation::ResetWifi(false),
                Operation::SelectHpActiveIcg,
                Operation::ApplyModemIcg,
                Operation::ApplySleepIcg,
                Operation::EnableModemBus,
                Operation::ConfigureHpActiveMap,
                Operation::ConfigureSharedMap,
                Operation::ConfigureModemSource,
                Operation::ResetBaseband(true),
                Operation::ResetBaseband(false),
                Operation::EnablePhyClocks,
                Operation::SelectI2c160Mhz,
                Operation::EnableI2cClock,
            ]
        );
    }

    #[test]
    fn failed_semantic_readback_names_the_exact_checkpoint() {
        let mut platform = FakePlatform::ready();
        platform.images.modem_source_clocks_configured = false;

        assert_eq!(
            execute_owned(&mut platform),
            Err(PowerError {
                checkpoint: PowerCheckpoint::ModemClockSource,
                expected: true,
                observed: false,
            })
        );
    }
}
