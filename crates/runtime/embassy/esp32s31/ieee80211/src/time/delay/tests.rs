use super::*;
use core::cell::{Cell, RefCell};
use core::task::Waker;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::task::Wake;

#[derive(Default)]
struct Notifications(AtomicUsize);

impl Wake for Notifications {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

struct Alarm<'a> {
    polls: &'a Cell<usize>,
    waker: &'a RefCell<Option<Waker>>,
}

impl Future for Alarm<'_> {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        self.polls.set(self.polls.get() + 1);
        self.waker.replace(Some(cx.waker().clone()));
        Poll::Pending
    }
}

#[test]
fn expired_delay_completes_without_registering_or_waking() {
    for now in [100, 101] {
        let polls = Cell::new(0);
        let registered = RefCell::new(None);
        let notifications = Arc::new(Notifications::default());
        let waker = Waker::from(notifications.clone());
        let mut cx = Context::from_waker(&waker);
        let mut delay = Deadline {
            deadline: Instant::from_ticks(100),
            timer: Alarm {
                polls: &polls,
                waker: &registered,
            },
            now: || Instant::from_ticks(now),
        };
        assert!(Pin::new(&mut delay).poll(&mut cx).is_ready());
        assert_eq!(polls.get(), 0);
        assert!(registered.borrow().is_none());
        assert_eq!(notifications.0.load(Ordering::Relaxed), 0);
    }
}

#[test]
fn future_deadline_registers_wake_and_never_completes_early() {
    let now = Cell::new(90);
    let polls = Cell::new(0);
    let registered = RefCell::new(None);
    let notifications = Arc::new(Notifications::default());
    let waker = Waker::from(notifications.clone());
    let mut cx = Context::from_waker(&waker);
    let mut delay = Deadline {
        deadline: Instant::from_ticks(100),
        timer: Alarm {
            polls: &polls,
            waker: &registered,
        },
        now: || Instant::from_ticks(now.get()),
    };
    assert!(Pin::new(&mut delay).poll(&mut cx).is_pending());
    now.set(99);
    // An unrelated or early wake cannot satisfy the minimum settling time.
    assert!(Pin::new(&mut delay).poll(&mut cx).is_pending());
    assert_eq!(polls.get(), 2);
    assert_eq!(notifications.0.load(Ordering::Relaxed), 0);
    now.set(100);
    registered.borrow_mut().take().unwrap().wake();
    assert_eq!(notifications.0.load(Ordering::Relaxed), 1);
    assert!(Pin::new(&mut delay).poll(&mut cx).is_ready());
    assert_eq!(polls.get(), 2);
}

#[test]
fn measured_delay_uses_the_original_deadline_without_extra_wakes() {
    use oer_esp32s31_phy::executor::wait::Event;
    let now = Cell::new(100);
    let polls = Cell::new(0);
    let registered = RefCell::new(None);
    let notifications = Arc::new(Notifications::default());
    let waker = Waker::from(notifications.clone());
    let mut cx = Context::from_waker(&waker);
    let events = RefCell::new(std::vec::Vec::new());
    let future = Deadline {
        deadline: Instant::from_micros(110),
        timer: Alarm {
            polls: &polls,
            waker: &registered,
        },
        now: || Instant::from_micros(now.get()),
    };
    let mut measured = core::pin::pin!(measure(
        future,
        Instant::from_micros(100),
        10,
        || Instant::from_micros(now.get()),
        true,
        |event| events.borrow_mut().push(event)
    ));
    assert!(
        events.borrow().is_empty(),
        "factory must not run observation before timer poll"
    );
    assert!(measured.as_mut().poll(&mut cx).is_pending());
    assert_eq!(polls.get(), 1);
    assert_eq!(notifications.0.load(Ordering::Relaxed), 0);
    now.set(125);
    assert!(measured.as_mut().poll(&mut cx).is_ready());
    assert_eq!(polls.get(), 1);
    assert_eq!(notifications.0.load(Ordering::Relaxed), 0);
    assert_eq!(
        *events.borrow(),
        [
            Event::Started {
                requested_micros: 10
            },
            Event::Completed {
                elapsed_micros: 25,
                lateness_micros: 15
            }
        ]
    );
}

