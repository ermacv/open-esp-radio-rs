//! Owned ESP32-S31 internal PHY clock-gate leaves.
//!
//! Platform clock/reset and power policy stay in the integration layer. This
//! module only sequences radio-internal registers described by the SVD/PAC.

#[cfg(target_arch = "riscv32")]
use crate::{owner::SharedPhyAccess, phy_pac_mut};

/// Clear the complete two-bit shared baseband image used by RF shutdown.
///
/// SOURCE\[BLOB_LIBPHY_PHY_XPD_RF_NEW_20260907]. This is one preserving
/// register transaction; it must not be decomposed into role-level toggles.
#[cfg(target_arch = "riscv32")]
pub fn clear_rf_baseband_control(registers: &mut impl SharedPhyAccess) {
    phy_pac_mut(registers).clear_phy_rf_baseband_control();
}

/// Clear the complete PMU immediate-clock upper nibble used by RF shutdown.
///
/// SOURCE\[BLOB_LIBPHY_PHY_XPD_RF_NEW_20260907].
#[cfg(target_arch = "riscv32")]
pub fn clear_rf_immediate_clock_power(registers: &mut impl SharedPhyAccess) {
    phy_pac_mut(registers).clear_phy_rf_immediate_clock_power();
}

/// Apply complete pinned `libphy.a[phy_init.o]::phy_close_fe_bb_clk`.
#[cfg(target_arch = "riscv32")]
pub fn close_frontend_baseband(registers: &mut impl SharedPhyAccess) {
    let registers = phy_pac_mut(registers);
    registers.close_frontend_baseband_clocks();
}
