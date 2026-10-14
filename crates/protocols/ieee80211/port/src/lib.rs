#![no_std]
#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![deny(
    clippy::let_underscore_must_use,
    clippy::let_underscore_untyped,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::unreachable,
    clippy::todo,
    clippy::unimplemented,
    clippy::disallowed_macros
)]
#![cfg_attr(
    test,
    allow(
        clippy::let_underscore_must_use,
        clippy::let_underscore_untyped,
        clippy::arithmetic_side_effects,
        clippy::indexing_slicing,
        clippy::panic,
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::unreachable,
        clippy::todo,
        clippy::unimplemented,
        clippy::disallowed_macros,
    )
)]

//! The lifecycle of the IEEE 802.11 lower-MAC port: the one authority over an installed
//! port, as types. The Wi-Fi block owns it; the vocabulary it shares with
//! the other radio blocks (the brand and [`Mint`], slots, permits and
//! seats, [`Poisoned`], [`PortResult`], [`EventsLost`], the clock
//! relation) stays in `oer-radio-port`.
//!
//! Every rule a caller controls is a type here, so breaking it does not
//! compile ([ownership and refusals]); only what the world controls (the
//! air, time, the hardware, a peer, the arbiter) stays a typed outcome.
//!
//! # Shape
//!
//! A port lives inside one body ([`PortBody`]) that [`run_port`] runs over a
//! backend ([`PortBackend`]), joined with the backend's driver
//! ([below](#the-driver)); no caller ever holds the backend. The body
//! receives an [`Owner`] in the [`Disabled`] state and must return
//! [`Ended`], which only the uninstalls and the `fail_stop`s construct.
//! Dropping or forgetting the owner is legal Rust, but leaves the body
//! without the owner it needs to construct `Ended`. The owner's state is
//! its type (`'p` is its borrow of the install `run_port` keeps,
//! `'id` the install's brand):
//!
//! ```text
//! Owner<'p, 'id, B, Disabled> ── enable(self).await ──▶ Ok(Owner<'p, 'id, B, Enabled>)
//!        ▲                                    Err(Failed::Refused(Owner<'p, 'id, B, Disabled>, refusal))
//!        │                                    Err(Failed::Poisoned(PoisonedOwner<'p, 'id, B>))
//!        └── Ok ── disable(self, sink).await ── Owner<'p, 'id, B, Enabled>
//!                  Err(PoisonedOwner<'p, 'id, B>)
//! uninstall / fail_stop ──▶ Ended<'id, B>
//! ```
//!
//! - [`Owner<'p, 'id, B, Disabled>`](Owner) lends the protocol's disabled-only
//!   settings ([`Owner::setup`]: a permanent channel move, interface
//!   configuration, port-lifetime resources such as keys and agreements,
//!   whose tokens the owner keeps and lends into each phase), and is the
//!   only state that asks for the uninstall.
//! - [`Owner<'p, 'id, B, Enabled>`](Owner) lends one running phase at a time
//!   ([`Owner::phase`]): one object under `&mut`, the body's handle over
//!   its queues to the driver ([below](#the-phase)), a [`Lent`] of the
//!   owner's borrow.
//! - The transitions take the owner by value and are bounded by contract:
//!   each ends within its backend's bound, or as poisoned. An unbounded wait
//!   (the radio arbiter's lease) is a separate, cancel-safe step before the
//!   transition, which takes its result. `disable` hands every leftover
//!   terminal of a phase's work, with the tokens it returns, to its `sink`.
//! - [`PoisonedOwner`] is the one exit of a poisoned backend: its cause,
//!   its uninstall or its fail-stop.
//! - An uninstall only asks: the body returns its [`Ended`], and
//!   [`run_port`] takes the backend apart ([below](#the-driver)).
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
//! | A phase ends before any owner call or transition | [`Lent`], invariant in the owner's borrow |
//! | One running phase, one event consumer | `phase(&mut self)` |
//! | A caller reaches the backend only through the owner and its phase | the owner's private field |
//! | The phase is not taken out of its loan | `Lent` gives nothing away |
//!
//! Each line has a compile-fail test below, pinned to its error code, with a
//! compiling twin.
//!
//! # What each backend proves
//!
//! The language cannot express three rules, so every backend proves them of
//! its own types:
//!
//! - its phase and every token it mints (transmit tickets and buffers,
//!   installed keys and agreements, gates, validated values) are not
//!   `Clone`, `Copy` or `Default` and have no public constructor
//!   (compile-fail tests);
//! - everything that can act on the port borrows the phase it came from
//!   (compile-fail tests); received data and returned bodies act on
//!   nothing and may outlive it;
//! - everything the hardware touches (descriptors, DMA buffers, key tables)
//!   is `'static` and owned by the port, so no abandoned owner is ever a
//!   memory-safety issue.
//!
//! # The phase
//!
//! The running phase is the body's handle over single-producer
//! single-consumer queues to the driver, which `held` keeps ([owner
//! decisions on #563]): commands go to the driver, events come from it.
//! [`next_event`](PortPhase::next_event) only reads the event queue and
//! every operation of the phase only writes the command queue, so nothing
//! of the phase awaits but `next_event`, no state is shared between the
//! body and the driver, and the backend needs no lock:
//!
//! - **Tokens bound the queues.** Every operation that ends in an event
//!   takes a token (a transmit ticket, a key slot, the gate), and the
//!   terminal gives it back; the event queue reserves a place for each
//!   token's terminal and the command queue for each token's command, so a
//!   terminal is never lost and a command never refused, by capacity.
//! - **Settings are cells, not commands.** A setting that takes no token
//!   (an interface's receive configuration, EDCA, the BSS color, a TBTT
//!   plan, monitor reception) is a last-value-wins cell with a "changed"
//!   flag, one per kind, which the driver takes before the next queued
//!   command, so a setting still precedes the next submission.
//! - **The handle owns the accounting** of the slots it hands out (keys,
//!   Block Ack agreements); the driver executes register commands and keeps
//!   no accounting of its own, so a synchronous answer of the handle is its
//!   own state.
//! - **Unsolicited reception is bounded:** a body that lags makes the
//!   driver keep refilling DMA and drop the surplus, and the stream says
//!   [`EventsLost`]. A received buffer goes back through the phase's own
//!   `recycle`, never through `Drop`. Keeping one occupies receive capacity;
//!   the host model poisons the installation when an unrecycled buffer is
//!   dropped. It does not panic or silently recycle that buffer.
//!
//! "One wait point" in the body's loop is a latency rule: a slow body
//! delays its own events, but the driver keeps serving the hardware.
//!
//! # Drop
//!
//! Rust's types are affine: an owner can be dropped and `mem::forget` is
//! safe. Neither operation returns `Ended`. The phase's parts run under
//! `join` over disjoint borrows inside the body's task. Dropping an owner
//! can be explicit or follow cancellation of a future that holds it; a
//! forgotten owner runs no destructor. The installation remains owned by
//! `run_port` until that enclosing future ends or is cancelled. The owner is a
//! state over its borrow of the backend, which [`run_port`] keeps; it has
//! no `Drop` of its own and never gives the backend up. Its sentry, dropped
//! with it (or with an `enable` or `disable` future holding it), sets the
//! install's atomic [`AbandonLatch`] at once: a driver that sees it
//! publishes no further attempt and stops, and [`run_port`], at the end of
//! that poll of the body, drops the body, calls [`PortBackend::abandon`]
//! (close transmission, mask interrupts, refuse any new install until a
//! reset) and returns `None`. Work the body posted in that poll is never
//! published. This is the Wi-Fi port's form of the safety net, a
//! consequence of the driver being the core's one owner in its own
//! executor ([refinement on #581]). A `run_port` cancelled before its body
//! ended abandons the backend the same way, without panicking in either
//! debug or release builds. Cancellation does not perform normal shutdown.
//!
//! # Several executor tasks
//!
//! A borrow does not cross executor tasks. The port's parts stay inside its
//! body's task; another task (a network stack) talks to the body as to an
//! actor, by owned requests and replies. A handle shared between tasks over
//! an internal mutex is not a form of this contract.
//!
//! # The driver
//!
//! [`run_port`] joins the body with the backend's driver
//! ([`PortBackend::drive`]), its runner, in the body's task, and polls both
//! at every wake until the body ends. So the driver's progress never
//! depends on what the body awaits (a foreign future, a permit the driver
//! itself frees), and a body takes on no liveness duty. The driver runs for
//! the body's whole life, enabled or disabled, and reaches the install only
//! through the backend's held part ([`PortBackend::Held`]): it owns the
//! backend's state by value (its radio role and hardware core), takes the
//! phase's commands and settings from the queues in `Held`, posts the
//! events there, and returns its state ([`PortBackend::State`]) when it
//! retires. The driver is the only owner of that state; the phase never
//! reaches it ([owner decisions on #563]).
//!
//! The driver is not the body; where it runs is the composition's choice.
//! `run_port` polls the driver's future it is given, which is the driver
//! itself or, for a driver in its own higher-priority executor (the
//! ESP32-S31's, so heavy synchronous work of the body never delays the
//! hardware), a proxy that ends when that driver retired. Heavy synchronous
//! work stays out of the body's step anyway: it is an action with a
//! ticket, done outside the loop.
//!
//! The uninstall is `run_port`'s, because a driver still pending holds the
//! state and borrows `Held`. [`Owner::uninstall`] and
//! [`PoisonedOwner::uninstall`] only record the request in the [`Ended`]
//! they return. Once the body returned it, `run_port`:
//!
//! 1. asks the driver to retire ([`Retire`]): the driver finishes its
//!    current step and returns its state within the bound the backend
//!    states ([`PortBackend::retire_bound`]); a driver that misses its
//!    bound is a fail-stop, and `run_port` abandons the backend and
//!    returns `None`;
//! 2. detaches the backend's interrupt entries synchronously
//!    ([`PortBackend::detach`]);
//! 3. calls [`PortBackend::uninstall`] with the held part and the driver's
//!    state by value, which returns the hardware owners by value.
//!
//! A fail-stop and a dropped owner set the abandon latch at once, and the
//! backend is abandoned (by the fail-stop itself, or by `run_port` at the
//! end of the poll that dropped the owner): `run_port` does not poll the
//! driver again. The driver is cancel-safe ([`PortBackend::drive`]).
//!
//! The host tests of this module check what types cannot: a body awaiting
//! a never-ready foreign future does not stall the driver; an uninstall
//! retires, then detaches, then uninstalls; a driver that misses its bound
//! fail-stops the port; a fail-stop and a dropped owner halt the driver at
//! its next poll.
//!
//! # Inside the backend
//!
//! The driver owns the backend's state of the install. Anything beside it
//! (the body's phase, an interrupt entry, another executor) reaches that
//! state only through the queues, atomics or signals in `Held`; no task
//! and no lock is shared with it.
//!
//! - **The interrupt latches, the driver serves:** the interrupt reads and
//!   clears its own status and posts it, as the interrupt rule of
//!   [ownership and refusals] decides.
//! - **A retune stays a bounded step of `enable`**, not of the driver: a
//!   command the driver executes, whose end `enable` awaits.
//!
//! The IEEE 802.15.4 port, which is not this crate's, keeps its interim
//! critical section until
//! [#668](https://github.com/ermacv/open-esp-radio-rs/issues/668).
//!
//! None of this reaches a caller as a refusal.
//!
//! # Compile-fail tests
//!
//! A body cannot end without an uninstall or a fail-stop:
//!
//! ```compile_fail,E0451
//! # use oer_ieee80211_port::{Disabled, Ended, Owner, PortBackend, PortBody};
//! struct Forget;
//! impl<B: PortBackend> PortBody<B> for Forget {
//!     async fn run<'id>(self, owner: Owner<'_, 'id, B, Disabled>, _: B::Resources<'id>) -> Ended<'id, B> {
//!         drop(owner);
//!         Ended { uninstall: None }
//!     }
//! }
//! ```
//!
//! Its compiling twin ends through the owner:
//!
//! ```
//! # use oer_ieee80211_port::{Disabled, Ended, Owner, PortBackend, PortBody};
//! struct FailStop;
//! impl<B: PortBackend> PortBody<B> for FailStop {
//!     async fn run<'id>(self, owner: Owner<'_, 'id, B, Disabled>, _: B::Resources<'id>) -> Ended<'id, B> {
//!         owner.fail_stop()
//!     }
//! }
//! ```
//!
//! No caller makes an owner outside a body:
//!
//! ```compile_fail,E0624
//! # use oer_ieee80211_port::{Disabled, Owner, PortBackend};
//! fn forge<'p, 'id, B: PortBackend>(backend: B) -> Owner<'p, 'id, B, Disabled> {
//!     Owner::new(backend)
//! }
//! ```
//!
//! A disabled port lends no phase:
//!
//! ```compile_fail,E0599
//! # use oer_ieee80211_port::{Disabled, Owner, PortBackend};
//! fn phase_while_disabled<'id, B: PortBackend>(owner: &mut Owner<'_, 'id, B, Disabled>) {
//!     let _ = owner.phase();
//! }
//! ```
//!
//! An enabled port takes no disabled-only setting and no uninstall:
//!
//! ```compile_fail,E0599
//! # use oer_ieee80211_port::{Enabled, Owner, PortBackend};
//! fn setup_while_enabled<'id, B: PortBackend>(owner: &mut Owner<'_, 'id, B, Enabled>) {
//!     let _ = owner.setup();
//! }
//! ```
//!
//! ```compile_fail,E0599
//! # use oer_ieee80211_port::{Enabled, Owner, PortBackend};
//! fn uninstall_while_enabled<'id, B: PortBackend>(owner: Owner<'_, 'id, B, Enabled>, control: B::Uninstall) {
//!     let _ = owner.uninstall(control);
//! }
//! ```
//!
//! An owner is gone once its command starts, so a second command cannot
//! start beside it:
//!
//! ```compile_fail,E0382
//! # use oer_ieee80211_port::{Disabled, Owner, PortBackend};
//! async fn two_commands<'id, B: PortBackend>(owner: Owner<'_, 'id, B, Disabled>) {
//!     let enabling = owner.enable();
//!     let _ = owner.enable();
//!     let _ = enabling.await;
//! }
//! ```
//!
//! A phase does not outlive itself into a transition:
//!
//! ```compile_fail,E0505
//! # use oer_ieee80211_port::{Enabled, Owner, PortBackend};
//! async fn keep_the_phase<'id, B: PortBackend>(mut owner: Owner<'_, 'id, B, Enabled>) {
//!     let phase = owner.phase();
//!     let _ = owner.disable(&mut |_| {}).await;
//!     drop(phase);
//! }
//! ```
//!
//! Nor does a second phase run beside the first:
//!
//! ```compile_fail,E0499
//! # use oer_ieee80211_port::{Enabled, Owner, PortBackend};
//! fn two_phases<'id, B: PortBackend>(owner: &mut Owner<'_, 'id, B, Enabled>) {
//!     let first = owner.phase();
//!     let second = owner.phase();
//!     drop((first, second));
//! }
//! ```
//!
//! A caller does not reach the backend past the owner:
//!
//! ```compile_fail,E0616
//! # use oer_ieee80211_port::{Enabled, Owner, PortBackend};
//! fn reach_the_backend<'id, B: PortBackend>(owner: &mut Owner<'_, 'id, B, Enabled>) {
//!     let _ = &mut owner.backend;
//! }
//! ```
//!
//! Nor takes the phase out of its loan:
//!
//! ```compile_fail,E0616
//! # use oer_ieee80211_port::{Enabled, Owner, PortBackend};
//! fn take_the_phase<'id, B: PortBackend>(owner: &mut Owner<'_, 'id, B, Enabled>) {
//!     let _ = owner.phase().part;
//! }
//! ```
//!
//! A driver's future borrows the install, never the backend, and does not
//! outlive the install's borrow:
//!
//! ```compile_fail,E0700
//! # use core::future::Future;
//! # use oer_ieee80211_port::{PortBackend, Retire};
//! fn keep_the_driver<'p, 'id, B: PortBackend + 'static>(
//!     backend: &mut B,
//!     held: &'p B::Held<'id>,
//!     retire: Retire<'p>,
//! ) -> impl Future<Output = B::State> + 'static {
//!     backend.drive(held, retire)
//! }
//! ```
//!
//! Its compiling twin keeps the driver for the install's borrow only, and
//! reaches the backend beside it:
//!
//! ```
//! # use core::future::Future;
//! # use oer_ieee80211_port::{PortBackend, Retire};
//! fn keep_the_driver<'p, 'id, B: PortBackend + 'static>(
//!     backend: &mut B,
//!     held: &'p B::Held<'id>,
//!     retire: Retire<'p>,
//! ) -> impl Future<Output = B::State> + use<'p, 'id, B> {
//!     let driver = backend.drive(held, retire);
//!     let _ = backend.now();
//!     driver
//! }
//! ```
//!
//! Only `run_port` asks a driver to retire:
//!
//! ```compile_fail,E0451
//! # use core::cell::Cell;
//! # use oer_ieee80211_port::Retire;
//! fn forge(requested: &Cell<bool>) -> Retire<'_> {
//!     Retire { requested }
//! }
//! ```
//!
//! Its compiling twin reads the request it was given:
//!
//! ```
//! # use oer_ieee80211_port::Retire;
//! fn read(retire: &Retire<'_>) -> bool {
//!     retire.is_requested()
//! }
//! ```
//!
//! The compiling twin of the lifecycle tests runs a phase between the
//! commands:
//!
//! ```
//! # use oer_ieee80211_port::{Disabled, Ended, Owner, PortBackend, PortBody};
//! struct OnePhase;
//! impl<B: PortBackend> PortBody<B> for OnePhase {
//!     async fn run<'id>(self, mut owner: Owner<'_, 'id, B, Disabled>, _: B::Resources<'id>) -> Ended<'id, B> {
//!         let _ = owner.setup();
//!         let mut enabled = match owner.enable().await {
//!             Ok(enabled) => enabled,
//!             Err(failed) => return failed.fail_stop(),
//!         };
//!         {
//!             let phase = enabled.phase();
//!             drop(phase);
//!         }
//!         match enabled.disable(&mut |_| {}).await {
//!             Ok(disabled) => disabled.fail_stop(),
//!             Err(poisoned) => poisoned.fail_stop(),
//!         }
//!     }
//! }
//! ```
//!
//! The compiling twins of the remaining tests differ only in the rule each
//! breaks: an owner comes from `run_port`, a disabled owner uninstalls, the
//! clock is reached through the owner, and the phase is borrowed through
//! its loan:
//!
//! ```
//! # use oer_ieee80211_port::{Disabled, Enabled, Ended, Owner, PortBackend, PortBody, run_port};
//! async fn own<B: PortBackend>(backend: B, body: impl PortBody<B>) -> Option<B::Uninstalled> {
//!     run_port(backend, body).await
//! }
//! fn uninstall_while_disabled<'id, B: PortBackend>(owner: Owner<'_, 'id, B, Disabled>, control: B::Uninstall) {
//!     let _ = owner.uninstall(control);
//! }
//! fn reach_through_the_owner<'id, B: PortBackend>(owner: &mut Owner<'_, 'id, B, Enabled>) {
//!     let _ = owner.now();
//! }
//! fn borrow_the_part<'id, B: PortBackend>(owner: &mut Owner<'_, 'id, B, Enabled>) {
//!     let _ = &mut *owner.phase();
//! }
//! ```
//!
//! [owner decisions on #563]: https://github.com/ermacv/open-esp-radio-rs/issues/563#issuecomment-6129037560
//! [ownership and refusals]: https://github.com/ermacv/open-esp-radio-rs/blob/main/docs/architecture.md#ownership-and-refusals
//! [refinement on #581]: https://github.com/ermacv/open-esp-radio-rs/issues/581#issuecomment-6133179528

