//! Waits derived from a contract, a protocol or a measurement, never a
//! literal.

use core::{fmt, num::NonZeroU32};
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{BoundName, Overflow, SpanName, StandProfile, arithmetic};

/// Bounds by name, in microseconds: a contract's ceilings
/// ([`Subject::ceilings`](crate::Subject::ceilings)) or the bounds an
/// image reports in its traces.
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

    pub fn iter(&self) -> impl Iterator<Item = (&BoundName, u64)> {
        self.0.iter().map(|(name, &micros)| (name, micros))
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
    pub const ONE: Self = Self::times(NonZeroU32::MIN);

    /// A whole multiple, which is never below one.
    pub const fn times(multiple: NonZeroU32) -> Self {
        Self {
            numerator: multiple.get(),
            denominator: NonZeroU32::MIN,
        }
    }

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
    pub fn apply(self, micros: u64) -> Result<u64, Overflow> {
        let product = u128::from(micros) * u128::from(self.numerator);
        let rounded = product.div_ceil(u128::from(self.denominator.get()));
        u64::try_from(rounded).map_err(|_| Overflow)
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

mod sealed {
    pub trait Sealed {}
}

/// A protocol interval typed by its protocol's unit. A [`Wait`] on a
/// protocol interval can only be made from one of these, never from a bare
/// number; a scenario's parameters hold them.
pub trait ProtocolQuantity: sealed::Sealed {
    /// What the interval is, for records.
    const NAME: &'static str;
    fn interval_micros(&self) -> Result<u64, Overflow>;
}

/// An IEEE 802.11 interval in time units of 1024 µs: a beacon interval, a
/// listen interval.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TimeUnits(pub u32);

/// A Bluetooth LE connection interval in its 1.25 ms units.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ConnectionInterval(pub u16);

/// An IEEE 802.15.4 period in symbols of the 2.4 GHz O-QPSK PHY, 16 µs
/// each: a backoff period, a turnaround, a superframe slot.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Symbols(pub u32);

impl sealed::Sealed for TimeUnits {}
impl sealed::Sealed for ConnectionInterval {}
impl sealed::Sealed for Symbols {}

impl ProtocolQuantity for TimeUnits {
    const NAME: &'static str = "ieee80211-time-units";

    fn interval_micros(&self) -> Result<u64, Overflow> {
        arithmetic::mul(u64::from(self.0), 1_024)
    }
}

impl ProtocolQuantity for ConnectionInterval {
    const NAME: &'static str = "ble-connection-interval";

    fn interval_micros(&self) -> Result<u64, Overflow> {
        arithmetic::mul(u64::from(self.0), 1_250)
    }
}

impl ProtocolQuantity for Symbols {
    const NAME: &'static str = "ieee802154-symbols";

    fn interval_micros(&self) -> Result<u64, Overflow> {
        arithmetic::mul(u64::from(self.0), 16)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Source {
    Contract {
        bound: BoundName,
        factor: Factor,
    },
    Protocol {
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
    /// A contract bound times a factor. A plan takes the bound from the
    /// contract's ceiling, which no backend may exceed.
    pub const fn contract(bound: BoundName, factor: Factor) -> Self {
        Self(Source::Contract { bound, factor })
    }

    /// `count` protocol intervals, for example six BLE connection intervals.
    pub fn protocol<Q: ProtocolQuantity>(
        quantity: &Q,
        count: NonZeroU32,
    ) -> Result<Self, Overflow> {
        Ok(Self(Source::Protocol {
            interval_micros: quantity.interval_micros()?,
            count,
        }))
    }

    /// A span measured on the stand ([`StandProfile`]).
    pub const fn measured(span: SpanName) -> Self {
        Self(Source::Measured { span })
    }
}

/// A stand span used although its samples are too few; every plan that
/// uses it carries the label.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct Provisional {
    pub span: SpanName,
    pub reason: String,
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
    /// The contract's ceilings have no such bound.
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
            Self::NoBound(name) => write!(f, "the contract has no ceiling `{name}`"),
            Self::NoSpan(name) => write!(f, "the stand profile has no span `{name}`"),
            Self::Overflow => Overflow.fmt(f),
        }
    }
}

impl std::error::Error for WaitError {}

impl Wait {
    /// The wait's length in microseconds, with its label when provisional.
    pub fn micros(
        &self,
        ceilings: &ReportedBounds,
        profile: &StandProfile,
    ) -> Result<Resolved, WaitError> {
        match &self.0 {
            Source::Contract { bound, factor } => {
                let ceiling = ceilings
                    .get(bound)
                    .ok_or_else(|| WaitError::NoBound(bound.clone()))?;
                Ok(Resolved {
                    micros: factor.apply(ceiling)?,
                    provisional: None,
                })
            }
            Source::Protocol {
                interval_micros,
                count,
            } => Ok(Resolved {
                micros: arithmetic::mul(*interval_micros, u64::from(count.get()))?,
                provisional: None,
            }),
            Source::Measured { span } => profile.budget(span),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::name;

    fn ratio(numerator: u32, denominator: u32) -> Factor {
        Factor::new(numerator, NonZeroU32::new(denominator).unwrap()).unwrap()
    }

    #[test]
    fn a_factor_below_one_is_rejected_and_an_overflow_is_reported() {
        assert_eq!(
            Factor::new(1, NonZeroU32::new(2).unwrap()),
            Err(InvalidFactor)
        );
        assert_eq!(ratio(3, 2).apply(1001), Ok(1502));
        assert_eq!(
            Factor::times(NonZeroU32::new(2).unwrap()).apply(u64::MAX),
            Err(Overflow)
        );
    }

    #[test]
    fn a_factor_below_one_is_refused_when_read() {
        assert_eq!(
            serde_json::from_str::<Factor>("[3,2]").unwrap(),
            ratio(3, 2)
        );
        assert!(serde_json::from_str::<Factor>("[1,2]").is_err());
        assert!(serde_json::from_str::<Factor>("[1,0]").is_err());
    }

    #[test]
    fn a_contract_wait_uses_the_ceiling() {
        let drain = name!(BoundName, "lower-mac.drain");
        let ceilings = ReportedBounds::default().with(drain.clone(), 4_000);
        let wait = Wait::contract(drain, Factor::times(NonZeroU32::new(2).unwrap()));
        assert_eq!(
            wait.micros(&ceilings, &StandProfile::empty()),
            Ok(Resolved {
                micros: 8_000,
                provisional: None
            })
        );
        let retune = name!(BoundName, "lower-mac.retune");
        assert_eq!(
            Wait::contract(retune.clone(), Factor::ONE).micros(&ceilings, &StandProfile::empty()),
            Err(WaitError::NoBound(retune))
        );
    }

    #[test]
    fn protocol_waits_come_from_typed_units() {
        let six = NonZeroU32::new(6).unwrap();
        let none = ReportedBounds::default();
        let profile = StandProfile::empty();
        let micros = |wait: Wait| wait.micros(&none, &profile).unwrap().micros;
        assert_eq!(
            micros(Wait::protocol(&ConnectionInterval(24), six).unwrap()),
            180_000
        );
        assert_eq!(
            micros(Wait::protocol(&TimeUnits(100), six).unwrap()),
            614_400
        );
        assert_eq!(micros(Wait::protocol(&Symbols(20), six).unwrap()), 1_920);
        let huge = Wait::protocol(&TimeUnits(u32::MAX), NonZeroU32::MAX).unwrap();
        assert_eq!(huge.micros(&none, &profile), Err(WaitError::Overflow));
    }
}
