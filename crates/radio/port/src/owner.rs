//! The one authority over an installed port, as types.
//!
//! Every rule a caller controls is a type here, so breaking it does not
//! compile ([ownership and refusals]); only what the world controls (the
//! air, time, the hardware, a peer, the arbiter) stays a typed outcome.
//!
//! # Shape
//!
//! A backend implements [`PortBackend`]; its install wraps the backend in an
//! [`Owner`] in the [`Disabled`] state, and no caller ever holds the backend
//! itself. The owner's state is its type:
//!
//! ```text
//! Owner<B, Disabled> ── enable(self) ──▶ Enabling<B> ── next() … ─▶ Owner<B, Enabled>
//!        ▲                                         └──────────────▶ Owner<B, Disabled> + failure
//!        └──────────── Disabling<B> ◀── disable(self) ── Owner<B, Enabled>
//! any step that meets a poisoned backend ──▶ PoisonedOwner<B>
//! ```
//!
//! - [`Owner<B, Disabled>`](Owner) lends the protocol's disabled-only
//!   settings ([`Owner::setup`]: a permanent channel move, interface
//!   configuration), and is the only state the backend's uninstall takes.
//! - [`Owner<B, Enabled>`](Owner) lends one running phase at a time
//!   ([`Owner::split`]): the phase's one event consumer, its control and
//!   its interface-scoped clients, each a [`Lent`] of the owner's borrow.
//! - [`Enabling`] and [`Disabling`] are the transitions. Their
//!   [`next`](Enabling::next) yields the events the port reports before the
//!   command's end, the leftover terminals of a phase's work included, and
//!   then the next owner. They hold the backend between calls, so dropping
//!   a `next` future loses nothing.
//! - [`PoisonedOwner`] is the one exit of a poisoned backend: its cause and
//!   the backend's uninstall.
//!
//! Rust has no linear types: dropping an owner, a transition or a poisoned
//! owner abandons the backend without its uninstall, and the hardware stays
//! as it was. Each is `#[must_use]`.
//!
//! A command never meets another command, a phase never meets a lifecycle
//! command and a disabled-only setting never meets an enabled port: there is
//! no lifecycle refusal and no "port enabled" or "port disabled" refusal.
//! A failed `Enable` (the radio refused it) is the world's outcome and
//! leaves an `Owner<B, Disabled>`.
//!
//! # What types guarantee
//!
//! | Guarantee | Enforced by |
//! | --- | --- |
//! | A disabled port lends no phase; an enabled one no disabled-only setting | the state parameter |
//! | One lifecycle command at a time; no phase or setting while one runs | `enable`/`disable` take the owner by value |
//! | A phase's parts end before any owner call or transition | [`Lent`], invariant in the owner's borrow |
//! | One running phase, one event consumer | `split(&mut self)` |
//! | A caller reaches the backend only through the owner and its parts | the owner's private field |
//! | A transition is not resolved before its end | `next` alone hands out the next owner |
//! | A part is not taken out of its loan | `Lent` gives no part away; see below |
//!
//! Each line has a compile-fail test below or in the crate's tests.
//!
//! # What each backend proves
//!
//! The language cannot express two rules, so every backend proves them of
//! its own types with compile-fail tests:
//!
//! - its backend, its phase parts and every token it mints (admitted work,
//!   leases, installed keys and agreements, validated values) are not
//!   `Clone`, `Copy` or `Default` and have no public constructor, so a
//!   mutable reference cannot exchange a part out of its loan;
//! - everything that can act on the port borrows the phase part it came
//!   from, so it lives no longer than the phase; received data and returned
//!   bodies act on nothing and may outlive it.
//!
//! # Several executor tasks
//!
//! A borrow does not cross executor tasks, which need `'static` futures.
//! Where several tasks need one port, the owner is an actor: one task holds
//! it, and the others send it requests and await its replies. A handle
//! shared between tasks over an internal mutex is not a form of this
//! contract.
//!
//! # Inside the backend
//!
//! The backend's interrupt entries and runner reach its state beside the
//! owner; that synchronization stays inside the backend and never reaches a
//! caller as a refusal.
//!
//! # Compile-fail tests
//!
//! A disabled port lends no phase:
//!
//! ```compile_fail,E0599
//! # use oer_radio_port::{Disabled, Owner, PortBackend};
//! fn phase_while_disabled<B: PortBackend>(owner: &mut Owner<B, Disabled>) {
//!     let _ = owner.split();
//! }
//! ```
//!
//! An enabled port takes no disabled-only setting:
//!
//! ```compile_fail,E0599
//! # use oer_radio_port::{Enabled, Owner, PortBackend};
//! fn setup_while_enabled<B: PortBackend>(owner: &mut Owner<B, Enabled>) {
//!     let _ = owner.setup();
//! }
//! ```
//!
//! An owner is gone once its command starts, so a second command cannot
//! start beside it:
//!
//! ```compile_fail,E0382
//! # use oer_radio_port::{Disabled, Owner, PortBackend};
//! fn two_commands<B: PortBackend>(owner: Owner<B, Disabled>) {
//!     let _enabling = owner.enable();
//!     let _ = owner.enable();
//! }
//! ```
//!
//! A phase's part does not outlive the phase:
//!
//! ```compile_fail,E0505
//! # use oer_radio_port::{Enabled, Owner, PortBackend};
//! fn keep_the_control<B: PortBackend>(mut owner: Owner<B, Enabled>) {
//!     let control = owner.split().control;
//!     let _disabling = owner.disable();
//!     drop(control);
//! }
//! ```
//!
//! Nor does a second phase run beside the first:
//!
//! ```compile_fail,E0499
//! # use oer_radio_port::{Enabled, Owner, PortBackend};
//! fn two_phases<B: PortBackend>(owner: &mut Owner<B, Enabled>) {
//!     let first = owner.split();
//!     let second = owner.split();
//!     drop((first, second));
//! }
//! ```
//!
//! A caller does not reach the backend past the owner:
//!
//! ```compile_fail,E0616
//! # use oer_radio_port::{Enabled, Owner, PortBackend};
//! fn reach_the_backend<B: PortBackend>(owner: &mut Owner<B, Enabled>) -> &mut B {
//!     &mut owner.backend
//! }
//! ```
//!
//! Nor takes a part out of its loan:
//!
//! ```compile_fail,E0616
//! # use oer_radio_port::{Enabled, Owner, PortBackend};
//! fn take_the_part<B: PortBackend>(owner: &mut Owner<B, Enabled>) {
//!     let _ = owner.split().control.part;
//! }
//! ```
//!
//! Nor does admitted work outlive the client that admitted it, so no token
//! names the work of a later phase:
//!
//! ```compile_fail,E0515
//! # use oer_radio_port::{RadioPortClient, Work};
//! fn keep_the_work<C: RadioPortClient>(
//!     admit: impl for<'c> Fn(&'c C) -> Work<'c, C::Token>,
//!     client: C,
//! ) -> Work<'static, C::Token> {
//!     admit(&client)
//! }
//! ```
//!
//! Its compiling twin keeps the work only as long as the client:
//!
//! ```
//! # use oer_radio_port::{RadioPortClient, Work};
//! fn keep_the_work<'c, C: RadioPortClient>(
//!     admit: impl Fn(&'c C) -> Work<'c, C::Token>,
//!     client: &'c C,
//! ) -> Work<'c, C::Token> {
//!     admit(client)
//! }
//! ```
//!
//! Each has a compiling twin:
//!
//! ```
//! # use oer_radio_port::{Disabled, EnableStep, Owner, PortBackend};
//! async fn phase_after_enable<B: PortBackend>(mut owner: Owner<B, Disabled>) {
//!     let _ = owner.setup();
//!     let Ok(mut enabling) = owner.enable() else { return };
//!     let mut enabled = loop {
//!         match enabling.next().await {
//!             EnableStep::Event(_) => {}
//!             EnableStep::Enabled(enabled) => break enabled,
//!             EnableStep::Failed(..) | EnableStep::Poisoned(_) => return,
//!         }
//!     };
//!     {
//!         let phase = enabled.split();
//!         drop(phase);
//!     }
//!     let _disabling = enabled.disable();
//! }
//! ```
//!
//! [ownership and refusals]: https://github.com/ermacv/open-esp-radio-rs/blob/main/docs/architecture.md#ownership-and-refusals

