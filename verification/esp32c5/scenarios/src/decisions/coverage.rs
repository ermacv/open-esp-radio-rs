//! Reviewed coverage decisions of the ESP32-C5 scenarios.
use oer_vendor_scenario_engine::coverage::{Decision, Place};

/// Reviewed exclusions of every scenario.
pub const DECISIONS: &[Decision] = &[
    Decision {
        reason: "the C library `memset` that `phy_i2c_init1` calls to clear its six 44-byte \
            stack arrays: the function then stores every byte of each array explicitly, so \
            no cleared byte reaches a command, and the other length and alignment paths of \
            `memset` belong to the C library, not to the PHY",
        places: &[Place::Function("memset")],
    },
    Decision {
        reason: "the pre-write busy wait of the constant-propagated `phy_chip_i2c_writeReg` \
            copy `phy_i2c_writeReg_Mask` calls: the same wait the compared writeReg leaf \
            covers on a busy port, while the preceding read of `phy_i2c_writeReg_Mask` polled \
            the same host idle before this write",
        places: &[Place::Range {
            function: "phy_chip_i2c_writeReg.constprop.2",
            start: 0x40,
            end: 0x44,
        }],
    },
];
