extern crate std;

use core::{
    future::{Future, pending, ready},
    pin::pin,
    task::{Context, Poll, Waker},
};

use oer_radio_port::{
    ClockError, Disabled, EnableResult, Ended, EventsLost, Lent, Mint, Owner, Permit, PermitPool,
    PhaseParts, Poisoned, PortBackend, PortBody, PortResult, RadioPortEvents, Retire, conformance,
    run_port,
};
use oer_time::RadioInstant;

use super::FifoPermitPool;

/// The permits' kind.
enum Kind {}

/// The probe's clock domain.
enum Domain {}

#[derive(Clone, Copy, Debug)]
struct Fault;

struct NoEvents;

impl RadioPortEvents for NoEvents {
    type Event = ();
    type Fault = Fault;

    fn next_event(&mut self) -> impl Future<Output = PortResult<(), EventsLost, Fault>> + '_ {
        pending()
    }
}

/// A backend whose only resource is `N` permits of one install.
struct Probe<const N: usize>;

impl<const N: usize> PortBackend for Probe<N> {
    type Event = ();
    type Domain = Domain;
    type Fault = Fault;
    type Refusal = ();
    type Setup<'a, 'id> = ();
    type Events<'a, 'id>
        = NoEvents
    where
        'id: 'a;
    type Control<'a, 'id>
        = ()
    where
        'id: 'a;
    type Clients<'a, 'id>
        = ()
    where
        'id: 'a;
    type Resources<'id> = [Permit<'id, Kind>; N];
    type Held<'id> = ();
    type State = ();
    type Uninstall = ();
    type Uninstalled = ();

    fn enable(&mut self) -> impl Future<Output = EnableResult<(), Fault>> + '_ {
        ready(Ok(Ok(())))
    }

    fn disable<'a, 'id>(
        &'a mut self,
        _: &'a (),
        _: &'a mut dyn FnMut(()),
    ) -> impl Future<Output = Result<(), Poisoned<Fault>>> + 'a
    where
        'id: 'a,
    {
        ready(Ok(()))
    }

    fn now(
        &self,
    ) -> impl Future<Output = PortResult<RadioInstant<Domain>, ClockError, Fault>> + '_ {
        ready(Ok(Ok(RadioInstant::from_micros(0))))
    }

    fn resources<'id>(&mut self, mut mint: Mint<'id>) -> ((), [Permit<'id, Kind>; N]) {
        ((), mint.permits())
    }

    fn setup<'a, 'id>(&'a mut self) -> Lent<'a, Self::Setup<'a, 'id>> {
        Lent::new(())
    }

    fn split<'a, 'id>(&'a mut self, _: &'a ()) -> PhaseParts<'a, NoEvents, (), ()>
    where
        'id: 'a,
    {
        PhaseParts {
            events: Lent::new(NoEvents),
            control: Lent::new(()),
            clients: Lent::new(()),
        }
    }

    fn drive<'p, 'id>(
        &mut self,
        _: &'p Self::Held<'id>,
        retire: Retire<'p>,
    ) -> impl Future<Output = ()> + use<'p, 'id, N> {
        core::future::poll_fn(move |_| {
            if retire.is_requested() {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        })
    }

    fn retire_bound(&self) -> impl Future<Output = ()> + '_ {
        pending()
    }

    fn detach<'id>(&mut self, _: &Self::Held<'id>) {}

    fn uninstall<'id>(self, _: Self::Held<'id>, (): (), (): ()) {}

    fn abandon(&mut self) {}
}

/// Poll `future` once with a waker that does nothing.
fn poll_once<F: Future>(future: core::pin::Pin<&mut F>) -> Poll<F::Output> {
    future.poll(&mut Context::from_waker(Waker::noop()))
}

/// Run `check` on `N` permits of one install.
fn with_permits<const N: usize, C>(check: C)
where
    C: for<'id> FnOnce([Permit<'id, Kind>; N]),
{
    struct Body<C>(C);
    impl<const N: usize, C> PortBody<Probe<N>> for Body<C>
    where
        C: for<'id> FnOnce([Permit<'id, Kind>; N]),
    {
        async fn run<'p, 'id>(
            self,
            owner: Owner<'p, 'id, Probe<N>, Disabled>,
            permits: [Permit<'id, Kind>; N],
        ) -> Ended<'id, Probe<N>> {
            (self.0)(permits);
            owner.uninstall(())
        }
    }
    let Poll::Ready(ended) = poll_once(pin!(run_port(Probe::<N>, Body(check)))) else {
        panic!("the probe answers at once");
    };
    assert_eq!(ended, Some(()));
}

#[test]
fn a_pool_of_one_serves_takers_in_arrival_order() {
    with_permits::<1, _>(|[permit]| {
        let pool = FifoPermitPool::<'_, Kind, 1, 4>::new([permit]);
        conformance::permits_serve_takers_in_arrival_order(&pool, []);
    });
}

#[test]
fn a_pool_with_a_permit_held_aside_serves_takers_in_arrival_order() {
    with_permits::<2, _>(|permits| {
        let pool = FifoPermitPool::<'_, Kind, 2, 4>::new(permits);
        let aside = match poll_once(pin!(pool.take())) {
            Poll::Ready(permit) => permit,
            Poll::Pending => panic!("a free permit serves at once"),
        };
        conformance::permits_serve_takers_in_arrival_order(&pool, [aside]);
    });
}

