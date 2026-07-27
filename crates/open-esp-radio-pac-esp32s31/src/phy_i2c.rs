//! Ownership-bound access to the recovered PHY analog-register I²C master.
//!
//! Layout and fields come from `svd/esp32s31-radio.svd`. Command encodings and
//! update order are evidenced by the complete rev0 ROM PHY-I²C bodies and the
//! pinned S31 `libphy.a[phy_i2c.o]` callbacks named on those SVD registers.

use super::RadioRegisters;

/// One of the two command hosts selected by the S31 libphy block table.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyI2cHost {
    Host0,
    Host1,
}

/// Test the instruction-recovered busy bit in a captured host-command image.
pub const fn phy_i2c_master_busy_in_image(command: u32) -> bool {
    command & (1 << 25) != 0
}

/// Build the two-bit BBPLL calibration field image selected by the ROM leaf.
pub const fn phy_i2c_bbpll_calibration_image(enabled: bool) -> u32 {
    (if enabled { 2 } else { 1 }) << 2
}

impl RadioRegisters {
    /// Replace the instruction-recovered PHY-I²C host map with `0x3fa0`.
    pub fn configure_phy_i2c_host_map(&mut self) {
        // SAFETY: 0x3fa0 fits the recovered 14-bit field and is the exact
        // value published by the complete S31 libphy host callback.
        unsafe {
            self.peripherals
                .phy_i2c_master
                .host_config()
                .modify(|_, w| w.host_map_unknown().bits(0x3fa0));
        }
    }

    /// Publish the one-bit reset command to a selected host.
    pub fn pulse_phy_i2c_master_reset(&mut self, host: PhyI2cHost) {
        // SAFETY: this publishes only the instruction-evidenced one-bit reset
        // command to a selected, uniquely owned host register.
        unsafe {
            match host {
                PhyI2cHost::Host0 => self
                    .peripherals
                    .phy_i2c_master
                    .host_command_0()
                    .write_with_zero(|w| w.start_or_reset().set_bit()),
                PhyI2cHost::Host1 => self
                    .peripherals
                    .phy_i2c_master
                    .host_command_1()
                    .write_with_zero(|w| w.start_or_reset().set_bit()),
            };
        }
    }

    /// Sample the selected host's hardware busy bit exactly once.
    pub fn phy_i2c_master_is_busy(&mut self, host: PhyI2cHost) -> bool {
        match host {
            PhyI2cHost::Host0 => self
                .peripherals
                .phy_i2c_master
                .host_command_0()
                .read()
                .busy()
                .bit_is_set(),
            PhyI2cHost::Host1 => self
                .peripherals
                .phy_i2c_master
                .host_command_1()
                .read()
                .busy()
                .bit_is_set(),
        }
    }

    /// Publish the exact full-word complement used by the read-mask callback.
    pub fn publish_phy_i2c_read_mask(&mut self, read_mask: u16) {
        // SAFETY: the complete S31 callback publishes the full 32-bit
        // complement, including ones above the recovered low 16-bit field.
        // Keeping the exact word avoids changing blob-observed behavior while
        // those upper bits remain undocumented.
        unsafe {
            self.peripherals
                .phy_i2c_master
                .read_mask()
                .write_with_zero(|w| w.bits(!u32::from(read_mask)));
        }
    }

    /// Publish one complete read or write command to the selected host.
    pub fn publish_phy_i2c_command(
        &mut self,
        host: PhyI2cHost,
        block: u8,
        register: u8,
        value: u8,
        write: bool,
    ) {
        match host {
            PhyI2cHost::Host0 => {
                // SAFETY: the byte arguments exactly fit their fields.
                unsafe {
                    self.peripherals
                        .phy_i2c_master
                        .host_command_0()
                        .write_with_zero(|w| {
                            w.block()
                                .bits(block)
                                .register()
                                .bits(register)
                                .data_or_result()
                                .bits(value)
                                .write()
                                .bit(write)
                                .start_or_reset()
                                .set_bit()
                        });
                }
            }
            PhyI2cHost::Host1 => {
                // SAFETY: the byte arguments exactly fit their fields.
                unsafe {
                    self.peripherals
                        .phy_i2c_master
                        .host_command_1()
                        .write_with_zero(|w| {
                            w.block()
                                .bits(block)
                                .register()
                                .bits(register)
                                .data_or_result()
                                .bits(value)
                                .write()
                                .bit(write)
                                .start_or_reset()
                                .set_bit()
                        });
                }
            }
        }
    }

