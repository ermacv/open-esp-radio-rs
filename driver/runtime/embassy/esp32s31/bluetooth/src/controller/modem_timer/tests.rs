use core::{
    future::Future,
    pin::Pin,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    task::{Context, Poll, Waker},
};
use std::{boxed::Box, sync::Arc, task::Wake};

use embassy_futures::block_on;
use embassy_sync::blocking_mutex::raw::NoopRawMutex;

use super::*;

struct WakeCounter(AtomicUsize);

impl Wake for WakeCounter {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

fn counting_waker() -> (Arc<WakeCounter>, Waker) {
    let counter = Arc::new(WakeCounter(AtomicUsize::new(0)));
    let waker = Waker::from(Arc::clone(&counter));
    (counter, waker)
}

fn poll_once<F: Future>(future: Pin<&mut F>, waker: &Waker) -> Poll<F::Output> {
    future.poll(&mut Context::from_waker(waker))
}

#[test]
fn source_127_notifies_only_a_fresh_durable_work_epoch() {
    let wakers = EmbassyBluetoothModemTimerWakers::<NoopRawMutex>::new();
    let (counter, task_waker) = counting_waker();
    wakers.worker_waker.register(&task_waker);

    assert_eq!(
        wakers.notify_modem_timer_service(
            BluetoothModemLpTimerPublishedInterruptStep::SoftwarePending(
                BluetoothModemLpTimerWorkerWakePublication::WakeWorker,
            ),
        ),
        BluetoothModemLpTimerPublishedInterruptStep::SoftwarePending(
            BluetoothModemLpTimerWorkerWakePublication::WakeWorker,
        )
    );
    assert_eq!(
        wakers.notify_modem_timer_service(
            BluetoothModemLpTimerPublishedInterruptStep::AwaitingSoftware(
                BluetoothModemLpTimerWorkerWakePublication::Coalesced,
            ),
        ),
        BluetoothModemLpTimerPublishedInterruptStep::AwaitingSoftware(
            BluetoothModemLpTimerWorkerWakePublication::Coalesced,
        )
    );
    assert_eq!(
        wakers.notify_modem_timer_service(BluetoothModemLpTimerPublishedInterruptStep::Spurious),
        BluetoothModemLpTimerPublishedInterruptStep::Spurious
    );
    assert_eq!(
        wakers.notify_modem_timer_service(BluetoothModemLpTimerPublishedInterruptStep::Rearmed),
        BluetoothModemLpTimerPublishedInterruptStep::Rearmed
    );

    assert_eq!(counter.0.load(Ordering::Relaxed), 1);
}

#[test]
fn cancelled_worker_wait_preserves_durable_readiness_for_replacement() {
    let wakers = EmbassyBluetoothModemTimerWakers::<NoopRawMutex>::new();
    let ready = AtomicBool::new(false);
    let (_counter, task_waker) = counting_waker();
    let mut cancelled = Box::pin(wakers.wait_worker_ready(|| ready.load(Ordering::Acquire)));

    assert!(poll_once(cancelled.as_mut(), &task_waker).is_pending());
    drop(cancelled);
    ready.store(true, Ordering::Release);
    assert_eq!(
        wakers.notify_modem_timer_service(
            BluetoothModemLpTimerPublishedInterruptStep::SoftwarePending(
                BluetoothModemLpTimerWorkerWakePublication::WakeWorker,
            ),
        ),
        BluetoothModemLpTimerPublishedInterruptStep::SoftwarePending(
            BluetoothModemLpTimerWorkerWakePublication::WakeWorker,
        )
    );

    block_on(wakers.wait_worker_ready(|| ready.load(Ordering::Acquire)));
    assert!(ready.load(Ordering::Acquire));
}

#[test]
fn opening_event_capacity_wakes_registered_backpressure_wait() {
    let wakers = EmbassyBluetoothModemTimerWakers::<NoopRawMutex>::new();
    let occupied = AtomicBool::new(true);
    let (counter, task_waker) = counting_waker();
    let mut wait = Box::pin(wakers.wait_event_capacity(|| !occupied.load(Ordering::Acquire)));

    assert!(poll_once(wait.as_mut(), &task_waker).is_pending());
    occupied.store(false, Ordering::Release);
    wakers.notify_event_capacity_opened();
    assert_eq!(counter.0.load(Ordering::Relaxed), 1);
    assert!(poll_once(wait.as_mut(), &task_waker).is_ready());
}

#[test]
fn cancelled_capacity_wait_does_not_change_the_capacity_predicate() {
    let wakers = EmbassyBluetoothModemTimerWakers::<NoopRawMutex>::new();
    let occupied = AtomicBool::new(true);
    let (_counter, task_waker) = counting_waker();
    let mut cancelled = Box::pin(wakers.wait_event_capacity(|| !occupied.load(Ordering::Acquire)));

    assert!(poll_once(cancelled.as_mut(), &task_waker).is_pending());
    drop(cancelled);
    assert!(occupied.load(Ordering::Acquire));

    occupied.store(false, Ordering::Release);
    wakers.notify_event_capacity_opened();
    block_on(wakers.wait_event_capacity(|| !occupied.load(Ordering::Acquire)));
}