#[test]
fn two_releases_before_any_taker_runs_serve_two_takers() {
    with_permits::<2, _>(|permits| {
        let pool = FifoPermitPool::<'_, Kind, 2, 4>::new(permits);
        let mut first = pin!(pool.take());
        let mut second = pin!(pool.take());
        let (Poll::Ready(one), Poll::Ready(two)) =
            (poll_once(first.as_mut()), poll_once(second.as_mut()))
        else {
            panic!("two free permits serve two takers");
        };
        let mut third = pin!(pool.take());
        let mut fourth = pin!(pool.take());
        assert!(poll_once(third.as_mut()).is_pending());
        assert!(poll_once(fourth.as_mut()).is_pending());
        pool.release(one);
        pool.release(two);
        assert!(poll_once(third.as_mut()).is_ready());
        assert!(poll_once(fourth.as_mut()).is_ready());
        assert_eq!(pool.available(), 0);
    });
}

#[test]
#[should_panic(expected = "more takers wait than the pool's line has places")]
fn a_taker_finding_the_line_full_is_a_fault() {
    with_permits::<1, _>(|[permit]| {
        let pool = FifoPermitPool::<'_, Kind, 1, 1>::new([permit]);
        let _held = match poll_once(pin!(pool.take())) {
            Poll::Ready(permit) => permit,
            Poll::Pending => panic!("a free permit serves at once"),
        };
        let mut in_line = pin!(pool.take());
        assert!(poll_once(in_line.as_mut()).is_pending());
        // The line's one place is taken: one taker more is a composition
        // fault.
        let _ = poll_once(pin!(pool.take()));
    });
}

/// A waker that counts its wakes.
struct Counter(std::sync::atomic::AtomicUsize);

impl std::task::Wake for Counter {
    fn wake(self: std::sync::Arc<Self>) {
        self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
}

/// A waker and its count of wakes.
fn counted() -> (Waker, std::sync::Arc<Counter>) {
    let counter = std::sync::Arc::new(Counter(std::sync::atomic::AtomicUsize::new(0)));
    (Waker::from(counter.clone()), counter)
}

fn wakes(counter: &Counter) -> usize {
    counter.0.load(std::sync::atomic::Ordering::Relaxed)
}

#[test]
fn releases_and_departures_wake_exactly_the_takers_they_let_through() {
    with_permits::<2, _>(|permits| {
        let pool = FifoPermitPool::<'_, Kind, 2, 2>::new(permits);
        let (one, two) = match (poll_once(pin!(pool.take())), poll_once(pin!(pool.take()))) {
            (Poll::Ready(one), Poll::Ready(two)) => (one, two),
            _ => panic!("two free permits serve two takers"),
        };
        let (first_waker, first_wakes) = counted();
        let (second_waker, second_wakes) = counted();
        let mut first = pin!(pool.take());
        let mut second = pin!(pool.take());
        assert!(
            first
                .as_mut()
                .poll(&mut Context::from_waker(&first_waker))
                .is_pending()
        );
        assert!(
            second
                .as_mut()
                .poll(&mut Context::from_waker(&second_waker))
                .is_pending()
        );
        // A release wakes the leader alone.
        pool.release(one);
        assert_eq!((wakes(&first_wakes), wakes(&second_wakes)), (1, 0));
        // A second release before the leader runs: the leader is woken
        // again; the taker behind it waits for the leader to be served.
        pool.release(two);
        assert_eq!(wakes(&second_wakes), 0);
        // The served leader leaves a permit free: it wakes the next leader.
        assert!(
            first
                .as_mut()
                .poll(&mut Context::from_waker(&first_waker))
                .is_ready()
        );
        assert_eq!(wakes(&second_wakes), 1);
        assert!(
            second
                .as_mut()
                .poll(&mut Context::from_waker(&second_waker))
                .is_ready()
        );
    });
}

#[test]
fn a_departing_leader_wakes_the_next_one_when_a_permit_is_free() {
    with_permits::<1, _>(|[permit]| {
        let pool = FifoPermitPool::<'_, Kind, 1, 2>::new([permit]);
        let held = match poll_once(pin!(pool.take())) {
            Poll::Ready(permit) => permit,
            Poll::Pending => panic!("a free permit serves at once"),
        };
        let (next_waker, next_wakes) = counted();
        let mut next = pin!(pool.take());
        {
            let mut leader = pin!(pool.take());
            assert!(poll_once(leader.as_mut()).is_pending());
            assert!(
                next.as_mut()
                    .poll(&mut Context::from_waker(&next_waker))
                    .is_pending()
            );
            pool.release(held);
            assert_eq!(wakes(&next_wakes), 0, "the release woke the leader");
        }
        // The woken leader gave up its place: the next one leads and is
        // woken to take the free permit.
        assert_eq!(wakes(&next_wakes), 1);
        assert!(
            next.as_mut()
                .poll(&mut Context::from_waker(&next_waker))
                .is_ready()
        );
    });
}
