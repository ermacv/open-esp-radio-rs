//! Finite ESP32-S31 modem/PHY prerequisite sequence.
//!
//! The order is the merged cold-boot path immediately preceding
//! `register_chipv7_phy` in the ESP32-S31 `esp-radio` and `esp-phy` oracle.
//! Wi-Fi MAC clocks are intentionally excluded: they belong to the later MAC
//! start transition.

pub(crate) trait PowerSequenceBackend {
    fn select_hp_active_modem_icg(&mut self);
    fn apply_modem_icg_selection(&mut self);
    fn apply_sleep_icg_selection(&mut self);
    fn enable_modem_register_bus_clock(&mut self);
    fn configure_modem_source_clocks(&mut self);
    fn platform_clock_power_observation(
        &self,
    ) -> open_esp_radio_esp32s31_pac::PlatformClockPowerObservation;
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

impl PowerSequenceBackend for open_esp_radio_esp32s31_pac::WifiColdRegisters {
    fn select_hp_active_modem_icg(&mut self) {
        self.select_hp_active_modem_icg();
    }
    fn apply_modem_icg_selection(&mut self) {
        self.apply_modem_icg_selection();
    }
    fn apply_sleep_icg_selection(&mut self) {
        self.apply_sleep_icg_selection();
    }
    fn enable_modem_register_bus_clock(&mut self) {
        self.enable_modem_register_bus_clock();
    }
    fn configure_modem_source_clocks(&mut self) {
        self.configure_modem_source_clocks();
    }
    fn platform_clock_power_observation(
        &self,
    ) -> open_esp_radio_esp32s31_pac::PlatformClockPowerObservation {
        self.platform_clock_power_observation()
    }
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

impl PowerSequenceBackend for open_esp_radio_esp32s31_pac::Ieee802154TaskRegisters {
    fn select_hp_active_modem_icg(&mut self) {
        self.select_hp_active_modem_icg();
    }
    fn apply_modem_icg_selection(&mut self) {
        self.apply_modem_icg_selection();
    }
    fn apply_sleep_icg_selection(&mut self) {
        self.apply_sleep_icg_selection();
    }
    fn enable_modem_register_bus_clock(&mut self) {
        self.enable_modem_register_bus_clock();
    }
    fn configure_modem_source_clocks(&mut self) {
        self.configure_modem_source_clocks();
    }
    fn platform_clock_power_observation(
        &self,
    ) -> open_esp_radio_esp32s31_pac::PlatformClockPowerObservation {
        self.platform_clock_power_observation()
    }
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
pub struct PowerClockReadback {
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

pub(crate) fn execute_owned(registers: &mut impl PowerSequenceBackend) -> Result<(), PowerError> {
    // Keep the operation order here: it is a lifecycle property recovered
    // from the qualified S31 esp-hal clock implementation, not a
    // property of the register layout.
    registers.set_wifi_baseband_and_mac_reset(true);
    registers.set_wifi_baseband_and_mac_reset(false);
    registers.select_hp_active_modem_icg();
    registers.apply_modem_icg_selection();
    registers.apply_sleep_icg_selection();
    registers.enable_modem_register_bus_clock();
    registers.configure_wifi_power_clock_map();
    registers.prepare_shared_modem_clock_map();
    registers.configure_modem_source_clocks();
    registers.set_wifi_baseband_reset(true);
    registers.set_wifi_baseband_reset(false);
    registers.enable_phy_calibration_clocks();
    registers.select_phy_i2c_160mhz_source();
    registers.retain_phy_i2c_master_clock();

    let platform = registers.platform_clock_power_observation();
    let modem = registers.modem_syscon_power_observation();
    let shared = registers.shared_modem_clock_observation();
    let readback = PowerClockReadback {
        reset_released: modem.wifi_reset_released,
        hp_active_icg_selected: platform.hp_active_icg_selected,
        modem_bus_clock_enabled: platform.modem_register_bus_clock_enabled,
        hp_active_clock_map_configured: modem.active_clock_map_configured,
        shared_clock_map_configured: shared.power_state_map_configured,
        modem_source_clocks_configured: platform.modem_source_clocks_configured,
        phy_calibration_clocks_enabled: modem.phy_calibration_clocks_enabled,
        phy_i2c_160mhz_selected: modem.phy_i2c_160mhz_selected,
        phy_i2c_master_clock_enabled: shared.phy_i2c_master_clock_enabled,
    };
    verify_state(PowerCheckpoint::ResetReleased, readback.reset_released)?;
    verify_state(
        PowerCheckpoint::HpActiveIcg,
        readback.hp_active_icg_selected,
    )?;
    verify_state(
        PowerCheckpoint::ModemBusClock,
        readback.modem_bus_clock_enabled,
    )?;
    verify_state(
        PowerCheckpoint::HpActiveClockMap,
        readback.hp_active_clock_map_configured,
    )?;
    verify_state(
        PowerCheckpoint::SharedClockMap,
        readback.shared_clock_map_configured,
    )?;
    verify_state(
        PowerCheckpoint::ModemClockSource,
        readback.modem_source_clocks_configured,
    )?;
    verify_state(
        PowerCheckpoint::PhyClocks,
        readback.phy_calibration_clocks_enabled,
    )?;
    verify_state(PowerCheckpoint::I2cSource, readback.phy_i2c_160mhz_selected)?;
    verify_state(
        PowerCheckpoint::I2cClock,
        readback.phy_i2c_master_clock_enabled,
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
mod tests;

pub(crate) mod clock;
