//! ESP32-S31 system Wi-Fi/baseband control used by the open PHY.
//!
//! `MODEM_SYSCON` is an official chip-level peripheral. Its ownership and
//! svd2rust field decoding therefore remain in the platform integration;
//! this module owns the instruction-evidenced operation order.

/// Platform capability for the `MODEM_SYSCON.WIFI_BB_CFG` fields used by PHY.
pub trait PhyWifiBbControl {
    /// Clear the unknown cold-start bit and Wi-Fi enable in one fresh RMW.
    fn clear_cold_start_wifi_control(&mut self);
    /// Sample the instruction-identified Wi-Fi enable bit.
    fn wifi_baseband_is_enabled(&self) -> bool;
    /// Set or clear the instruction-identified Wi-Fi enable bit.
    fn set_wifi_baseband_enabled(&mut self, enabled: bool);
    /// Select the recovered digital BSS 40-MHz encoding.
    fn set_bss_cbw_40_digital(&mut self, enabled: bool);
    /// Replace the three-bit baseband/AGC update encoding.
    fn set_bb_agc_update_encoding(&mut self, encoding: u8);
    /// Set or clear the recovered MAC/baseband gate.
    fn set_mac_baseband_enabled(&mut self, enabled: bool);
}

/// Clear both low Wi-Fi control bits before the open cold transition.
///
/// SOURCE[`libphy.a[phy_init.o]::register_chipv7_phy`, size `0x1e6`].
/// Bit 1 is independently identified by complete ROM
/// `phy_wifi_enable_set`; bit 0 retains an `UNKNOWN` PAC name.
pub fn prepare_cold_start(platform: &mut impl PhyWifiBbControl) {
    platform.clear_cold_start_wifi_control();
}

/// Apply the single low update bit used by complete ROM `phy_bb_reg_init`.
pub fn set_baseband_init_control(platform: &mut impl PhyWifiBbControl) {
    platform.set_bb_agc_update_encoding(1);
}

/// Select the digital BSS bandwidth branch used by `phy_bb_bss_cbw40_dig`.
pub fn set_bss_cbw_40_digital(platform: &mut impl PhyWifiBbControl, enabled: bool) {
    platform.set_bss_cbw_40_digital(enabled);
}

/// Apply complete ROM `phy_wifi_enable_set`.
pub fn set_wifi_enabled(platform: &mut impl PhyWifiBbControl, enabled: bool) {
    platform.set_wifi_baseband_enabled(enabled);
}

/// Apply the final system-register update from ROM `phy_bb_agc_reg_update`.
pub fn enable_agc_register_update(platform: &mut impl PhyWifiBbControl) {
    platform.set_bb_agc_update_encoding(7);
}

/// Apply complete ROM `phy_mac_enable_bb`.
///
/// SOURCE[rev0 ROM `phy_mac_enable_bb` at `0x2f82_7836`, size `0x2a`].
/// The three separate calls deliberately preserve its three fresh RMW edges.
pub fn enable_mac_baseband(platform: &mut impl PhyWifiBbControl) {
    platform.set_mac_baseband_enabled(true);
    platform.set_wifi_baseband_enabled(false);
    platform.set_wifi_baseband_enabled(true);
}

#[cfg(test)]
mod tests {
    use std::vec::Vec;

    use super::*;

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum Operation {
        ClearColdStart,
        SetWifi(bool),
        SetBss40(bool),
        SetAgc(u8),
        SetMacBaseband(bool),
    }

    #[derive(Default)]
    struct FakePlatform {
        wifi_enabled: bool,
        operations: Vec<Operation>,
    }

    impl PhyWifiBbControl for FakePlatform {
        fn clear_cold_start_wifi_control(&mut self) {
            self.wifi_enabled = false;
            self.operations.push(Operation::ClearColdStart);
        }

        fn wifi_baseband_is_enabled(&self) -> bool {
            self.wifi_enabled
        }

        fn set_wifi_baseband_enabled(&mut self, enabled: bool) {
            self.wifi_enabled = enabled;
            self.operations.push(Operation::SetWifi(enabled));
        }

        fn set_bss_cbw_40_digital(&mut self, enabled: bool) {
            self.operations.push(Operation::SetBss40(enabled));
        }

        fn set_bb_agc_update_encoding(&mut self, encoding: u8) {
            self.operations.push(Operation::SetAgc(encoding));
        }

        fn set_mac_baseband_enabled(&mut self, enabled: bool) {
            self.operations.push(Operation::SetMacBaseband(enabled));
        }
    }

    #[test]
    fn mac_enable_retains_three_separate_rom_edges() {
        let mut platform = FakePlatform::default();
        enable_mac_baseband(&mut platform);
        assert_eq!(
            platform.operations,
            [
                Operation::SetMacBaseband(true),
                Operation::SetWifi(false),
                Operation::SetWifi(true),
            ]
        );
        assert!(platform.wifi_baseband_is_enabled());
    }

    #[test]
    fn recovered_encodings_are_not_widened_by_the_adapter() {
        let mut platform = FakePlatform::default();
        set_baseband_init_control(&mut platform);
        enable_agc_register_update(&mut platform);
        set_bss_cbw_40_digital(&mut platform, true);
        assert_eq!(
            platform.operations,
            [
                Operation::SetAgc(1),
                Operation::SetAgc(7),
                Operation::SetBss40(true),
            ]
        );
    }
}
