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

pub use oer_time::{Duration, RadioInstant};

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
}

#[cfg(test)]
mod tests;
