extern crate std;

use core::{
    future::{Future, pending, ready},
    pin::{Pin, pin},
    task::{Context, Poll, Waker},
};
use std::{
    boxed::Box,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    task::Wake,
};

use oer_ieee802154_port::{
    ClockError, Disabled, EnableResult, Ended, Lent, Owner, PhaseParts, PortBackend, PortBody,
    RadioPortEvents, Retire, run_port,
};
use oer_radio_port::{
    EventsLost, Line, Mint, Permit, PermitPool, Poisoned, PortResult, Seat, conformance,
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

/// The resources of a probe: `N` permits and the line of `S` seats of one
/// install.
type Tokens<'id, const N: usize, const S: usize> = (
    [Permit<'id, Kind>; N],
    Line<'id, Kind, S>,
    [Seat<'id, Kind>; S],
);

/// A backend whose only resources are `N` permits and `S` seats of one
/// install.
struct Probe<const N: usize, const S: usize>;

impl<const N: usize, const S: usize> PortBackend for Probe<N, S> {
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
    type Pools = oer_radio_port::Permits<Kind, N, S>;
    type Resources<'id> = Tokens<'id, N, S>;
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

    fn resources<'id>(&mut self, mint: Mint<'id, Self::Pools>) -> ((), Tokens<'id, N, S>) {
        let oer_radio_port::PermitTokens {
            permits,
            line,
            seats,
        } = mint.into_pools();
        ((), (permits, line, seats))
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
        _: &'p (),
        retire: Retire<'p>,
    ) -> impl Future<Output = ()> + use<'p, 'id, N, S> {
        // The probe has nothing to serve: it retires when asked.
        async move { retire.requested().await }
    }

    fn retire_bound(&self) -> impl Future<Output = ()> + '_ {
        pending()
    }

    fn detach<'id>(&mut self, (): &()) {}

    fn uninstall<'id>(self, (): (), (): (), (): ()) {}

    fn abandon(&mut self) {}
}

/// Poll `future` once with a waker that does nothing.
fn poll_once<F: Future>(future: Pin<&mut F>) -> Poll<F::Output> {
    future.poll(&mut Context::from_waker(Waker::noop()))
}

/// Run `check` on a pool of `N` permits of one install and the `S` seats
/// of its line.
fn with_pool<const N: usize, const S: usize, C>(check: C)
where
    C: for<'id> FnOnce(&FifoPermitPool<'id, Kind, N, S>, [Seat<'id, Kind>; S]),
{
    struct Body<C>(C);
    impl<const N: usize, const S: usize, C> PortBody<Probe<N, S>> for Body<C>
    where
        C: for<'id> FnOnce(&FifoPermitPool<'id, Kind, N, S>, [Seat<'id, Kind>; S]),
    {
        async fn run<'id>(
            self,
            owner: Owner<'_, 'id, Probe<N, S>, Disabled>,
            (permits, line, seats): Tokens<'id, N, S>,
        ) -> Ended<'id, Probe<N, S>> {
            (self.0)(&FifoPermitPool::new(permits, line), seats);
            owner.uninstall(())
        }
    }
    let Poll::Ready(ended) = poll_once(pin!(run_port(Probe::<N, S>, Body(check)))) else {
        panic!("the probe answers at once");
    };
    assert_eq!(ended, Some(()));
}

/// Take a free permit at once.
fn take_now<'id, const N: usize, const S: usize>(
    pool: &FifoPermitPool<'id, Kind, N, S>,
    seat: &mut Seat<'id, Kind>,
) -> Permit<'id, Kind> {
    match poll_once(pin!(pool.take(seat))) {
        Poll::Ready(permit) => permit,
        Poll::Pending => panic!("a free permit serves at once"),
    }
}

#[test]
fn a_pool_of_one_serves_takers_in_arrival_order() {
    with_pool::<1, 3, _>(|pool, mut seats| {
        conformance::permits_serve_takers_in_arrival_order(pool, &mut seats, []);
    });
}

