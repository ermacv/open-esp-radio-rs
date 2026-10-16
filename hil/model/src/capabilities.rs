//! The target capabilities a scenario needs from the image.

use core::fmt;
use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

/// A product composition crate, by package name, such as
/// `oer-esp32s31-ieee80211-system`. The HIL image composes the same crate
/// as the product; only the observer parameter and the agent's capability
/// servers differ (#761, owner rule of 2026-10-16).
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct CompositionName(pub String);

/// A capability the HIL agent serves, by its `hil/protocol` module path,
/// such as `conformance/lower-mac` or `measure/p2-steps`. The image keys
/// derive from these through `oer-hil-image-keys`.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct CapabilityKey(pub String);

/// The composition and agent capabilities a scenario needs. `None`
/// composition means any (a platform scenario such as boot smoke).
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct TargetCapabilities {
    pub composition: Option<CompositionName>,
    pub capabilities: BTreeSet<CapabilityKey>,
}

/// Two scenarios need different product compositions, so they cannot share
/// an image.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompositionConflict(pub CompositionName, pub CompositionName);

impl fmt::Display for CompositionConflict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "compositions `{}` and `{}` cannot share an image",
            self.0.0, self.1.0
        )
    }
}

impl std::error::Error for CompositionConflict {}

impl TargetCapabilities {
    pub fn of(composition: Option<&str>, capabilities: &[&str]) -> Self {
        Self {
            composition: composition.map(|name| CompositionName(name.into())),
            capabilities: capabilities
                .iter()
                .map(|key| CapabilityKey((*key).into()))
                .collect(),
        }
    }

    /// The image serving both: the union of capabilities over one
    /// composition.
    pub fn union(&self, other: &Self) -> Result<Self, CompositionConflict> {
        let composition = match (&self.composition, &other.composition) {
            (Some(a), Some(b)) if a != b => return Err(CompositionConflict(a.clone(), b.clone())),
            (Some(a), _) | (None, Some(a)) => Some(a.clone()),
            (None, None) => None,
        };
        Ok(Self {
            composition,
            capabilities: self
                .capabilities
                .union(&other.capabilities)
                .cloned()
                .collect(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_image_serves_the_union_over_one_composition() {
        let smoke = TargetCapabilities::of(None, &["base/boot"]);
        let conformance = TargetCapabilities::of(
            Some("oer-esp32s31-ieee80211-system"),
            &["conformance/lower-mac"],
        );
        let union = smoke.union(&conformance).unwrap();
        assert_eq!(union.composition, conformance.composition);
        assert_eq!(union.capabilities.len(), 2);
        let other = TargetCapabilities::of(Some("oer-esp32s31-bluetooth-system"), &[]);
        assert!(union.union(&other).is_err());
    }
}
