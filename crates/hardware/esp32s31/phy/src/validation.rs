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
    platform: &mut impl oer_esp32s31_hal::owner::SharedPhyAccess,
    block: u8,
) -> u32 {
    let Some(block) = crate::analog::i2c::PhyI2cBlock::from_vendor_abi(block) else {
        return u32::MAX;
    };

    match oer_esp32s31_hal::phy::i2c::configure_and_select_host(platform, block) {
        oer_esp32s31_hal::phy::i2c::PhyI2cHost::Host0 => 0,
        oer_esp32s31_hal::phy::i2c::PhyI2cHost::Host1 => 1,
    }
}
