//! A scenario's one purpose.

use serde::{Deserialize, Serialize};

/// What a stand scenario proves. There is deliberately no protocol purpose:
/// protocol behaviour is proven on the host against the model, and a stand
/// scenario that would only repeat it cannot name a purpose (#761, layer
/// rule).
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case", tag = "purpose")]
pub enum Purpose {
    /// The backend keeps its port contract: the port's conformance suite,
    /// the same one the host model passes.
    Conformance,
    /// The backend works with a real peer (hostapd, wpa_supplicant,
    /// OpenWrt, BlueZ, OpenThread).
    Interop,
    /// A physical budget: a step or answer bound, a latency, a rate.
    Measurement,
    /// An open question, named by its issue. Without an open issue an
    /// investigation is deleted (#761, owner decision 2).
    Investigation { issue: u32 },
}

impl Purpose {
    /// Whether a qualification requirement may name a scenario of this
    /// purpose; investigations never qualify.
    pub const fn qualifies(self) -> bool {
        !matches!(self, Self::Investigation { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_investigations_cannot_qualify() {
        assert!(Purpose::Conformance.qualifies());
        assert!(Purpose::Interop.qualifies());
        assert!(Purpose::Measurement.qualifies());
        assert!(!Purpose::Investigation { issue: 364 }.qualifies());
    }
}
