//! The one authority over an installed port, as types.
//!
//! Every rule a caller controls is a type here, so breaking it does not
//! compile ([ownership and refusals]); only what the world controls (the
//! air, time, the hardware, a peer, the arbiter) stays a typed outcome.
//!
//! # Shape
//!
//! A port lives inside one body ([`PortBody`]) that [`run_port`] runs over a
//! backend ([`PortBackend`]); no caller ever holds the backend. The body
//! receives an [`Owner`] in the [`Disabled`] state and must return
//! [`Ended`], which only the uninstalls and the `fail_stop`s construct, so
//! a body that drops or forgets its owner does not compile. The owner's
//! state is its type:
//!
//! ```text
//! Owner<B, Disabled> ── enable(self).await ──▶ Ok(Owner<B, Enabled>)
//!        ▲                                    Err(Failed::Refused(Owner<B, Disabled>, refusal))
//!        │                                    Err(Failed::Poisoned(PoisonedOwner<B>))
//!        └── Ok ── disable(self, sink).await ── Owner<B, Enabled>
//!                  Err(PoisonedOwner<B>)
//! uninstall / fail_stop ──▶ Ended<B>
//! ```
//!
//! - [`Owner<B, Disabled>`](Owner) lends the protocol's disabled-only
//!   settings ([`Owner::setup`]: a permanent channel move, interface
//!   configuration, port-lifetime resources such as keys and agreements,
//!   whose tokens the owner keeps and lends into each phase), and is the
//!   only state that uninstalls.
//! - [`Owner<B, Enabled>`](Owner) lends one running phase at a time
//!   ([`Owner::split`]): the phase's one event consumer, its control and
//!   its interface-scoped clients, each a [`Lent`] of the owner's borrow.
//! - The transitions take the owner by value and are bounded by contract:
//!   each ends within its backend's bound, or as poisoned. An unbounded wait
//!   (the radio arbiter's lease) is a separate, cancel-safe step before the
//!   transition, which takes its result. `disable` hands every leftover
//!   terminal of a phase's work, with what it returns, to its `sink`, and
//!   ends only when every permit is back.
//! - [`PoisonedOwner`] is the one exit of a poisoned backend: its cause,
//!   its uninstall or its fail-stop.
//!
//! A command never meets another command, a phase never meets a lifecycle
//! command, a disabled-only setting never meets an enabled port and an
//! enabled port is never uninstalled: there is no lifecycle refusal and no
//! "port enabled", "port disabled" or "port running" refusal. A refused
//! `Enable` (the radio refused it) is the world's outcome and returns the
//! disabled owner.
//!
//! # What types guarantee
//!
//! | Guarantee | Enforced by |
//! | --- | --- |
//! | A port ends through an uninstall or a fail-stop | the body returns [`Ended`], which only they construct |
//! | A caller holds an owner only inside a body | [`Owner`] has no public constructor |
//! | A disabled port lends no phase; an enabled one no disabled-only setting and no uninstall | the state parameter |
//! | One lifecycle command at a time; no phase or setting while one runs | the transitions take the owner by value |
//! | A phase's parts end before any owner call or transition | [`Lent`], invariant in the owner's borrow |
//! | One running phase, one event consumer | `split(&mut self)` |
//! | A caller reaches the backend only through the owner and its parts | the owner's private field |
//! | Admitted work names only its own phase's work | [`Work`] borrows its client |
//! | Admitted work ends only through the client that admitted it | [`Work`] holds that client and its token, which nothing takes out |
//! | A part or a token is not taken out of its loan | `Lent` and `Work` give nothing away; see below |
//!
//! Each line has a compile-fail test below, pinned to its error code, with a
//! compiling twin.
//!
//! # What each backend proves
//!
//! The language cannot express three rules, so every backend proves them of
//! its own types:
//!
//! - its phase parts and every token it mints (admitted work, permits,
//!   leases, installed keys and agreements, validated values) are not
//!   `Clone`, `Copy` or `Default` and have no public constructor, so a
//!   mutable reference cannot exchange a part out of its loan
//!   (compile-fail tests);
//! - everything that can act on the port borrows the phase part it came
//!   from (compile-fail tests); received data and returned bodies act on
//!   nothing and may outlive it;
//! - everything the hardware touches (descriptors, DMA buffers, key tables)
//!   is `'static` and owned by the port, so no abandoned owner is ever a
//!   memory-safety issue.
//!
//! # Port-lifetime and phase-scoped tokens
//!
//! A resource that outlives a phase (an installed key, an agreement, a
//! configured interface) has its token kept by the owner and lent into each
//! phase. Admitted work and the permits and leases it holds are
//! phase-scoped: [`Work`] borrows its client. A submission consumes a
//! permit (a transmit slot, a terminal-event cell) and its terminal returns
//! it, so no queue overflows and no terminal is lost; only unsolicited
//! reception may lose events. A `'static` sans-IO core holds no
//! phase-scoped token: the phase keeps a boundary table from the core's own
//! identity to the token, which carries out the core's decisions.
//!
//! # Drop
//!
//! Rust's types are affine: an owner can be dropped and `mem::forget` is
//! safe. A body cannot do either and still end, and it runs as an executor
//! task that is not cancelled; inside it, the phase's parts run under
//! `join` over disjoint borrows. An owner is dropped only by a panic (on the
//! target, a reset) or by a body raced outside its task. Its `Drop` is a
//! safety net, as [`PlatformClockGuard`]'s: it calls
//! [`PortBackend::abandon`] (close transmission, mask interrupts, refuse any
//! new install until a reset) and asserts in debug builds; it is never the
//! shutdown.
//!
//! # Several executor tasks
//!
//! A borrow does not cross executor tasks. The port's parts stay inside its
//! body's task; another task (a network stack) talks to the body as to an
//! actor, by owned requests and replies. A handle shared between tasks over
//! an internal mutex is not a form of this contract.
//!
//! # Inside the backend
//!
//! The backend's interrupt entries and runner reach its state beside the
//! owner; that synchronization stays inside the backend and never reaches a
//! caller as a refusal.
//!
//! # Compile-fail tests
//!
//! A body cannot end without an uninstall or a fail-stop:
//!
//! ```compile_fail,E0451
//! # use oer_radio_port::{Disabled, Ended, Owner, PortBackend, PortBody};
//! struct Forget;
//! impl<B: PortBackend> PortBody<B> for Forget {
//!     async fn run(self, owner: Owner<B, Disabled>) -> Ended<B> {
//!         drop(owner);
//!         Ended { uninstalled: None }
//!     }
//! }
//! ```
//!
//! Its compiling twin ends through the owner:
//!
//! ```
//! # use oer_radio_port::{Disabled, Ended, Owner, PortBackend, PortBody};
//! struct FailStop;
//! impl<B: PortBackend> PortBody<B> for FailStop {
//!     async fn run(self, owner: Owner<B, Disabled>) -> Ended<B> {
//!         owner.fail_stop()
//!     }
//! }
//! ```
//!
//! No caller makes an owner outside a body:
//!
//! ```compile_fail,E0624
//! # use oer_radio_port::{Disabled, Owner, PortBackend};
//! fn forge<B: PortBackend>(backend: B) -> Owner<B, Disabled> {
//!     Owner::new(backend)
//! }
//! ```
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
//! An enabled port takes no disabled-only setting and no uninstall:
//!
//! ```compile_fail,E0599
//! # use oer_radio_port::{Enabled, Owner, PortBackend};
//! fn setup_while_enabled<B: PortBackend>(owner: &mut Owner<B, Enabled>) {
//!     let _ = owner.setup();
//! }
//! ```
//!
//! ```compile_fail,E0599
//! # use oer_radio_port::{Enabled, Owner, PortBackend};
//! fn uninstall_while_enabled<B: PortBackend>(owner: Owner<B, Enabled>, control: B::Uninstall) {
//!     let _ = owner.uninstall(control);
//! }
//! ```
//!
//! An owner is gone once its command starts, so a second command cannot
//! start beside it:
//!
//! ```compile_fail,E0382
//! # use oer_radio_port::{Disabled, Owner, PortBackend};
//! async fn two_commands<B: PortBackend>(owner: Owner<B, Disabled>) {
//!     let enabling = owner.enable();
//!     let _ = owner.enable();
//!     let _ = enabling.await;
//! }
//! ```
//!
//! A phase's part does not outlive the phase:
//!
//! ```compile_fail,E0505
//! # use oer_radio_port::{Enabled, Owner, PortBackend};
//! async fn keep_the_control<B: PortBackend>(mut owner: Owner<B, Enabled>) {
//!     let control = owner.split().control;
//!     let _ = owner.disable(&mut |_| {}).await;
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
//! fn reach_the_backend<B: PortBackend>(owner: &mut Owner<B, Enabled>) {
//!     let _ = &mut owner.backend;
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
//!     admit: impl for<'c> Fn(&'c C) -> Work<'c, C>,
//!     client: C,
//! ) -> Work<'static, C> {
//!     admit(&client)
//! }
//! ```
//!
//! Nor does a caller take the token out of a work, so a work ends only
//! through the client that admitted it:
//!
//! ```compile_fail,E0616
//! # use oer_radio_port::{RadioPortClient, Work};
//! fn take_the_token<C: RadioPortClient>(work: Work<'_, C>) {
//!     let _ = work.token;
//! }
//! ```
//!
//! Its compiling twin reads only the work's routing key:
//!
//! ```
//! # use oer_radio_port::{RadioPortClient, Work};
//! fn read_the_key<C: RadioPortClient>(work: Work<'_, C>) {
//!     let _ = work.id();
//! }
//! ```
//!
//! The compiling twin of the lifecycle tests runs a phase between the
//! commands and keeps the work only as long as its client:
//!
//! ```
//! # use oer_radio_port::{Disabled, Ended, Owner, PortBackend, PortBody, RadioPortClient, Work};
//! struct OnePhase;
//! impl<B: PortBackend> PortBody<B> for OnePhase {
//!     async fn run(self, mut owner: Owner<B, Disabled>) -> Ended<B> {
//!         let _ = owner.setup();
//!         let mut enabled = match owner.enable().await {
//!             Ok(enabled) => enabled,
//!             Err(failed) => return failed.fail_stop(),
//!         };
//!         {
//!             let phase = enabled.split();
//!             drop(phase);
//!         }
//!         match enabled.disable(&mut |_| {}).await {
//!             Ok(disabled) => disabled.fail_stop(),
//!             Err(poisoned) => poisoned.fail_stop(),
//!         }
//!     }
//! }
//! fn keep_the_work<'c, C: RadioPortClient>(
//!     admit: impl Fn(&'c C) -> Work<'c, C>,
//!     client: &'c C,
//! ) -> Work<'c, C> {
//!     admit(client)
//! }

