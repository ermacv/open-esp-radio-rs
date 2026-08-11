//! Ownership-bound access to recovered PHY analog-I²C command RAM.
//!
//! The chip-level `I2C_ANA_MST` register block is owned by the platform PAC.
//! Only the undocumented command-memory window remains in this radio PAC.

#![forbid(unsafe_code)]

use super::RadioRegisters;

impl RadioRegisters {
    /// Publish one recovered command-RAM word.
    ///
    /// Returns false for an invalid index or for bits outside the three
    /// recovered byte fields.
    pub fn write_phy_i2c_command_memory(&mut self, index: usize, command: u32) -> bool {
        if index >= 45 || command & 0xff00_0000 != 0 {
            return false;
        }
        let bytes = command.to_le_bytes();
        open_esp_radio_esp32s31_pac_raw::zero_based_field_write::phy_i2c_command_memory(
            &self.peripherals.phy_i2c_command_ram,
            index,
            bytes[0],
            bytes[1],
            bytes[2],
        );
        true
    }
}