#[test]
fn a_pool_with_a_permit_held_aside_serves_takers_in_arrival_order() {
    with_pool::<2, 4, _>(|pool, [mut aside, first, second, third]| {
        let held = take_now(pool, &mut aside);
        conformance::permits_serve_takers_in_arrival_order(
            pool,
            &mut [first, second, third],
            [held],
        );
    });
}

#[test]
fn every_seat_waits_in_arrival_order_whatever_its_index() {
    with_pool::<1, 4, _>(|pool, [mut zero, mut one, mut two, mut three]| {
        let mut held = take_now(pool, &mut zero);
        // Every seat of the line waits at once: the line has a place for
        // each, so none is ever turned away.
        let mut first = pin!(pool.take(&mut three));
        let mut second = pin!(pool.take(&mut one));
        let mut third = pin!(pool.take(&mut zero));
        let mut fourth = pin!(pool.take(&mut two));
        for taker in [
            poll_once(first.as_mut()),
            poll_once(second.as_mut()),
            poll_once(third.as_mut()),
            poll_once(fourth.as_mut()),
        ] {
            assert!(taker.is_pending());
        }
        assert_eq!(pool.waiting(), 4);
        for taker in [first, second, third, fourth] {
            let mut taker = taker;
            pool.release(held);
            held = match poll_once(taker.as_mut()) {
                Poll::Ready(permit) => permit,
                Poll::Pending => panic!("the earliest taker gets the released permit"),
            };
        }
        assert_eq!(pool.waiting(), 0);
        pool.release(held);
    });
}

#[test]
fn a_dropped_take_gives_up_its_place_and_its_seat_waits_again() {
    with_pool::<1, 2, _>(|pool, [mut mine, mut other]| {
        let held = take_now(pool, &mut other);
        {
            let mut gives_up = pin!(pool.take(&mut mine));
            assert!(poll_once(gives_up.as_mut()).is_pending());
            assert_eq!(pool.waiting(), 1);
        }
        assert_eq!(pool.waiting(), 0);
        // The seat came back with the dropped take, and waits again.
        let mut again = pin!(pool.take(&mut mine));
        assert!(poll_once(again.as_mut()).is_pending());
        pool.release(held);
        let Poll::Ready(permit) = poll_once(again.as_mut()) else {
            panic!("the seat's new take is served");
        };
        pool.release(permit);
    });
}

#[test]
fn a_seat_whose_take_leaked_takes_its_place_again() {
    with_pool::<1, 2, _>(|pool, [mut mine, mut other]| {
        let held = take_now(pool, &mut other);
        let mut leaked = Box::pin(pool.take(&mut mine));
        assert!(poll_once(leaked.as_mut()).is_pending());
        // A leaked take never drops, and no one polls it again.
        core::mem::forget(leaked);
        let mut again = pin!(pool.take(&mut mine));
        assert!(poll_once(again.as_mut()).is_pending());
        assert_eq!(pool.waiting(), 1);
        pool.release(held);
        let Poll::Ready(permit) = poll_once(again.as_mut()) else {
            panic!("the seat's new take leads the line");
        };
        pool.release(permit);
    });
}

#[test]
fn two_releases_before_any_taker_runs_serve_two_takers() {
    with_pool::<2, 4, _>(|pool, [mut a, mut b, mut c, mut d]| {
        let one = take_now(pool, &mut a);
        let two = take_now(pool, &mut b);
        let mut third = pin!(pool.take(&mut c));
        let mut fourth = pin!(pool.take(&mut d));
        assert!(poll_once(third.as_mut()).is_pending());
        assert!(poll_once(fourth.as_mut()).is_pending());
        pool.release(one);
        pool.release(two);
        assert!(poll_once(third.as_mut()).is_ready());
        assert!(poll_once(fourth.as_mut()).is_ready());
        assert_eq!(pool.available(), 0);
    });
}

/// A waker that counts its wakes.
#[derive(Default)]
struct Count(AtomicUsize);