//! ```
//!
//! [ownership and refusals]: https://github.com/ermacv/open-esp-radio-rs/blob/main/docs/architecture.md#ownership-and-refusals
//! [`PlatformClockGuard`]: https://github.com/ermacv/open-esp-radio-rs/blob/main/crates/hardware/esp32s31/hal/src/power/clock/executor.rs

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

/// The result of a backend's `Enable`: the radio's refusal inside, a
/// poisoned backend outside.
pub type EnableResult<R, F> = Result<Result<(), R>, Poisoned<F>>;

/// What a backend implements; callers never hold it. [`run_port`] wraps it
/// in the body's owner, and the owner calls each method only in the state
/// its documentation names.
pub trait PortBackend: Sized {
    /// One owned event.
    type Event;
    /// The clock domain of the port's radio instants.
    type Domain;
    /// The cause a poisoned backend reports.
    type Fault: Copy + Debug;
    /// Why the radio refused `Enable` (a refused retune): the world's
    /// outcome, after which the port is disabled.
    type Refusal;
    /// The protocol's disabled-only settings, lent by [`Owner::setup`].
    type Setup<'a>
    where
        Self: 'a;
    /// The phase's one event consumer.
    type Events<'a>: RadioPortEvents<Event = Self::Event, Fault = Self::Fault>
    where
        Self: 'a;
    /// The phase control: the radio-wide operations that only gate or tune
    /// admission while clients run, as guards on its sub-resources; each
    /// guard proves only its own state (a closed transmit gate proves this
    /// port publishes nothing, `TxGated`, not that the air is quiet).
    type Control<'a>
    where
        Self: 'a;
    /// The protocol's client handles, one per configured interface.
    type Clients<'a>
    where
        Self: 'a;
    /// What the uninstall takes beside the backend (the composition's
    /// control).
    type Uninstall;
    /// The hardware owners the uninstall returns.
    type Uninstalled;

