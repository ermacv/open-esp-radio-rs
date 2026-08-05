//! Owned access to the ESP32-S31 PHY analog-register I2C master.

#[cfg(target_arch = "riscv32")]
use open_esp_radio_esp32s31_registers::RadioRegisters;

/// One of the two analog-register command hosts.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyI2cHost {
    Host0,
    Host1,
}

/// Official `I2C_ANA_MST` capability needed by the open PHY.
///
/// The integration layer owns the chip-level singleton. The recovered radio
/// PAC deliberately does not duplicate this peripheral.
pub trait PhyI2cMasterControl {
    fn configure_phy_i2c_host_map(&mut self);
    fn pulse_phy_i2c_master_reset(&mut self, host: PhyI2cHost);
    fn phy_i2c_master_is_busy(&self, host: PhyI2cHost) -> bool;
    fn publish_phy_i2c_read_mask(&mut self, read_mask: u16);
    fn publish_phy_i2c_command(
        &mut self,
        host: PhyI2cHost,
        block: u8,
        register: u8,
        value: u8,
        write: bool,
    );
    fn sample_phy_i2c_result(&self, host: PhyI2cHost) -> u8;
    fn set_phy_i2c_clock_selection_high(&mut self, index: usize, value: u8) -> bool;
    fn set_phy_i2c_clock_selection_low(&mut self, index: usize, value: u8) -> bool;
    fn set_phy_i2c_register_mode(&mut self, mode: u8) -> bool;
    fn enable_phy_i2c_register_mode(&mut self);
    fn set_phy_i2c_bbpll_calibration(&mut self, enabled: bool);
}

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
    command & (1 << 25) != 0
}

/// Publish one full-word PHY-I2C master reset command.
///
/// The complete ROM parent writes only bit 26 to a busy host and then polls
/// bit 25. This finite method publishes that one edge; retry and timeout
/// ownership remain in the Rust transition.
pub fn pulse_master_reset(platform: &mut impl PhyI2cMasterControl, host: PhyI2cHost) {
    platform.pulse_phy_i2c_master_reset(host);
}

/// Sample one PHY-I2C master reset busy edge without retrying.
pub fn sample_master_reset_busy(platform: &impl PhyI2cMasterControl, host: PhyI2cHost) -> bool {
    platform.phy_i2c_master_is_busy(host)
}

/// Publish one PHY-I2C read after a single fail-fast busy observation.
///
/// Basis: complete rev0 ROM `phy_chip_i2c_readReg_org` at `0x2f829ffa`,
/// combined with the complete S31 `libphy.a[phy_i2c.o]` host-config and
/// read-mask callbacks. Rust additionally rejects a busy host before
/// publication; it never reproduces the ROM polling loop.
pub fn try_start_read(
    platform: &mut impl PhyI2cMasterControl,
    host: PhyI2cHost,
    block: u8,
    register: u8,
    read_mask: u16,
) -> Result<(), PhyI2cError> {
    platform.configure_phy_i2c_host_map();
    if platform.phy_i2c_master_is_busy(host) {
        return Err(PhyI2cError::Busy);
    }
    platform.publish_phy_i2c_read_mask(read_mask);
    platform.publish_phy_i2c_command(host, block, register, 0, false);
    Ok(())
}

/// Observe one previously published PHY-I2C read exactly once.
///
/// Basis: the completion/result suffix of complete rev0 ROM
/// `phy_chip_i2c_readReg_org`. A set busy bit returns `Busy` to the external
/// async owner instead of spinning; otherwise bits 23:16 are the exact byte
/// result.
pub fn try_finish_read(
    platform: &impl PhyI2cMasterControl,
    host: PhyI2cHost,
) -> Result<u8, PhyI2cError> {
    if platform.phy_i2c_master_is_busy(host) {
        Err(PhyI2cError::Busy)
    } else {
        Ok(platform.sample_phy_i2c_result(host))
    }
}

/// Publish one PHY-I2C write after a single fail-fast busy observation.
///
/// Basis: complete rev0 ROM `phy_chip_i2c_writeReg` at `0x2f82a30e` and the
/// complete S31 libphy host-config callback. Command bytes and bits 24/26 are
/// instruction-exact; completion remains externally driven.
pub fn try_start_write(
    platform: &mut impl PhyI2cMasterControl,
    host: PhyI2cHost,
    block: u8,
    register: u8,
    value: u8,
) -> Result<(), PhyI2cError> {
    platform.configure_phy_i2c_host_map();
    if platform.phy_i2c_master_is_busy(host) {
        return Err(PhyI2cError::Busy);
    }
    platform.publish_phy_i2c_command(host, block, register, value, true);
    Ok(())
}

/// Observe one previously published PHY-I2C write exactly once.
///
/// Basis: the busy-test suffix of complete rev0 ROM
/// `phy_chip_i2c_writeReg`. This method deliberately performs no retry.
pub fn try_finish_write(
    platform: &impl PhyI2cMasterControl,
    host: PhyI2cHost,
) -> Result<(), PhyI2cError> {
    if platform.phy_i2c_master_is_busy(host) {
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
pub fn configure_clock_selection(platform: &mut impl PhyI2cMasterControl, selection: u32) {
    let high_value = ((selection >> 2) & 0x1f) as u8;
    let low_value = ((selection >> 1) & 0x3f) as u8;
    for index in 0..3 {
        let high_written = platform.set_phy_i2c_clock_selection_high(index, high_value);
        let low_written = platform.set_phy_i2c_clock_selection_low(index, low_value);
        debug_assert!(high_written && low_written);
    }
}

/// Configure the PHY-I2C master register mode and enable bit.
///
/// Basis: complete rev0 ROM `phy_i2cmst_reg_init` at `0x2f8276c4`, size
/// `0x22`. It writes `MASTER_CONTROL.REGISTER_MODE = 2`, then sets
/// `REGISTER_ENABLE`, using a fresh read for each update.
pub fn configure_master_registers(platform: &mut impl PhyI2cMasterControl) {
    let mode_written = platform.set_phy_i2c_register_mode(2);
    // The write is part of the production sequence. Keeping it inside
    // `debug_assert!` would erase it in release builds. SOURCE: rev0 ROM
    // `phy_i2cmst_reg_init` at 0x2f82_76c4.
    debug_assert!(mode_written);
    platform.enable_phy_i2c_register_mode();
}

#[cfg(test)]
fn bbpll_calibration_bits(enabled: bool) -> u32 {
    (if enabled { 2 } else { 1 }) << 2
}

/// Select the complete rev0 ROM `phy_bbpll_cal` mode.
///
/// The body at `0x2f82_7dbc`, size `0x1c`, performs one fresh-read
/// replacement of `MASTER_CONTROL` bits 3:2. Zero selects encoded mode one;
/// every nonzero input selects encoded mode two. The boolean API makes that
/// two-state contract explicit while preserving all unrelated shared fields.
pub fn configure_bbpll_calibration(platform: &mut impl PhyI2cMasterControl, enabled: bool) {
    platform.set_phy_i2c_bbpll_calibration(enabled);
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
