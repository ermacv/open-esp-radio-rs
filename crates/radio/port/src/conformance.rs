//! The shared conformance suite of every radio port.
//!
//! A backend's test wraps it in a [`Harness`]: the port, the way to drive
//! its futures, and the hooks that make the backend's hardware (or model)
//! do what a rule needs: admit work, end it, produce data the protocol does
//! not promise to deliver, and poison itself. Each harness rule below is a
//! function over a harness; [`check_all`] runs them in turn. The one rule
//! of a permit pool, [`permits_serve_takers_in_arrival_order`], runs apart
//! from it: its pool's owner (a backend's held part, a runtime) calls it
//! with the pool and at most eight permits it holds aside. A rule that a
//! backend or pool breaks panics with what it observed, so the test fails.
//!
//! The rules are the shared contract of [`RadioPort`]
//! (`docs/architecture.md`, "Radio ports"); each port's own rules, such as
//! a refusal returning its arguments, live beside its definition and use the
//! same harness.

use core::{
    fmt::Debug,
    future::Future,
    pin::pin,
    task::{Context, Poll, Waker},
};

use crate::{
    BACKEND_RESERVED, CancelError, Correlation, CorrelationIds, EventsLost, LifecycleCommand,
    LifecycleEvent, PortResult, RadioPort,
};

/// What an event is, as far as the shared rules care.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EventClass<I> {
    /// The terminal event of the admitted operation `I`.
    Terminal(I),
    /// The terminal event of a lifecycle command.
    Lifecycle(LifecycleEvent),
    /// Any other event: data the protocol does not promise to deliver, or
    /// an extension event.
    Other,
}

/// A backend under test: its port and the hooks the rules need.
///
/// The hooks act on the backend the way its hardware (or its model's test
/// controls) would; none of them is a port call.
pub trait Harness {
    /// The port the backend serves; the rules compare its identities and
    /// poisoning causes.
    type Port: RadioPort<Id: Debug + PartialEq, Fault: PartialEq>;

    /// The port.
    fn port(&self) -> &Self::Port;

    /// Drive `future` to its end, polling beside it whatever the backend
    /// needs to make progress (a runner). A port's future that never ends
    /// is a broken rule.
    fn drive<F: Future>(&self, future: F) -> F::Output;

    /// Admit one operation with a terminal event that stays in flight until
    /// it is cancelled or [`Self::finish`]ed, on an enabled port; its
    /// identity. A synchronous setting has no terminal and is not one, and
    /// neither is a resting state that cannot be cancelled.
    fn admit(&self) -> <Self::Port as RadioPort>::Id;

    /// End the admitted operation `id` the way its hardware does on its own.
    /// The rules call it only for an operation that has not ended.
    fn finish(&self, id: <Self::Port as RadioPort>::Id);

    /// Run the backend until it has nothing left to do now (its runner), so
    /// every event it owes is ready. A backend whose events are ready when
    /// they occur, such as a model, does nothing.
    fn settle(&self) {}

    /// Produce one event the protocol does not promise to deliver (a
    /// received frame, an advertising report), on an enabled port. A
    /// backend that delivers such data only with the end of an operation of
    /// its own (a scan event) names that operation: its terminal is
    /// reserved and must never be lost.
    fn produce_undelivered(&self) -> Option<<Self::Port as RadioPort>::Id>;

    /// How many events [`Self::produce_undelivered`] makes fit before the
    /// next one is lost, with nothing admitted (a backend's terminal
    /// reservations may share the queue).
    fn undelivered_capacity(&self) -> usize;

    /// What `event` is.
    fn classify(
        event: &<Self::Port as RadioPort>::Event,
    ) -> EventClass<<Self::Port as RadioPort>::Id>;
}

/// A backend that can poison itself. A backend that never poisons (its
/// `Fault` is uninhabited, every failure recoverable) does not implement it,
/// and the poisoning rule does not run on it.
pub trait PoisoningHarness: Harness {
    /// Poison the backend, as an unrecoverable fault does.
    fn poison(&self);
}

type Id<H> = <<H as Harness>::Port as RadioPort>::Id;
type Fault<H> = <<H as Harness>::Port as RadioPort>::Fault;
type Taken<H> = PortResult<EventClass<Id<H>>, EventsLost, Fault<H>>;

/// The next event, classified; a port that has none is a broken rule.
fn take<H: Harness>(harness: &H) -> Taken<H> {
    harness
        .drive(harness.port().next_event())
        .map(|event| event.map(|event| H::classify(&event)))
}

