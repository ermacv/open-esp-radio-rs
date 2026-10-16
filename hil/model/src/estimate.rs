//! How long one scenario instance takes.

use serde::{Deserialize, Serialize};

use crate::{MissingBound, ReportedBounds, StandProfile, Wait};

/// The derived time of one instance on the stand, before image costs.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct Estimate {
    /// Once per instance: fixture preparation, reset to Hello.
    pub setup_micros: u64,
    /// Each repetition, from the waits it may spend in full.
    pub repetition_micros: u64,
    pub repetitions: u64,
}

impl Estimate {
    /// An estimate whose setup and repetition are the sums of their waits.
    pub fn from_waits(
        setup: &[Wait],
        repetition: &[Wait],
        repetitions: u64,
        bounds: &ReportedBounds,
        profile: &StandProfile,
    ) -> Result<Self, MissingBound> {
        let sum = |waits: &[Wait]| {
            waits.iter().try_fold(0_u64, |total, wait| {
                Ok(total.saturating_add(wait.micros(bounds, profile)?))
            })
        };
        Ok(Self {
            setup_micros: sum(setup)?,
            repetition_micros: sum(repetition)?,
            repetitions,
        })
    }

    pub const fn total_micros(self) -> u64 {
        self.setup_micros
            .saturating_add(self.repetition_micros.saturating_mul(self.repetitions))
    }
}

#[cfg(test)]
mod tests {
    use core::num::NonZeroU32;

    use super::*;
    use crate::{BoundName, Factor};

    #[test]
    fn an_estimate_sums_its_waits_and_repetitions() {
        let bounds = ReportedBounds::default().with("lower-mac.terminal", 2_000);
        let terminal = Wait::Contract {
            bound: BoundName::new("lower-mac.terminal"),
            factor: Factor::new(2, NonZeroU32::MIN).unwrap(),
        };
        let interval = Wait::Protocol {
            interval_micros: 102_400,
            count: NonZeroU32::new(3).unwrap(),
            basis: "three beacon intervals".into(),
        };
        let estimate = Estimate::from_waits(
            &[interval],
            &[terminal],
            5,
            &bounds,
            &StandProfile::default(),
        )
        .unwrap();
        assert_eq!(estimate.total_micros(), 307_200 + 5 * 4_000);
    }
}
