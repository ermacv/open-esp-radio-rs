use super::*;
use core::{
    future::Future,
    task::{Context, Poll, Waker},
};
use oer_esp32s31_phy::state::client::PhyPllTrackClock;
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
struct Timer {
    now: u64,
    waits: std::vec::Vec<u64>,
    park: bool,
}
impl PhyPllTrackClock for Timer {
    fn now_micros(&mut self) -> u64 {
        self.now
    }
}
impl PhyTrackingTimer for Timer {
    async fn wait_until_micros(&mut self, deadline: u64) {
        self.waits.push(deadline);
        if self.park {
            core::future::pending::<()>().await;
        }
        self.now = deadline;
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
    let mut timer = Timer {
        now: 0,
        waits: std::vec::Vec::new(),
        park: false,
    };
    assert_eq!(run(stop_mac(&mut hw, &mut timer, 100)), Ok(()));
    assert!(timer.waits.is_empty());
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
        let mut timer = Timer {
            now: 0,
            waits: std::vec::Vec::new(),
            park: false,
        };
        assert_eq!(run(stop_mac(&mut hw, &mut timer, 40)), expected);
        assert_eq!(timer.waits, [20, 40]);
    }
}
#[test]
fn pending_timer_does_not_repoll_mmio_and_cancellation_retains_borrowed_owner() {
    let mut hw = Hardware {
        reads: 0,
        ready_after: 99,
        requested: false,
    };
    let mut timer = Timer {
        now: 0,
        waits: std::vec::Vec::new(),
        park: true,
    };
    {
        let mut future = core::pin::pin!(stop_mac(&mut hw, &mut timer, 40));
        let mut cx = Context::from_waker(Waker::noop());
        assert!(future.as_mut().poll(&mut cx).is_pending());
        assert!(future.as_mut().poll(&mut cx).is_pending());
    }
    assert_eq!(hw.reads, 1);
    assert_eq!(timer.waits, [20]);
    assert!(hw.requested);
}

#[test]
fn unrepresentable_deadline_does_not_start_hardware_stop() {
    let mut hw = Hardware {
        reads: 0,
        ready_after: 99,
        requested: false,
    };
    let mut timer = Timer {
        now: u64::MAX,
        waits: std::vec::Vec::new(),
        park: false,
    };
    assert_eq!(
        run(stop_mac(&mut hw, &mut timer, 1)),
        Err(StopError::DeadlineOverflow)
    );
    assert!(!hw.requested);
    assert_eq!(hw.reads, 0);
}