use core::{
    fmt::Debug,
    future::Future,
    marker::PhantomData,
    ops::{Deref, DerefMut},
};

use oer_time::RadioInstant;

use crate::{CancelError, ClockError, EventsLost, Poisoned, PortResult};

/// The state of an owner whose port admits no work and receives nothing.
pub enum Disabled {}

/// The state of an owner whose port runs.
pub enum Enabled {}

/// What a backend implements; callers never hold it. A backend's install
/// wraps it with [`Owner::install`], which a caller cannot do without a
/// backend value.
///
/// The methods are the backend's side of the [`Owner`]'s transitions and
/// loans; the owner calls each only in the state its documentation names.
pub trait PortBackend: Sized {
    /// One owned event.
    type Event;
    /// The clock domain of the port's radio instants.
    type Domain;
    /// The cause a poisoned backend reports.
    type Fault: Copy + Debug;
    /// Why the radio did not enable (a refused retune): the world's
    /// outcome, after which the port is disabled.
    type EnableFailure;
    /// The protocol's disabled-only settings, lent by [`Owner::setup`].
    type Setup<'a>
    where
        Self: 'a;
    /// The phase's one event consumer.
    type Events<'a>: RadioPortEvents<Event = Self::Event, Fault = Self::Fault>
    where
        Self: 'a;
    /// The phase control: the radio-wide operations that only gate or tune
    /// admission while clients run, as guards on its sub-resources.
    type Control<'a>
    where
        Self: 'a;
    /// The protocol's client handles, one per configured interface.
    type Clients<'a>
    where
        Self: 'a;
    /// What the backend's uninstall takes beside it (the composition's
    /// control) and what it returns (its hardware owners).
    type Uninstall;
    /// The hardware owners the uninstall returns.
    type Uninstalled;