    /// Enable the disabled port, within the backend's bound.
    fn enable(&mut self) -> impl Future<Output = EnableResult<Self::Refusal, Self::Fault>> + '_;

    /// Disable the enabled port within the backend's bound: end every
    /// admitted work, hand its terminal event with what it returns to
    /// `sink`, and return when every permit is back.
    fn disable<'a>(
        &'a mut self,
        sink: &'a mut dyn FnMut(Self::Event),
    ) -> impl Future<Output = Result<(), Poisoned<Self::Fault>>> + 'a;

    /// The radio clock.
    fn now(
        &self,
    ) -> impl Future<Output = PortResult<RadioInstant<Self::Domain>, ClockError, Self::Fault>> + '_;

    /// Lend the disabled-only settings.
    fn setup(&mut self) -> Lent<'_, Self::Setup<'_>>;

    /// Lend one running phase of the enabled port.
    fn split(&mut self) -> PhaseParts<'_, Self::Events<'_>, Self::Control<'_>, Self::Clients<'_>>;

    /// Take the disabled or poisoned backend apart with the composition's
    /// `control`.
    fn uninstall(self, control: Self::Uninstall) -> Self::Uninstalled;

    /// Stop the backend at once, synchronously: close transmission, mask its
    /// interrupts and refuse any new install until a reset. A fail-stop and
    /// a dropped owner call it; the hardware stays installed and
    /// unreachable.
    fn abandon(&mut self);
}

