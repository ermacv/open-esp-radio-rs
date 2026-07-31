//! Owned ESP32-S31 PHY/baseband configuration leaves.
//!
//! The operations in this module preserve every fresh-read update from the
//! complete rev0 ROM and pinned `libphy.a` bodies while delegating register
//! layout and legal field images to the generated PAC.

#[cfg(target_arch = "riscv32")]
use open_esp_radio_esp32s31_pac::RadioRegisters;

/// Enable the two IQ-correction modes selected by PHY initialization.
///
/// Complete rev0 ROM `phy_iq_corr_enable` at `0x2f82_7d8c`, size `0x24`,
/// sets RX- and TX-IQ mode fields with two independent fresh-read updates.
#[cfg(target_arch = "riscv32")]
pub fn enable_iq_correction(registers: &mut RadioRegisters) {
    registers.enable_iq_correction_modes();
}

/// Preserve the two fresh status publications at RXIQ root entry.
///
/// Complete pinned `libphy.a[phy_rx_gain.o]::phy_rxiq_cal_init`, size
/// `0x198`, sets the shared status/clock word's bits 14 and 15 through two
/// independent reads.
#[cfg(target_arch = "riscv32")]
pub fn configure_rxiq_root_status(registers: &mut RadioRegisters) {
    registers.configure_rxiq_root_status();
}

/// Apply one complete RXIQ root correction-mode prefix or suffix.
///
/// Each branch retains the complete pinned parent's four separately ordered
/// fresh-read field updates.
#[cfg(target_arch = "riscv32")]
pub fn configure_rxiq_root_correction(registers: &mut RadioRegisters, begin: bool) {
    registers.configure_rxiq_root_correction(begin);
}

/// Configure the fourteen-edge baseband TX-power tracking leaf.
///
/// Basis: complete pinned
/// `libphy.a[phy_reg.o]::phy_bb_txpwr_track`, size `0xf4`.
#[cfg(target_arch = "riscv32")]
pub fn configure_tx_power_tracking(registers: &mut RadioRegisters, enabled: bool) {
    registers.configure_tx_power_tracking(enabled);
}

/// Configure PHY I²C TX-rate fields and TX-gain compensation bytes.
///
/// Complete rev0 ROM `phy_i2c_txrate_init` at `0x2f82_86d0`, size `0x38`,
/// performs two rate updates, then dispatches to complete pinned
/// `phy_txgain_comp_pacfg_new(1)` for four ordered byte updates.
#[cfg(target_arch = "riscv32")]
pub fn configure_i2c_tx_rate(registers: &mut RadioRegisters) {
    registers.configure_i2c_tx_rate();
}

/// Configure the complete baseband watchdog leaf.
///
/// Basis: complete rev0 ROM `phy_bb_wdg_cfg` at `0x2f82_7860`, size `0x2c`.
#[cfg(target_arch = "riscv32")]
pub fn configure_watchdog(registers: &mut RadioRegisters) {
    registers.configure_baseband_watchdog();
}

/// Enable the four recovered automatic noise-floor controls.
///
/// Basis: complete rev0 ROM `phy_noise_floor_auto_set` at `0x2f82_7d3c`,
/// size `0x36`.
#[cfg(target_arch = "riscv32")]
pub fn configure_noise_floor_auto(registers: &mut RadioRegisters) {
    registers.configure_noise_floor_auto();
}

/// Apply the complete baseband register initialization leaf.
///
/// Complete rev0 ROM `phy_bb_reg_init` at `0x2f82_79c6`, size `0x140`,
/// supplies all local writes. Calls to the already owned NRX and platform
/// baseband-control leaves remain at their original positions.
#[cfg(target_arch = "riscv32")]
pub fn initialize_baseband(
    platform: &mut impl crate::wifi_bb::PhyWifiBbControl,
    registers: &mut RadioRegisters,
) {
    use crate::phy_frequency;

    registers.initialize_baseband_prefix();
    phy_frequency::initialize_nrx_baseband(registers);
    registers.initialize_baseband_middle();
    phy_frequency::set_baseband_init_control(platform);
    registers.initialize_baseband_tail();
}

/// Apply the complete six-edge PA-on configuration leaf.
///
/// Basis: complete rev0 ROM `phy_tx_paon_set` at `0x2f82_764c`, size `0x78`.
#[cfg(target_arch = "riscv32")]
pub fn configure_tx_pa_on(registers: &mut RadioRegisters) {
    registers.configure_tx_pa_on();
}
