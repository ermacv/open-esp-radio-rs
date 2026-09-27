//! Reviewed coverage decisions of the ESP32-C5 scenarios.
use oer_vendor_scenario_engine::coverage::{Decision, Place};

/// Reviewed exclusions of every scenario.
pub const DECISIONS: &[Decision] = &[Decision {
    reason: "the pre-write busy wait of the constant-propagated `phy_chip_i2c_writeReg` \
            copy `phy_i2c_writeReg_Mask` calls: the same wait the compared writeReg leaf \
            covers on a busy port, while the preceding read of `phy_i2c_writeReg_Mask` polled \
            the same host idle before this write",
    places: &[Place::Range {
        function: "phy_chip_i2c_writeReg.constprop.2",
        start: 0x40,
        end: 0x44,
    }],
}];