/// The one lifetime of a port, from its install to its [`Ended`]; see the
/// [module](self) documentation.
pub trait PortBody<B: PortBackend> {
    /// Run the port: the owner arrives disabled and leaves through an
    /// uninstall or a fail-stop.
    fn run(self, owner: Owner<B, Disabled>) -> impl Future<Output = Ended<B>>;
}

/// Run `body` over a just-installed, disabled `backend`. A backend's install
/// calls it, in the executor task that runs the port, and receives its end.
pub async fn run_port<B: PortBackend>(backend: B, body: impl PortBody<B>) -> Ended<B> {
    body.run(Owner::new(backend)).await
}

/// How a port's body ended: its hardware owners, after an uninstall, or
/// nothing after a fail-stop, which leaves the hardware installed until a
/// reset. Only the uninstalls and the `fail_stop`s construct it.
#[must_use = "an ended port returns its hardware owners"]
pub struct Ended<B: PortBackend> {
    uninstalled: Option<B::Uninstalled>,
}

impl<B: PortBackend> Ended<B> {
    /// The hardware owners an uninstall returned, or `None` after a
    /// fail-stop.
    pub fn into_uninstalled(self) -> Option<B::Uninstalled> {
        self.uninstalled
    }
}

/// The one authority over an installed port, in state `S`; see the
/// [module](self) documentation.
#[must_use = "an owner leaves only through a transition, an uninstall or a fail-stop"]
pub struct Owner<B: PortBackend, S> {
    /// `None` only once a consuming method took it.
    backend: Option<B>,
    state: PhantomData<S>,
}

impl<B: PortBackend, S> Owner<B, S> {
    fn new(backend: B) -> Self {
        Self {
            backend: Some(backend),
            state: PhantomData,
        }
    }

    fn backend(&mut self) -> &mut B {
        self.backend.as_mut().expect(HELD)
    }

    fn take(mut self) -> B {
        self.backend.take().expect(HELD)
    }

    /// The radio clock.
    pub fn now(
        &self,
    ) -> impl Future<Output = PortResult<RadioInstant<B::Domain>, ClockError, B::Fault>> + '_ {
        self.backend.as_ref().expect(HELD).now()
    }

    /// Stop the backend at once and end the port without its uninstall:
    /// the hardware stays installed and unreachable until a reset.
    pub fn fail_stop(self) -> Ended<B> {
        self.take().abandon();
        Ended { uninstalled: None }
    }
}