    /// Admit `Enable` on a disabled port.
    fn admit_enable(&mut self) -> Result<(), Poisoned<Self::Fault>>;

    /// Admit `Disable` on an enabled port, which ends every admitted work
    /// with its terminal event first.
    fn admit_disable(&mut self) -> Result<(), Poisoned<Self::Fault>>;

    /// The next step of an admitted `Enable`: an event, or its end.
    fn next_enabling(
        &mut self,
    ) -> impl Future<
        Output = Result<Step<Self::Event, Result<(), Self::EnableFailure>>, Poisoned<Self::Fault>>,
    > + '_;

    /// The next step of an admitted `Disable`: an event (a leftover terminal
    /// with its bodies), or its end.
    fn next_disabling(
        &mut self,
    ) -> impl Future<Output = Result<Step<Self::Event, ()>, Poisoned<Self::Fault>>> + '_;

    /// The radio clock.
    fn now(
        &self,
    ) -> impl Future<Output = PortResult<RadioInstant<Self::Domain>, ClockError, Self::Fault>> + '_;

    /// Lend the disabled-only settings.
    fn setup(&mut self) -> Lent<'_, Self::Setup<'_>>;

    /// Lend one running phase of an enabled port.
    fn split(&mut self) -> PhaseParts<'_, Self::Events<'_>, Self::Control<'_>, Self::Clients<'_>>;

    /// Take the disabled or poisoned backend apart.
    fn uninstall(self, control: Self::Uninstall) -> Self::Uninstalled;
}

/// One step of a backend's transition: an event before the command's end,
/// or the end.
pub enum Step<E, R> {
    /// An event the port reports before the command ends.
    Event(E),
    /// The command ended.
    Ended(R),
}

/// The one authority over an installed port, in state `S`; see the
/// [module](self) documentation.
#[must_use = "an owner leaves only through a transition or its backend's uninstall"]
pub struct Owner<B, S> {
    backend: B,
    state: PhantomData<S>,
}

impl<B: PortBackend> Owner<B, Disabled> {
    /// Wrap a just-installed, disabled backend. A backend's install calls
    /// it; a caller has no backend value to pass.
    pub const fn install(backend: B) -> Self {
        Self {
            backend,
            state: PhantomData,
        }
    }

    /// Lend the protocol's disabled-only settings.
    pub fn setup(&mut self) -> Lent<'_, B::Setup<'_>> {
        self.backend.setup()
    }

    /// Start `Enable`; [`Enabling::next`] ends it.
    ///
    /// # Errors
    ///
    /// A poisoned backend, which only its uninstall leaves.
    #[allow(clippy::result_large_err, reason = "the owner moves on by value")]
    pub fn enable(mut self) -> Result<Enabling<B>, PoisonedOwner<B>> {
        match self.backend.admit_enable() {
            Ok(()) => Ok(Enabling {
                backend: Some(self.backend),
            }),
            Err(poisoned) => Err(PoisonedOwner::new(self.backend, poisoned)),
        }
    }

    /// Take the disabled backend apart with the composition's `control`.
    pub fn uninstall(self, control: B::Uninstall) -> B::Uninstalled {
        self.backend.uninstall(control)
    }
}

impl<B: PortBackend> Owner<B, Enabled> {
    /// Lend one running phase: its event consumer, its control and its
    /// clients, each a [`Lent`] of this borrow.
    pub fn split(&mut self) -> PhaseParts<'_, B::Events<'_>, B::Control<'_>, B::Clients<'_>> {
        self.backend.split()
    }

