//! The one authority over an installed port.
//!
//! An installed port has exactly one owner ([`RadioPortOwner`]), which is
//! neither `Clone` nor `Copy`. The operations that conflict with work in
//! flight take `&mut` on it, between running phases: the lifecycle and a
//! protocol's permanent radio moves where it has them (a Wi-Fi channel
//! move; an IEEE 802.15.4 request names its own channel, so that port has
//! none).
//! Between phases the owner also consumes the event stream and ends any
//! admitted work.
//!
//! For one running phase the owner lends the port out:
//! [`RadioPortOwner::split`] borrows it mutably and returns three parts
//! ([`PhaseParts`]), each a [`Lent`] that carries that borrow:
//!
//! - the phase's one event consumer ([`RadioPortEvents`]);
//! - the phase control, one value that is neither `Clone` nor `Copy`, for
//!   the radio-wide operations that only gate or tune admission while
//!   clients run (a Wi-Fi transmit gate, beacon-receive priority, EDCA or
//!   modem-sleep membership), held by the policy that drives them;
//! - the protocol's client handles, each scoped to its interface
//!   ([`RadioPortClient`]); their count is fixed at the split and may follow
//!   the backend's capabilities (one IEEE 802.15.4 interface, or up to four
//!   with multi-PAN).
//!
//! The parts are disjoint resources, each with one holder: a client owns
//! its own work, the control owns the policy of what reaches the air next,
//! and neither can act for the other. Each kind of client names its work
//! with its own identity type ([`RadioPortClient::Id`]), which the client
//! chooses, as a sans-IO core does: another client's identity does not
//! type-check, and a port with one client simply uses its identity.
//!
//! Nothing of the port reaches another executor task but requests to its
//! actor. A measurement the backend publishes (an IEEE 802.15.4 recent
//! RSSI) is not part of the port: as the radio clock, it is the backend's
//! own published value, with one writer and readers of an atomic snapshot.
//!
//! # The phase control
//!
//! The control has one holder, the policy that arbitrates admission (for
//! Wi-Fi, the service that pairs the station and the access point); the
//! other policies that need it (a station's power save, coexistence slices)
//! send that holder requests and never hold the control themselves.
//!
//! None of its operations refuses as busy. An operation that stops
//! admission takes effect at once, also when its future is dropped; the
//! work already on the air ends by itself, and the operation's future
//! resolves to the protocol's typed proof that the air is quiet (for
//! Wi-Fi, closing the transmit gate yields a proof that no attempt is
//! published), which only the backend constructs. A policy that needs a
//! quiet air (a power-save frame announcing sleep) takes that proof
//! instead of retrying a refusal.
//!
//! The borrow is in the contract's signature, not in the backend's types:
//! `split(&mut self)` returns `PhaseParts<'_, …>`, and [`Lent`] is
//! invariant in its lifetime, so an implementation cannot hand out parts
//! that outlive the phase. A part comes out of its loan only by exchange
//! for a second value of its type, and a backend's part types have no
//! value outside it (no `Default`, `Clone`, `Copy` or public constructor),
//! which each backend proves with compile-fail tests ([`Lent`]).
//!
//! While any of them lives, the borrow checker refuses every owner call, so
//! an operation that conflicts with work in flight never meets a client's
//! work at run time and no backend refuses one as busy. Dropping the parts
//! ends the phase; work the clients admitted stays in flight, and its
//! terminal events, with what they return (a Wi-Fi attempt's bodies), reach
//! the owner's stream: the owner is where they go back to their pools.
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
//! Nor does a part kept past its phase:
//!
//! ```compile_fail,E0499
//! # use oer_radio_port::{LifecycleCommand, RadioPortOwner};
//! async fn keep_the_control<O: RadioPortOwner>(owner: &mut O) {
//!     let control = owner.split().control;
//!     let _ = owner.lifecycle(LifecycleCommand::Disable).await;
//!     drop(control);
//! }
//! ```
//!
//! Nor an implementation whose parts would outlive the phase:
//!
//! ```compile_fail
//! # use core::future::{Future, Ready, ready};
//! # use oer_radio_port::*;
//! # struct Owner; struct Events;
//! # impl RadioPortEvents for Events {
//! #     type Event = (); type Fault = ();
//! #     fn next_event(&mut self) -> impl Future<Output = PortResult<(), EventsLost, ()>> + '_ { ready(Ok(Err(EventsLost))) }
//! # }
//! impl RadioPortOwner for Owner {
//! #   type Event = (); type Id = (); type Domain = (); type Fault = ();
//! #   type Events = Events; type Control = (); type Clients = ();
//! #   fn next_event(&mut self) -> impl Future<Output = PortResult<(), EventsLost, ()>> + '_ { ready(Ok(Err(EventsLost))) }
//! #   fn now(&self) -> impl Future<Output = PortResult<oer_time::RadioInstant<()>, ClockError, ()>> + '_ { ready(Ok(Err(ClockError::Unavailable))) }
//! #   fn cancel(&mut self, _: ()) -> impl Future<Output = PortResult<(), CancelError, ()>> + '_ { ready(Ok(Ok(()))) }
//! #   fn lifecycle(&mut self, _: LifecycleCommand) -> impl Future<Output = PortResult<(), LifecycleError, ()>> + '_ { ready(Ok(Ok(()))) }
//!     // The parts claim a loan longer than the borrow of `self`.
//!     fn split(&mut self) -> PhaseParts<'static, Events, (), ()> {
//!         PhaseParts {
//!             events: Lent::new(Events),
//!             control: Lent::new(()),
//!             clients: Lent::new(()),
//!         }
//!     }
//! }
//! ```
//!
//! Its compiling twin lends the parts for the borrow of `self`:
//!
//! ```
//! # use core::future::{Future, Ready, ready};
//! # use oer_radio_port::*;
//! # struct Owner; struct Events;
//! # impl RadioPortEvents for Events {
//! #     type Event = (); type Fault = ();
//! #     fn next_event(&mut self) -> impl Future<Output = PortResult<(), EventsLost, ()>> + '_ { ready(Ok(Err(EventsLost))) }
//! # }
//! impl RadioPortOwner for Owner {
//! #   type Event = (); type Id = (); type Domain = (); type Fault = ();
//! #   type Events = Events; type Control = (); type Clients = ();
//! #   fn next_event(&mut self) -> impl Future<Output = PortResult<(), EventsLost, ()>> + '_ { ready(Ok(Err(EventsLost))) }
//! #   fn now(&self) -> impl Future<Output = PortResult<oer_time::RadioInstant<()>, ClockError, ()>> + '_ { ready(Ok(Err(ClockError::Unavailable))) }
//! #   fn cancel(&mut self, _: ()) -> impl Future<Output = PortResult<(), CancelError, ()>> + '_ { ready(Ok(Ok(()))) }
//! #   fn lifecycle(&mut self, _: LifecycleCommand) -> impl Future<Output = PortResult<(), LifecycleError, ()>> + '_ { ready(Ok(Ok(()))) }
//!     fn split(&mut self) -> PhaseParts<'_, Events, (), ()> {
//!         PhaseParts {
//!             events: Lent::new(Events),
//!             control: Lent::new(()),
//!             clients: Lent::new(()),
//!         }
//!     }
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

