//! ABI-only bridges for compiled production comparison.
//!
//! This module is absent from ordinary builds. It performs argument/result
//! conversion only and delegates the complete operation to the same private
//! production function used by both PHY-I2C command paths.

#![cfg(feature = "validation-probes")]

/// Execute the accredited-domain production behavior of
/// `phy_get_i2c_hostid_new` and project its typed host to the vendor ABI.
#[cfg(target_arch = "riscv32")]
pub fn configure_and_select_phy_i2c_host(
    platform: &mut impl open_esp_radio_esp32s31_hal::phy_i2c::PhyI2cMasterControl,
    block: u8,
) -> u32 {
    let address = crate::phy_i2c::PhyI2cAddress::new(block, 0)
        .unwrap_or_else(|| unreachable!("comparison profile owns the accredited block domain"));

    match crate::phy_i2c::configure_and_select_phy_i2c_host(platform, address) {
        open_esp_radio_esp32s31_hal::phy_i2c::PhyI2cHost::Host0 => 0,
        open_esp_radio_esp32s31_hal::phy_i2c::PhyI2cHost::Host1 => 1,
    }
}
