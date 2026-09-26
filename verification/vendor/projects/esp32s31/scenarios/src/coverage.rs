//! Reviewed decisions on vendor code the scenarios do not reach.
//!
//! Blobray measures which blocks and branch directions of a claimed root's
//! closure the executions reached. Every uncovered location is either excluded
//! by a decision below, with its reason, or reported as untriaged in the
//! evidence index. A decision that no longer excludes anything fails its
//! scenario, so the table cannot silently outlive the code it describes.
use crate::harness::{Result, invalid};
use crate::session::evidence_index::{Location, LocationKind};
use std::collections::BTreeSet;

/// What one decision excludes.
#[derive(Clone, Copy, Debug)]
pub enum Place {
    /// Every uncovered location of the vendor function. Applies to a scenario
    /// only when the function is in one of its closures.
    Function(&'static str),
    /// Uncovered locations of the vendor function at offsets from `start`
    /// up to `end`: one path of a function whose other paths are compared.
    Range {
        function: &'static str,
        start: u32,
        end: u32,
    },
}

impl Place {
    fn function(&self) -> &'static str {
        match self {
            Place::Function(name) | Place::Range { function: name, .. } => name,
        }
    }

    fn excludes(&self, location: &Location) -> bool {
        match self {
            Place::Function(name) => location.function == *name,
            Place::Range {
                function,
                start,
                end,
            } => location.function == *function && (*start..*end).contains(&location.offset),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Decision {
    pub reason: &'static str,
    pub places: &'static [Place],
}

/// Reviewed exclusions of every scenario. Vendor runtime helpers inside the
/// PHY closures: their paths depend on operand magnitudes, alignment and
/// diagnostic formatting, not on driver behavior; production has its own Rust
/// arithmetic, copies and no vendor console output, and the comparisons
/// observe the helpers only through the PHY effects that use their results.
/// Vendor configurations production rejects fail-closed have no production
/// path to compare.
pub const DECISIONS: &[Decision] = &[
    Decision {
        reason: "vendor diagnostic formatting; production emits no vendor console output",
        places: &[
            Place::Function("ets_printf"),
            Place::Function("ets_vprintf"),
            Place::Function("_cvt"),
            Place::Function("strlen"),
        ],
    },
    Decision {
        reason: "compiler and C runtime helpers; production uses Rust core arithmetic and copies",
        places: &[
            Place::Function("__divdi3"),
            Place::Function("__udivdi3"),
            Place::Function("__umoddi3"),
            Place::Function("__clzsi2"),
            Place::Function("memcpy"),
        ],
    },
    Decision {
        reason: "compiler prologue and epilogue millicode; its entry point depends on register pressure",
        places: &[
            Place::Function("__riscv_save_10"),
            Place::Function("__riscv_save_12"),
            Place::Function("__riscv_restore_10"),
            Place::Function("__riscv_restore_12"),
        ],
    },
    Decision {
        reason: "direct RFPLL programming for a frequency outside the 2400-2484 MHz channel \
            table: production rejects such a channel request fail-closed, and inside the \
            claimed closures only that branch of `phy_set_channel_rfpll_freq` reaches the chain",
        places: &[
            // The frequency-table bound check's out-of-table path.
            Place::Range {
                function: "phy_set_channel_rfpll_freq",
                start: 0x24,
                end: 0x44,
            },
            Place::Function("phy_set_rf_freq_offset"),
            Place::Function("phy_set_rfpll_freq"),
            Place::Function("phy_rfpll_set_freq"),
            Place::Function("phy_rfpll_cap_init_cal"),
            Place::Function("phy_wait_rfpll_cal_end"),
            Place::Function("phy_write_rfpll_sdm"),
        ],
    },
    Decision {
        reason: "2484-MHz (channel 14) and 5-GHz MHz requests: production accepts only \
            channels 1 to 13 and rejects the others fail-closed",
        places: &[
            // The 2484-MHz equality and the above-2483-MHz directions ...
            Place::Range {
                function: "phy_mhz2ieee",
                start: 0x6,
                end: 0x7,
            },
            Place::Range {
                function: "phy_mhz2ieee",
                start: 0xe,
                end: 0xf,
            },
            // ... and their 5-GHz and channel-14 blocks.
            Place::Range {
                function: "phy_mhz2ieee",
                start: 0x26,
                end: 0x3a,
            },
        ],
    },
    Decision {
        reason: "estimator mode argument: every `phy_dc_iq_est` caller in the claimed \
            closures passes zero",
        places: &[
            Place::Range {
                function: "phy_dc_iq_est",
                start: 0x10,
                end: 0x14,
            },
            Place::Range {
                function: "phy_dc_iq_est",
                start: 0x5c,
                end: 0x60,
            },
        ],
    },
    Decision {
        reason: "RX-DC minimum search that admits no estimate: readiness activity without a \
            detected RX saturation, whose first radio search returns the ROM's \
            never-initialized output slot; characterized as a DIFF, not claimed \
            (user decision 2026-09-26)",
        places: &[
            // The rejected-estimate direction of the saturation guard ...
            Place::Range {
                function: "phy_rxdc_est_min",
                start: 0x52,
                end: 0x53,
            },
            // ... the retry while no estimate lowered the minimum, and the
            // exhausted search's power sentinel.
            Place::Range {
                function: "phy_rxdc_est_min",
                start: 0x62,
                end: 0x63,
            },
            Place::Range {
                function: "phy_rxdc_est_min",
                start: 0x76,
                end: 0x7e,
            },
        ],
    },
    Decision {
        reason: "channel-14 MIC configuration: production rejects an enabled MIC option \
            and channel 14 fail-closed, as the qualified AP/STA profile requires",
        places: &[
            Place::Function("phy_chan14_mic_cfg_new"),
            // The `phy_param[0x26]` guard's enabled path up to the 802.11p guard.
            Place::Range {
                function: "phy_chip_set_chan",
                start: 0xac,
                end: 0xc0,
            },
        ],
    },
];

/// Uncovered locations of one scenario's claimed closures, and the functions
/// those closures contain.
#[derive(Default)]
pub struct Observed {
    pub uncovered: BTreeSet<Location>,
    pub functions: BTreeSet<String>,
    pub closures: Vec<Closure>,
}

impl Observed {
    /// Split `locations` into (excluded, untriaged) under `decisions`.
    pub fn classify(
        decisions: &[Decision],
        locations: &BTreeSet<Location>,
    ) -> (BTreeSet<Location>, BTreeSet<Location>) {
        let excluded = |location: &Location| {
            decisions
                .iter()
                .any(|d| d.places.iter().any(|p| p.excludes(location)))
        };
        locations.iter().cloned().partition(excluded)
    }

    /// Every place must still exclude an uncovered location when its
    /// function is in one of this scenario's closures.
    pub fn check(&self, suite: &str, decisions: &[Decision]) -> Result<()> {
        for place in decisions.iter().flat_map(|d| d.places) {
            if self.functions.contains(place.function())
                && !self.uncovered.iter().any(|l| place.excludes(l))
            {
                return Err(invalid(format!(
                    "{suite}: coverage decision {place:?} excludes nothing; \
                     its locations are covered"
                )));
            }
        }
        Ok(())
    }
}

/// Closure functions of one claim and the locations its executions left
/// uncovered.
#[derive(Clone, Debug, Default)]
pub struct Closure {
    pub functions: BTreeSet<String>,
    pub uncovered: BTreeSet<Location>,
}

/// Locations of `untriaged` that no closure covers: a closure covers a
/// location when it contains the location's function and its executions
/// reached it.
pub fn uncovered_everywhere(
    closures: &[Closure],
    untriaged: BTreeSet<Location>,
) -> BTreeSet<Location> {
    untriaged
        .into_iter()
        .filter(|location| {
            closures.iter().all(|closure| {
                !closure.functions.contains(&location.function)
                    || closure.uncovered.contains(location)
            })
        })
        .collect()
}

/// A block or a branch direction, for the index.
pub fn location(function: &str, entry: u32, address: u32, kind: LocationKind) -> Location {
    Location {
        function: function.to_owned(),
        offset: address.wrapping_sub(entry),
        kind,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DECISIONS: &[Decision] = &[Decision {
        reason: "test",
        places: &[Place::Function("helper")],
    }];

    fn at(function: &str, offset: u32) -> Location {
        Location {
            function: function.into(),
            offset,
            kind: LocationKind::Block,
        }
    }

    #[test]
    fn decisions_exclude_their_functions_and_leave_the_rest_untriaged() {
        let locations = BTreeSet::from([at("helper", 4), at("phy_root", 8)]);
        let (excluded, untriaged) = Observed::classify(DECISIONS, &locations);
        assert_eq!(excluded, BTreeSet::from([at("helper", 4)]));
        assert_eq!(untriaged, BTreeSet::from([at("phy_root", 8)]));
    }

    #[test]
    fn a_decision_on_a_fully_covered_closure_function_is_stale() {
        let mut observed = Observed::default();
        // Absent from every closure: the decision does not apply.
        observed.check("suite", DECISIONS).unwrap();
        observed.functions.insert("helper".into());
        assert!(observed.check("suite", DECISIONS).is_err());
        observed.uncovered.insert(at("helper", 0));
        observed.check("suite", DECISIONS).unwrap();
    }

    #[test]
    fn a_location_another_closure_covers_is_not_untriaged() {
        let closure = |functions: &[&str], uncovered: &[Location]| Closure {
            functions: functions.iter().map(|f| f.to_string()).collect(),
            uncovered: uncovered.iter().cloned().collect(),
        };
        let untriaged = BTreeSet::from([at("phy_root", 4), at("phy_root", 8)]);
        let closures = [
            closure(&["phy_root"], &[at("phy_root", 4), at("phy_root", 8)]),
            // Reaches offset 8, not 4.
            closure(&["phy_root"], &[at("phy_root", 4)]),
            // Does not contain the function, so it covers nothing of it.
            closure(&["other"], &[]),
        ];
        assert_eq!(
            uncovered_everywhere(&closures, untriaged),
            BTreeSet::from([at("phy_root", 4)])
        );
    }

    #[test]
    fn a_range_excludes_only_its_offsets_and_goes_stale_when_covered() {
        const RANGE: &[Decision] = &[Decision {
            reason: "test",
            places: &[Place::Range {
                function: "phy_root",
                start: 4,
                end: 8,
            }],
        }];
        let locations = BTreeSet::from([at("phy_root", 4), at("phy_root", 8)]);
        let (excluded, untriaged) = Observed::classify(RANGE, &locations);
        assert_eq!(excluded, BTreeSet::from([at("phy_root", 4)]));
        assert_eq!(untriaged, BTreeSet::from([at("phy_root", 8)]));
        let mut observed = Observed::default();
        observed.functions.insert("phy_root".into());
        observed.uncovered.insert(at("phy_root", 8));
        assert!(observed.check("suite", RANGE).is_err());
        observed.uncovered.insert(at("phy_root", 6));
        observed.check("suite", RANGE).unwrap();
    }
}