    /// Start `Disable`; [`Disabling::next`] yields the leftover terminals
    /// and ends it.
    ///
    /// # Errors
    ///
    /// A poisoned backend, which only its uninstall leaves.
    #[allow(clippy::result_large_err, reason = "the owner moves on by value")]
    pub fn disable(mut self) -> Result<Disabling<B>, PoisonedOwner<B>> {
        match self.backend.admit_disable() {
            Ok(()) => Ok(Disabling {
                backend: Some(self.backend),
            }),
            Err(poisoned) => Err(PoisonedOwner::new(self.backend, poisoned)),
        }
    }
}

impl<B: PortBackend, S> Owner<B, S> {
    /// The radio clock.
    pub fn now(
        &self,
    ) -> impl Future<Output = PortResult<RadioInstant<B::Domain>, ClockError, B::Fault>> + '_ {
        self.backend.now()
    }
}

/// An admitted `Enable` until its end. It holds the backend between calls of
/// [`Self::next`], so dropping a `next` future loses nothing.
#[must_use = "an enabling port resolves only through next"]
pub struct Enabling<B> {
    backend: Option<B>,
}

/// One step of [`Enabling::next`].
pub enum EnableStep<B: PortBackend> {
    /// An event the port reported before the command ended.
    Event(B::Event),
    /// The port is enabled.
    Enabled(Owner<B, Enabled>),
    /// The radio did not enable; the port is disabled.
    Failed(Owner<B, Disabled>, B::EnableFailure),
    /// The backend is poisoned.
    Poisoned(PoisonedOwner<B>),
}

impl<B: PortBackend> Enabling<B> {
    /// The next event, or the command's end with the next owner.
    ///
    /// # Panics
    ///
    /// Called again after it returned the next owner, as a completed future
    /// polled again does.
    pub async fn next(&mut self) -> EnableStep<B> {
        let backend = self.backend.as_mut().expect(RESOLVED);
        let step = backend.next_enabling().await;
        match step {
            Ok(Step::Event(event)) => EnableStep::Event(event),
            Ok(Step::Ended(Ok(()))) => EnableStep::Enabled(Owner {
                backend: self.backend.take().expect(RESOLVED),
                state: PhantomData,
            }),
            Ok(Step::Ended(Err(failure))) => EnableStep::Failed(
                Owner {
                    backend: self.backend.take().expect(RESOLVED),
                    state: PhantomData,
                },
                failure,
            ),
            Err(poisoned) => EnableStep::Poisoned(PoisonedOwner::new(
                self.backend.take().expect(RESOLVED),
                poisoned,
            )),
        }
    }
}

/// An admitted `Disable` until its end. It holds the backend between calls
/// of [`Self::next`], so dropping a `next` future loses nothing.
#[must_use = "a disabling port resolves only through next"]
pub struct Disabling<B> {
    backend: Option<B>,
}

/// One step of [`Disabling::next`].
pub enum DisableStep<B: PortBackend> {
    /// An event the port reported before the command ended: a leftover
    /// terminal of a phase's work, with what it returns.
    Event(B::Event),
    /// The port is disabled.
    Disabled(Owner<B, Disabled>),
    /// The backend is poisoned.
    Poisoned(PoisonedOwner<B>),
}

impl<B: PortBackend> Disabling<B> {
    /// The next event, or the command's end with the next owner.
    ///
    /// # Panics
    ///
    /// Called again after it returned the next owner, as a completed future
    /// polled again does.
    pub async fn next(&mut self) -> DisableStep<B> {
        let backend = self.backend.as_mut().expect(RESOLVED);
        let step = backend.next_disabling().await;
        match step {
            Ok(Step::Event(event)) => DisableStep::Event(event),
            Ok(Step::Ended(())) => DisableStep::Disabled(Owner {
                backend: self.backend.take().expect(RESOLVED),
                state: PhantomData,
            }),
            Err(poisoned) => DisableStep::Poisoned(PoisonedOwner::new(
                self.backend.take().expect(RESOLVED),
                poisoned,
            )),
        }
    }
}

const RESOLVED: &str = "a resolved transition is not stepped again";

/// The one exit of a poisoned backend: its state is unknown, and only its
/// uninstall leaves it.
#[must_use = "a poisoned backend leaves only through its uninstall"]
pub struct PoisonedOwner<B: PortBackend> {
    backend: B,
    cause: B::Fault,
}

