//! Waits derived from a contract, a protocol or a measurement, never a
//! literal.

use core::{fmt, num::NonZeroU32, time::Duration};
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{BoundName, Overflow, RunRef, SpanName, StandProfile, arithmetic};

/// The bounds an image reports for its contracts, in microseconds of its
/// monotonic clock.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct ReportedBounds(BTreeMap<BoundName, u64>);

impl ReportedBounds {
    pub fn with(mut self, name: BoundName, micros: u64) -> Self {
        self.0.insert(name, micros);
        self
    }

    pub fn get(&self, name: &BoundName) -> Option<u64> {
        self.0.get(name).copied()
    }
}

/// Where a plan's bounds come from (#761, owner decision on the review of
/// #775).
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BoundsSource {
    /// An image of the same recipe reported them in a recorded run.
    Recorded(RunRef),
    /// The recipe is new: the contract's declared constants, so every wait
    /// on them is provisional.
    Declared { contract: &'static str },
}

/// The bounds a plan or a run uses, with their source.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Bounds {
    pub values: ReportedBounds,
    pub source: BoundsSource,
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

    /// `micros × factor`, rounded up.
    pub const fn apply(self, micros: u64) -> Result<u64, Overflow> {
        let product = micros as u128 * self.numerator as u128;
        let rounded = product.div_ceil(self.denominator.get() as u128);
        if rounded > u64::MAX as u128 {
            Err(Overflow)
        } else {
            Ok(rounded as u64)
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

/// A protocol interval typed by its protocol: a beacon interval in time
/// units, a BLE connection interval in 1.25 ms units. A scenario's
/// parameters hold such values; a [`Wait`] on a protocol interval can only
/// be made from one, never from a bare number.
pub trait ProtocolQuantity {
    /// What the interval is, for records: `beacon-interval`.
    const NAME: &'static str;
    fn interval_micros(&self) -> Result<u64, Overflow>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Source {
    Contract {
        bound: BoundName,
        factor: Factor,
    },
    Protocol {
        quantity: &'static str,
        interval_micros: u64,
        count: NonZeroU32,
    },
    Measured {
        span: SpanName,
    },
}

/// How long a scenario waits for something. Scenario and executor code
/// holds no duration literal: every wait names where its length comes
/// from.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Wait(Source);

impl Wait {
    /// A contract bound the image reports, times a factor.
    pub const fn contract(bound: BoundName, factor: Factor) -> Self {
        Self(Source::Contract { bound, factor })
    }

    /// `count` protocol intervals, for example six BLE connection intervals.
    pub fn protocol<Q: ProtocolQuantity>(
        quantity: &Q,
        count: NonZeroU32,
    ) -> Result<Self, Overflow> {
        Ok(Self(Source::Protocol {
            quantity: Q::NAME,
            interval_micros: quantity.interval_micros()?,
            count,
        }))
    }

    /// A span measured on the stand ([`StandProfile`]).
    pub const fn measured(span: SpanName) -> Self {
        Self(Source::Measured { span })
    }
}

/// A value used although its evidence is not yet sufficient; every plan
/// and record that uses it carries the label.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case", tag = "provisional")]
pub enum Provisional {
    /// A stand span with too few samples.
    Span { span: SpanName, reason: String },
    /// A contract's declared constant, because no run of the image's recipe
    /// reported the bound yet.
    DeclaredBound {
        contract: &'static str,
        bound: BoundName,
    },
}

/// A wait's length, and its label when it is provisional.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Resolved {
    pub micros: u64,
    pub provisional: Option<Provisional>,
}

/// Why a wait has no length.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WaitError {
    /// Neither the image's recorded bounds nor the contract's declared
    /// constants have the bound.
    NoBound(BoundName),
    /// The stand profile has no such span.
    NoSpan(SpanName),
    Overflow,
}

impl From<Overflow> for WaitError {
    fn from(_: Overflow) -> Self {
        Self::Overflow
    }
}

impl fmt::Display for WaitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoBound(name) => write!(f, "no bound `{name}` is recorded or declared"),
            Self::NoSpan(name) => write!(f, "the stand profile has no span `{name}`"),
            Self::Overflow => Overflow.fmt(f),
        }
    }
}

impl std::error::Error for WaitError {}

