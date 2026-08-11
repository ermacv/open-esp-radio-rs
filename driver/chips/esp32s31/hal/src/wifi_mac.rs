//! Wi-Fi-specific MAC operations reached from the vendor PHY ABI.
//!
//! These leaves are deliberately separate from the shared PHY modules: their
//! physical registers belong to the 802.11 MAC and are not reusable by
//! Bluetooth, BLE or IEEE 802.15.4 PHY paths.

#[cfg(target_arch = "riscv32")]
use open_esp_radio_esp32s31_pac::RadioRegisters;

/// Apply complete rev0 ROM `phy_enable_cca` or `phy_disable_cca`.
#[cfg(target_arch = "riscv32")]
pub fn set_cca_enabled(registers: &mut RadioRegisters, enabled: bool) {
    registers.set_phy_wifi_cca_enabled(enabled);
}

/// Apply complete rev0 ROM `phy_sifs_reg_init`.
#[cfg(target_arch = "riscv32")]
pub fn initialize_sifs(registers: &mut RadioRegisters) {
    registers.initialize_phy_wifi_sifs();
}
