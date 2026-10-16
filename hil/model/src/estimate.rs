//! How long one scenario instance takes.

use crate::{Bounds, Overflow, Provisional, StandProfile, Wait, WaitError, arithmetic};

/// The derived time of one instance on the stand, before image costs.
/// Its setup includes the instance's boot to Hello, which nothing else
/// counts.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Estimate {
    /// Once per instance: boot to Hello, fixture preparation.
    pub setup_micros: u64,
    /// Each repetition, from the waits it may spend in full.
    pub repetition_micros: u64,
    pub repetitions: u64,
    /// The provisional values the waits used.
    pub provisional: Vec<Provisional>,
}

impl Estimate {
    /// An estimate whose setup and repetition are the sums of their waits.
    pub fn from_waits(
        setup: &[Wait],
        repetition: &[Wait],
        repetitions: u64,
        bounds: &Bounds,
        profile: &StandProfile,
    ) -> Result<Self, WaitError> {
        let mut provisional = Vec::new();
        let mut sum = |waits: &[Wait]| -> Result<u64, WaitError> {
            let mut total = 0_u64;
            for wait in waits {
                let resolved = wait.micros(bounds, profile)?;
                total = arithmetic::add(total, resolved.micros)?;
                provisional.extend(resolved.provisional);
            }
            Ok(total)
        };
        let setup_micros = sum(setup)?;
        let repetition_micros = sum(repetition)?;
        provisional.sort();
        provisional.dedup();
        Ok(Self {
            setup_micros,
            repetition_micros,
            repetitions,
            provisional,
        })
    }

    pub fn total_micros(&self) -> Result<u64, Overflow> {
        arithmetic::add(
            self.setup_micros,
            arithmetic::mul(self.repetition_micros, self.repetitions)?,
        )
    }
}

#[cfg(test)]
mod tests {
    use core::num::NonZeroU32;

    use super::*;
    use crate::{
        BoundsSource, Factor, MeasuredSpan, RawStandProfile, ReportedBounds, Support, name,
        profile::tests::span,
    };

    #[test]
    fn an_estimate_sums_its_waits_and_collects_provisional_labels() {
        let terminal = name!(BoundName, "lower-mac.terminal");
        let boot = name!(SpanName, "boot-to-hello");
        let bounds = Bounds {
            values: ReportedBounds::default().with(terminal.clone(), 2_000),
            source: BoundsSource::Declared {
                contract: "toy::Port",
            },
        };
        let mut raw = RawStandProfile::default();
        let entry: MeasuredSpan = span(
            1_000_000,
            Support::Provisional {
                reason: "first runs".into(),
            },
        );
        raw.spans.insert(boot.clone(), entry);
        let profile = StandProfile::try_from(raw).unwrap();
        let estimate = Estimate::from_waits(
            &[Wait::measured(boot)],
            &[Wait::contract(
                terminal,
                Factor::new(2, NonZeroU32::MIN).unwrap(),
            )],
            5,
            &bounds,
            &profile,
        )
        .unwrap();
        assert_eq!(estimate.total_micros(), Ok(1_200_000 + 5 * 4_000));
        assert_eq!(estimate.provisional.len(), 2);
    }

    #[test]
    fn an_overflowing_total_is_an_error() {
        let estimate = Estimate {
            setup_micros: 1,
            repetition_micros: u64::MAX,
            repetitions: 2,
            provisional: Vec::new(),
        };
        assert_eq!(estimate.total_micros(), Err(Overflow));
    }
}