    /// Sample the selected host's result byte exactly once.
    pub fn sample_phy_i2c_result(&mut self, host: PhyI2cHost) -> u8 {
        match host {
            PhyI2cHost::Host0 => self
                .peripherals
                .phy_i2c_master
                .host_command_0()
                .read()
                .data_or_result()
                .bits(),
            PhyI2cHost::Host1 => self
                .peripherals
                .phy_i2c_master
                .host_command_1()
                .read()
                .data_or_result()
                .bits(),
        }
    }

    /// Replace the high selection field of one of three clock words.
    pub fn set_phy_i2c_clock_selection_high(&mut self, index: usize, value: u8) -> bool {
        if value > 0x1f {
            return false;
        }
        // SAFETY: the range check proves the value fits the five-bit fields.
        unsafe {
            match index {
                0 => self
                    .peripherals
                    .phy_i2c_master
                    .clock_selection_0()
                    .modify(|_, w| w.selection_high_unknown().bits(value)),
                1 => self
                    .peripherals
                    .phy_i2c_master
                    .clock_selection_1()
                    .modify(|_, w| w.selection_high_unknown().bits(value)),
                2 => self
                    .peripherals
                    .phy_i2c_master
                    .clock_selection_2()
                    .modify(|_, w| w.selection_high_unknown().bits(value)),
                _ => return false,
            };
        }
        true
    }

    /// Replace the low selection field of one of three clock words.
    pub fn set_phy_i2c_clock_selection_low(&mut self, index: usize, value: u8) -> bool {
        if value > 0x3f {
            return false;
        }
        // SAFETY: the range check proves the value fits the six-bit fields.
        unsafe {
            match index {
                0 => self
                    .peripherals
                    .phy_i2c_master
                    .clock_selection_0()
                    .modify(|_, w| w.selection_low_unknown().bits(value)),
                1 => self
                    .peripherals
                    .phy_i2c_master
                    .clock_selection_1()
                    .modify(|_, w| w.selection_low_unknown().bits(value)),
                2 => self
                    .peripherals
                    .phy_i2c_master
                    .clock_selection_2()
                    .modify(|_, w| w.selection_low_unknown().bits(value)),
                _ => return false,
            };
        }
        true
    }

    /// Replace the recovered two-bit PHY-I²C register mode.
    pub fn set_phy_i2c_register_mode(&mut self, mode: u8) -> bool {
        if mode > 3 {
            return false;
        }
        // SAFETY: the range check proves `mode` fits the two-bit field.
        unsafe {
            self.peripherals
                .phy_i2c_master
                .master_control()
                .modify(|_, w| w.register_mode().bits(mode));
        }
        true
    }

    /// Enable the PHY-I²C register mode in a separate fresh RMW edge.
    pub fn enable_phy_i2c_register_mode(&mut self) {
        self.peripherals
            .phy_i2c_master
            .master_control()
            .modify(|_, w| w.register_enable().set_bit());
    }

    /// Select one of the two instruction-evidenced BBPLL calibration modes.
    pub fn set_phy_i2c_bbpll_calibration(&mut self, enabled: bool) {
        let mode = if enabled { 2 } else { 1 };
        // SAFETY: both encodings fit the recovered two-bit field and are the
        // only values selected by complete rev0 ROM `phy_bbpll_cal`.
        unsafe {
            self.peripherals
                .phy_i2c_master
                .master_control()
                .modify(|_, w| w.bbpll_cal_mode_unknown().bits(mode));
        }
    }

    /// Publish one recovered command-RAM word.
    ///
    /// Returns false for an invalid index or for bits outside the three
    /// recovered byte fields.
    pub fn write_phy_i2c_command_memory(&mut self, index: usize, command: u32) -> bool {
        if index >= 45 || command & 0xff00_0000 != 0 {
            return false;
        }
        let bytes = command.to_le_bytes();
        // SAFETY: each input is one byte and therefore exactly fits its field.
        unsafe {
            self.peripherals
                .phy_i2c_command_ram
                .command_memory(index)
                .write_with_zero(|w| {
                    w.block()
                        .bits(bytes[0])
                        .register()
                        .bits(bytes[1])
                        .data()
                        .bits(bytes[2])
                });
        }
        true
    }
}
