extern crate std;

use core::{
    future::{Future, pending, ready},
    pin::pin,
    task::{Context, Poll, Waker},
};

use oer_radio_port::{
    ClockError, Disabled, EnableResult, Ended, EventsLost, Lent, Mint, Owner, Permit, PermitPool,
    PhaseParts, Poisoned, PortBackend, PortBody, PortResult, RadioPortEvents, conformance,
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

    fn uninstall(self, (): ()) {}

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
        async fn run<'id>(
            self,
            owner: Owner<'id, Probe<N>, Disabled>,
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
fn a_taker_outside_a_full_line_takes_a_place_once_one_frees() {
    with_permits::<1, _>(|[permit]| {
        let pool = FifoPermitPool::<'_, Kind, 1, 1>::new([permit]);
        let held = match poll_once(pin!(pool.take())) {
            Poll::Ready(permit) => permit,
            Poll::Pending => panic!("a free permit serves at once"),
        };
        let mut in_line = pin!(pool.take());
        let mut outside = pin!(pool.take());
        assert!(poll_once(in_line.as_mut()).is_pending());
        // The line's one place is taken: this taker waits outside it.
        assert!(poll_once(outside.as_mut()).is_pending());
        pool.release(held);
        let Poll::Ready(permit) = poll_once(in_line.as_mut()) else {
            panic!("the taker in line is served first");
        };
        // Its place freed; the taker outside takes it and is served next.
        assert!(poll_once(outside.as_mut()).is_pending());
        pool.release(permit);
        assert!(poll_once(outside.as_mut()).is_ready());
    });
}
