//! Owned access to the ESP32-S31 PHY analog-register I2C master.

#[cfg(target_arch = "riscv32")]
use open_esp_radio_pac_esp32s31::{
    power::{phy_i2c_command_ram, phy_i2c_master},
    RadioRegisters, Register32,
};

/// One of the two command hosts selected by the S31 libphy block table.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyI2cHost {
    Host0,
    Host1,
}

/// A finite PHY-I2C operation could not be published or observed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyI2cError {
    Busy,
    CommandMemoryIndexOutOfRange,
}

#[cfg(target_arch = "riscv32")]
fn host_register(host: PhyI2cHost) -> Register32 {
    match host {
        PhyI2cHost::Host0 => phy_i2c_master::HOST_COMMAND_0,
        PhyI2cHost::Host1 => phy_i2c_master::HOST_COMMAND_1,
    }
}

#[cfg(target_arch = "riscv32")]
fn command_is_busy(command: u32) -> bool {
    phy_i2c_master::host_command_0::BUSY.extract(command) != 0
}

#[cfg(target_arch = "riscv32")]
fn encode_command(block: u8, register: u8, value: u8, write: bool) -> u32 {
    u32::from(block)
        | (u32::from(register) << 8)
        | (u32::from(value) << 16)
        | if write {
            phy_i2c_master::host_command_0::WRITE.mask()
        } else {
            0
        }
        | phy_i2c_master::host_command_0::START_OR_RESET.mask()
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
    let host_map = phy_i2c_master::host_config::HOST_MAP_UNKNOWN;
    registers.modify32(
        phy_i2c_master::HOST_CONFIG,
        host_map.mask(),
        host_map.checked_value(0x3fa0).unwrap_or(0),
    );

    let command_register = host_register(host);
    if command_is_busy(registers.read32(command_register)) {
        return Err(PhyI2cError::Busy);
    }
    registers.write32(phy_i2c_master::READ_MASK, !u32::from(read_mask));
    registers.write32(command_register, encode_command(block, register, 0, false));
    Ok(())
}

/// Observe one previously published PHY-I2C read exactly once.
///
/// Basis: the completion/result suffix of complete rev0 ROM
/// `phy_chip_i2c_readReg_org`. A set busy bit returns `Busy` to the external
/// async owner instead of spinning; otherwise bits 23:16 are the exact byte
/// result.
#[cfg(target_arch = "riscv32")]
pub fn try_finish_read(registers: &RadioRegisters, host: PhyI2cHost) -> Result<u8, PhyI2cError> {
    let command = registers.read32(host_register(host));
    if command_is_busy(command) {
        Err(PhyI2cError::Busy)
    } else {
        Ok(phy_i2c_master::host_command_0::DATA_OR_RESULT.extract(command) as u8)
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
    let host_map = phy_i2c_master::host_config::HOST_MAP_UNKNOWN;
    registers.modify32(
        phy_i2c_master::HOST_CONFIG,
        host_map.mask(),
        host_map.checked_value(0x3fa0).unwrap_or(0),
    );

    let command_register = host_register(host);
    if command_is_busy(registers.read32(command_register)) {
        return Err(PhyI2cError::Busy);
    }
    registers.write32(
        command_register,
        encode_command(block, register, value, true),
    );
    Ok(())
}

/// Observe one previously published PHY-I2C write exactly once.
///
/// Basis: the busy-test suffix of complete rev0 ROM
/// `phy_chip_i2c_writeReg`. This method deliberately performs no retry.
#[cfg(target_arch = "riscv32")]
pub fn try_finish_write(registers: &RadioRegisters, host: PhyI2cHost) -> Result<(), PhyI2cError> {
    if command_is_busy(registers.read32(host_register(host))) {
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
    let high = phy_i2c_master::clock_selection_0::SELECTION_HIGH_UNKNOWN;
    let low = phy_i2c_master::clock_selection_0::SELECTION_LOW_UNKNOWN;
    let high_value = high.extract(selection << 4);
    let low_value = (selection >> 1) & low.max_value();
    for register in [
        phy_i2c_master::CLOCK_SELECTION_0,
        phy_i2c_master::CLOCK_SELECTION_1,
        phy_i2c_master::CLOCK_SELECTION_2,
    ] {
        registers.modify32(
            register,
            high.mask(),
            high.checked_value(high_value).unwrap_or(0),
        );
        registers.modify32(
            register,
            low.mask(),
            low.checked_value(low_value).unwrap_or(0),
        );
    }
}

/// Configure the PHY-I2C master register mode and enable bit.
///
/// Basis: complete rev0 ROM `phy_i2cmst_reg_init` at `0x2f8276c4`, size
/// `0x22`. It writes `MASTER_CONTROL.REGISTER_MODE = 2`, then sets
/// `REGISTER_ENABLE`, using a fresh read for each update.
#[cfg(target_arch = "riscv32")]
pub fn configure_master_registers(registers: &mut RadioRegisters) {
    let mode = phy_i2c_master::master_control::REGISTER_MODE;
    registers.modify32(
        phy_i2c_master::MASTER_CONTROL,
        mode.mask(),
        mode.checked_value(2).unwrap_or(0),
    );
    let enable = phy_i2c_master::master_control::REGISTER_ENABLE.mask();
    registers.modify32(phy_i2c_master::MASTER_CONTROL, enable, enable);
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
    let register = phy_i2c_command_ram::COMMAND_MEMORY
        .get(index)
        .copied()
        .ok_or(PhyI2cError::CommandMemoryIndexOutOfRange)?;
    registers.write32(register, command);
    Ok(())
}