use core::{
    cell::Cell,
    fmt::Debug,
    future::{Future, poll_fn},
    marker::PhantomData,
    ops::{Deref, DerefMut},
    pin::pin,
    sync::atomic::{AtomicBool, Ordering},
    task::Poll,
};

use oer_time::RadioInstant;

use oer_radio_port::{EventsLost, Mint, Poisoned, PortResult, branded};

/// The brand of one install: invariant, so no install's brand converts to
/// another's.
type Brand<'id> = PhantomData<fn(&'id ()) -> &'id ()>;

/// The state of an owner whose port admits no work and receives nothing.
pub enum Disabled {}

/// The state of an owner whose port runs.
pub enum Enabled {}

/// The result of a backend's `Enable`: the radio's refusal inside, a
/// poisoned backend outside.
pub type EnableResult<R, F> = Result<Result<(), R>, Poisoned<F>>;

/// What a backend implements; callers never hold it. [`run_port`] wraps it
/// in the body's owner, joins its driver with the body, and the owner and
/// `run_port` call each method only in the state its documentation names.
pub trait PortBackend: Sized {
    /// One owned event.
    type Event<'id>;
    /// The clock domain of the port's radio instants.
    type Domain;
    /// The cause a poisoned backend reports.
    type Fault: Copy + Debug;
    /// Why the radio refused `Enable` (a refused retune): the world's
    /// outcome, after which the port is disabled.
    type Refusal;
    /// The protocol's disabled-only settings, lent by [`Owner::setup`].
    type Setup<'a, 'id>
    where
        Self: 'a;
    /// The running phase, lent while the port is enabled: one object under
    /// `&mut`, the body's handle over its queues to the driver.
    type Phase<'a, 'id>: PortPhase<Event = Self::Event<'id>, Fault = Self::Fault>
    where
        Self: 'a,
        'id: 'a;
    /// The port-lifetime resources the body receives once with its owner:
    /// slot pools, permit pools and the seats of their lines, minted
    /// through [`Mint`].
    type Resources<'id>;
    /// The port-lifetime part of the install the backend keeps, shared by
    /// the body's parts and the driver: [`run_port`] keeps it for the
    /// port's life, lends it into each phase, disable and driver step, and
    /// hands it to the uninstall by value. It may carry the brand (the
    /// pools of the permits its work holds, which it gives back before it
    /// hands a terminal event out). Whatever the driver serves for the
    /// parts lives here: the state itself, reached synchronously, or the
    /// request channel, outcome queue and wakes to a driver that owns the
    /// state by value. `()` when it keeps none.
    type Held<'id>;
    /// The handle's own state of the install `'id`, which only the body's
    /// side reaches: the accounting of the slots it hands out (keys, Block
    /// Ack agreements). The owner keeps it and lends it by `&mut` into each
    /// setting, phase and disable, so it needs no lock and the driver never
    /// sees it ([owner decisions on #563]).
    type Local<'id>;
    /// What the driver owns by value and returns when it retires: the
    /// backend's state of a driver that owns it (the parts reach it by
    /// messages through `Held`), or `()` when the parts reach the state
    /// directly through `Held`.
    type State;
    /// What the uninstall takes beside the backend (the composition's
    /// control).
    type Uninstall;
    /// The hardware owners the uninstall returns.
    type Uninstalled;