impl<B: PortBackend> Owner<B, Disabled> {
    /// Lend the protocol's disabled-only settings.
    pub fn setup(&mut self) -> Lent<'_, B::Setup<'_>> {
        self.backend().setup()
    }

    /// Enable the port, within the backend's bound.
    ///
    /// # Errors
    ///
    /// The radio's refusal, with the disabled owner, or a poisoned backend.
    pub async fn enable(mut self) -> Result<Owner<B, Enabled>, Failed<B>> {
        // The owner holds the backend across the wait, so a dropped future
        // drops the owner and its safety net runs.
        let enabled = self.backend().enable().await;
        let backend = self.take();
        match enabled {
            Ok(Ok(())) => Ok(Owner::new(backend)),
            Ok(Err(refusal)) => Err(Failed::Refused(Owner::new(backend), refusal)),
            Err(poisoned) => Err(Failed::Poisoned(PoisonedOwner::new(backend, poisoned))),
        }
    }

    /// Take the disabled backend apart with the composition's `control`.
    pub fn uninstall(self, control: B::Uninstall) -> Ended<B> {
        Ended {
            uninstalled: Some(self.take().uninstall(control)),
        }
    }
}

impl<B: PortBackend> Owner<B, Enabled> {
    /// Lend one running phase: its event consumer, its control and its
    /// clients, each a [`Lent`] of this borrow.
    pub fn split(&mut self) -> PhaseParts<'_, B::Events<'_>, B::Control<'_>, B::Clients<'_>> {
        self.backend().split()
    }

    /// Disable the port within the backend's bound: every leftover terminal
    /// of a phase's work, with what it returns, goes to `sink`, and the port
    /// is disabled once every permit is back.
    ///
    /// # Errors
    ///
    /// A poisoned backend.
    pub async fn disable(
        mut self,
        sink: &mut dyn FnMut(B::Event),
    ) -> Result<Owner<B, Disabled>, PoisonedOwner<B>> {
        // The owner holds the backend across the wait, as `enable`'s does.
        let disabled = self.backend().disable(sink).await;
        let backend = self.take();
        match disabled {
            Ok(()) => Ok(Owner::new(backend)),
            Err(poisoned) => Err(PoisonedOwner::new(backend, poisoned)),
        }
    }
}

impl<B: PortBackend, S> Drop for Owner<B, S> {
    fn drop(&mut self) {
        abandoned(&mut self.backend);
    }
}

/// Why `Enable` did not leave an enabled owner.
pub enum Failed<B: PortBackend> {
    /// The radio refused; the port is disabled.
    Refused(Owner<B, Disabled>, B::Refusal),
    /// The backend is poisoned.
    Poisoned(PoisonedOwner<B>),
}

impl<B: PortBackend> Failed<B> {
    /// Stop the backend at once and end the port.
    pub fn fail_stop(self) -> Ended<B> {
        match self {
            Self::Refused(owner, _) => owner.fail_stop(),
            Self::Poisoned(poisoned) => poisoned.fail_stop(),
        }
    }
}

/// The one exit of a poisoned backend: its state is unknown, and it leaves
/// through its uninstall or a fail-stop.
#[must_use = "a poisoned backend leaves only through its uninstall or a fail-stop"]
pub struct PoisonedOwner<B: PortBackend> {
    /// `None` only once a consuming method took it.
    backend: Option<B>,
    cause: B::Fault,
}

impl<B: PortBackend> PoisonedOwner<B> {
    fn new(backend: B, poisoned: Poisoned<B::Fault>) -> Self {
        Self {
            backend: Some(backend),
            cause: poisoned.cause,
        }
    }

    fn take(mut self) -> B {
        self.backend.take().expect(HELD)
    }

    /// Why the backend's state is unknown.
    pub fn cause(&self) -> B::Fault {
        self.cause
    }

    /// Take the backend apart with the composition's `control`.
    pub fn uninstall(self, control: B::Uninstall) -> Ended<B> {
        Ended {
            uninstalled: Some(self.take().uninstall(control)),
        }
    }

