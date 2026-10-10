#![no_std]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

//! The vocabulary every radio port shares.
//!
//! A radio port (`docs/architecture.md`, "Radio ports") is the contract
//! between the portable logic of one radio protocol and the backend that
//! executes it. The IEEE 802.11 lower-MAC port, the IEEE 802.15.4 radio port
//! and the Bluetooth LE radio port each extend [`RadioPort`] with their own
//! requests, events and capabilities; this package declares what they have
//! in common, so a caller handles any protocol's refusals, losses, poisoning
//! and lifecycle alike:
//!
//! - [`RadioPort`]: the base trait, with the event stream, the radio clock,
//!   cancellation and the lifecycle.
//! - [`Poisoned`] and [`PortResult`]: the only outer error of a port call,
//!   carrying the backend's cause; every refusal is the inner `Err`.
//! - [`EventsLost`]: the one loss marker, with its ordering rule.
//! - [`LifecycleCommand`], [`LifecycleEvent`], [`LifecycleError`],
//!   [`CancelError`] and [`ClockError`]: enable, disable and quiesce with
//!   terminal events, and the shared refusals.
//! - [`Correlation`] and [`CorrelationIds`]: caller-chosen identities with
//!   one backend-reserved range.
//! - [`ClockInfo`] and [`RadioEpoch`]: the resolution of a port's radio
//!   clock and how its epoch relates to the image's monotonic time.
//!
//! # A port exists only while its backend is installed
//!
//! A backend's `install` returns its port value, for the protocol's one
//! event consumer, and a control, for the composition; the control's
//! `uninstall` consumes both, so no call reaches a backend that is not
//! installed: there is no "not installed" refusal. Install and uninstall are a port's birth
//! and death; [`LifecycleCommand`] states live inside an installed port.
//! Maintenance that must stop the radio, such as shared PHY tracking, is a
//! layer over [`LifecycleCommand::Quiesce`], not a removal of the backend.
//!
//! A port's event stream lives as long as the port. Handing the port to
//! `uninstall` ends it: the events it did not deliver, the terminal events
//! still owed and a pending loss are discarded, and the next install's port
//! starts with an empty stream. Every guarantee about events (a terminal
//! event is never lost, a loss is reported once) holds for the port's
//! lifetime.
//!
//! # Event model
//!
//! Every port has exactly one consumer of its events: one task owns the
//! port's [`RadioPort::next_event`] and dispatches what it takes. A second
//! task that awaited the same stream would take events the first one waits
//! for; several exchanges share one port through a router that owns the
//! stream and hands each terminal event to the exchange whose identity it
//! carries.
//!
//! Taking an event only dequeues it. Timed work a backend runs in software
//! (backoffs, retry delays, watchdogs, retunes) progresses in the backend's
//! own runner future, which the composition polls for as long as the port
//! exists, next to the consumer and independently of it.
//!
//! This package owns no state and never waits.

use core::{fmt::Debug, future::Future, ops::RangeInclusive};

use oer_time::{Duration, Instant, RadioInstant};

/// The base of every radio port: its event stream, its radio clock,
/// cancellation of admitted work and its lifecycle.
///
/// Each protocol's port extends it with its submission, the portable view
/// of its events and its capabilities. Every call returns a
/// [`PortResult`]: the inner `Err` is a refusal (nothing changed, the port
/// serves on), the outer one is [`Poisoned`] (the backend's state is
/// unknown and only a reset restores it). The port value exists only while
/// its backend is installed, so no call is refused as not installed.
///
/// The calls are asynchronous because a backend may need a bounded wait for
/// its hardware to answer (a fresh clock latch, a radio its runner holds
/// across a stop sequence); the wait never depends on another call or on an
/// event being taken. A backend that decides at once returns a ready
/// future.
///
/// A backend reserves the slot of every event whose loss would break a
/// guarantee: an admitted operation's terminal event, and data its protocol
/// promises the peer to deliver once the hardware acknowledged it (Bluetooth
/// LE connection data). It reserves them when it admits the work, so they
/// are never lost: an [`EventsLost`] only ever stands for data the protocol
/// does not promise to deliver (received frames, advertising reports), and
/// a consumer continues after it.
pub trait RadioPort {
    /// One owned event.
    type Event;
    /// The correlation identity of the port's admitted work.
    type Id: Correlation;
    /// The clock domain of the port's radio instants.
    type Domain;
    /// The cause a poisoned backend reports.
    type Fault: Copy + Debug;