    /// Enable the disabled port, within the backend's bound.
    fn enable(&mut self) -> impl Future<Output = EnableResult<Self::Refusal, Self::Fault>> + '_;

    /// Disable the enabled port of the install `'id` within the backend's
    /// bound: end every admitted work, hand its terminal event with what it
    /// returns to `sink`, and return when every permit is back in `held`.
    fn disable<'a, 'id>(
        &'a mut self,
        held: &'a Self::Held<'id>,
        local: &'a mut Self::Local<'id>,
        sink: &'a mut dyn FnMut(Self::Event<'id>),
    ) -> impl Future<Output = Result<(), Poisoned<Self::Fault>>> + 'a
    where
        'id: 'a;

    /// The radio clock.
    fn now(
        &self,
    ) -> impl Future<Output = PortResult<RadioInstant<Self::Domain>, ClockError, Self::Fault>> + '_;

    /// The port-lifetime part the backend keeps and the resources the body
    /// receives of the install `'id`, minted with `mint`; [`run_port`] calls
    /// it once.
    #[allow(clippy::type_complexity, reason = "the install's three parts")]
    fn resources<'id>(
        &mut self,
        mint: Mint<'id>,
    ) -> (Self::Held<'id>, Self::Local<'id>, Self::Resources<'id>);

    /// Lend the disabled-only settings of the install `'id`. An
    /// implementation names the associated types with both lifetimes
    /// (`Self::Setup<'a, 'id>`), as the trait does, so its `'id` stays a
    /// parameter of the method.
    fn setup<'a, 'id>(&'a mut self, local: &'a mut Self::Local<'id>) -> Self::Setup<'a, 'id>;

    /// Lend the running phase of the enabled port of the install `'id`:
    /// the body's handle over the queues to the driver in `held`.
    fn phase<'a, 'id>(
        &'a mut self,
        held: &'a Self::Held<'id>,
        local: &'a mut Self::Local<'id>,
    ) -> Self::Phase<'a, 'id>
    where
        'id: 'a;

    /// The driver of the install `'id`: the backend's runner, which
    /// [`run_port`] calls once, after [`resources`](Self::resources), and
    /// joins with the body in its task for the body's whole life, enabled
    /// or disabled. The call may move what the driver owns out of the
    /// backend; the future it returns borrows only `held` and `retire`,
    /// never the backend, which the owner lends to the body meanwhile.
    ///
    /// The driver serves what the backend runs in software (an interrupt
    /// status posted to it, a deadline, a backoff, a periodic refresh). It
    /// runs until `retire` is requested, then finishes its current step
    /// and returns its state within [`retire_bound`](Self::retire_bound).
    /// It may return earlier, when it has nothing more to serve;
    /// [`run_port`] keeps the state for the uninstall.
    ///
    /// # Cancel safety
    ///
    /// The driver must be cancel-safe: [`run_port`] drops it at any await
    /// point, at a fail-stop, at a missed retire bound and with its own
    /// future, and stops polling it once a fail-stop or a dropped owner
    /// abandoned the backend. A step that needs a resource across an await
    /// takes it out of `held` and puts it back when it completes, or from a
    /// guard when it drops; meanwhile the driver defers the work that needs
    /// it and the status stays latched.
    fn drive<'p, 'id>(
        &mut self,
        held: &'p Self::Held<'id>,
        retire: Retire<'p>,
    ) -> impl Future<Output = Self::State> + use<'p, 'id, Self>;

    /// The bound within which a driver asked to retire returns its state:
    /// a future that is ready once the bound has passed, measured on the
    /// backend's own timer. [`run_port`] awaits it beside the retiring
    /// driver; if it is ready first, the driver missed its bound and the
    /// port ends as a fail-stop.
    fn retire_bound(&self) -> impl Future<Output = ()> + '_;

    /// Detach the backend's interrupt entries of the install `'id`
    /// synchronously, so none reaches the install's state again.
    /// [`run_port`] calls it once, after the driver retired and before
    /// [`uninstall`](Self::uninstall).
    fn detach<'id>(&mut self, held: &Self::Held<'id>);

    /// Take the disabled or poisoned backend apart with the composition's
    /// `control`, its held part and its retired driver's state, each by
    /// value, and return the hardware owners by value. [`run_port`] calls
    /// it once, after [`detach`](Self::detach).
    fn uninstall<'id>(
        self,
        held: Self::Held<'id>,
        local: Self::Local<'id>,
        state: Self::State,
        control: Self::Uninstall,
    ) -> Self::Uninstalled;

    /// Stop the backend at once, synchronously: close transmission, mask its
    /// interrupts and refuse any new install until a reset. A fail-stop, a
    /// dropped owner and a driver that missed its retire bound call it;
    /// [`run_port`] polls the driver no more, and the hardware stays
    /// installed and unreachable.
    fn abandon(&mut self);
}

