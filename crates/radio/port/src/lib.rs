#![no_std]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

//! The vocabulary every radio port shares.
//!
//! A radio port (`docs/architecture.md`, "Radio ports") is the contract
//! between the portable logic of one radio protocol and the backend that
//! executes it. The IEEE 802.11 lower-MAC port, the IEEE 802.15.4 radio port
//! and the Bluetooth LE radio port each declare their own requests and
//! events; this package declares what they have in common, so a caller
//! handles any protocol's failures, losses and lifecycle alike:
//!
//! - [`FailureClass`] and [`PortError`]: every port error is `Rejected`,
//!   `Recoverable` or `Poisoned`.
//! - [`EventsLost`]: the one loss marker, with its ordering rule.
//! - [`Poisoned`]: the terminal event of a port whose backend state is
//!   unknown.
//! - [`LifecycleCommand`], [`LifecycleEvent`], [`LifecycleError`] and
//!   [`CancelError`]: enable, disable and quiesce with terminal events, and
//!   the refusal of a cancellation.
//! - [`Correlation`] and [`CorrelationIds`]: caller-chosen identities with
//!   one backend-reserved range.
//! - [`ClockInfo`] and [`RadioEpoch`]: the resolution of a port's radio
//!   clock and how its epoch relates to the image's monotonic time.
//!
//! # Event model
//!
//! Every port has exactly one consumer of its events: one task owns the
//! port's `next_event` (or `next_outcome`) and dispatches what it takes. A
//! second task that awaited the same stream would take events the first one
//! waits for; several exchanges share one port through a router that owns
//! the stream and hands each terminal event to the exchange whose identity
//! it carries.
//!
//! Taking an event only dequeues it. Timed work a backend runs in software
//! (backoffs, retry delays, watchdogs, retunes) progresses in the backend's
//! own runner future, which the composition polls for as long as the port
//! exists, next to the consumer and independently of it.
//!
//! This package owns no state and never waits.

use core::ops::RangeInclusive;

pub use oer_time::{Duration, Instant, RadioInstant};

/// How a failure left the port.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum FailureClass {
    /// The request was not admitted and nothing changed; the port stays
    /// usable. A port that is not installed, paused or disabled refuses
    /// work in this class: installing, resuming or enabling it serves again.
    Rejected,
    /// Admitted work ended without its result; the port stays usable in the
    /// state the terminal event names.
    Recoverable,
    /// The backend's state is unknown. The port reports [`Poisoned`] and
    /// refuses every later call until a reset restores it.
    Poisoned,
}

/// An error a port returns, with the class that tells the caller whether the
/// port still serves.
pub trait PortError {
    /// The failure class of this error.
    fn class(&self) -> FailureClass;

    /// Whether only a reset restores the port.
    fn is_poisoned(&self) -> bool {
        self.class() == FailureClass::Poisoned
    }
}

impl PortError for core::convert::Infallible {
    fn class(&self) -> FailureClass {
        match *self {}
    }
}

/// The backend dropped events it could not hold.
///
/// The marker takes the place of the first dropped event: every event
/// reported before it was produced before the gap, and every event reported
/// after it was produced after every dropped one. It is reported once per
/// gap, and never silently discarded, not even across an uninstall and a
/// later install of the backend.
///
/// A consumer may continue after it. Received data in the gap is gone; work
/// whose terminal event may have been lost is recovered by cancelling it by
/// its identity: the cancellation either produces the work's terminal event
/// again or is refused as [`CancelError::NotRunning`], which proves the work
/// already ended.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct EventsLost;

/// The terminal event of a poisoned port: the backend's state is unknown
/// and only a reset restores it.
///
/// A backend reports it after every event produced before the fault, and
/// from then on every call that takes an event returns it again, so a
/// consumer that awaits events learns of the fault. Every other call
/// returns the port's error, whose [`PortError::class`] is
/// [`FailureClass::Poisoned`].
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct Poisoned;

/// A lifecycle command; each admitted one ends with one [`LifecycleEvent`].
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum LifecycleCommand {
    /// Start admitting work and receiving; ends with
    /// [`LifecycleEvent::Enabled`] or [`LifecycleEvent::Failed`].
    Enable,
    /// Stop admitting work, end the work in flight and stop receiving; ends
    /// with [`LifecycleEvent::Disabled`] after the terminal event of every
    /// admitted operation.
    Disable,
    /// Stop admitting work and let the work in flight complete; ends with
    /// [`LifecycleEvent::Quiesced`] after the last terminal event.
    /// [`Self::Enable`] resumes admission.
    Quiesce,
}

