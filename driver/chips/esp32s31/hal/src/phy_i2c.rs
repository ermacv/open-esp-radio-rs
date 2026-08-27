//! Owned access to the ESP32-S31 PHY analog-register I2C master.

use crate::{SharedPhyAccess, phy_pac, phy_pac_mut};
pub use open_esp_radio_esp32s31_pac::{
    BluetoothTxPowerControlAction, BluetoothTxPowerControlCompletion, BluetoothTxPowerControlError,
    BluetoothTxPowerControlObservation, BluetoothTxPowerControlOperation,
    BluetoothTxPowerControlTransaction, PhyFilterDcapInputs, PhyI2cCommandMemoryInputs,
    PhyI2cConfigurationAction, PhyI2cConfigurationError, PhyI2cConfigurationObservation,
    PhyI2cConfigurationOperation, PhyI2cConfigurationTransaction, PhyI2cHost,
    PhyI2cInitializationStageOneInputs,
};

/// Start the current command of one PAC-owned PHY-I²C configuration.
pub fn start_configuration(
    transaction: &mut PhyI2cConfigurationTransaction,
    registers: &mut impl SharedPhyAccess,
) -> Result<(), PhyI2cConfigurationError> {
    transaction.start(phy_pac_mut(registers))
}

/// Consume one configuration-completion edge without exposing analog-register
/// identities or value encodings outside the PAC.
pub fn observe_configuration(
    transaction: &mut PhyI2cConfigurationTransaction,
    registers: &mut impl SharedPhyAccess,
) -> Result<PhyI2cConfigurationObservation, PhyI2cConfigurationError> {
    transaction.observe_completion_edge(phy_pac_mut(registers))
}

/// Start the current command of one PAC-owned Bluetooth TX-power transaction.
pub fn start_bluetooth_tx_power_control(
    transaction: &mut BluetoothTxPowerControlTransaction,
    registers: &mut impl SharedPhyAccess,
) -> Result<(), BluetoothTxPowerControlError> {
    transaction.start(phy_pac_mut(registers))
}

/// Consume one independently delivered completion edge without exposing the
/// analog register, mask or retained values outside the PAC.
pub fn observe_bluetooth_tx_power_control(
    transaction: &mut BluetoothTxPowerControlTransaction,
    registers: &mut impl SharedPhyAccess,
) -> Result<BluetoothTxPowerControlObservation, BluetoothTxPowerControlError> {
    transaction.observe_completion_edge(phy_pac_mut(registers))
}

/// Install the complete reviewed host map through the affine PHY owner.
pub fn configure_host_map(registers: &mut impl SharedPhyAccess) {
    phy_pac_mut(registers).configure_phy_i2c_host_map();
}

/// Publish the complete read-mask word through the affine PHY owner.
pub fn publish_read_mask(registers: &mut impl SharedPhyAccess, read_mask: u16) {
    phy_pac_mut(registers).publish_phy_i2c_read_mask(read_mask);
}

/// Publish one complete command through the selected affine host.
pub fn publish_command(
    registers: &mut impl SharedPhyAccess,
    host: PhyI2cHost,
    block: u8,
    register: u8,
    value: u8,
    write: bool,
) {
    phy_pac_mut(registers).publish_phy_i2c_command(host, block, register, value, write);
}

/// Sample one completed host result through the affine PHY owner.
pub fn sample_result(registers: &impl SharedPhyAccess, host: PhyI2cHost) -> u8 {
    phy_pac(registers).sample_phy_i2c_result(host)
}

/// Publish one full-word PHY-I2C master reset command.
///
/// The complete ROM parent writes only bit 26 to a busy host and then polls
/// bit 25. This finite method publishes that one edge; retry and timeout
/// ownership remain in the Rust transition.
pub fn pulse_master_reset(registers: &mut impl SharedPhyAccess, host: PhyI2cHost) {
    phy_pac_mut(registers).pulse_phy_i2c_master_reset(host);
}

/// Sample one PHY-I2C master reset busy edge without retrying.
pub fn sample_master_reset_busy(registers: &impl SharedPhyAccess, host: PhyI2cHost) -> bool {
    phy_pac(registers).phy_i2c_master_is_busy(host)
}

/// Apply all six writes of the recovered PHY-I2C clock selection.
///
/// Basis: complete rev0 ROM `phy_i2c_clk_sel` at `0x2f829f1c`, size `0x68`.
/// Each of three registers receives a high-field update followed by a fresh
/// read and low-field update, preserving all instruction-evidenced
/// intermediate states.
pub fn configure_clock_selection(registers: &mut impl SharedPhyAccess, selection: u32) {
    phy_pac_mut(registers).configure_phy_i2c_clock_selection(selection);
}

/// Configure the PHY-I2C master register mode and enable bit.
///
/// Basis: complete rev0 ROM `phy_i2cmst_reg_init` at `0x2f8276c4`, size
/// `0x22`. It writes `MASTER_CONTROL.REGISTER_MODE = 2`, then sets
/// `REGISTER_ENABLE`, using a fresh read for each update.
pub fn configure_master_registers(registers: &mut impl SharedPhyAccess) {
    phy_pac_mut(registers).configure_phy_i2c_master_registers();
}

/// Select the complete rev0 ROM `phy_bbpll_cal` mode.
///
/// The body at `0x2f82_7dbc`, size `0x1c`, performs one fresh-read
/// replacement of `MASTER_CONTROL` bits 3:2. Zero selects encoded mode one;
/// every nonzero input selects encoded mode two. The boolean API makes that
/// two-state contract explicit while preserving all unrelated shared fields.
pub fn configure_bbpll_calibration(registers: &mut impl SharedPhyAccess, enabled: bool) {
    phy_pac_mut(registers).set_phy_i2c_bbpll_calibration(enabled);
}

/// Program the complete PAC-owned PHY-I²C command memory.
///
/// Basis: complete S31
/// `libphy.a[phy_i2c.o]::phy_i2c_master_cmd_mem_init`. The SVD `dim=45`
/// array localizes every destination; its indices, internal analog addresses,
/// fixed values and derived images remain private to the PAC.
pub fn configure_command_memory(
    registers: &mut impl SharedPhyAccess,
    inputs: PhyI2cCommandMemoryInputs,
) {
    phy_pac_mut(registers).configure_phy_i2c_command_memory(inputs);
}
