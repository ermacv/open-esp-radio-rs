//! ESP-HAL clock sources that the radio arbiter's modem clocks depend on.
//!
//! ESP-HAL owns the SoC clock tree and keeps its own reference counts: the
//! 160 MHz PLL output through its clock-tree nodes, and the analog-I2C master
//! clock shared with its regi2c accesses. The radio arbiter reaches both only
//! through [`PlatformClockProvider`].

use esp_hal::clock::ll::{
    ClockTree, acquire_analog_i2c_master_clock, release_analog_i2c_master_clock, release_pll_f160m,
    request_pll_f160m,
};
use oer_esp32s31_hal::power::{PlatformClockError, PlatformClockProvider};

/// Platform clock provider backed by ESP-HAL's reference-counted clocks.
#[derive(Debug, Default)]
pub struct EspHalRadioClocks {
    _private: (),
}

impl EspHalRadioClocks {
    pub const fn new() -> Self {
        Self { _private: () }
    }
}

impl PlatformClockProvider for EspHalRadioClocks {
    fn acquire_pll_f160m(&mut self) -> Result<(), PlatformClockError> {
        ClockTree::with(request_pll_f160m);
        Ok(())
    }

    fn release_pll_f160m(&mut self) -> Result<(), PlatformClockError> {
        ClockTree::with(release_pll_f160m);
        Ok(())
    }

    fn acquire_analog_i2c_clock(&mut self) -> Result<(), PlatformClockError> {
        acquire_analog_i2c_master_clock();
        Ok(())
    }

    fn release_analog_i2c_clock(&mut self) -> Result<(), PlatformClockError> {
        release_analog_i2c_master_clock();
        Ok(())
    }
}
