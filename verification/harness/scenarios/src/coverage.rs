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
    /// Every uncovered location of a vendor function that only produces
    /// diagnostic output. The triage report proposes a block whose calls
    /// all reach such functions as a diagnostic-only candidate.
    Diagnostic(&'static str),
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
            Place::Function(name)
            | Place::Diagnostic(name)
            | Place::Range { function: name, .. } => name,
        }
    }

    fn excludes(&self, location: &Location) -> bool {
        match self {
            Place::Function(name) | Place::Diagnostic(name) => location.function == *name,
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

/// Functions `decisions` review as diagnostic output.
pub fn diagnostic(decisions: &[Decision]) -> BTreeSet<String> {
    decisions
        .iter()
        .flat_map(|d| d.places)
        .filter_map(|place| match place {
            Place::Diagnostic(name) => Some((*name).to_owned()),
            Place::Function(_) | Place::Range { .. } => None,
        })
        .collect()
}

/// Uncovered locations of one scenario's claimed closures, and the functions
/// those closures contain.
#[derive(Default)]
pub struct Observed {
    pub uncovered: BTreeSet<Location>,
    pub functions: BTreeSet<String>,
    pub closures: Vec<Closure>,
}

impl Observed {
    /// Functions and uncovered locations over `closures`.
    pub fn of(closures: &[Closure]) -> Self {
        let mut observed = Self::default();
        for closure in closures {
            observed.functions.extend(closure.functions.iter().cloned());
            observed.uncovered.extend(closure.uncovered.iter().cloned());
        }
        observed
    }

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

/// A block, a branch direction or an open transfer site, for the index.
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