impl<B: PortBackend> PoisonedOwner<B> {
    fn new(backend: B, poisoned: Poisoned<B::Fault>) -> Self {
        Self {
            backend,
            cause: poisoned.cause,
        }
    }

    /// Why the backend's state is unknown.
    pub fn cause(&self) -> B::Fault {
        self.cause
    }

    /// Take the backend apart with the composition's `control`.
    pub fn uninstall(self, control: B::Uninstall) -> B::Uninstalled {
        self.backend.uninstall(control)
    }
}

/// The parts of one running phase ([`Owner::split`]), each lent for the
/// phase.
pub struct PhaseParts<'a, E, C, K> {
    /// The phase's one event consumer.
    pub events: Lent<'a, E>,
    /// The phase control, with one holder.
    pub control: Lent<'a, C>,
    /// The protocol's client handles, one per configured interface.
    pub clients: Lent<'a, K>,
}

/// One part lent for the borrow `'a` of the port's owner. It reaches its
/// part by shared or mutable reference and gives the part away by no
/// method; `'a` is invariant, so no loan outlasts the borrow that made it.
/// A part's type has no value outside its backend, so a mutable reference
/// cannot exchange it out ([module](self) documentation).
pub struct Lent<'a, T> {
    part: T,
    loan: PhantomData<fn(&'a ()) -> &'a ()>,
}

impl<T> Lent<'_, T> {
    /// Lend `part` for the borrow of the owner that the backend's `split` or
    /// `setup` received.
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

/// The one consumer of a running phase's events: a router that hands each
/// terminal event to the holder of its work ([`RadioWork::id`]).
pub trait RadioPortEvents {
    /// One owned event.
    type Event;
    /// The cause a poisoned backend reports.
    type Fault: Copy + Debug;

    /// The next event. Taking it only dequeues it; dropping the future
    /// loses no event. `Ok(Err(EventsLost))` takes the place of the first
    /// dropped event. A poisoned backend returns [`Poisoned`] after every
    /// event it produced before the fault, and again at every later call.
    fn next_event(
        &mut self,
    ) -> impl Future<Output = PortResult<Self::Event, EventsLost, Self::Fault>> + '_;
}

/// One admitted operation: what a client's submission returns. The backend
/// mints its `token`, which is neither `Clone` nor `Copy`, so it names
/// exactly one operation and only its holder ends it. A `Work` borrows the
/// client that admitted it (`'c`, invariant), so no token outlives its
/// phase and none can name the work of a later one.
#[must_use = "dropping a work forgoes ending it; its terminal still arrives"]
pub struct Work<'c, T> {
    token: T,
    loan: PhantomData<fn(&'c ()) -> &'c ()>,
}

impl<T: RadioWork> Work<'_, T> {
    /// Admitted work for the client borrow the backend's submission
    /// received.
    pub const fn new(token: T) -> Self {
        Self {
            token,
            loan: PhantomData,
        }
    }

    /// The key a router matches the operation's terminal event by.
    pub fn id(&self) -> T::Id {
        self.token.id()
    }
}

/// A backend's token of one admitted operation ([`Work`]).
pub trait RadioWork {
    /// The key a router matches the operation's terminal event by. It
    /// grants nothing: ending the work takes the token itself.
    type Id: Copy + Eq;

    /// The routing key of this operation.
    fn id(&self) -> Self::Id;
}

/// What every client handle of a running phase does beside its protocol's
/// own submissions, settings and leases: read the radio clock and end its
/// own work.
///
/// A client exists only for an interface the disabled port configured, and
/// its protocol's calls name only that interface. Its submissions return a
/// [`Work`] borrowing it.
pub trait RadioPortClient {
    /// The backend's token of one admitted operation of this client.
    type Token: RadioWork;
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

    /// End the operation `work` names: the token is the authority. Its
    /// terminal event is the operation's own, which may be its natural end;
    /// a refusal as [`CancelError::NotRunning`] says the air ended it
    /// first, the world's outcome. Dropping the future may leave the work
    /// running or ended; its terminal arrives either way.
    fn cancel<'c>(
        &'c self,
        work: Work<'c, Self::Token>,
    ) -> impl Future<Output = PortResult<(), CancelError, Self::Fault>> + 'c {
        self.end(work.token)
    }

    /// The backend's side of [`Self::cancel`]. Only a [`Work`] holds a
    /// token, and nothing takes it out but `cancel`, so a caller reaches
    /// this only through `cancel`.
    fn end(
        &self,
        token: Self::Token,
    ) -> impl Future<Output = PortResult<(), CancelError, Self::Fault>> + '_;
}
