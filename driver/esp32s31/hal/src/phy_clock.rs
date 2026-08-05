//! Owned ESP32-S31 internal PHY clock-gate leaves.
//!
//! Platform clock/reset and power policy stay in the integration layer. This
//! module only sequences radio-internal registers described by the SVD/PAC.

#[cfg(target_arch = "riscv32")]
use open_esp_radio_esp32s31_registers::RadioRegisters;

/// Apply complete pinned `libphy.a[phy_init.o]::phy_close_fe_bb_clk`.
#[cfg(target_arch = "riscv32")]
pub fn close_frontend_baseband(registers: &mut RadioRegisters) {
    registers.close_frontend_baseband_clocks();
}