/// The terminal event of a lifecycle command.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum LifecycleEvent {
    /// [`LifecycleCommand::Enable`] completed.
    Enabled,
    /// [`LifecycleCommand::Disable`] completed.
    Disabled,
    /// [`LifecycleCommand::Quiesce`] completed.
    Quiesced,
    /// An admitted command failed. A [`FailureClass::Recoverable`] failure
    /// leaves the port in the state it was in before the command; a
    /// [`FailureClass::Poisoned`] one is followed by [`Poisoned`].
    Failed {
        /// The command that failed.
        command: LifecycleCommand,
        /// How it left the port.
        class: FailureClass,
    },
}

/// Why a lifecycle command was refused; nothing changed.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum LifecycleError {
    /// The port is already in the requested state.
    AlreadyInState,
    /// The command does not apply in the port's current state, or the
    /// backend does not implement it.
    InvalidState,
    /// Another lifecycle command or an operation that owns the radio has
    /// not ended yet.
    Busy,
}

/// Why a cancellation was refused; nothing changed.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum CancelError {
    /// No admitted work has this identity: it already ended, and its
    /// terminal event was reported or lost, or it never ran.
    NotRunning,
}

/// A caller-chosen correlation identity of one port's submissions.
///
/// Each port keeps its own identity type (a Wi-Fi `TxId`, an
/// IEEE 802.15.4 `RequestId`, a Bluetooth LE `EventId`), so the identity of
/// one port cannot complete another's work; all of them are 32-bit values
/// with the same [`BACKEND_RESERVED`] range, which callers never choose and
/// a backend may use for work it submits on its own.
pub trait Correlation: Copy + Eq {
    /// The identity with this raw value.
    fn from_raw(raw: u32) -> Self;

    /// The raw value.
    fn raw(self) -> u32;

    /// Whether the identity lies in the backend's range.
    fn is_backend_reserved(self) -> bool {
        BACKEND_RESERVED.contains(&self.raw())
    }
}

/// Raw identities a caller never chooses: a backend may name its own work
/// with them, and a caller's allocator skips them.
pub const BACKEND_RESERVED: RangeInclusive<u32> = 0xFFFF_FF00..=u32::MAX;

/// The `index`-th backend-reserved identity; `None` beyond the range.
pub fn backend_reserved<I: Correlation>(index: u8) -> Option<I> {
    BACKEND_RESERVED
        .start()
        .checked_add(u32::from(index))
        .map(I::from_raw)
}

/// A caller's allocator of correlation identities: consecutive values that
/// wrap before [`BACKEND_RESERVED`].
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct CorrelationIds {
    next: u32,
}

impl Default for CorrelationIds {
    fn default() -> Self {
        Self::new()
    }
}

impl CorrelationIds {
    /// Identities from zero.
    pub const fn new() -> Self {
        Self::starting_at(0)
    }

    /// Identities from `first`; a reserved `first` starts at zero.
    pub const fn starting_at(first: u32) -> Self {
        Self {
            next: if first >= *BACKEND_RESERVED.start() {
                0
            } else {
                first
            },
        }
    }

    /// The next identity.
    #[allow(
        clippy::should_implement_trait,
        reason = "the allocator never ends and yields any identity type"
    )]
    pub fn next<I: Correlation>(&mut self) -> I {
        let id = self.next;
        self.next = match id.checked_add(1) {
            Some(next) if next < *BACKEND_RESERVED.start() => next,
            _ => 0,
        };
        I::from_raw(id)
    }
}

/// How a port's radio epoch relates to the image's monotonic time
/// (`oer_time::Instant`).
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum RadioEpoch {
    /// The radio clock is the image's monotonic clock: a radio instant and a
    /// monotonic instant with the same microseconds are the same time.
    Monotonic,
    /// The radio clock advances at the monotonic rate within `drift_ppm`
    /// parts per million, from an offset the port does not publish. A
    /// caller that reads both clocks together may project between them over
    /// a bounded span, widening by the drift.
    Affine {
        /// The largest rate difference, in parts per million.
        drift_ppm: u32,
    },
    /// No relation to the monotonic time is known: radio instants are
    /// compared only with other instants of the same port.
    Unrelated,
}

