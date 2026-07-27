//! Owned access to the ESP32-S31 PHY analog-register I2C master.

#[cfg(test)]
use open_esp_radio_pac_esp32s31::phy_i2c::phy_i2c_bbpll_calibration_image;
use open_esp_radio_pac_esp32s31::phy_i2c::phy_i2c_master_busy_in_image;
pub use open_esp_radio_pac_esp32s31::phy_i2c::PhyI2cHost;
#[cfg(target_arch = "riscv32")]
use open_esp_radio_pac_esp32s31::RadioRegisters;

/// A finite PHY-I2C operation could not be published or observed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyI2cError {
    Busy,
    CommandMemoryIndexOutOfRange,
    CommandMemoryWordOutOfRange,
}

/// Test the exact busy bit shared by both PHY-I2C master hosts.
///
/// Complete rev0 ROM `phy_i2c_master_reset` at `0x2f8260d0`, size `0x74`,
/// samples bit 25 of both host words. Keeping this transform in the safe HAL
/// lets the outer state machine carry only a semantic `busy` observation.
pub fn master_reset_busy(command: u32) -> bool {
    phy_i2c_master_busy_in_image(command)
}

/// Publish one full-word PHY-I2C master reset command.
///
/// The complete ROM parent writes only bit 26 to a busy host and then polls
/// bit 25. This finite method publishes that one edge; retry and timeout
/// ownership remain in the Rust transition.
#[cfg(target_arch = "riscv32")]
pub fn pulse_master_reset(registers: &mut RadioRegisters, host: PhyI2cHost) {
    registers.pulse_phy_i2c_master_reset(host);
}

/// Sample one PHY-I2C master reset busy edge without retrying.
#[cfg(target_arch = "riscv32")]
pub fn sample_master_reset_busy(registers: &mut RadioRegisters, host: PhyI2cHost) -> bool {
    registers.phy_i2c_master_is_busy(host)
}

/// Publish one PHY-I2C read after a single fail-fast busy observation.
///
/// Basis: complete rev0 ROM `phy_chip_i2c_readReg_org` at `0x2f829ffa`,
/// combined with the complete S31 `libphy.a[phy_i2c.o]` host-config and
/// read-mask callbacks. Rust additionally rejects a busy host before
/// publication; it never reproduces the ROM polling loop.
#[cfg(target_arch = "riscv32")]
pub fn try_start_read(
    registers: &mut RadioRegisters,
    host: PhyI2cHost,
    block: u8,
    register: u8,
    read_mask: u16,
) -> Result<(), PhyI2cError> {
    registers.configure_phy_i2c_host_map();
    if registers.phy_i2c_master_is_busy(host) {
        return Err(PhyI2cError::Busy);
    }
    registers.publish_phy_i2c_read_mask(read_mask);
    registers.publish_phy_i2c_command(host, block, register, 0, false);
    Ok(())
}

/// Observe one previously published PHY-I2C read exactly once.
///
/// Basis: the completion/result suffix of complete rev0 ROM
/// `phy_chip_i2c_readReg_org`. A set busy bit returns `Busy` to the external
/// async owner instead of spinning; otherwise bits 23:16 are the exact byte
/// result.
#[cfg(target_arch = "riscv32")]
pub fn try_finish_read(
    registers: &mut RadioRegisters,
    host: PhyI2cHost,
) -> Result<u8, PhyI2cError> {
    if registers.phy_i2c_master_is_busy(host) {
        Err(PhyI2cError::Busy)
    } else {
        Ok(registers.sample_phy_i2c_result(host))
    }
}

/// Publish one PHY-I2C write after a single fail-fast busy observation.
///
/// Basis: complete rev0 ROM `phy_chip_i2c_writeReg` at `0x2f82a30e` and the
/// complete S31 libphy host-config callback. Command bytes and bits 24/26 are
/// instruction-exact; completion remains externally driven.
#[cfg(target_arch = "riscv32")]
pub fn try_start_write(
    registers: &mut RadioRegisters,
    host: PhyI2cHost,
    block: u8,
    register: u8,
    value: u8,
) -> Result<(), PhyI2cError> {
    registers.configure_phy_i2c_host_map();
    if registers.phy_i2c_master_is_busy(host) {
        return Err(PhyI2cError::Busy);
    }
    registers.publish_phy_i2c_command(host, block, register, value, true);
    Ok(())
}

