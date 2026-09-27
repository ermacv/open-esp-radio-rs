//! Reviewed coverage decisions of the ESP32-C5 scenarios.
use oer_vendor_scenario_engine::coverage::{Decision, Place};

/// Reviewed exclusions of every scenario.
pub const DECISIONS: &[Decision] = &[Decision {
    reason: "the read mask of a block outside 0x61..=0x6f, which the vendor returns as zero: \
        production rejects such a block before any bus action, so it has no path to compare",
    places: &[
        // The range check's out-of-range edge and its zero return.
        Place::Range {
            function: "phy_get_i2c_read_mask_",
            start: 0x0a,
            end: 0x0e,
        },
        Place::Range {
            function: "phy_get_i2c_read_mask_",
            start: 0x22,
            end: 0x26,
        },
    ],
}];
