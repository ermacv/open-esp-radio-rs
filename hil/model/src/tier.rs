//! Runner tiers and their budgets.

use serde::{Deserialize, Serialize};

/// A selection of scenarios with a wall-clock budget the plan checks
/// before anything is built or leased.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Tier {
    /// Every candidate a HIL change needs: minutes, one image.
    Smoke,
    /// Rare, at a stage: about one hour (#761, owner decision 1).
    StageGate,
    /// Before a milestone; its duration is reported, not capped.
    Full,
    /// Long scenarios, if a nightly mode is ever scheduled (decision 1).
    Nightly,
}

impl Tier {
    /// The wall-clock budget in microseconds, or `None` when the tier's
    /// duration is only reported.
    pub const fn budget_micros(self) -> Option<u64> {
        const MINUTE: u64 = 60_000_000;
        match self {
            // #761 architecture, section 6: smoke within five minutes.
            Self::Smoke => Some(5 * MINUTE),
            // #761 owner decision 1: a stage gate targets about one hour.
            Self::StageGate => Some(60 * MINUTE),
            Self::Full | Self::Nightly => None,
        }
    }
}
