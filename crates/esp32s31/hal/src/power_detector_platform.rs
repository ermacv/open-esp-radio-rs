//! ESP32-S31 system power-detector control used by the open PHY.
//!
//! `LP_AON_CLKRST` is an official chip-level peripheral. Its ownership and
//! svd2rust field decoding therefore remain in the platform integration;
//! this module exposes only the two encodings evidenced by complete ROM code.

/// Platform capability for `LP_AON_CLKRST.RTC_SAR2_PWDET_CCT`.
pub trait PhyPowerDetectorPlatformControl {
    /// Select encoding four, used by PWDET register initialization and enable.
    fn select_power_detector_initialization_mode(&mut self);

    /// Select encoding two, used by TX-calibration debug mode.
    fn select_power_detector_calibration_mode(&mut self);
}

/// Apply the final system-register edge of complete ROM `phy_pwdet_reg_init`.
///
/// SOURCE[rev0 ROM `phy_pwdet_reg_init` at `0x2f82_634a`, size `0x5c`].
pub fn select_initialization_mode(platform: &mut impl PhyPowerDetectorPlatformControl) {
    platform.select_power_detector_initialization_mode();
}

/// Apply the final system-register edge of complete ROM `phy_en_pwdet` through
/// its complete `phy_pwdet_sar2_init` callee.
///
/// SOURCE[rev0 ROM `phy_pwdet_sar2_init` at `0x2f82_63a6`, size `0x34`].
pub fn select_enabled_mode(platform: &mut impl PhyPowerDetectorPlatformControl) {
    platform.select_power_detector_initialization_mode();
}

/// Apply the system-register edge of complete ROM `phy_txcal_debuge_mode_`.
///
/// SOURCE[rev0 ROM `phy_txcal_debuge_mode_` at `0x2f82_44fe`, size `0x56`].
pub fn select_calibration_mode(platform: &mut impl PhyPowerDetectorPlatformControl) {
    platform.select_power_detector_calibration_mode();
}

#[cfg(test)]
mod tests {
    use std::vec::Vec;

    use super::*;

    #[derive(Debug, Eq, PartialEq)]
    enum Operation {
        Initialization,
        Calibration,
    }

    #[derive(Default)]
    struct FakePlatform {
        operations: Vec<Operation>,
    }

    impl PhyPowerDetectorPlatformControl for FakePlatform {
        fn select_power_detector_initialization_mode(&mut self) {
            self.operations.push(Operation::Initialization);
        }

        fn select_power_detector_calibration_mode(&mut self) {
            self.operations.push(Operation::Calibration);
        }
    }

    #[test]
    fn public_api_exposes_only_rom_evidenced_encodings() {
        let mut platform = FakePlatform::default();
        select_initialization_mode(&mut platform);
        select_enabled_mode(&mut platform);
        select_calibration_mode(&mut platform);
        assert_eq!(
            platform.operations,
            [
                Operation::Initialization,
                Operation::Initialization,
                Operation::Calibration,
            ]
        );
    }
}