/// The one lifetime of a port, from its install to its [`Ended`]; see the
/// [module](self) documentation.
pub trait PortBody<B: PortBackend> {
    /// Run the port: the owner arrives disabled and leaves through an
    /// uninstall or a fail-stop.
    ///
    /// The body is generic over the install's brand `'id`: it names no
    /// concrete one and cannot prove one `'static`, so nothing branded by
    /// `'id` (the owner, its resources, the tokens its clients mint)
    /// leaves the body or reaches another install. `'p` is the owner's
    /// borrow of the install [`run_port`] keeps.
    fn run<'p, 'id>(
        self,
        owner: Owner<'p, 'id, B, Disabled>,
        resources: B::Resources<'id>,
    ) -> impl Future<Output = Ended<'id, B>>;
}

/// The retire request [`run_port`] makes of a driver
/// ([`PortBackend::drive`]) once the body asked for the uninstall, and the
/// install's [`AbandonLatch`]. It is neither `Clone` nor `Copy`, and only
/// [`run_port`] makes one.
pub struct Retire<'p> {
    requested: &'p Cell<bool>,
    latch: &'p AbandonLatch,
}

impl<'p> Retire<'p> {
    /// Whether [`run_port`] asked the driver to retire.
    pub fn is_requested(&self) -> bool {
        self.requested.get()
    }