/// Observe one previously published PHY-I2C write exactly once.
///
/// Basis: the busy-test suffix of complete rev0 ROM
/// `phy_chip_i2c_writeReg`. This method deliberately performs no retry.
#[cfg(target_arch = "riscv32")]
pub fn try_finish_write(
    registers: &mut RadioRegisters,
    host: PhyI2cHost,
) -> Result<(), PhyI2cError> {
    if registers.phy_i2c_master_is_busy(host) {
        Err(PhyI2cError::Busy)
    } else {
        Ok(())
    }
}

/// Apply all six writes of the recovered PHY-I2C clock selection.
///
/// Basis: complete rev0 ROM `phy_i2c_clk_sel` at `0x2f829f1c`, size `0x68`.
/// Each of three registers receives a high-field update followed by a fresh
/// read and low-field update, preserving all instruction-evidenced
/// intermediate states.
#[cfg(target_arch = "riscv32")]
pub fn configure_clock_selection(registers: &mut RadioRegisters, selection: u32) {
    let high_value = ((selection >> 2) & 0x1f) as u8;
    let low_value = ((selection >> 1) & 0x3f) as u8;
    for index in 0..3 {
        let high_written = registers.set_phy_i2c_clock_selection_high(index, high_value);
        let low_written = registers.set_phy_i2c_clock_selection_low(index, low_value);
        debug_assert!(high_written && low_written);
    }
}

/// Configure the PHY-I2C master register mode and enable bit.
///
/// Basis: complete rev0 ROM `phy_i2cmst_reg_init` at `0x2f8276c4`, size
/// `0x22`. It writes `MASTER_CONTROL.REGISTER_MODE = 2`, then sets
/// `REGISTER_ENABLE`, using a fresh read for each update.
#[cfg(target_arch = "riscv32")]
pub fn configure_master_registers(registers: &mut RadioRegisters) {
    debug_assert!(registers.set_phy_i2c_register_mode(2));
    registers.enable_phy_i2c_register_mode();
}

#[cfg(test)]
fn bbpll_calibration_bits(enabled: bool) -> u32 {
    phy_i2c_bbpll_calibration_image(enabled)
}

/// Select the complete rev0 ROM `phy_bbpll_cal` mode.
///
/// The body at `0x2f82_7dbc`, size `0x1c`, performs one fresh-read
/// replacement of `MASTER_CONTROL` bits 3:2. Zero selects encoded mode one;
/// every nonzero input selects encoded mode two. The boolean API makes that
/// two-state contract explicit while preserving all unrelated shared fields.
#[cfg(target_arch = "riscv32")]
pub fn configure_bbpll_calibration(registers: &mut RadioRegisters, enabled: bool) {
    registers.set_phy_i2c_bbpll_calibration(enabled);
}

/// Write one of the 45 recovered PHY-I2C command-RAM words.
///
/// Basis: complete S31
/// `libphy.a[phy_i2c.o]::phy_i2c_master_cmd_mem_init`. The SVD `dim=45`
/// array localizes every valid destination; the caller remains responsible
/// for the instruction-recovered block/register/data word.
#[cfg(target_arch = "riscv32")]
pub fn write_command_memory(
    registers: &mut RadioRegisters,
    index: usize,
    command: u32,
) -> Result<(), PhyI2cError> {
    if index >= 45 {
        return Err(PhyI2cError::CommandMemoryIndexOutOfRange);
    }
    registers
        .write_phy_i2c_command_memory(index, command)
        .then_some(())
        .ok_or(PhyI2cError::CommandMemoryWordOutOfRange)
}

#[cfg(test)]
mod tests {
    use super::{bbpll_calibration_bits, master_reset_busy};

    #[test]
    fn bbpll_calibration_retains_both_complete_rom_encodings() {
        assert_eq!(bbpll_calibration_bits(false), 0x04);
        assert_eq!(bbpll_calibration_bits(true), 0x08);
    }

    #[test]
    fn master_reset_busy_uses_only_the_pac_busy_field() {
        assert!(!master_reset_busy(0xfdff_ffff));
        assert!(master_reset_busy(0x0200_0000));
        assert!(master_reset_busy(u32::MAX));
    }
}
