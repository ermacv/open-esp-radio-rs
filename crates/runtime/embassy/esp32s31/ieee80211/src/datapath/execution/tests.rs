use super::*;
use core::{
    future::Future,
    task::{Context, Poll},
};
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use std::{
    sync::{Arc, atomic::AtomicUsize},
    task::{Wake, Waker},
};

#[derive(Default)]
struct WakeCount(AtomicUsize);
impl Wake for WakeCount {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
    fn wake_by_ref(self: &Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

#[test]
fn cancellation_does_not_lose_a_request_and_duplicate_pauses_coalesce() {
    let control = Control::<NoopRawMutex>::new();
    let wakes = Arc::new(WakeCount::default());
    let waker = Waker::from(wakes.clone());
    let mut cx = Context::from_waker(&waker);
    {
        let mut wait = core::pin::pin!(control.wait_boundary());
        assert!(wait.as_mut().poll(&mut cx).is_pending());
    }
    control.request_pause();
    control.request_pause();
    assert_eq!(wakes.0.load(Ordering::Relaxed), 1);
    {
        let mut wait = core::pin::pin!(control.wait_boundary());
        assert_eq!(wait.as_mut().poll(&mut cx), Poll::Ready(()));
    }
    control.acknowledge_pause();
    let mut wait = core::pin::pin!(control.wait_boundary());
    assert!(
        wait.as_mut().poll(&mut cx).is_pending(),
        "stale wake must not authorize another pause"
    );
    control.request_stop();
    control.request_pause();
    control.request_stop();
    assert_eq!(wakes.0.load(Ordering::Relaxed), 2);
    assert_eq!(wait.as_mut().poll(&mut cx), Poll::Ready(()));
    control.acknowledge_pause();
    assert!(control.stop_requested());
}