    /// Ready once [`run_port`] asked the driver to retire. It registers no
    /// waker: [`run_port`] polls the driver right after it asks.
    pub fn requested(&self) -> impl Future<Output = ()> + '_ {
        poll_fn(|_| {
            if self.requested.get() {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        })
    }

    /// Whether the install was abandoned: a driver that sees it publishes
    /// no further attempt and stops.
    pub fn is_abandoned(&self) -> bool {
        self.latch.is_set()
    }

    /// The install's abandon latch, for a driver part that runs elsewhere
    /// (another executor, an interrupt entry) and must see it too.
    pub const fn latch(&self) -> &'p AbandonLatch {
        self.latch
    }
}

/// The latch of an abandoned install. A dropped owner sets it at once, as
/// do a fail-stop and a driver that missed its retire bound; it is never
/// cleared. It is atomic, so a driver in another executor or an interrupt
/// entry sees it: a driver that sees it publishes no further attempt and
/// stops. Only [`run_port`] makes one.
pub struct AbandonLatch(AtomicBool);

impl AbandonLatch {
    const fn new() -> Self {
        Self(AtomicBool::new(false))
    }

    fn set(&self) {
        self.0.store(true, Ordering::Release);
    }

    /// Whether the install was abandoned.
    pub fn is_set(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

/// The install [`run_port`] keeps for the port's life: the backend's held
/// part, which the body's parts and the driver share, and the driver's
/// signals.
struct Install<'id, B: PortBackend> {
    held: B::Held<'id>,
    /// Set by a dropped owner, a fail-stop and a missed retire bound: the
    /// driver is polled no more and publishes nothing.
    latch: AbandonLatch,
    /// Whether [`PortBackend::abandon`] ran, so it runs once.
    abandoned: Cell<bool>,
    /// Set by `run_port` once the body asked for the uninstall.
    retiring: Cell<bool>,
}

impl<B: PortBackend> Install<'_, B> {
    /// Abandon `backend` once: set the latch, then stop the backend
    /// (close transmission, mask its interrupts, halt the driver).
    fn abandon(&self, backend: &mut B) {
        self.latch.set();
        if !self.abandoned.replace(true) {
            backend.abandon();
        }
    }
}