/// A port's radio clock: how finely it counts and which epoch it counts in.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ClockInfo {
    /// The smallest step two readings can differ by.
    pub resolution: Duration,
    /// The relation of the radio epoch to the monotonic time.
    pub epoch: RadioEpoch,
}

impl ClockInfo {
    /// A radio clock that is the image's monotonic clock, counting in
    /// microseconds.
    pub const MONOTONIC_MICROS: Self = Self {
        resolution: Duration::from_micros(1),
        epoch: RadioEpoch::Monotonic,
    };

    /// The monotonic instant of the radio instant `at` of this port.
    ///
    /// Exact for a [`RadioEpoch::Monotonic`] clock. An affine clock needs a
    /// paired reading of both clocks ([`Self::to_monotonic_with`]), and an
    /// unrelated clock has no monotonic instant at all.
    pub const fn to_monotonic<D>(self, at: RadioInstant<D>) -> Result<Instant, EpochError> {
        match self.epoch {
            RadioEpoch::Monotonic => Ok(Instant::from_micros(at.as_micros())),
            RadioEpoch::Affine { .. } => Err(EpochError::NeedsSample),
            RadioEpoch::Unrelated => Err(EpochError::Unrelated),
        }
    }

    /// The radio instant of this port at the monotonic instant `at`; the
    /// inverse of [`Self::to_monotonic`], with the same errors.
    pub const fn from_monotonic<D>(self, at: Instant) -> Result<RadioInstant<D>, EpochError> {
        match self.epoch {
            RadioEpoch::Monotonic => Ok(RadioInstant::from_micros(at.as_micros())),
            RadioEpoch::Affine { .. } => Err(EpochError::NeedsSample),
            RadioEpoch::Unrelated => Err(EpochError::Unrelated),
        }
    }
}

/// Why a port's radio instant has no monotonic counterpart.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum EpochError {
    /// The port's clock bears no known relation to the monotonic time.
    Unrelated,
    /// The port's clock is affine: converting needs a [`ClockSample`].
    NeedsSample,
    /// The stamp and the sample belong to different generations: the
    /// relation between the clocks broke between them.
    StaleSample,
    /// The converted instant lies outside the representable range.
    OutOfRange,
}

/// A radio instant with the generation of the clock relation it was taken
/// in.
///
/// A port's generation changes whenever the relation between its radio
/// clock and the monotonic clock breaks (a wake from sleep), so a stamp
/// converts only with a [`ClockSample`] of its own generation.
pub struct RadioStamp<D> {
    /// The radio instant.
    pub at: RadioInstant<D>,
    /// The relation `at` belongs to.
    pub generation: u32,
}

impl<D> Clone for RadioStamp<D> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<D> Copy for RadioStamp<D> {}

impl<D> PartialEq for RadioStamp<D> {
    fn eq(&self, other: &Self) -> bool {
        self.at == other.at && self.generation == other.generation
    }
}

impl<D> Eq for RadioStamp<D> {}

impl<D> core::fmt::Debug for RadioStamp<D> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("RadioStamp")
            .field("at", &self.at)
            .field("generation", &self.generation)
            .finish()
    }
}

/// One paired reading of a port's radio clock and the image's monotonic
/// clock, taken back to back by the backend.
///
/// A port returns one on demand, read when asked; `generation` is the
/// relation it was read in, the same counter its [`RadioStamp`]s carry.
pub struct ClockSample<D> {
    /// The radio clock's reading.
    pub radio: RadioInstant<D>,
    /// The monotonic clock's reading beside it.
    pub monotonic: Instant,
    /// How far apart the two readings may lie.
    pub uncertainty: Duration,
    /// The relation the sample belongs to; it changes whenever the relation
    /// between the clocks breaks.
    pub generation: u32,
}

impl<D> Clone for ClockSample<D> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<D> Copy for ClockSample<D> {}

impl<D> PartialEq for ClockSample<D> {
    fn eq(&self, other: &Self) -> bool {
        self.radio == other.radio
            && self.monotonic == other.monotonic
            && self.uncertainty == other.uncertainty
            && self.generation == other.generation
    }
}

impl<D> Eq for ClockSample<D> {}

impl<D> ClockSample<D> {
    /// The sample's radio reading as a stamp of its generation.
    pub const fn stamp(&self) -> RadioStamp<D> {
        RadioStamp {
            at: self.radio,
            generation: self.generation,
        }
    }
}

