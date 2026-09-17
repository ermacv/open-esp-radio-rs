use super::*;
use core::{
    cell::Cell,
    future::{Future, pending, poll_fn, ready},
    pin::pin,
    task::{Context, Poll, Waker},
};

fn window() -> TrackingDeadline {
    TrackingDeadline::new(100, NonZeroU64::new(20).unwrap()).unwrap()
}

#[test]
fn completes_before_deadline_and_preserves_work_error() {
    for output in [Ok(7), Err("hardware failure")] {
        let mut task = pin!(run(window(), || Some(119), |_| pending(), ready(output)));
        assert_eq!(
            task.as_mut().poll(&mut Context::from_waker(Waker::noop())),
            Poll::Ready(Ok(output))
        );
    }
}

#[test]
fn already_expired_window_never_polls_hardware_or_arms_another_timer() {
    let polled = Cell::new(false);
    let work = poll_fn(|_| {
        polled.set(true);
        Poll::Ready(())
    });
    let mut task = pin!(run(
        window(),
        || Some(120),
        |_| -> core::future::Pending<()> {
            panic!("expired window must not be renewed");
        },
        work
    ));
    assert_eq!(
        task.as_mut().poll(&mut Context::from_waker(Waker::noop())),
        Poll::Ready(Err(TrackingDeadlineError::Expired {
            deadline_micros: 120,
            observed_micros: 120,
        }))
    );
    assert!(!polled.get());
}

#[test]
fn synchronous_overrun_cannot_be_reported_as_success() {
    let clock = Cell::new(100);
    let work = poll_fn(|_| {
        clock.set(150);
        Poll::Ready("completed calibration")
    });
    let mut task = pin!(run(window(), || Some(clock.get()), |_| pending(), work));
    assert_eq!(
        task.as_mut().poll(&mut Context::from_waker(Waker::noop())),
        Poll::Ready(Err(TrackingDeadlineError::Expired {
            deadline_micros: 120,
            observed_micros: 150,
        }))
    );
}

#[test]
fn independent_alarm_wakes_suspended_child_and_expiry_wins_completion_tie() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    struct WakeCount(AtomicUsize);
    impl std::task::Wake for WakeCount {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
    let clock = Cell::new(107);
    let wake = Arc::new(WakeCount(AtomicUsize::new(0)));
    let waker = Waker::from(wake.clone());
    let mut cx = Context::from_waker(&waker);
    let timer_waker = core::cell::RefCell::new(None::<Waker>);
    let work_polls = Cell::new(0);
    let work = poll_fn(|_| {
        work_polls.set(work_polls.get() + 1);
        if clock.get() >= 120 {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    });
    let mut task = pin!(run(
        window(),
        || Some(clock.get()),
        |remaining| {
            assert_eq!(
                remaining, 13,
                "delayed first poll must not renew the budget"
            );
            poll_fn(|cx| {
                *timer_waker.borrow_mut() = Some(cx.waker().clone());
                Poll::Pending
            })
        },
        work
    ));
    assert!(task.as_mut().poll(&mut cx).is_pending());
    clock.set(120);
    timer_waker.borrow_mut().take().unwrap().wake();
    assert_eq!(wake.0.load(Ordering::SeqCst), 1);
    assert_eq!(
        task.as_mut().poll(&mut cx),
        Poll::Ready(Err(TrackingDeadlineError::Expired {
            deadline_micros: 120,
            observed_micros: 120,
        }))
    );
    assert_eq!(work_polls.get(), 1, "do not poll hardware again at expiry");
}

#[test]
fn clock_loss_or_reversal_during_work_rejects_completion() {
    for (after, expected) in [
        (None, TrackingDeadlineError::ClockUnavailable),
        (
            Some(109),
            TrackingDeadlineError::ClockWentBackwards {
                previous_micros: 110,
                observed_micros: 109,
            },
        ),
    ] {
        let clock = Cell::new(Some(110));
        let work = poll_fn(|_| {
            clock.set(after);
            Poll::Ready(())
        });
        let mut task = pin!(run(window(), || clock.get(), |_| pending(), work));
        assert_eq!(
            task.as_mut().poll(&mut Context::from_waker(Waker::noop())),
            Poll::Ready(Err(expected))
        );
    }
}

#[test]
fn mismatched_timer_cannot_turn_into_a_busy_loop_or_run_hardware() {
    let polled = Cell::new(false);
    let work = poll_fn(|_| {
        polled.set(true);
        Poll::Ready(())
    });
    let mut task = pin!(run(window(), || Some(100), |_| ready(()), work));
    assert_eq!(
        task.as_mut().poll(&mut Context::from_waker(Waker::noop())),
        Poll::Ready(Err(TrackingDeadlineError::WakeBeforeDeadline {
            deadline_micros: 120,
            observed_micros: 100,
        }))
    );
    assert!(!polled.get());
}

#[test]
fn absolute_deadline_overflow_is_rejected() {
    assert_eq!(
        TrackingDeadline::new(u64::MAX, NonZeroU64::new(1).unwrap()),
        None
    );
}