impl Wake for Count {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

impl Count {
    fn wakes(&self) -> usize {
        self.0.load(Ordering::Relaxed)
    }
}

#[test]
fn a_release_wakes_the_leader_and_a_served_leader_wakes_the_next() {
    with_pool::<2, 4, _>(|pool, [mut a, mut b, mut c, mut d]| {
        let one = take_now(pool, &mut a);
        let two = take_now(pool, &mut b);
        let (leader, next) = (Arc::new(Count::default()), Arc::new(Count::default()));
        let (leader_waker, next_waker) = (Waker::from(leader.clone()), Waker::from(next.clone()));
        let mut first = pin!(pool.take(&mut c));
        let mut second = pin!(pool.take(&mut d));
        assert!(
            first
                .as_mut()
                .poll(&mut Context::from_waker(&leader_waker))
                .is_pending()
        );
        assert!(
            second
                .as_mut()
                .poll(&mut Context::from_waker(&next_waker))
                .is_pending()
        );
        pool.release(one);
        pool.release(two);
        // Each release wakes the leader; the taker behind it sleeps on.
        assert_eq!((leader.wakes(), next.wakes()), (2, 0));
        assert!(
            first
                .as_mut()
                .poll(&mut Context::from_waker(&leader_waker))
                .is_ready()
        );
        // The served leader left a permit free: it wakes the next leader.
        assert_eq!(next.wakes(), 1);
        assert!(
            second
                .as_mut()
                .poll(&mut Context::from_waker(&next_waker))
                .is_ready()
        );
    });
}

#[test]
fn a_departing_leader_wakes_the_next_one_when_a_permit_is_free() {
    with_pool::<1, 3, _>(|pool, [mut aside, mut leads, mut follows]| {
        let held = take_now(pool, &mut aside);
        let next_wakes = Arc::new(Count::default());
        let next_waker = Waker::from(next_wakes.clone());
        let mut next = pin!(pool.take(&mut follows));
        {
            let mut leader = pin!(pool.take(&mut leads));
            assert!(poll_once(leader.as_mut()).is_pending());
            assert!(
                next.as_mut()
                    .poll(&mut Context::from_waker(&next_waker))
                    .is_pending()
            );
            pool.release(held);
            assert_eq!(next_wakes.wakes(), 0, "the release woke the leader");
        }
        // The woken leader gave up its place: the next one leads and is
        // woken to take the free permit.
        assert_eq!(next_wakes.wakes(), 1);
        assert!(
            next.as_mut()
                .poll(&mut Context::from_waker(&next_waker))
                .is_ready()
        );
    });
}

#[test]
fn a_leaked_leader_replaced_by_its_seat_wakes_the_taker_behind_it() {
    with_pool::<1, 3, _>(|pool, [mut aside, mut mine, mut other]| {
        let held = take_now(pool, &mut aside);
        let mut leaked = Box::pin(pool.take(&mut mine));
        assert!(poll_once(leaked.as_mut()).is_pending());
        let behind_wakes = Arc::new(Count::default());
        let behind_waker = Waker::from(behind_wakes.clone());
        let mut behind = pin!(pool.take(&mut other));
        assert!(
            behind
                .as_mut()
                .poll(&mut Context::from_waker(&behind_waker))
                .is_pending()
        );
        // The leaked leader never drops; the release wakes it alone.
        core::mem::forget(leaked);
        pool.release(held);
        assert_eq!(behind_wakes.wakes(), 0);
        // Its seat's next take replaces it and leads no more: the taker
        // behind it now leads and is woken to take the free permit.
        let mut again = pin!(pool.take(&mut mine));
        assert!(poll_once(again.as_mut()).is_pending());
        assert_eq!(behind_wakes.wakes(), 1);
        let Poll::Ready(permit) = behind
            .as_mut()
            .poll(&mut Context::from_waker(&behind_waker))
        else {
            panic!("the taker behind the leaked leader is served");
        };
        pool.release(permit);
        assert!(poll_once(again.as_mut()).is_ready());
    });
}