/// Run `body` over a just-installed, disabled `backend`. A backend's install
/// calls it, in the executor task that runs the port, and receives the
/// hardware owners an uninstall returned, or `None` after a fail-stop or a
/// dropped owner.
///
/// It joins the body with the backend's driver ([`PortBackend::drive`]),
/// polling both at every wake until the body ends, so the driver's
/// progress never depends on what the body awaits. After an uninstall it
/// asks the driver to retire within [`PortBackend::retire_bound`] (a
/// missed bound is a fail-stop), then calls [`PortBackend::detach`] and
/// [`PortBackend::uninstall`]. A body that dropped its owner is dropped at
/// the end of that poll, and the port ends abandoned.
pub async fn run_port<B: PortBackend>(
    backend: B,
    body: impl PortBody<B>,
) -> Option<B::Uninstalled> {
    branded(async move |mint| install(backend, body, mint).await).await
}

/// [`run_port`] over the install `'id`, whose `mint` [`branded`] made.
async fn install<'id, B: PortBackend>(
    mut backend: B,
    body: impl PortBody<B>,
    mint: Mint<'id>,
) -> Option<B::Uninstalled> {
    let (held, local, resources) = backend.resources(mint);
    let install = Install {
        held,
        latch: AbandonLatch::new(),
        abandoned: Cell::new(false),
        retiring: Cell::new(false),
    };
    let mut owned = Owned { backend, local };
    // Until the uninstall takes the backend, a cancelled `run_port`
    // abandons it.
    let armed = Armed {
        owned: &mut owned,
        install: &install,
        armed: true,
    };
    let (state, control) = {
        let mut driver = pin!(armed.owned.backend.drive(
            &install.held,
            Retire {
                requested: &install.retiring,
                latch: &install.latch,
            },
        ));
        let (ended, state) = {
            let mut body = pin!(body.run(Owner::new(&mut *armed.owned, &install), resources));
            let mut state = None;
            let ended = poll_fn(|cx| {
                if let Poll::Ready(ended) = body.as_mut().poll(cx) {
                    return Poll::Ready(Some(ended));
                }
                if install.latch.is_set() {
                    // The body dropped its owner (a fail-stop's body ends
                    // on: the backend is already abandoned).
                    if !install.abandoned.get() {
                        return Poll::Ready(None);
                    }
                } else if state.is_none()
                    && let Poll::Ready(retired) = driver.as_mut().poll(cx)
                {
                    state = Some(retired);
                }
                Poll::Pending
            })
            .await;
            (ended, state)
        };
        let Some(ended) = ended else {
            armed.abandon();
            return None;
        };
        let End::Uninstall(control) = ended.end else {
            // A fail-stop already abandoned the backend.
            armed.disarm();
            return None;
        };
        let state = match state {
            Some(state) => state,
            None => {
                install.retiring.set(true);
                let Some(state) = retired(&armed.owned.backend, driver).await else {
                    armed.abandon();
                    return None;
                };
                state
            }
        };
        (state, control)
    };
    armed.disarm();
    let Owned { mut backend, local } = owned;
    backend.detach(&install.held);
    Some(backend.uninstall(install.held, local, state, control))
}