impl Wait {
    /// The wait's length in microseconds, with its label when provisional.
    pub fn micros(&self, bounds: &Bounds, profile: &StandProfile) -> Result<Resolved, WaitError> {
        match &self.0 {
            Source::Contract { bound, factor } => {
                let value = bounds
                    .values
                    .get(bound)
                    .ok_or_else(|| WaitError::NoBound(bound.clone()))?;
                let provisional = match bounds.source {
                    BoundsSource::Recorded(_) => None,
                    BoundsSource::Declared { contract } => Some(Provisional::DeclaredBound {
                        contract,
                        bound: bound.clone(),
                    }),
                };
                Ok(Resolved {
                    micros: factor.apply(value)?,
                    provisional,
                })
            }
            Source::Protocol {
                interval_micros,
                count,
                ..
            } => Ok(Resolved {
                micros: arithmetic::mul(*interval_micros, u64::from(count.get()))?,
                provisional: None,
            }),
            Source::Measured { span } => profile.budget(span),
        }
    }

    /// The wait as a duration: the one place HIL code turns a derived wait
    /// into a `Duration`.
    pub fn resolve(
        &self,
        bounds: &Bounds,
        profile: &StandProfile,
    ) -> Result<(Duration, Option<Provisional>), WaitError> {
        let resolved = self.micros(bounds, profile)?;
        #[allow(
            clippy::disallowed_methods,
            reason = "the one conversion of a derived wait, whose source the Wait names, into a Duration"
        )]
        let duration = Duration::from_micros(resolved.micros);
        Ok((duration, resolved.provisional))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::name;

    fn ratio(numerator: u32, denominator: u32) -> Factor {
        Factor::new(numerator, NonZeroU32::new(denominator).unwrap()).unwrap()
    }

    fn recorded(values: ReportedBounds) -> Bounds {
        Bounds {
            values,
            source: BoundsSource::Recorded(crate::profile::tests::run_ref()),
        }
    }

    /// A BLE connection interval in its 1.25 ms units.
    struct ConnectionInterval(u16);

    impl ProtocolQuantity for ConnectionInterval {
        const NAME: &'static str = "ble-connection-interval";

        fn interval_micros(&self) -> Result<u64, Overflow> {
            arithmetic::mul(u64::from(self.0), 1_250)
        }
    }

    #[test]
    fn a_factor_below_one_is_rejected_and_an_overflow_is_reported() {
        assert_eq!(
            Factor::new(1, NonZeroU32::new(2).unwrap()),
            Err(InvalidFactor)
        );
        assert_eq!(ratio(3, 2).apply(1001), Ok(1502));
        assert_eq!(ratio(2, 1).apply(u64::MAX), Err(Overflow));
    }

    #[test]
    fn a_contract_wait_uses_the_recorded_bound() {
        let drain = name!(BoundName, "lower-mac.drain");
        let bounds = recorded(ReportedBounds::default().with(drain.clone(), 4_000));
        let wait = Wait::contract(drain, ratio(2, 1));
        assert_eq!(
            wait.micros(&bounds, &StandProfile::empty()),
            Ok(Resolved {
                micros: 8_000,
                provisional: None
            })
        );
        let retune = name!(BoundName, "lower-mac.retune");
        assert_eq!(
            Wait::contract(retune.clone(), Factor::ONE).micros(&bounds, &StandProfile::empty()),
            Err(WaitError::NoBound(retune))
        );
    }

    #[test]
    fn a_declared_bound_is_provisional() {
        let drain = name!(BoundName, "lower-mac.drain");
        let bounds = Bounds {
            values: ReportedBounds::default().with(drain.clone(), 4_000),
            source: BoundsSource::Declared {
                contract: "toy::Port",
            },
        };
        let resolved = Wait::contract(drain.clone(), Factor::ONE)
            .micros(&bounds, &StandProfile::empty())
            .unwrap();
        assert_eq!(
            resolved.provisional,
            Some(Provisional::DeclaredBound {
                contract: "toy::Port",
                bound: drain
            })
        );
    }

    #[test]
    fn a_protocol_wait_comes_from_a_typed_quantity() {
        let wait = Wait::protocol(&ConnectionInterval(24), NonZeroU32::new(6).unwrap()).unwrap();
        let bounds = recorded(ReportedBounds::default());
        assert_eq!(
            wait.micros(&bounds, &StandProfile::empty()).unwrap().micros,
            180_000
        );
    }
}
