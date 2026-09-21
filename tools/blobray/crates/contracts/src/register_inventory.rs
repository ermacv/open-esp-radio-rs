//! Physical subjects and explicit uncertainty shared by register queries.
//!
//! Access widths belong to observations, not subject identities. Names and
//! inferred geometry can change without invalidating evidence references.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

/// A claim retains its origin even when another source disagrees with it.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PropertyClaim<T> {
    pub value: T,
    pub evidence: String,
}

/// Absence of knowledge is not an implicit hardware default.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "state",
    rename_all = "kebab-case",
    deny_unknown_fields,
    bound(deserialize = "T: Ord + Deserialize<'de>")
)]
pub enum KnowledgeProperty<T> {
    #[default]
    Unknown,
    Known {
        claims: BTreeSet<PropertyClaim<T>>,
    },
    Conflicted {
        claims: BTreeSet<PropertyClaim<T>>,
    },
}

impl<T: Clone + Ord> KnowledgeProperty<T> {
    /// Union is commutative and idempotent; distinct claims never overwrite.
    pub fn insert(&mut self, value: T, evidence: String) {
        let mut claims = match std::mem::take(self) {
            Self::Unknown => BTreeSet::new(),
            Self::Known { claims } | Self::Conflicted { claims } => claims,
        };
        claims.insert(PropertyClaim { value, evidence });
        let values = claims
            .iter()
            .map(|claim| &claim.value)
            .collect::<BTreeSet<_>>();
        *self = if values.len() > 1 {
            Self::Conflicted { claims }
        } else {
            Self::Known { claims }
        };
    }

    pub fn values(&self) -> BTreeSet<&T> {
        match self {
            Self::Unknown => BTreeSet::new(),
            Self::Known { claims } | Self::Conflicted { claims } => {
                claims.iter().map(|claim| &claim.value).collect()
            }
        }
    }

    /// Reject contradictory serialized state labels instead of trusting them.
    pub fn validate(&self) -> Result<(), String> {
        let count = self.values().len();
        if matches!(self, Self::Known { .. }) && count != 1
            || matches!(self, Self::Conflicted { .. }) && count < 2
        {
            return Err("knowledge state disagrees with its distinct claims".to_owned());
        }
        Ok(())
    }

    pub fn claims(&self) -> impl Iterator<Item = &PropertyClaim<T>> {
        match self {
            Self::Unknown => None,
            Self::Known { claims } | Self::Conflicted { claims } => Some(claims),
        }
        .into_iter()
        .flatten()
    }

    pub const fn is_unknown(&self) -> bool {
        matches!(self, Self::Unknown)
    }
}

/// A location remains addressable before register boundaries are known.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegisterSubject {
    pub chip: String,
    pub address_space: String,
    pub route: String,
    pub bank: Option<String>,
    pub address: u64,
}

impl RegisterSubject {
    pub fn id(&self) -> String {
        // Length framing prevents delimiters in source-provided components
        // from aliasing another route or bank.
        let component = |value: &str| format!("{}:{value}", value.len());
        format!(
            "register-location/{}/{}/{}/{}/{:#x}",
            component(&self.chip),
            component(&self.address_space),
            component(&self.route),
            self.bank
                .as_deref()
                .map_or_else(|| "none".to_owned(), component),
            self.address,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "kebab-case", deny_unknown_fields)]
pub enum SourceState {
    Loaded,
    Missing,
    Failed { reason: String },
    Stale { reason: String },
}

/// An analysis limitation, never proof of absent hardware behavior.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoverageGap {
    pub source: String,
    pub scope: String,
    pub reason: String,
}

/// Maximal contiguous slices of an exact bitset, including full-word uses.
pub fn bit_slices(mask: u32, width: u8) -> Vec<(u8, u8, u32)> {
    let width = width.min(32);
    let mut result = Vec::new();
    let mut bit = 0;
    while bit < width {
        if mask & (1_u32 << bit) == 0 {
            bit += 1;
            continue;
        }
        let first = bit;
        while bit + 1 < width && mask & (1_u32 << (bit + 1)) != 0 {
            bit += 1;
        }
        let count = bit - first + 1;
        let slice = if count == 32 {
            u32::MAX
        } else {
            ((1_u32 << count) - 1) << first
        };
        result.push((first, bit, slice));
        bit += 1;
    }
    result
}

/// One bit of a call argument and its original producer. No branch guard is
/// required for this relation; it also covers logging and save/restore users.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArgumentBitSource {
    pub position: usize,
    pub expression: String,
    pub kind: String,
    pub token: u32,
    pub output_bit: u8,
    pub source_bit: u8,
    pub inverted: bool,
    pub address: Option<u32>,
    pub register_bit: Option<u8>,
    pub producer: Option<String>,
    pub producer_path: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn union_retains_conflicting_origins_independently_of_load_order() {
        let mut forward = KnowledgeProperty::Unknown;
        let mut reverse = KnowledgeProperty::Unknown;
        for (value, evidence) in [("first", "a"), ("second", "b"), ("first", "a")] {
            forward.insert(value, evidence.to_owned());
        }
        for (value, evidence) in [("second", "b"), ("first", "a")] {
            reverse.insert(value, evidence.to_owned());
        }
        assert_eq!(forward, reverse);
        assert!(matches!(forward, KnowledgeProperty::Conflicted { claims } if claims.len() == 2));
    }

    #[test]
    fn slices_preserve_holes_and_whole_words() {
        assert_eq!(bit_slices(0b101, 8), [(0, 0, 1), (2, 2, 4)]);
        assert_eq!(bit_slices(u32::MAX, 32), [(0, 31, u32::MAX)]);
    }
}