    /// Stop the backend at once and end the port without its uninstall.
    pub fn fail_stop(self) -> Ended<B> {
        self.take().abandon();
        Ended { uninstalled: None }
    }
}

impl<B: PortBackend> Drop for PoisonedOwner<B> {
    fn drop(&mut self) {
        abandoned(&mut self.backend);
    }
}

const HELD: &str = "an owner holds its backend until a consuming method";

/// The safety net of a dropped owner: abandon the backend it still holds,
/// and assert in debug builds, as `PlatformClockGuard` does.
fn abandoned<B: PortBackend>(backend: &mut Option<B>) {
    let Some(mut backend) = backend.take() else {
        return;
    };
    backend.abandon();
    // A host test unwinding a panic drops its owners; asserting then would
    // abort the test process and hide the first panic.
    #[cfg(any(test, all(feature = "test-support", not(target_os = "none"))))]
    if std::thread::panicking() {
        return;
    }
    debug_assert!(
        false,
        "a port owner was dropped without an uninstall or a fail-stop"
    );
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
/// terminal event to the holder of its work ([`Work::id`]).
pub trait RadioPortEvents {
    /// One owned event.
    type Event;
    /// The cause a poisoned backend reports.
    type Fault: Copy + Debug;

    /// The next event. Taking it only dequeues it; dropping the future
    /// loses no event. `Ok(Err(EventsLost))` takes the place of the first
    /// dropped event, which only unsolicited reception may lose. A poisoned
    /// backend returns [`Poisoned`] after every event it produced before
    /// the fault, and again at every later call.
    fn next_event(
        &mut self,
    ) -> impl Future<Output = PortResult<Self::Event, EventsLost, Self::Fault>> + '_;
}

/// One admitted operation: what a client's submission returns. It holds
/// the backend's token, which is neither `Clone` nor `Copy` and only the
/// backend mints, and the client that admitted it, so it names exactly one
/// operation, only its holder ends it, and it ends through the client that
/// admitted it ([`Self::cancel`]). It borrows that client (`'c`), so no
/// work outlives its phase and none can name the work of a later one.
#[must_use = "dropping a work forgoes ending it; its terminal still arrives"]
pub struct Work<'c, C: RadioPortClient> {
    token: C::Token,
    client: &'c C,
}

impl<'c, C: RadioPortClient> Work<'c, C> {
    /// Admitted work of `client`, which the backend's submission minted
    /// `token` for.
    pub const fn new(client: &'c C, token: C::Token) -> Self {
        Self { token, client }
    }

    /// The key a router matches the operation's terminal event by.
    pub fn id(&self) -> <C::Token as RadioWork>::Id {
        self.token.id()
    }

    /// End the operation through the client that admitted it. Its terminal
    /// event is the operation's own, which may be its natural end; a
    /// refusal as [`CancelError::NotRunning`] says the air ended it first,
    /// the world's outcome. Dropping the future may leave the work running
    /// or ended; its terminal arrives either way.
    pub fn cancel(self) -> impl Future<Output = PortResult<(), CancelError, C::Fault>> + 'c {
        self.client.end(self.token)
    }
}

/// A backend's token of one admitted operation ([`Work`]).
pub trait RadioWork {
    /// The key a router matches the operation's terminal event by. It
    /// grants nothing: ending the work takes the [`Work`] itself.
    type Id: Copy + Eq;

    /// The routing key of this operation.
    fn id(&self) -> Self::Id;
}

/// What every client handle of a running phase does beside its protocol's
/// own submissions, settings and permits: read the radio clock and end its
/// own work.
///
/// A client exists only for an interface the disabled port configured, and
/// its protocol's calls name only that interface. Its submissions return a
/// [`Work`] that borrows it and ends through it.
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

    /// The backend's side of [`Work::cancel`]: end the operation `token`
    /// names, which this client admitted. Only a [`Work`] holds a token and
    /// nothing takes it out but `cancel`, which calls the client that
    /// admitted it, so a caller reaches this only through that `cancel`.
    fn end(
        &self,
        token: Self::Token,
    ) -> impl Future<Output = PortResult<(), CancelError, Self::Fault>> + '_;
}
