//! Owned ESP32-S31 PHY AGC register leaves.
//!
//! Register layout and legal field images live in the generated PAC. This
//! module retains only cross-peripheral sequencing and the async delay
//! boundaries owned by upper PHY state machines.

#![forbid(unsafe_code)]

#[cfg(target_arch = "riscv32")]
use crate::{ForcedRxGain, PhyAccess, phy_pac, phy_pac_mut};

/// Apply complete rev0 ROM `phy_bb_agc_reg_update`.
///
/// The body at `0x2f82_860e`, size `0xa6`, performs fourteen internal radio
/// operations before its official-platform enable edge.
#[cfg(target_arch = "riscv32")]
pub fn update_baseband_registers(
    platform: &mut impl crate::wifi_bb::PhyWifiBbControl,
    registers: &mut impl PhyAccess,
) {
    let registers = phy_pac_mut(registers);
    registers.update_agc_baseband_registers();
    crate::wifi_bb::enable_agc_register_update(platform);
}

/// Select complete rev0 ROM `phy_enable_agc` or `phy_disable_agc`.
#[cfg(target_arch = "riscv32")]
pub fn set_enabled(registers: &mut impl PhyAccess, enabled: bool) {
    let registers = phy_pac_mut(registers);
    registers.set_agc_enabled(enabled);
}

/// Select either complete ROM low-rate configuration leaf.
#[cfg(target_arch = "riscv32")]
pub fn set_low_rate_enabled(registers: &mut impl PhyAccess, enabled: bool) {
    let registers = phy_pac_mut(registers);
    registers.configure_phy_low_rate(enabled);
}

/// Read complete ROM `phy_is_low_rate_enabled`.
#[cfg(target_arch = "riscv32")]
pub fn low_rate_enabled(registers: &impl PhyAccess) -> bool {
    let registers = phy_pac(registers);
    registers.phy_low_rate_enabled()
}

/// Apply all ten fresh-read updates of complete `phy_agc_reg_init`.
#[cfg(target_arch = "riscv32")]
pub fn initialize_registers(registers: &mut impl PhyAccess, parameter_121: u8, parameter_120: u8) {
    let registers = phy_pac_mut(registers);
    registers.initialize_agc_registers(parameter_121, parameter_120);
}

/// Apply complete pinned `phy_set_rx_comp_new`.
#[cfg(target_arch = "riscv32")]
pub fn configure_rx_compensation(registers: &mut impl PhyAccess) {
    let registers = phy_pac_mut(registers);
    registers.configure_rx_compensation();
}

/// Pulse the complete pinned `phy_dc_mem_clr` field.
#[cfg(target_arch = "riscv32")]
pub fn clear_dc_memory(registers: &mut impl PhyAccess) {
    let registers = phy_pac_mut(registers);
    registers.clear_agc_dc_memory();
}

/// Apply the two MMIO updates after the 1 µs PBus work-mode edge.
#[cfg(target_arch = "riscv32")]
pub fn configure_pbus_work_mode_pulse(registers: &mut impl PhyAccess) {
    let registers = phy_pac_mut(registers);
    registers.configure_pbus_work_mode_pulse();
}

/// Clear the shared pulse after the caller-owned 2 µs edge.
#[cfg(target_arch = "riscv32")]
pub fn clear_pbus_work_mode_pulse(registers: &mut impl PhyAccess) {
    let registers = phy_pac_mut(registers);
    registers.clear_pbus_work_mode_pulse();
}

/// Apply all three fresh-read updates of complete rev0 ROM `phy_ant_init`.
#[cfg(target_arch = "riscv32")]
pub fn configure_antenna(registers: &mut impl PhyAccess) {
    let registers = phy_pac_mut(registers);
    registers.configure_agc_antenna();
}

/// Apply complete rev0 ROM `phy_ant_dft_cfg`.
#[cfg(target_arch = "riscv32")]
pub fn configure_antenna_diversity(registers: &mut impl PhyAccess, enabled: bool) {
    let registers = phy_pac_mut(registers);
    registers.configure_antenna_diversity(enabled);
}

/// Apply complete rev0 ROM `phy_force_rx_gain`.
#[cfg(target_arch = "riscv32")]
pub fn configure_forced_rx_gain(registers: &mut impl PhyAccess, enabled: bool, gain: ForcedRxGain) {
    let registers = phy_pac_mut(registers);
    registers.configure_forced_rx_gain(enabled, gain);
}

/// Apply complete rev0 ROM `phy_rx11blr_cfg`.
#[cfg(target_arch = "riscv32")]
pub fn configure_rx_11b_low_rate(registers: &mut impl PhyAccess, input: u32) {
    let registers = phy_pac_mut(registers);
    registers.configure_rx_11b_low_rate(input);
}

/// Apply either complete branch of rev0 ROM `phy_rfrx_sat_rst`.
#[cfg(target_arch = "riscv32")]
pub fn configure_rf_rx_saturation(registers: &mut impl PhyAccess, enabled: bool) {
    let registers = phy_pac_mut(registers);
    registers.configure_rf_rx_saturation(enabled);
}

/// Publish both final limits from complete pinned `phy_set_rx_gain_table`.
#[cfg(target_arch = "riscv32")]
pub fn configure_rx_gain_limits(registers: &mut impl PhyAccess, wifi_last_index: u8) {
    let registers = phy_pac_mut(registers);
    registers.configure_rx_gain_limits(wifi_last_index);
}

/// Publish one Wi-Fi AGC saturation-gain word to both destinations.
#[cfg(target_arch = "riscv32")]
pub fn set_saturation_gain(registers: &mut impl PhyAccess, value: u32) {
    let registers = phy_pac_mut(registers);
    registers.set_agc_saturation_gain(value);
}

/// Apply complete pinned `libphy.a[phy_reg.o]::phy_set_ftm_en`.
#[cfg(target_arch = "riscv32")]
pub fn set_ftm_enabled(registers: &mut impl PhyAccess, input: u32) {
    let registers = phy_pac_mut(registers);
    registers.set_ftm_enabled(input & 1 != 0);
}

/// Apply complete pinned `phy_reg_update_new` and its finite children.
#[cfg(target_arch = "riscv32")]
pub fn update_post_initialization(registers: &mut impl PhyAccess) {
    let registers = phy_pac_mut(registers);
    registers.update_agc_post_initialization();
}

/// Apply either complete branch of rev0 ROM `phy_rx_11b_opt`.
#[cfg(target_arch = "riscv32")]
pub fn configure_rx_11b_optimization(registers: &mut impl PhyAccess, enabled: bool) {
    let registers = phy_pac_mut(registers);
    registers.configure_rx_11b_optimization(enabled);
}
