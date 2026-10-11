//! The one authority over an installed port.
//!
//! An installed port has exactly one owner ([`RadioPortOwner`]), which is
//! neither `Clone` nor `Copy`. Every exclusive operation takes `&mut` on it:
//! the lifecycle, and each protocol's radio-wide settings (a Wi-Fi channel,
//! transmit gate or EDCA, an IEEE 802.15.4 channel, a Bluetooth LE
//! activity). Between running phases the owner also consumes the event
//! stream and cancels admitted work.
//!
//! For one running phase the owner lends the port out:
//! [`RadioPortOwner::split`] borrows it mutably and returns the phase's one
//! event consumer ([`RadioPortEvents`]) and the protocol's client handles,
//! each scoped to its interface ([`RadioPortClient`]). While any of them
//! lives, the borrow checker refuses every owner call, so an exclusive
//! operation never meets a client's work at run time and no backend refuses
//! one as busy. Dropping the handles ends the phase; work they admitted
//! stays in flight, and its terminal events reach the owner's stream.
//!
//! ```
//! # use oer_radio_port::{LifecycleCommand, RadioPortOwner};
//! async fn move_between_phases<O: RadioPortOwner>(owner: &mut O) {
//!     {
//!         let phase = owner.split();
//!         // The clients run, the events consumer dispatches.
//!         drop(phase);
//!     }
//!     // No client exists: the exclusive operation cannot overlap one.
//!     let _ = owner.lifecycle(LifecycleCommand::Disable).await;
//! }
//! ```
//!
//! An exclusive operation while a phase's handles live does not compile:
//!
//! ```compile_fail,E0499
//! # use oer_radio_port::{LifecycleCommand, RadioPortOwner};
//! async fn overlap<O: RadioPortOwner>(owner: &mut O) {
//!     let phase = owner.split();
//!     let _ = owner.lifecycle(LifecycleCommand::Disable).await;
//!     drop(phase);
//! }
//! ```
//!
//! Nor does a second phase beside the first, which would make two event
//! consumers:
//!
//! ```compile_fail,E0499
//! # use oer_radio_port::RadioPortOwner;
//! fn two_phases<O: RadioPortOwner>(owner: &mut O) {
//!     let first = owner.split();
//!     let second = owner.split();
//!     drop((first, second));
//! }
//! ```
//!
//! # Several executor tasks
//!
//! A borrow does not cross executor tasks, which need `'static` futures.
//! Where several tasks need one port, the owner is an actor: one task holds
//! it and its `&mut`, and the others send it requests and await its
//! replies. A handle shared between tasks over an internal mutex is not a
//! form of this contract. The actor's request and reply types are its
//! protocol's, and its channel the executor binding's; this package names
//! no executor.
//!
//! # Inside the backend
//!
//! The backend's interrupt entries and runner still reach its state beside
//! the handles; that synchronization stays inside the backend and never
//! reaches a caller as a refusal.

use core::{fmt::Debug, future::Future};

use oer_time::RadioInstant;

use crate::{
    CancelError, ClockError, Correlation, EventsLost, LifecycleCommand, LifecycleError, PortResult,
};

/// The one authority over an installed port; see the [module](self)
/// documentation.
///
/// A backend's `install` returns it, with the composition's control, and the
/// control's `uninstall` consumes it. Its implementations are neither
/// `Clone` nor `Copy`.
pub trait RadioPortOwner {
    /// One owned event.
    type Event;
    /// The correlation identity of the port's admitted work.
    type Id: Correlation;
    /// The clock domain of the port's radio instants.
    type Domain;
    /// The cause a poisoned backend reports.
    type Fault: Copy + Debug;
    /// The handles one running phase lends.
    type Split<'a>: RadioPortSplit<Fault = Self::Fault>
    where
        Self: 'a;

    /// The next event between running phases: the terminals of the owner's
    /// own commands, and of the work a phase left in flight. It keeps
    /// [`RadioPortEvents::next_event`]'s guarantees.
    fn next_event(
        &mut self,
    ) -> impl Future<Output = PortResult<Self::Event, EventsLost, Self::Fault>> + '_;

    /// The radio clock, as [`RadioPortClient::now`] reads it.
    fn now(
        &self,
    ) -> impl Future<Output = PortResult<RadioInstant<Self::Domain>, ClockError, Self::Fault>> + '_;

    /// End one admitted operation, as [`RadioPortClient::cancel`] does,
    /// whichever client admitted it.
    fn cancel(
        &mut self,
        id: Self::Id,
    ) -> impl Future<Output = PortResult<(), CancelError, Self::Fault>> + '_;

    /// Start one lifecycle command; its terminal
    /// [`LifecycleEvent`](crate::LifecycleEvent) follows through
    /// [`Self::next_event`].
    fn lifecycle(
        &mut self,
        command: LifecycleCommand,
    ) -> impl Future<Output = PortResult<(), LifecycleError, Self::Fault>> + '_;

    /// Lend the port for one running phase: its event consumer and its
    /// client handles, borrowing the owner until the last of them is
    /// dropped.
    fn split(&mut self) -> Self::Split<'_>;
}

/// The handles one running phase lends: its one event consumer and the
/// protocol's client handles.
pub trait RadioPortSplit {
    /// The cause a poisoned backend reports.
    type Fault: Copy + Debug;
    /// The phase's one event consumer.
    type Events: RadioPortEvents<Fault = Self::Fault>;
    /// The protocol's client handles, one per interface.
    type Clients;

    /// The event consumer and the client handles, as separate values.
    fn into_parts(self) -> (Self::Events, Self::Clients);
}

/// The one consumer of a running phase's events.
///
/// It is the stream's only consumer while the phase lasts: a router that
/// hands each terminal event to the exchange whose identity it carries.
pub trait RadioPortEvents {
    /// One owned event.
    type Event;
    /// The cause a poisoned backend reports.
    type Fault: Copy + Debug;

    /// The next event. Taking it only dequeues it; dropping the future
    /// loses no event. `Ok(Err(EventsLost))` takes the place of the first
    /// dropped event. A poisoned backend returns
    /// [`Poisoned`](crate::Poisoned) after every event it produced before
    /// the fault, and again at every later call.
    fn next_event(
        &mut self,
    ) -> impl Future<Output = PortResult<Self::Event, EventsLost, Self::Fault>> + '_;
}

/// What every client handle of a running phase does beside its protocol's
/// own submissions: read the radio clock and end its own work.
///
/// A client is scoped to its interface: its protocol's submissions and
/// settings name only that interface, so one client cannot act for another.
pub trait RadioPortClient {
    /// The correlation identity of the port's admitted work.
    type Id: Correlation;
    /// The clock domain of the port's radio instants.
    type Domain;
    /// The cause a poisoned backend reports.
    type Fault: Copy + Debug;

    /// The radio clock: the epoch of the port's scheduled work and receive
    /// timestamps, at the resolution its [`ClockInfo`](crate::ClockInfo)
    /// states.
    fn now(
        &self,
    ) -> impl Future<Output = PortResult<RadioInstant<Self::Domain>, ClockError, Self::Fault>> + '_;

    /// End one operation this client admitted. Its terminal event is the
    /// operation's own, which may be its natural end. A refusal as
    /// [`CancelError::NotRunning`] proves that the operation already ended,
    /// or that this client did not admit it.
    fn cancel(
        &self,
        id: Self::Id,
    ) -> impl Future<Output = PortResult<(), CancelError, Self::Fault>> + '_;
}
