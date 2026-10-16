//! The target capabilities a scenario needs from the image, and an
//! image's recipe digest.

use core::fmt;
use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::{CapabilityKey, CompositionName, Sha256, digest::canonical_json};

/// The product composition an image is built on. The HIL image composes
/// the same crate as the product; only the observer parameter and the
/// agent's capability servers differ (#761, owner rule of 2026-10-16).
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case", tag = "composition", content = "crate")]
pub enum Composition {
    /// A platform scenario, such as boot smoke, that any composition
    /// serves.
    Any,
    Product(CompositionName),
}

/// The composition and agent capabilities an image must serve.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct TargetCapabilities {
    pub composition: Composition,
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
            self.0, self.1
        )
    }
}

impl std::error::Error for CompositionConflict {}

impl TargetCapabilities {
    /// The image serving both: the union of capabilities over one
    /// composition.
    pub fn union(&self, other: &Self) -> Result<Self, CompositionConflict> {
        let composition = match (&self.composition, &other.composition) {
            (Composition::Product(a), Composition::Product(b)) if a != b => {
                return Err(CompositionConflict(a.clone(), b.clone()));
            }
            (Composition::Product(a), _) | (Composition::Any, Composition::Product(a)) => {
                Composition::Product(a.clone())
            }
            (Composition::Any, Composition::Any) => Composition::Any,
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

    /// The recipe digest of an image with these capabilities built from
    /// `source` (the source snapshot's digest): the key under which the
    /// stand profile records the bounds such an image reported.
    pub fn recipe(&self, source: &Sha256) -> Sha256 {
        let value = serde_json::json!({ "source": source, "image": self });
        Sha256::of(&canonical_json(&value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::name;

    fn of(composition: Composition, keys: &[CapabilityKey]) -> TargetCapabilities {
        TargetCapabilities {
            composition,
            capabilities: keys.iter().cloned().collect(),
        }
    }

    #[test]
    fn one_image_serves_the_union_over_one_composition() {
        let station = Composition::Product(name!(CompositionName, "oer-esp32s31-ieee80211-system"));
        let smoke = of(Composition::Any, &[name!(CapabilityKey, "base.boot")]);
        let conformance = of(
            station.clone(),
            &[name!(CapabilityKey, "conformance.lower-mac")],
        );
        let union = smoke.union(&conformance).unwrap();
        assert_eq!(union.composition, station);
        assert_eq!(union.capabilities.len(), 2);
        let ble = of(
            Composition::Product(name!(CompositionName, "oer-esp32s31-bluetooth-system")),
            &[],
        );
        assert!(union.union(&ble).is_err());
    }

    #[test]
    fn the_recipe_follows_source_and_capabilities() {
        let image = of(Composition::Any, &[name!(CapabilityKey, "base.boot")]);
        let source = Sha256([1; 32]);
        assert_eq!(image.recipe(&source), image.recipe(&source));
        assert_ne!(image.recipe(&source), image.recipe(&Sha256([2; 32])));
        let wider = of(
            Composition::Any,
            &[
                name!(CapabilityKey, "base.boot"),
                name!(CapabilityKey, "base.trace"),
            ],
        );
        assert_ne!(image.recipe(&source), wider.recipe(&source));
    }
}