#[test]
fn observer_work_cannot_expire_the_first_poll_or_inflate_its_completion_sample() {
    use oer_esp32s31_phy::executor::wait::Event;

    for initially_ready in [false, true] {
        let now = Cell::new(if initially_ready { 101 } else { 100 });
        let polls = Cell::new(0);
        let registered = RefCell::new(None);
        let events = RefCell::new(std::vec::Vec::new());
        let future = Deadline {
            deadline: Instant::from_micros(101),
            timer: Alarm {
                polls: &polls,
                waker: &registered,
            },
            now: || Instant::from_micros(now.get()),
        };
        let mut measured = core::pin::pin!(measure(
            future,
            Instant::from_micros(100),
            1,
            || Instant::from_micros(now.get()),
            true,
            |event| {
                events.borrow_mut().push(event);
                // Recording is slower than the requested hardware delay.
                now.set(now.get() + 10);
            }
        ));
        let mut cx = Context::from_waker(Waker::noop());
        let first = measured.as_mut().poll(&mut cx);
        assert_eq!(first.is_ready(), initially_ready);
        assert_eq!(polls.get(), usize::from(!initially_ready));
        if !initially_ready {
            registered.borrow_mut().take().unwrap().wake();
            assert!(measured.as_mut().poll(&mut cx).is_ready());
        }
        let elapsed = if initially_ready { 1 } else { 10 };
        assert_eq!(
            *events.borrow(),
            [
                Event::Started {
                    requested_micros: 1
                },
                Event::Completed {
                    elapsed_micros: elapsed,
                    lateness_micros: elapsed - 1
                }
            ]
        );
    }
}

#[test]
fn disabled_delay_measurement_has_no_observation_clock_or_callback() {
    let future = core::future::ready(());
    let mut measured = core::pin::pin!(measure(
        future,
        Instant::from_micros(100),
        10,
        || panic!("observation clock must remain unused"),
        false,
        |_| panic!("disabled observer must remain unused")
    ));
    assert!(
        measured
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
            .is_ready()
    );
}

#[test]
fn short_settle_never_selects_readiness_or_zero_or_long_waits() {
    use oer_esp32s31_phy::executor::wait::Kind;
    for micros in [0, 1, 2, 5, 10, 20, 21, 100, u64::MAX] {
        assert!(!synchronous_settle(Kind::BusBusy, micros, 20));
        assert!(!synchronous_settle(Kind::Completion, micros, 20));
        assert!(!synchronous_settle(Kind::Settle, micros, 0));
        assert_eq!(
            synchronous_settle(Kind::Settle, micros, 20),
            (1..=20).contains(&micros)
        );
    }
}

#[test]
fn synchronous_settle_invokes_the_target_delay_once_without_timer_or_wake() {
    for micros in [1, 2, 5, 10, 20] {
        let calls = Cell::new(0);
        let requested = Cell::new(0);
        let mut delay: HardwareDelay<core::future::Pending<()>, fn() -> Instant, _> =
            HardwareDelay::Settle {
                micros,
                delay: |value| {
                    calls.set(calls.get() + 1);
                    requested.set(value);
                },
            };
        let notifications = Arc::new(Notifications::default());
        let waker = Waker::from(notifications.clone());
        assert!(
            Pin::new(&mut delay)
                .poll(&mut Context::from_waker(&waker))
                .is_ready()
        );
        assert_eq!(calls.get(), 1);
        assert_eq!(requested.get(), micros);
        assert_eq!(notifications.0.load(Ordering::Relaxed), 0);
    }
}

#[test]
fn timer_branch_still_yields_and_keeps_its_deadline() {
    let polls = Cell::new(0);
    let registered = RefCell::new(None);
    let now = Cell::new(100);
    let mut delay: HardwareDelay<_, _, fn(u32)> = HardwareDelay::Timer(Deadline {
        deadline: Instant::from_ticks(110),
        timer: Alarm {
            polls: &polls,
            waker: &registered,
        },
        now: || Instant::from_ticks(now.get()),
    });
    let mut cx = Context::from_waker(Waker::noop());
    assert!(Pin::new(&mut delay).poll(&mut cx).is_pending());
    assert_eq!(polls.get(), 1);
    now.set(110);
    assert!(Pin::new(&mut delay).poll(&mut cx).is_ready());
    assert_eq!(polls.get(), 1);
}