/// The install's backend in `run_port`, from the install to its uninstall:
/// dropped armed with a cancelled `run_port`, it abandons the backend through
/// the same once-only latch as every other fail-stop. Drop never panics.
struct Armed<'o, 'id, B: PortBackend> {
    owned: &'o mut Owned<'id, B>,
    install: &'o Install<'id, B>,
    armed: bool,
}

impl<B: PortBackend> Armed<'_, '_, B> {
    /// The port ended, or the uninstall takes the backend.
    fn disarm(mut self) {
        self.armed = false;
    }

    /// The port ends abandoned: a dropped owner or a missed retire bound.
    fn abandon(mut self) {
        self.install.abandon(&mut self.owned.backend);
        self.armed = false;
    }
}

impl<B: PortBackend> Drop for Armed<'_, '_, B> {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        self.install.abandon(&mut self.owned.backend);
    }
}

/// The state of a driver asked to retire, or `None` once the backend's
/// retire bound passed first.
async fn retired<B: PortBackend>(
    backend: &B,
    mut driver: core::pin::Pin<&mut impl Future<Output = B::State>>,
) -> Option<B::State> {
    let mut bound = pin!(backend.retire_bound());
    poll_fn(|cx| {
        if let Poll::Ready(state) = driver.as_mut().poll(cx) {
            return Poll::Ready(Some(state));
        }
        if bound.as_mut().poll(cx).is_ready() {
            return Poll::Ready(None);
        }
        Poll::Pending
    })
    .await
}

/// How a port's body ended: the uninstall it asked for, which [`run_port`]
/// carries out, or a fail-stop, which leaves the hardware installed until a
/// reset. Only the uninstalls and the `fail_stop`s construct it.
#[must_use = "an ended port returns its hardware owners"]
pub struct Ended<'id, B: PortBackend> {
    end: End<B::Uninstall>,
    brand: Brand<'id>,
}

/// What [`Ended`] asks of [`run_port`].
enum End<U> {
    /// The uninstall, with the composition's control.
    Uninstall(U),
    /// Nothing: the backend is abandoned.
    FailStop,
}

impl<'id, B: PortBackend> Ended<'id, B> {
    const fn uninstall(control: B::Uninstall) -> Self {
        Self {
            end: End::Uninstall(control),
            brand: PhantomData,
        }
    }

    const fn fail_stop() -> Self {
        Self {
            end: End::FailStop,
            brand: PhantomData,
        }
    }
}

/// The sentry an owner carries: dropped with its owner, it sets the
/// install's [`AbandonLatch`] at once; the uninstalls and the `fail_stop`s
/// disarm it.
struct Sentry<'p> {
    latch: &'p AbandonLatch,
}

impl Sentry<'_> {
    fn disarm(self) {
        core::mem::forget(self);
    }
}

impl Drop for Sentry<'_> {
    fn drop(&mut self) {
        self.latch.set();
    }
}

/// The one authority over an installed port, in state `S`; see the
/// [module](self) documentation. `'p` is its borrow of the install
/// [`run_port`] keeps; `'id` is the install's brand.
#[must_use = "an owner leaves only through a transition, an uninstall or a fail-stop"]
pub struct Owner<'p, 'id, B: PortBackend, S> {
    /// The backend and the handle's own state, which `run_port` keeps.
    owned: &'p mut Owned<'id, B>,
    /// The install: the part the backend keeps, and the driver's latch.
    install: &'p Install<'id, B>,
    sentry: Sentry<'p>,
    state: PhantomData<S>,
}

impl<'p, 'id, B: PortBackend, S> Owner<'p, 'id, B, S> {
    fn new(owned: &'p mut Owned<'id, B>, install: &'p Install<'id, B>) -> Self {
        Self::with(
            owned,
            install,
            Sentry {
                latch: &install.latch,
            },
        )
    }

    const fn with(
        owned: &'p mut Owned<'id, B>,
        install: &'p Install<'id, B>,
        sentry: Sentry<'p>,
    ) -> Self {
        Self {
            owned,
            install,
            sentry,
            state: PhantomData,
        }
    }

    /// The radio clock.
    pub fn now(
        &self,
    ) -> impl Future<Output = PortResult<RadioInstant<B::Domain>, ClockError, B::Fault>> + '_ {
        self.owned.backend.now()
    }

    /// Stop the backend and its driver at once and end the port without its
    /// uninstall: the hardware stays installed and unreachable until a
    /// reset.
    pub fn fail_stop(self) -> Ended<'id, B> {
        let Self {
            owned,
            install,
            sentry,
            ..
        } = self;
        install.abandon(&mut owned.backend);
        sentry.disarm();
        Ended::fail_stop()
    }
}

