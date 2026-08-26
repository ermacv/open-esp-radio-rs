//! Finite ESP32-S31 modem/PHY prerequisite sequence.
//!
//! The order is the merged cold-boot path immediately preceding
//! `register_chipv7_phy` in the ESP32-S31 `esp-radio` and `esp-phy` oracle.
//! Wi-Fi MAC clocks are intentionally excluded: they belong to the later MAC
//! start transition.

/// Official chip-platform capability needed by the open radio power sequence.
///
/// The open driver owns the order of operations. Route-owned modem words stay
/// behind typed custom-PAC operations; the integration owns only the remaining
/// platform system-register capability. The common-PHY
/// `MODEM_LPCON.TICK_CONF` carveout is not part of this trait.
pub trait PowerClockControl {
    fn select_hp_active_modem_icg(&mut self);
    fn apply_modem_icg_selection(&mut self);
    fn apply_sleep_icg_selection(&mut self);
    fn enable_modem_register_bus_clock(&mut self);
    fn configure_modem_source_clocks(&mut self);
    fn platform_power_clock_images(&self) -> PlatformPowerClockImages;
}

/// Platform-owned portion of the power checkpoint. MODEM_LPCON fields are
/// deliberately absent and are joined from the route PAC by the executor.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PlatformPowerClockImages {
    pub hp_active_icg_selected: bool,
    pub modem_bus_clock_enabled: bool,
    pub modem_source_clocks_configured: bool,
}

pub(crate) trait SharedPowerClockControl {
    fn set_wifi_baseband_and_mac_reset(&mut self, asserted: bool);
    fn set_wifi_baseband_reset(&mut self, asserted: bool);
    fn configure_wifi_power_clock_map(&mut self);
    fn enable_phy_calibration_clocks(&mut self);
    fn select_phy_i2c_160mhz_source(&mut self);
    fn modem_syscon_power_observation(
        &self,
    ) -> open_esp_radio_esp32s31_pac::ModemSysconPowerObservation;
    fn prepare_shared_modem_clock_map(&mut self);
    fn retain_phy_i2c_master_clock(&mut self);
    fn shared_modem_clock_observation(
        &self,
    ) -> open_esp_radio_esp32s31_pac::SharedModemClockObservation;
}

impl SharedPowerClockControl for open_esp_radio_esp32s31_pac::WifiColdRegisters {
    fn set_wifi_baseband_and_mac_reset(&mut self, asserted: bool) {
        self.set_wifi_baseband_and_mac_reset(asserted);
    }
    fn set_wifi_baseband_reset(&mut self, asserted: bool) {
        self.set_wifi_baseband_reset(asserted);
    }
    fn configure_wifi_power_clock_map(&mut self) {
        self.configure_wifi_power_clock_map();
    }
    fn enable_phy_calibration_clocks(&mut self) {
        self.enable_phy_calibration_clocks();
    }
    fn select_phy_i2c_160mhz_source(&mut self) {
        self.select_phy_i2c_160mhz_source();
    }
    fn modem_syscon_power_observation(
        &self,
    ) -> open_esp_radio_esp32s31_pac::ModemSysconPowerObservation {
        self.modem_syscon_power_observation()
    }
    fn prepare_shared_modem_clock_map(&mut self) {
        open_esp_radio_esp32s31_pac::WifiColdRegisters::prepare_shared_modem_clock_map(self);
    }

    fn retain_phy_i2c_master_clock(&mut self) {
        open_esp_radio_esp32s31_pac::WifiColdRegisters::retain_phy_i2c_master_clock(self);
    }

    fn shared_modem_clock_observation(
        &self,
    ) -> open_esp_radio_esp32s31_pac::SharedModemClockObservation {
        open_esp_radio_esp32s31_pac::WifiColdRegisters::shared_modem_clock_observation(self)
    }
}

impl SharedPowerClockControl for open_esp_radio_esp32s31_pac::Ieee802154TaskRegisters {
    fn set_wifi_baseband_and_mac_reset(&mut self, asserted: bool) {
        self.set_wifi_baseband_and_mac_reset(asserted);
    }
    fn set_wifi_baseband_reset(&mut self, asserted: bool) {
        self.set_wifi_baseband_reset(asserted);
    }
    fn configure_wifi_power_clock_map(&mut self) {
        self.configure_wifi_power_clock_map();
    }
    fn enable_phy_calibration_clocks(&mut self) {
        self.enable_phy_calibration_clocks();
    }
    fn select_phy_i2c_160mhz_source(&mut self) {
        self.select_phy_i2c_160mhz_source();
    }
    fn modem_syscon_power_observation(
        &self,
    ) -> open_esp_radio_esp32s31_pac::ModemSysconPowerObservation {
        self.modem_syscon_power_observation()
    }
    fn prepare_shared_modem_clock_map(&mut self) {
        self.prepare_shared_modem_clock_map();
    }

