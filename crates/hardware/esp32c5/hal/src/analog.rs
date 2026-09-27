//! The ESP32-C5 analog register bus of the PHY.
//!
//! [`AnalogI2c`] implements the chip-neutral
//! [`oer_radio_analog::AnalogRegisterBus`] over the PAC's analog I2C master
//! owner, whose addresses carry the libphy host map, host set, read masks and
//! block alias.

use oer_esp32c5_pac::{PhyI2cAccessError, PhyI2cAddress, PhyI2cRegisters};
use oer_radio_analog::{AnalogRegisterBus, Busy};

pub use oer_esp32c5_pac::{PhyI2cBlock, PhyI2cHost};

/// Unique owner of the ESP32-C5 analog register bus.
#[must_use = "dropping the analog register bus loses the analog I2C master"]
pub struct AnalogI2c {
    registers: PhyI2cRegisters,
}

impl AnalogI2c {
    pub const fn new(registers: PhyI2cRegisters) -> Self {
        Self { registers }
    }

    pub fn into_registers(self) -> PhyI2cRegisters {
        self.registers
    }
}

const fn busy(_: PhyI2cAccessError) -> Busy {
    Busy
}

impl AnalogRegisterBus for AnalogI2c {
    type Address = PhyI2cAddress;

    fn try_start_read(&mut self, address: PhyI2cAddress) -> Result<(), Busy> {
        self.registers.try_start_read(address).map_err(busy)
    }

    fn try_finish_read(&self, address: PhyI2cAddress) -> Result<u8, Busy> {
        self.registers.try_finish_read(address).map_err(busy)
    }

    fn try_start_write(&mut self, address: PhyI2cAddress, value: u8) -> Result<(), Busy> {
        self.registers.try_start_write(address, value).map_err(busy)
    }

    fn try_finish_write(&self, address: PhyI2cAddress) -> Result<(), Busy> {
        self.registers.try_finish_write(address).map_err(busy)
    }
}