    /// The next event. Taking it only dequeues it; dropping the future
    /// loses no event. `Ok(Err(EventsLost))` takes the place of the first
    /// dropped event. A poisoned backend returns [`Poisoned`] after every
    /// event it produced before the fault, and again at every later call.
    fn next_event(
        &self,
    ) -> impl Future<Output = PortResult<Self::Event, EventsLost, Self::Fault>> + '_;

    /// The radio clock: the epoch of the port's scheduled work and receive
    /// timestamps, at the resolution its [`ClockInfo`] states.
    fn now(
        &self,
    ) -> impl Future<Output = PortResult<RadioInstant<Self::Domain>, ClockError, Self::Fault>> + '_;

    /// End one admitted operation. Its terminal event is the operation's
    /// own, which may be its natural end. A refusal as
    /// [`CancelError::NotRunning`] proves that the operation already ended.
    fn cancel(
        &self,
        id: Self::Id,
    ) -> impl Future<Output = PortResult<(), CancelError, Self::Fault>> + '_;

    /// Start one lifecycle command; its terminal [`LifecycleEvent`] follows
    /// through [`Self::next_event`].
    fn lifecycle(
        &self,
        command: LifecycleCommand,
    ) -> impl Future<Output = PortResult<(), LifecycleError, Self::Fault>> + '_;
}

/// The result of a port call: `Ok(Ok(_))` served, `Ok(Err(refusal))`
/// refused with nothing changed, `Err(Poisoned)` a poisoned backend.
pub type PortResult<T, R, C> = Result<Result<T, R>, Poisoned<C>>;

/// The backend dropped events it could not hold.
///
/// The marker takes the place of the first dropped event: every event
/// reported before it was produced before the gap, and every event reported
/// after it was produced after every dropped one. It is reported once per
/// gap, and never silently discarded while the port lives; a gap the port's
/// uninstall leaves ends with its stream.
///
/// A backend reserves the slot of every terminal event, and of data its
/// protocol promises to deliver, when it admits the work, so the gap holds
/// only data the protocol does not promise (received frames, advertising
/// reports), which is gone; the consumer continues after it.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct EventsLost;

/// A poisoned backend: its state is unknown and only a reset restores it.
///
/// It is the only outer error of a port call. [`RadioPort::next_event`]
/// returns it after every event produced before the fault and again at
/// every later call, so a consumer that awaits events learns of the fault;
/// every other call returns it at once. `cause` is the backend's own
/// account of the fault, which portable code passes on without
/// interpreting it.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Poisoned<C> {
    /// Why the backend's state is unknown.
    pub cause: C,
}

/// Why the radio clock could not be read; nothing changed.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ClockError {
    /// The backend could not take a reading now (a latch failed, the clock
    /// belongs to a replaced radio start); a later call may succeed.
    Unavailable,
}

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
    /// An admitted command failed and left the port in the state it was in
    /// before the command. A failure that leaves the backend's state
    /// unknown is not reported here: [`Poisoned`] follows instead.
    Failed {
        /// The command that failed.
        command: LifecycleCommand,
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
    /// Another lifecycle command has not ended yet.
    Busy,
}

/// Why a cancellation was refused; nothing changed.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum CancelError {
    /// No admitted work has this identity: it already ended and its terminal
    /// event was reported, or it never ran.
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
}

/// Why a port's radio instant has no monotonic counterpart.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum EpochError {
    /// The port's clock bears no known relation to the monotonic time.
    Unrelated,
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