/// The next event if one is ready after the backend settled; `None` when
/// it has none. A port has no peek: polling an event takes it.
fn try_take<H: Harness>(harness: &H) -> Option<Taken<H>> {
    harness.settle();
    let mut next = pin!(harness.port().next_event());
    match next.as_mut().poll(&mut Context::from_waker(Waker::noop())) {
        Poll::Ready(event) => Some(event.map(|event| event.map(|event| H::classify(&event)))),
        Poll::Pending => None,
    }
}

/// The next event if one is ready; otherwise the backend still holds the
/// operation `id`, which then ends the way its hardware ends it, and its
/// event is taken.
fn take_or_finish<H: Harness>(harness: &H, id: Id<H>) -> Taken<H> {
    try_take(harness).unwrap_or_else(|| {
        harness.finish(id);
        take(harness)
    })
}

/// Whether the settled port has no event ready. An event that is ready is
/// taken, so this only closes a rule.
pub fn idle<H: Harness>(harness: &H) -> bool {
    try_take(harness).is_none()
}

/// What a burst of produced events gave back.
#[derive(Debug, Default, PartialEq)]
struct Burst {
    /// Other events before the loss marker.
    before: usize,
    /// Loss markers.
    losses: usize,
    /// Other events after the marker.
    after: usize,
    /// Terminals of the operations that carried the events.
    terminals: usize,
}

/// Take every ready event, counting it; the poisoning cause, if the port
/// poisoned, ends the count.
fn drain<H: Harness>(harness: &H) -> (Burst, Option<crate::Poisoned<Fault<H>>>) {
    let mut burst = Burst::default();
    while let Some(taken) = try_take(harness) {
        match taken {
            Ok(Ok(EventClass::Other)) if burst.losses == 0 => burst.before += 1,
            Ok(Ok(EventClass::Other)) => burst.after += 1,
            Ok(Err(EventsLost)) => burst.losses += 1,
            Ok(Ok(EventClass::Terminal(_))) => burst.terminals += 1,
            Ok(Ok(EventClass::Lifecycle(event))) => {
                panic!("no lifecycle command ran, yet {event:?} came")
            }
            Err(poisoned) => return (burst, Some(poisoned)),
        }
    }
    (burst, None)
}

/// Produce `count` undelivered events; how many reserved terminals they
/// caused.
fn produce<H: Harness>(harness: &H, count: usize) -> usize {
    (0..count)
        .filter(|_| harness.produce_undelivered().is_some())
        .count()
}

/// Run a lifecycle command and take its terminal, which must be `expected`
/// and must come before any other event.
fn lifecycle<H: Harness>(harness: &H, command: LifecycleCommand, expected: LifecycleEvent) {
    assert!(
        matches!(harness.drive(harness.port().lifecycle(command)), Ok(Ok(()))),
        "{command:?} is admitted"
    );
    assert_eq!(
        take(harness),
        Ok(Ok(EventClass::Lifecycle(expected))),
        "{command:?} ends with {expected:?}"
    );
}

/// Enable the port and take its terminal.
pub fn enable<H: Harness>(harness: &H) {
    lifecycle(harness, LifecycleCommand::Enable, LifecycleEvent::Enabled);
}

/// Every event produced before a loss comes before the loss marker, the
/// marker comes once, in place of the first lost event, and an event
/// produced after the loss comes after it. A reserved terminal of an
/// operation that carried such events is never lost. Nothing else is
/// reported.
pub fn losses_follow_the_events_before_them<H: Harness>(harness: &H) {
    enable(harness);
    let capacity = harness.undelivered_capacity();
    let terminals = produce(harness, capacity + 2);
    let (burst, poisoned) = drain(harness);
    assert!(poisoned.is_none(), "the port did not poison");
    assert_eq!(
        burst,
        Burst {
            before: capacity,
            losses: 1,
            after: 0,
            terminals,
        },
        "the {capacity} events that fit, one loss marker, every reserved terminal"
    );
    let terminals = produce(harness, 1);
    let (burst, poisoned) = drain(harness);
    assert!(poisoned.is_none(), "the port did not poison");
    assert_eq!(
        burst,
        Burst {
            before: 1,
            losses: 0,
            after: 0,
            terminals,
        },
        "an event produced after the loss follows its marker, nothing else"
    );
}