impl<D> core::fmt::Debug for ClockSample<D> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("ClockSample")
            .field("radio", &self.radio)
            .field("monotonic", &self.monotonic)
            .field("uncertainty", &self.uncertainty)
            .field("generation", &self.generation)
            .finish()
    }
}

/// A converted instant and how far the true instant may lie from it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Projected<T> {
    /// The converted instant.
    pub at: T,
    /// How far the true instant may lie from `at`, either way.
    pub uncertainty: Duration,
}

impl ClockInfo {
    /// The monotonic instant of the radio stamp `at`, projected from
    /// `sample`.
    ///
    /// Exact for a [`RadioEpoch::Monotonic`] clock, which ignores the
    /// sample. For an [`RadioEpoch::Affine`] clock the stamp must be of the
    /// sample's generation ([`EpochError::StaleSample`] otherwise); the
    /// radio distance from the sample is carried over unchanged, and the
    /// uncertainty is the sample's plus the drift over that distance,
    /// rounded up to whole microseconds. An unrelated clock has no monotonic
    /// instant.
    pub fn to_monotonic_with<D>(
        self,
        at: RadioStamp<D>,
        sample: &ClockSample<D>,
    ) -> Result<Projected<Instant>, EpochError> {
        match self.epoch {
            RadioEpoch::Monotonic => Ok(Projected {
                at: Instant::from_micros(at.at.as_micros()),
                uncertainty: Duration::ZERO,
            }),
            RadioEpoch::Affine { drift_ppm } => {
                if at.generation != sample.generation {
                    return Err(EpochError::StaleSample);
                }
                let at = at.at;
                let (at, distance) = shift(
                    sample.monotonic.as_micros(),
                    sample.radio.as_micros(),
                    at.as_micros(),
                )?;
                Ok(Projected {
                    at: Instant::from_micros(at),
                    uncertainty: drift_bound(sample.uncertainty, distance, drift_ppm)?,
                })
            }
            RadioEpoch::Unrelated => Err(EpochError::Unrelated),
        }
    }

    /// The radio stamp of the monotonic instant `at`, of the sample's
    /// generation, projected from `sample`; the inverse of
    /// [`Self::to_monotonic_with`], with the same uncertainty.
    pub fn from_monotonic_with<D>(
        self,
        at: Instant,
        sample: &ClockSample<D>,
    ) -> Result<Projected<RadioStamp<D>>, EpochError> {
        match self.epoch {
            RadioEpoch::Monotonic => Ok(Projected {
                at: RadioStamp {
                    at: RadioInstant::from_micros(at.as_micros()),
                    generation: sample.generation,
                },
                uncertainty: Duration::ZERO,
            }),
            RadioEpoch::Affine { drift_ppm } => {
                let (at, distance) = shift(
                    sample.radio.as_micros(),
                    sample.monotonic.as_micros(),
                    at.as_micros(),
                )?;
                Ok(Projected {
                    at: RadioStamp {
                        at: RadioInstant::from_micros(at),
                        generation: sample.generation,
                    },
                    uncertainty: drift_bound(sample.uncertainty, distance, drift_ppm)?,
                })
            }
            RadioEpoch::Unrelated => Err(EpochError::Unrelated),
        }
    }
}

/// `target_base` moved by `at - source_base`, and that distance.
fn shift(target_base: u64, source_base: u64, at: u64) -> Result<(u64, u64), EpochError> {
    if at >= source_base {
        let distance = at - source_base;
        target_base
            .checked_add(distance)
            .map(|at| (at, distance))
            .ok_or(EpochError::OutOfRange)
    } else {
        let distance = source_base - at;
        target_base
            .checked_sub(distance)
            .map(|at| (at, distance))
            .ok_or(EpochError::OutOfRange)
    }
}

/// `uncertainty` plus `drift_ppm` parts per million of `distance`, rounded
/// up.
fn drift_bound(
    uncertainty: Duration,
    distance: u64,
    drift_ppm: u32,
) -> Result<Duration, EpochError> {
    let drift = (u128::from(distance) * u128::from(drift_ppm)).div_ceil(1_000_000);
    let drift = u64::try_from(drift).map_err(|_| EpochError::OutOfRange)?;
    uncertainty
        .as_micros()
        .checked_add(drift)
        .map(Duration::from_micros)
        .ok_or(EpochError::OutOfRange)
}

#[cfg(test)]
mod tests;
