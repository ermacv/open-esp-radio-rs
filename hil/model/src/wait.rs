//! Waits derived from a contract, a protocol or a measurement, never a
//! literal.

use core::{fmt, num::NonZeroU32, time::Duration};
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{SpanName, StandProfile};

/// The name of a contract bound or backend parameter an image reports,
/// such as `lower-mac.drain` or `radio-system.retune.answer`.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct BoundName(pub String);

impl BoundName {
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }
}

impl fmt::Display for BoundName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The bounds the image under test reports for its contracts, in
/// microseconds of its monotonic clock. They come from the image, never
/// from host constants, so a backend's actual value is the one used.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ReportedBounds(pub BTreeMap<BoundName, u64>);

impl ReportedBounds {
    pub fn with(mut self, name: &str, micros: u64) -> Self {
        self.0.insert(BoundName::new(name), micros);
        self
    }
}

/// A multiplier of at least one, as a ratio, so that a margin is exact and
/// reviewable.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(try_from = "(u32, NonZeroU32)", into = "(u32, NonZeroU32)")]
pub struct Factor {
    numerator: u32,
    denominator: NonZeroU32,
}

/// A factor below one would shorten a bound.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidFactor;

impl fmt::Display for InvalidFactor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a factor is a ratio of at least one")
    }
}

impl std::error::Error for InvalidFactor {}

impl Factor {
    pub const ONE: Self = Self {
        numerator: 1,
        denominator: NonZeroU32::MIN,
    };

    pub const fn new(numerator: u32, denominator: NonZeroU32) -> Result<Self, InvalidFactor> {
        if numerator < denominator.get() {
            return Err(InvalidFactor);
        }
        Ok(Self {
            numerator,
            denominator,
        })
    }

    /// `micros × factor`, rounded up and saturating.
    pub const fn apply(self, micros: u64) -> u64 {
        let product = micros as u128 * self.numerator as u128;
        let denominator = self.denominator.get() as u128;
        let rounded = product.div_ceil(denominator);
        if rounded > u64::MAX as u128 {
            u64::MAX
        } else {
            rounded as u64
        }
    }
}

impl TryFrom<(u32, NonZeroU32)> for Factor {
    type Error = InvalidFactor;

    fn try_from((numerator, denominator): (u32, NonZeroU32)) -> Result<Self, Self::Error> {
        Self::new(numerator, denominator)
    }
}

impl From<Factor> for (u32, NonZeroU32) {
    fn from(factor: Factor) -> Self {
        (factor.numerator, factor.denominator)
    }
}

/// How long a scenario waits for something. Scenario and executor code
/// holds no duration literal: every wait names where its length comes
/// from.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case", tag = "from")]
pub enum Wait {
    /// A contract bound the image reports, times a factor.
    Contract { bound: BoundName, factor: Factor },
    /// A protocol interval times a count, for example six BLE connection
    /// intervals or one Thread leader-election period; `basis` says which.
    Protocol {
        interval_micros: u64,
        count: NonZeroU32,
        basis: String,
    },
    /// A span measured on the stand ([`StandProfile`]), such as boot to
    /// Hello or a fixture's readiness.
    Measured { span: SpanName },
}

/// A wait whose source has no value.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MissingBound {
    /// The image reports no such bound.
    Contract(BoundName),
    /// The stand profile has no such span, or not enough samples for it
    /// and no provisional label.
    Measured(SpanName),
}

impl fmt::Display for MissingBound {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Contract(name) => write!(f, "the image reports no bound `{name}`"),
            Self::Measured(name) => write!(f, "the stand profile has no usable span `{name}`"),
        }
    }
}

impl std::error::Error for MissingBound {}

impl Wait {
    /// The wait's length in microseconds.
    pub fn micros(
        &self,
        bounds: &ReportedBounds,
        profile: &StandProfile,
    ) -> Result<u64, MissingBound> {
        match self {
            Self::Contract { bound, factor } => bounds
                .0
                .get(bound)
                .map(|&micros| factor.apply(micros))
                .ok_or_else(|| MissingBound::Contract(bound.clone())),
            Self::Protocol {
                interval_micros,
                count,
                ..
            } => Ok(interval_micros.saturating_mul(u64::from(count.get()))),
            Self::Measured { span } => profile
                .budget_micros(span)
                .ok_or_else(|| MissingBound::Measured(span.clone())),
        }
    }

    /// The wait as a duration: the one place host HIL code turns a derived
    /// wait into a `Duration`.
    pub fn resolve(
        &self,
        bounds: &ReportedBounds,
        profile: &StandProfile,
    ) -> Result<Duration, MissingBound> {
        self.micros(bounds, profile).map(Duration::from_micros)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MeasuredSpan, RunRef};

    fn ratio(numerator: u32, denominator: u32) -> Factor {
        Factor::new(numerator, NonZeroU32::new(denominator).unwrap()).unwrap()
    }

    #[test]
    fn a_factor_below_one_is_rejected() {
        assert_eq!(
            Factor::new(1, NonZeroU32::new(2).unwrap()),
            Err(InvalidFactor)
        );
        assert_eq!(ratio(3, 2).apply(1001), 1502);
        assert_eq!(Factor::ONE.apply(7), 7);
    }

    #[test]
    fn a_contract_wait_uses_the_bound_the_image_reports() {
        let bounds = ReportedBounds::default().with("lower-mac.drain", 4_000);
        let wait = Wait::Contract {
            bound: BoundName::new("lower-mac.drain"),
            factor: ratio(2, 1),
        };
        assert_eq!(wait.micros(&bounds, &StandProfile::default()), Ok(8_000));
        let missing = Wait::Contract {
            bound: BoundName::new("lower-mac.retune"),
            factor: Factor::ONE,
        };
        assert_eq!(
            missing.micros(&bounds, &StandProfile::default()),
            Err(MissingBound::Contract(BoundName::new("lower-mac.retune")))
        );
    }

    #[test]
    fn a_protocol_wait_is_its_interval_times_its_count() {
        let wait = Wait::Protocol {
            interval_micros: 30_000,
            count: NonZeroU32::new(6).unwrap(),
            basis: "a connection the DUT never received ends within six intervals".into(),
        };
        assert_eq!(
            wait.micros(&ReportedBounds::default(), &StandProfile::default()),
            Ok(180_000)
        );
    }

    #[test]
    fn a_measured_wait_needs_a_supported_span() {
        let span = SpanName::new("boot-to-hello");
        let mut profile = StandProfile::default();
        let wait = Wait::Measured { span: span.clone() };
        assert_eq!(
            wait.micros(&ReportedBounds::default(), &profile),
            Err(MissingBound::Measured(span.clone()))
        );
        profile.spans.insert(
            span,
            MeasuredSpan {
                quantile_micros: 900_000,
                samples: 3_000,
                margin: ratio(3, 2),
                margin_reason: "USB re-enumeration varies with the hub".into(),
                provisional: None,
                evidence: vec![RunRef {
                    run: "r1".into(),
                    sha256: "0".repeat(64),
                }],
            },
        );
        assert_eq!(
            wait.micros(&ReportedBounds::default(), &profile),
            Ok(1_350_000)
        );
    }
}