use core::{
    fmt::Debug,
    future::Future,
    marker::PhantomData,
    ops::{Deref, DerefMut},
};

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
    /// The port's identity of admitted work: the client that admitted it
    /// together with that client's identity, as the backend keys the work,
    /// so two clients of one kind that chose the same value stay apart.
    /// A client handle forms it ([`RadioPortClient::port_id`]), and
    /// terminal events carry it.
    type Id: Copy + Eq;
    /// The clock domain of the port's radio instants.
    type Domain;
    /// The cause a poisoned backend reports.
    type Fault: Copy + Debug;
    /// The phase's one event consumer.
    type Events: RadioPortEvents<Event = Self::Event, Fault = Self::Fault>;
    /// The phase control: the radio-wide operations that only gate or tune
    /// admission while clients run. Neither `Clone` nor `Copy`, with one
    /// holder; none of its operations refuses as busy, and one that stops
    /// admission yields a typed proof of a quiet air (see the
    /// [module](self) documentation).
    type Control;
    /// The protocol's client handles, one per interface; their count is
    /// fixed at the split. Neither `Clone` nor `Copy`.
    type Clients;

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
    ///
    /// The owner runs one command at a time, and the backend keeps one
    /// terminal slot reserved for it, so neither cause of
    /// [`LifecycleError::Busy`] (another command, no slot for the terminal)
    /// reaches the owner: a backend never refuses its command as busy.
    /// The variant leaves the contract with [`RadioPort`](crate::RadioPort)'s
    /// lifecycle in #581.
    fn lifecycle(
        &mut self,
        command: LifecycleCommand,
    ) -> impl Future<Output = PortResult<(), LifecycleError, Self::Fault>> + '_;

    /// Lend the port for one running phase: its event consumer, its control
    /// and its client handles ([`PhaseParts`]), each a [`Lent`] of this
    /// borrow of the owner, which lasts until the last of them is dropped.
    fn split(&mut self) -> PhaseParts<'_, Self::Events, Self::Control, Self::Clients>;
}

