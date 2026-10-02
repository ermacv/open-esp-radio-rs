use super::*;
use core::{
    future::Future,
    task::{Context, Poll, Waker},
};

struct Hardware {
    reads: usize,
    ready_after: usize,
    requested: bool,
}
impl MacRuntimeStopHardware for Hardware {
    fn request_mac_runtime_stop(&mut self) {
        self.requested = true;
    }
    fn mac_runtime_active_state(&mut self) -> u8 {
        assert!(self.requested);
        self.reads += 1;
        if self.reads > self.ready_after { 0 } else { 3 }
    }
    fn resume_mac_runtime(&mut self) {
        panic!("stop must never resume MAC");
    }
}
/// Time that moves to each deadline waited for, recording the waits, or
/// parks every wait.
struct Timer {
    now: core::cell::Cell<u64>,
    waits: core::cell::RefCell<std::vec::Vec<u64>>,
    park: bool,
}
impl Timer {
    fn new(now: u64, park: bool) -> Self {
        Self {
            now: core::cell::Cell::new(now),
            waits: core::cell::RefCell::new(std::vec::Vec::new()),
            park,
        }
    }
    fn waits(&self) -> std::vec::Vec<u64> {
        self.waits.borrow().clone()
    }
}
impl oer_time::Clock for Timer {
    fn now(&self) -> oer_time::Instant {
        oer_time::Instant::from_micros(self.now.get())
    }
}
impl oer_time::Timer for Timer {
    async fn wait_until(&self, deadline: oer_time::Instant) {
        self.waits.borrow_mut().push(deadline.as_micros());
        if self.park {
            core::future::pending::<()>().await;
        }
        self.now.set(deadline.as_micros());
    }
}
fn run(future: impl Future<Output = Result<(), StopError>>) -> Result<(), StopError> {
    match core::pin::pin!(future)
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
    {
        Poll::Ready(result) => result,
        Poll::Pending => panic!("model timer should complete"),
    }
}
#[test]
fn stopped_readback_needs_no_delay_and_never_resumes() {
    let mut hw = Hardware {
        reads: 0,
        ready_after: 0,
        requested: false,
    };
    let timer = Timer::new(0, false);
    assert_eq!(
        run(stop_mac(
            &mut hw,
            &timer,
            oer_time::Duration::from_micros(100)
        )),
        Ok(())
    );
    assert!(timer.waits().is_empty());
}
#[test]
fn active_mac_parks_until_readback_or_explicit_timeout() {
    for (ready_after, expected) in [
        (2, Ok(())),
        (99, Err(StopError::TimedOut { active_state: 3 })),
    ] {
        let mut hw = Hardware {
            reads: 0,
            ready_after,
            requested: false,
        };
        let timer = Timer::new(0, false);
        assert_eq!(
            run(stop_mac(
                &mut hw,
                &timer,
                oer_time::Duration::from_micros(40)
            )),
            expected
        );
        assert_eq!(timer.waits(), [20, 40]);
    }
}
#[test]
fn pending_timer_does_not_repoll_mmio_and_cancellation_retains_borrowed_owner() {
    let mut hw = Hardware {
        reads: 0,
        ready_after: 99,
        requested: false,
    };
    let timer = Timer::new(0, true);
    {
        let mut future = core::pin::pin!(stop_mac(
            &mut hw,
            &timer,
            oer_time::Duration::from_micros(40)
        ));
        let mut cx = Context::from_waker(Waker::noop());
        assert!(future.as_mut().poll(&mut cx).is_pending());
        assert!(future.as_mut().poll(&mut cx).is_pending());
    }
    assert_eq!(hw.reads, 1);
    assert_eq!(timer.waits(), [20]);
    assert!(hw.requested);
}

#[test]
fn unrepresentable_deadline_does_not_start_hardware_stop() {
    let mut hw = Hardware {
        reads: 0,
        ready_after: 99,
        requested: false,
    };
    let timer = Timer::new(u64::MAX, false);
    assert_eq!(
        run(stop_mac(
            &mut hw,
            &timer,
            oer_time::Duration::from_micros(1)
        )),
        Err(StopError::DeadlineOverflow)
    );
    assert!(!hw.requested);
    assert_eq!(hw.reads, 0);
}