/// A cancel proves an operation's end: an admitted operation ends with its
/// own terminal event, exactly once, and a cancel of an ended one is
/// [`CancelError::NotRunning`].
pub fn a_cancel_proves_the_end<H: Harness>(harness: &H) {
    enable(harness);
    let id = harness.admit();
    assert_eq!(
        harness.drive(harness.port().cancel(id)).ok(),
        Some(Ok(())),
        "a cancel of an admitted operation is admitted"
    );
    // A published operation may end only when its hardware does.
    assert_eq!(
        take_or_finish(harness, id),
        Ok(Ok(EventClass::Terminal(id))),
        "the cancelled operation ends with its own terminal"
    );
    assert!(idle(harness), "it ends exactly once");
    assert_eq!(
        harness.drive(harness.port().cancel(id)).ok(),
        Some(Err(CancelError::NotRunning)),
        "a cancel of an ended operation proves its end"
    );
}

/// `Disable` ends the admitted work first: the terminal of every admitted
/// operation comes before `Disabled`, and nothing after it.
pub fn disable_ends_admitted_work_before_its_terminal<H: Harness>(harness: &H) {
    enable(harness);
    let id = harness.admit();
    assert!(
        matches!(
            harness.drive(harness.port().lifecycle(LifecycleCommand::Disable)),
            Ok(Ok(()))
        ),
        "Disable is admitted with work in flight"
    );
    assert_eq!(
        take_or_finish(harness, id),
        Ok(Ok(EventClass::Terminal(id))),
        "the admitted operation ends before Disabled"
    );
    assert_eq!(
        take(harness),
        Ok(Ok(EventClass::Lifecycle(LifecycleEvent::Disabled))),
        "then Disabled"
    );
    assert!(idle(harness), "nothing follows Disabled");
}

/// A poisoned backend reports every event produced before the fault, then
/// [`Poisoned`](crate::Poisoned) at that and every later event call, and at
/// once from every other call.
pub fn poisoning_repeats_after_the_events_before_it<H: PoisoningHarness>(harness: &H) {
    enable(harness);
    let terminals = produce(harness, 1);
    harness.poison();
    let (burst, poisoned) = drain(harness);
    assert_eq!(
        burst,
        Burst {
            before: 1,
            losses: 0,
            after: 0,
            terminals,
        },
        "the events before the fault are reported"
    );
    let first = poisoned.expect("then the port is poisoned");
    assert_eq!(
        take(harness).err(),
        Some(first),
        "every later event call is poisoned"
    );
    assert_eq!(
        harness
            .drive(harness.port().lifecycle(LifecycleCommand::Enable))
            .err(),
        Some(first),
        "a lifecycle command is poisoned"
    );
    assert_eq!(
        harness
            .drive(harness.port().cancel(Id::<H>::from_raw(0)))
            .err(),
        Some(first),
        "a cancel is poisoned"
    );
    assert!(
        harness.drive(harness.port().now()).is_err(),
        "the clock is poisoned"
    );
}

/// `Quiesce` stops admission and lets the admitted work end: its terminals
/// come before `Quiesced`, nothing follows, and `Enable` admits work again.
pub fn quiesce_ends_after_admitted_work<H: Harness>(harness: &H) {
    enable(harness);
    let id = harness.admit();
    assert!(
        matches!(
            harness.drive(harness.port().lifecycle(LifecycleCommand::Quiesce)),
            Ok(Ok(()))
        ),
        "Quiesce is admitted with work in flight"
    );
    assert_eq!(
        take_or_finish(harness, id),
        Ok(Ok(EventClass::Terminal(id))),
        "the admitted operation ends before Quiesced"
    );
    assert_eq!(
        take(harness),
        Ok(Ok(EventClass::Lifecycle(LifecycleEvent::Quiesced))),
        "then Quiesced"
    );
    assert!(idle(harness), "nothing follows Quiesced");
    enable(harness);
    let id = harness.admit();
    harness.finish(id);
    assert_eq!(
        take(harness),
        Ok(Ok(EventClass::Terminal(id))),
        "an enabled port admits and ends work again"
    );
}

/// The caller's identities never enter the range the backend reserves.
pub fn caller_identities_avoid_the_backend_range<I: Correlation>() {
    let mut ids = CorrelationIds::starting_at(*BACKEND_RESERVED.start() - 2);
    for _ in 0..4 {
        let id: I = ids.next();
        assert!(
            !BACKEND_RESERVED.contains(&id.raw()),
            "{} lies in the backend's range",
            id.raw()
        );
    }
}

