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
//! - [`RadioPortOwner`], [`RadioPortSplit`], [`RadioPortEvents`] and
//!   [`RadioPortClient`]: the one owner of an installed port, which takes
//!   exclusive operations on `&mut` and lends running phases
//!   ([`PhaseParts`]) of an event consumer, a phase control and
//!   interface-scoped clients.
//! - [`Poisoned`] and [`PortResult`]: the only outer error of a port call,
//!   carrying the backend's cause; every refusal is the inner `Err`.
//! - [`EventsLost`]: the one loss marker, with its ordering rule.
//! - [`LifecycleCommand`], [`LifecycleEvent`], [`LifecycleError`],
//!   [`CancelError`] and [`ClockError`]: enable, disable and quiesce with
//!   terminal events, and the shared refusals.
//! - [`Correlation`] and [`CorrelationIds`]: caller-chosen identities with
//!   one backend-reserved range.
//! - [`ClockInfo`]: the resolution of a port's radio clock and its
//!   [`ClockRelation`] to the image's monotonic time, if one is known.
//! - [`ClockRelation`], [`RelationSample`], [`Generation`] and [`Stamp`]:
//!   the one relation between two clock domains and its projection.
//!
//! # A port exists only while its backend is installed
//!
//! A backend's `install` returns its port value, for the protocol's one
//! event consumer, and a control, for the composition; the control's
//! `uninstall` consumes both, so no call reaches a backend that is not
//! installed: there is no "not installed" refusal. Install and uninstall
//! are a port's birth and death; [`LifecycleCommand`] states live inside an
//! installed port. Shared-PHY maintenance is neither: it never pauses a
//! port, it runs under the radio arbiter's lease, and an operation that
//! loses RF to it ends with its ordinary terminal event.
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

use oer_time::RadioInstant;

#[cfg(any(test, feature = "test-support"))]
pub mod conformance;
mod owner;
mod relation;

pub use owner::{PhaseParts, RadioPortClient, RadioPortEvents, RadioPortOwner, RadioPortSplit};
pub use relation::{
    ClockInfo, ClockRelation, ClockSample, Generation, GenerationSource, Projected,
    ProjectionError, RadioStamp, RelationRate, RelationSample, Stamp,
};

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
    /// The radio epoch has no later instant: the clock cannot advance, and
    /// no later call succeeds. The port still settles the work it admitted.
    EpochExhausted,
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

#[cfg(test)]
mod tests;