impl<'p, 'id, B: PortBackend> Owner<'p, 'id, B, Disabled> {
    /// Lend the protocol's disabled-only settings.
    pub fn setup(&mut self) -> Lent<'_, B::Setup<'_, 'id>> {
        let owned = &mut *self.owned;
        Lent::new(owned.backend.setup(&mut owned.local))
    }

    /// Enable the port, within the backend's bound.
    ///
    /// # Errors
    ///
    /// The radio's refusal, with the disabled owner, or a poisoned backend.
    pub async fn enable(self) -> Result<Owner<'p, 'id, B, Enabled>, Failed<'p, 'id, B>> {
        // The future holds the owner's sentry across the wait, so a
        // dropped future sets the latch.
        let Self {
            owned,
            install,
            sentry,
            ..
        } = self;
        match owned.backend.enable().await {
            Ok(Ok(())) => Ok(Owner::with(owned, install, sentry)),
            Ok(Err(refusal)) => Err(Failed::Refused(
                Owner::with(owned, install, sentry),
                refusal,
            )),
            Err(poisoned) => Err(Failed::Poisoned(PoisonedOwner {
                owned,
                install,
                sentry,
                cause: poisoned.cause,
            })),
        }
    }

    /// Ask for the uninstall with the composition's `control`: the body
    /// returns the [`Ended`], and [`run_port`] retires the driver, detaches
    /// the interrupt entries and takes the backend apart.
    pub fn uninstall(self, control: B::Uninstall) -> Ended<'id, B> {
        self.sentry.disarm();
        Ended::uninstall(control)
    }
}

impl<'p, 'id, B: PortBackend> Owner<'p, 'id, B, Enabled> {
    /// Lend the running phase, a [`Lent`] of this borrow.
    pub fn phase(&mut self) -> Lent<'_, B::Phase<'_, 'id>> {
        let owned = &mut *self.owned;
        Lent::new(owned.backend.phase(&self.install.held, &mut owned.local))
    }

    /// Disable the port within the backend's bound: every leftover terminal
    /// of a phase's work, with what it returns, goes to `sink`, and the port
    /// is disabled once every permit is back.
    ///
    /// # Errors
    ///
    /// A poisoned backend.
    pub async fn disable(
        self,
        sink: &mut dyn FnMut(B::Event<'id>),
    ) -> Result<Owner<'p, 'id, B, Disabled>, PoisonedOwner<'p, 'id, B>> {
        // The future holds the owner's sentry across the wait, as
        // `enable`'s does.
        let Self {
            owned,
            install,
            sentry,
            ..
        } = self;
        let disabled = {
            let Owned { backend, local } = &mut *owned;
            backend.disable(&install.held, local, sink).await
        };
        match disabled {
            Ok(()) => Ok(Owner::with(owned, install, sentry)),
            Err(poisoned) => Err(PoisonedOwner {
                owned,
                install,
                sentry,
                cause: poisoned.cause,
            }),
        }
    }
}

/// Why `Enable` did not leave an enabled owner.
pub enum Failed<'p, 'id, B: PortBackend> {
    /// The radio refused; the port is disabled.
    Refused(Owner<'p, 'id, B, Disabled>, B::Refusal),
    /// The backend is poisoned.
    Poisoned(PoisonedOwner<'p, 'id, B>),
}

impl<'id, B: PortBackend> Failed<'_, 'id, B> {
    /// Stop the backend and its driver at once and end the port.
    pub fn fail_stop(self) -> Ended<'id, B> {
        match self {
            Self::Refused(owner, _) => owner.fail_stop(),
            Self::Poisoned(poisoned) => poisoned.fail_stop(),
        }
    }
}

/// The one exit of a poisoned backend: its state is unknown, and it leaves
/// through its uninstall or a fail-stop.
#[must_use = "a poisoned backend leaves only through its uninstall or a fail-stop"]
pub struct PoisonedOwner<'p, 'id, B: PortBackend> {
    owned: &'p mut Owned<'id, B>,
    install: &'p Install<'id, B>,
    sentry: Sentry<'p>,
    cause: B::Fault,
}

impl<'id, B: PortBackend> PoisonedOwner<'_, 'id, B> {
    /// Why the backend's state is unknown.
    pub const fn cause(&self) -> B::Fault {
        self.cause
    }

    /// Ask for the uninstall with the composition's `control`, as
    /// [`Owner::uninstall`] does.
    pub fn uninstall(self, control: B::Uninstall) -> Ended<'id, B> {
        self.sentry.disarm();
        Ended::uninstall(control)
    }

    /// Stop the backend and its driver at once and end the port without
    /// its uninstall.
    pub fn fail_stop(self) -> Ended<'id, B> {
        let Self {
            owned,
            install,
            sentry,
            ..
        } = self;
        install.abandon(&mut owned.backend);
        sentry.disarm();
        Ended::fail_stop()
    }
}

/// The backend with the handle's own state of the install `'id`, which
/// `run_port` keeps and its owners borrow.
struct Owned<'id, B: PortBackend> {
    backend: B,
    local: B::Local<'id>,
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
    /// Lend `part` for the borrow of the owner that the backend's `phase` or
    /// `setup` received.
    const fn new(part: T) -> Self {
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

/// The running phase of a port: the body's handle over its queues to the
/// driver, whose event stream it reads.
pub trait PortPhase {
    /// An event of the stream.
    type Event;
    /// The cause a poisoned backend reports.
    type Fault: Copy + Debug;

    /// The next event, in the order the events occurred. It only reads the
    /// driver's event queue, so it is cancel-safe: a dropped future loses
    /// no event.
    fn next_event(
        &mut self,
    ) -> impl Future<Output = PortResult<Self::Event, EventsLost, Self::Fault>> + '_;
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

#[cfg(test)]
mod tests;