/// A [`PermitPool`](crate::PermitPool) of `held.len() + 1` permits whose
/// documented order is first in, first out: with every permit but one
/// held, three takers wait in arrival order, and each released permit goes
/// to the earliest of them; a taker that gave up its place before then
/// stalls none of them. `held` are the other permits, taken by the caller,
/// at most eight.
///
/// # Panics
///
/// The pool serves a later taker first, or none at all.
pub fn permits_serve_takers_in_arrival_order<'id, K, P: crate::PermitPool<'id, K>>(
    pool: &P,
    held: impl IntoIterator<Item = crate::Permit<'id, K>>,
) {
    let mut context = Context::from_waker(Waker::noop());
    let held: alloc_free::Held<'id, K> = alloc_free::Held::new(held);
    let mut last = match pin!(pool.take()).as_mut().poll(&mut context) {
        Poll::Ready(permit) => permit,
        Poll::Pending => panic!("a pool with a free permit serves its first taker"),
    };
    // A taker that gives up its place before it is served stalls no one.
    {
        let mut gives_up = pin!(pool.take());
        assert!(
            gives_up.as_mut().poll(&mut context).is_pending(),
            "a taker waits while every permit is held"
        );
    }
    let mut first = pin!(pool.take());
    let mut second = pin!(pool.take());
    let mut third = pin!(pool.take());
    for taker in [
        first.as_mut().poll(&mut context),
        second.as_mut().poll(&mut context),
        third.as_mut().poll(&mut context),
    ] {
        assert!(
            taker.is_pending(),
            "a taker waits while every permit is held"
        );
    }
    pool.release(last);
    assert!(
        second.as_mut().poll(&mut context).is_pending(),
        "a released permit goes to the earliest taker, not a later one"
    );
    last = match first.as_mut().poll(&mut context) {
        Poll::Ready(permit) => permit,
        Poll::Pending => panic!("the earliest taker gets the released permit"),
    };
    pool.release(last);
    let Poll::Ready(permit) = second.as_mut().poll(&mut context) else {
        panic!("the next taker gets the next released permit");
    };
    pool.release(permit);
    let Poll::Ready(permit) = third.as_mut().poll(&mut context) else {
        panic!("every waiting taker is served in turn");
    };
    pool.release(permit);
    held.release_into(pool);
}

/// The permits a rule holds aside, without an allocator.
mod alloc_free {
    /// At most this many permits held aside.
    const HELD: usize = 8;

    pub struct Held<'id, K> {
        permits: [Option<crate::Permit<'id, K>>; HELD],
    }

    impl<'id, K> Held<'id, K> {
        pub fn new(held: impl IntoIterator<Item = crate::Permit<'id, K>>) -> Self {
            let mut permits = [const { None }; HELD];
            let mut held = held.into_iter();
            for place in &mut permits {
                *place = held.next();
            }
            assert!(
                held.next().is_none(),
                "the rule holds at most {HELD} permits aside"
            );
            Self { permits }
        }

        pub fn release_into<P: crate::PermitPool<'id, K>>(self, pool: &P) {
            for permit in self.permits.into_iter().flatten() {
                pool.release(permit);
            }
        }
    }
}

/// Run every shared rule but poisoning on fresh harnesses `new` builds, one
/// per rule, so one rule's state never meets another's.
pub fn check_all<H: Harness>(mut new: impl FnMut() -> H) {
    losses_follow_the_events_before_them(&new());
    a_cancel_proves_the_end(&new());
    disable_ends_admitted_work_before_its_terminal(&new());
    quiesce_ends_after_admitted_work(&new());
    caller_identities_avoid_the_backend_range::<Id<H>>();
}

/// Run every shared rule, poisoning included, for a backend that poisons.
pub fn check_all_poisoning<H: PoisoningHarness>(mut new: impl FnMut() -> H) {
    check_all(&mut new);
    poisoning_repeats_after_the_events_before_it(&new());
}

/// Poll `future` with a waker that does nothing until it is ready; for a
/// harness whose backend needs nothing polled beside its futures (a model
/// whose calls end at once). A future pending after `limit` polls is a
/// broken rule.
pub fn drive_alone<F: Future>(future: F, limit: usize) -> F::Output {
    let mut future = pin!(future);
    let mut context = Context::from_waker(Waker::noop());
    for _ in 0..limit {
        if let Poll::Ready(output) = future.as_mut().poll(&mut context) {
            return output;
        }
    }
    panic!("a port future did not end within {limit} polls");
}