/// The parts of one running phase ([`RadioPortOwner::split`]), each lent
/// for the phase.
pub struct PhaseParts<'a, E, C, K> {
    /// The phase's one event consumer.
    pub events: Lent<'a, E>,
    /// The phase control, with one holder.
    pub control: Lent<'a, C>,
    /// The protocol's client handles, one per interface.
    pub clients: Lent<'a, K>,
}

/// One part of a running phase, lent for the borrow `'a` of the port's
/// owner. It reaches its part by shared or mutable reference
/// (`Deref`, `DerefMut`) and has no method that gives the part up. `'a` is
/// invariant, so a loan cannot be stretched beyond the borrow that made
/// it.
///
/// A mutable reference could still exchange the part for a second value
/// of its type (`core::mem::take`, `core::mem::replace`). A backend
/// therefore gives its part types no value outside itself: no `Default`,
/// `Clone` or `Copy` and no public constructor (multi-PAN clients go in
/// the backend's own collection type, not a bare `Default` one). Each
/// backend proves that of its own part types with compile-fail tests, so
/// the part lives no longer than the phase.
pub struct Lent<'a, T> {
    part: T,
    loan: PhantomData<fn(&'a ()) -> &'a ()>,
}

impl<T> Lent<'_, T> {
    /// Lend `part` for the phase the owner's `split` opens.
    pub const fn new(part: T) -> Self {
        Self {
            part,
            loan: PhantomData,
        }
    }
}

impl<T> Deref for Lent<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.part
    }
}

impl<T> DerefMut for Lent<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.part
    }
}

/// The one consumer of a running phase's events.
///
/// It is the stream's only consumer while the phase lasts: a router that
/// hands each terminal event to the exchange whose port identity (client
/// and identity, [`RadioPortOwner::Id`]) it carries.
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
/// Each kind of client names its work with its own identity type, which it
/// chooses, so a client names only its own work, by type.
///
/// Clients of one kind (IEEE 802.15.4 multi-PAN interfaces, two Wi-Fi
/// interfaces of one role) share the identity type and choose its values
/// independently. The backend therefore keys admitted work by the client
/// and its identity: two clients of one kind may choose the same value
/// without colliding, and one client's cancel never reaches another's
/// work, so [`CancelError::NotRunning`] proves that this client's work
/// ended.
pub trait RadioPortClient {
    /// The correlation identity of this client's work: its own type, so
    /// another kind of client's identity does not type-check.
    type Id: Correlation;
    /// The port's identity of this client's work: the owner's
    /// [`RadioPortOwner::Id`].
    type PortId: Copy + Eq;
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
    /// [`CancelError::NotRunning`] proves that the operation already ended:
    /// the air ends work, so its end is an outcome, not a caller's state.
    fn cancel(
        &self,
        id: Self::Id,
    ) -> impl Future<Output = PortResult<(), CancelError, Self::Fault>> + '_;

    /// The port's identity of this client's work `id`: this client with its
    /// identity. The client handle forms it, as only it knows which client
    /// it is; the owner ends a work a phase left in flight by it, and a
    /// router matches terminal events against it.
    fn port_id(&self, id: Self::Id) -> Self::PortId;
}
