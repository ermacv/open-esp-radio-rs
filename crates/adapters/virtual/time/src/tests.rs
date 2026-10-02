extern crate std;

use core::{
    future::Future,
    pin::{Pin, pin},
    sync::atomic::{AtomicUsize, Ordering},
    task::{Context, Poll, Waker},
};
use std::{sync::Arc, task::Wake};

use oer_time::{Clock, Duration, Instant, TimeOverflow, Timer};

use super::{SkipClock, VirtualClock};

#[derive(Default)]
struct Wakes(AtomicUsize);

impl Wake for Wakes {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

fn poll<F: Future>(future: Pin<&mut F>, waker: &Waker) -> Poll<F::Output> {
    future.poll(&mut Context::from_waker(waker))
}

#[test]
fn time_stands_still_until_the_owner_advances_it() {
    let clock: VirtualClock = VirtualClock::starting_at(Instant::from_micros(5));
    assert_eq!(clock.now(), Instant::from_micros(5));
    assert_eq!(clock.now(), Instant::from_micros(5));
    assert_eq!(
        clock.advance(Duration::from_micros(10)),
        Ok(Instant::from_micros(15))
    );
    assert_eq!(clock.now(), Instant::from_micros(15));
}

#[test]
fn a_wait_ends_when_an_advance_reaches_its_deadline_and_wakes_its_task() {
    let clock: VirtualClock = VirtualClock::new();
    let wakes = Arc::new(Wakes::default());
    let waker = Waker::from(wakes.clone());
    let mut wait = pin!(clock.wait_for(Duration::from_millis(2)));
    assert_eq!(poll(wait.as_mut(), &waker), Poll::Pending);
    assert_eq!(clock.next_deadline(), Some(Instant::from_micros(2_000)));

    clock.advance(Duration::from_millis(1)).unwrap();
    assert_eq!(wakes.0.load(Ordering::Relaxed), 1);
    assert_eq!(clock.next_deadline(), None);
    assert_eq!(poll(wait.as_mut(), &waker), Poll::Pending);

    clock.advance(Duration::from_millis(1)).unwrap();
    assert_eq!(wakes.0.load(Ordering::Relaxed), 2);
    assert_eq!(poll(wait.as_mut(), &waker), Poll::Ready(Ok(())));
}

#[test]
fn a_reached_deadline_completes_at_once() {
    let clock: VirtualClock = VirtualClock::starting_at(Instant::from_micros(100));
    let mut wait = pin!(clock.wait_until(Instant::from_micros(100)));
    assert_eq!(poll(wait.as_mut(), Waker::noop()), Poll::Ready(()));
}

#[test]
fn the_next_deadline_is_the_earliest_pending_one_and_time_jumps_to_it() {
    let clock: VirtualClock = VirtualClock::new();
    let mut late = pin!(clock.wait_until(Instant::from_micros(900)));
    let mut early = pin!(clock.wait_until(Instant::from_micros(300)));
    assert_eq!(poll(late.as_mut(), Waker::noop()), Poll::Pending);
    assert_eq!(poll(early.as_mut(), Waker::noop()), Poll::Pending);

    assert_eq!(
        clock.advance_to_next_deadline(),
        Some(Instant::from_micros(300))
    );
    assert_eq!(poll(early.as_mut(), Waker::noop()), Poll::Ready(()));
    assert_eq!(poll(late.as_mut(), Waker::noop()), Poll::Pending);
    assert_eq!(clock.next_deadline(), Some(Instant::from_micros(900)));
}

#[test]
fn the_clock_never_goes_back_and_refuses_to_overflow() {
    let clock: VirtualClock = VirtualClock::starting_at(Instant::from_micros(50));
    clock.advance_to(Instant::from_micros(10));
    assert_eq!(clock.now(), Instant::from_micros(50));
    assert_eq!(clock.advance_to_next_deadline(), None);

    let clock: VirtualClock = VirtualClock::starting_at(Instant::from_micros(u64::MAX));
    assert_eq!(clock.advance(Duration::from_micros(1)), Err(TimeOverflow));
    assert_eq!(clock.now(), Instant::from_micros(u64::MAX));
}

#[test]
fn clocks_keep_independent_times() {
    let first: VirtualClock = VirtualClock::new();
    let second: VirtualClock = VirtualClock::new();
    first.advance(Duration::from_secs(1)).unwrap();
    assert_eq!(first.now(), Instant::from_micros(1_000_000));
    assert_eq!(second.now(), Instant::EPOCH);
}

#[test]
fn more_waiters_than_slots_are_all_woken() {
    let clock: VirtualClock<1> = VirtualClock::new();
    let first = Arc::new(Wakes::default());
    let second = Arc::new(Wakes::default());
    let (first_waker, second_waker) = (Waker::from(first.clone()), Waker::from(second.clone()));
    let mut a = pin!(clock.wait_until(Instant::from_micros(10)));
    let mut b = pin!(clock.wait_until(Instant::from_micros(10)));
    assert_eq!(poll(a.as_mut(), &first_waker), Poll::Pending);
    assert_eq!(poll(b.as_mut(), &second_waker), Poll::Pending);
    clock.advance(Duration::from_micros(10)).unwrap();
    assert_eq!(first.0.load(Ordering::Relaxed), 1);
    assert_eq!(second.0.load(Ordering::Relaxed), 1);
    assert_eq!(poll(a.as_mut(), &first_waker), Poll::Ready(()));
    assert_eq!(poll(b.as_mut(), &second_waker), Poll::Ready(()));
}

#[test]
fn a_skip_clock_wait_moves_time_to_its_deadline_and_never_back() {
    let clock = SkipClock::starting_at(Instant::from_micros(5));
    let waker = Waker::from(Arc::new(Wakes::default()));
    let wait = pin!(clock.wait_until(Instant::from_micros(20)));
    assert_eq!(poll(wait, &waker), Poll::Ready(()));
    assert_eq!(clock.now(), Instant::from_micros(20));

    let past = pin!(clock.wait_until(Instant::from_micros(10)));
    assert_eq!(poll(past, &waker), Poll::Ready(()));
    assert_eq!(clock.now(), Instant::from_micros(20));

    clock.advance_to(Instant::from_micros(15));
    assert_eq!(clock.now(), Instant::from_micros(20));
    clock.advance_to(Instant::from_micros(30));
    assert_eq!(clock.now(), Instant::from_micros(30));
}