    fn retain_phy_i2c_master_clock(&mut self) {
        self.retain_phy_i2c_master_clock();
    }

    fn shared_modem_clock_observation(
        &self,
    ) -> open_esp_radio_esp32s31_pac::SharedModemClockObservation {
        self.shared_modem_clock_observation()
    }
}

/// Semantic read-back state captured after the cold clock/reset sequence.
///
/// Route-owned field decoding stays in the custom PAC; remaining platform
/// decoding stays behind the integration capability. Neither register handles,
/// addresses nor system-register masks cross into this layer.
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

pub(crate) fn execute_owned(
    platform: &mut impl PowerClockControl,
    registers: &mut impl SharedPowerClockControl,
) -> Result<(), PowerError> {
    // Keep the operation order here: it is a lifecycle property recovered
    // from the qualified S31 esp-hal clock implementation, not a
    // property of the register layout.
    registers.set_wifi_baseband_and_mac_reset(true);
    registers.set_wifi_baseband_and_mac_reset(false);
    platform.select_hp_active_modem_icg();
    platform.apply_modem_icg_selection();
    platform.apply_sleep_icg_selection();
    platform.enable_modem_register_bus_clock();
    registers.configure_wifi_power_clock_map();
    registers.prepare_shared_modem_clock_map();
    platform.configure_modem_source_clocks();
    registers.set_wifi_baseband_reset(true);
    registers.set_wifi_baseband_reset(false);
    registers.enable_phy_calibration_clocks();
    registers.select_phy_i2c_160mhz_source();
    registers.retain_phy_i2c_master_clock();

    let platform_images = platform.platform_power_clock_images();
    let modem = registers.modem_syscon_power_observation();
    let shared = registers.shared_modem_clock_observation();
    let images = PowerClockImages {
        reset_released: modem.wifi_reset_released,
        hp_active_icg_selected: platform_images.hp_active_icg_selected,
        modem_bus_clock_enabled: platform_images.modem_bus_clock_enabled,
        hp_active_clock_map_configured: modem.active_clock_map_configured,
        shared_clock_map_configured: shared.power_state_map_configured,
        modem_source_clocks_configured: platform_images.modem_source_clocks_configured,
        phy_calibration_clocks_enabled: modem.phy_calibration_clocks_enabled,
        phy_i2c_160mhz_selected: modem.phy_i2c_160mhz_selected,
        phy_i2c_master_clock_enabled: shared.phy_i2c_master_clock_enabled,
    };
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
    use std::{cell::RefCell, rc::Rc, vec::Vec};

    use open_esp_radio_esp32s31_pac::SharedModemClockObservation;

    use super::{
        PlatformPowerClockImages, PowerCheckpoint, PowerClockControl, PowerError,
        SharedPowerClockControl, execute_owned,
    };

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum Operation {
        ResetWifi(bool),
        SelectHpActiveIcg,
        ApplyModemIcg,
        ApplySleepIcg,
        EnableModemBus,
        ConfigureHpActiveMap,
        PrepareSharedMap,
        ConfigureModemSource,
        ResetBaseband(bool),
        EnablePhyClocks,
        SelectI2c160Mhz,
        RetainI2cClock,
    }

    struct FakePlatform {
        operations: Rc<RefCell<Vec<Operation>>>,
        images: PlatformPowerClockImages,
    }

    impl FakePlatform {
        fn ready(operations: Rc<RefCell<Vec<Operation>>>) -> Self {
            Self {
                operations,
                images: PlatformPowerClockImages {
                    hp_active_icg_selected: true,
                    modem_bus_clock_enabled: true,
                    modem_source_clocks_configured: true,
                },
            }
        }
    }

    impl PowerClockControl for FakePlatform {
        fn select_hp_active_modem_icg(&mut self) {
            self.operations
                .borrow_mut()
                .push(Operation::SelectHpActiveIcg);
        }

        fn apply_modem_icg_selection(&mut self) {
            self.operations.borrow_mut().push(Operation::ApplyModemIcg);
        }

        fn apply_sleep_icg_selection(&mut self) {
            self.operations.borrow_mut().push(Operation::ApplySleepIcg);
        }

        fn enable_modem_register_bus_clock(&mut self) {
            self.operations.borrow_mut().push(Operation::EnableModemBus);
        }

        fn configure_modem_source_clocks(&mut self) {
            self.operations
                .borrow_mut()
                .push(Operation::ConfigureModemSource);
        }

        fn platform_power_clock_images(&self) -> PlatformPowerClockImages {
            self.images
        }
    }

    struct FakeShared {
        operations: Rc<RefCell<Vec<Operation>>>,
        prepare_calls: u8,
        retain_calls: u8,
        modem: open_esp_radio_esp32s31_pac::ModemSysconPowerObservation,
        observation: SharedModemClockObservation,
    }

    impl FakeShared {
        fn ready(operations: Rc<RefCell<Vec<Operation>>>) -> Self {
            Self {
                operations,
                prepare_calls: 0,
                retain_calls: 0,
                modem: open_esp_radio_esp32s31_pac::ModemSysconPowerObservation {
                    wifi_reset_released: true,
                    active_clock_map_configured: true,
                    phy_calibration_clocks_enabled: true,
                    phy_i2c_160mhz_selected: true,
                },
                observation: SharedModemClockObservation {
                    power_state_map_configured: true,
                    coexistence_clock_enabled: false,
                    phy_i2c_master_clock_enabled: true,
                    low_power_timer_clock_enabled: false,
                },
            }
        }
    }

    impl SharedPowerClockControl for FakeShared {
        fn set_wifi_baseband_and_mac_reset(&mut self, asserted: bool) {
            self.operations
                .borrow_mut()
                .push(Operation::ResetWifi(asserted));
        }

        fn set_wifi_baseband_reset(&mut self, asserted: bool) {
            self.operations
                .borrow_mut()
                .push(Operation::ResetBaseband(asserted));
        }

        fn configure_wifi_power_clock_map(&mut self) {
            self.operations
                .borrow_mut()
                .push(Operation::ConfigureHpActiveMap);
        }

        fn enable_phy_calibration_clocks(&mut self) {
            self.operations
                .borrow_mut()
                .push(Operation::EnablePhyClocks);
        }

        fn select_phy_i2c_160mhz_source(&mut self) {
            self.operations
                .borrow_mut()
                .push(Operation::SelectI2c160Mhz);
        }

        fn modem_syscon_power_observation(
            &self,
        ) -> open_esp_radio_esp32s31_pac::ModemSysconPowerObservation {
            self.modem
        }

        fn prepare_shared_modem_clock_map(&mut self) {
            self.operations
                .borrow_mut()
                .push(Operation::PrepareSharedMap);
            self.prepare_calls += 1;
        }

        fn retain_phy_i2c_master_clock(&mut self) {
            self.operations.borrow_mut().push(Operation::RetainI2cClock);
            self.retain_calls += 1;
        }

        fn shared_modem_clock_observation(&self) -> SharedModemClockObservation {
            self.observation
        }
    }

    #[test]
    fn exact_semantic_sequence_is_finite_and_ordered() {
        let operations = Rc::new(RefCell::new(Vec::new()));
        let mut platform = FakePlatform::ready(operations.clone());
        let mut shared = FakeShared::ready(operations.clone());
        assert_eq!(execute_owned(&mut platform, &mut shared), Ok(()));
        assert_eq!(
            operations.borrow().as_slice(),
            [
                Operation::ResetWifi(true),
                Operation::ResetWifi(false),
                Operation::SelectHpActiveIcg,
                Operation::ApplyModemIcg,
                Operation::ApplySleepIcg,
                Operation::EnableModemBus,
                Operation::ConfigureHpActiveMap,
                Operation::PrepareSharedMap,
                Operation::ConfigureModemSource,
                Operation::ResetBaseband(true),
                Operation::ResetBaseband(false),
                Operation::EnablePhyClocks,
                Operation::SelectI2c160Mhz,
                Operation::RetainI2cClock,
            ]
        );
        assert_eq!((shared.prepare_calls, shared.retain_calls), (1, 1));
    }

    #[test]
    fn failed_semantic_readback_names_the_exact_checkpoint() {
        let operations = Rc::new(RefCell::new(Vec::new()));
        let mut platform = FakePlatform::ready(operations.clone());
        platform.images.modem_source_clocks_configured = false;
        let mut shared = FakeShared::ready(operations);

        assert_eq!(
            execute_owned(&mut platform, &mut shared),
            Err(PowerError {
                checkpoint: PowerCheckpoint::ModemClockSource,
                expected: true,
                observed: false,
            })
        );
    }
}
