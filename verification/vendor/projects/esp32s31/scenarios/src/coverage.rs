//! Reviewed decisions on vendor code the scenarios do not reach.
//!
//! Blobray measures which blocks and branch directions of a claimed root's
//! closure the executions reached. Every uncovered location is either excluded
//! by a decision below, with its reason, or reported as untriaged in the
//! evidence index. A decision that no longer excludes anything fails its
//! scenario, so the table cannot silently outlive the code it describes.
use crate::harness::{Result, invalid};
use crate::session::evidence_index::{Location, LocationKind};
use std::collections::{BTreeMap, BTreeSet};

/// What one decision excludes.
#[derive(Clone, Copy, Debug)]
pub enum Place {
    /// Every uncovered location of the vendor function. Applies to a scenario
    /// only when the function is in one of its closures.
    Function(&'static str),
}

#[derive(Clone, Copy, Debug)]
pub struct Decision {
    pub reason: &'static str,
    pub places: &'static [Place],
}

/// Vendor runtime helpers inside the PHY closures. Their paths depend on
/// operand magnitudes, alignment and diagnostic formatting, not on driver
/// behavior: production has its own Rust arithmetic, copies and no vendor
/// console output, and the comparisons observe the helpers only through the
/// PHY effects that use their results.
pub const RUNTIME: &[Decision] = &[
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
];

/// Uncovered locations of one scenario's claimed closures, and the functions
/// those closures contain.
#[derive(Default)]
pub struct Observed {
    pub uncovered: BTreeSet<Location>,
    pub functions: BTreeSet<String>,
}

impl Observed {
    /// Split `locations` into (excluded, untriaged) under `decisions`.
    pub fn classify(
        decisions: &[Decision],
        locations: &BTreeSet<Location>,
    ) -> (BTreeSet<Location>, BTreeSet<Location>) {
        let excluded = |location: &Location| {
            decisions.iter().any(|d| {
                d.places.iter().any(|p| match p {
                    Place::Function(name) => location.function == *name,
                })
            })
        };
        locations.iter().cloned().partition(excluded)
    }

    /// Every decision must still exclude an uncovered location of a function
    /// this scenario's closures contain.
    pub fn check(&self, suite: &str, decisions: &[Decision]) -> Result<()> {
        let mut uncovered: BTreeMap<&str, usize> = BTreeMap::new();
        for location in &self.uncovered {
            *uncovered.entry(location.function.as_str()).or_default() += 1;
        }
        for decision in decisions {
            for place in decision.places {
                match place {
                    Place::Function(name) => {
                        if self.functions.contains(*name) && !uncovered.contains_key(name) {
                            return Err(invalid(format!(
                                "{suite}: coverage decision for {name} excludes nothing; \
                                 its function is fully covered"
                            )));
                        }
                    }
                }
            }
        }
        Ok(())
    }
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
}
