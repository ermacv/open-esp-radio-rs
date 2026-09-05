use core::{
    future::{Future, pending, ready},
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
fn serialized_primary_notification_wakes_both_fresh_ordinary_consumers() {
    let wakers = EmbassyBluetoothRuntimeWakers::<NoopRawMutex>::new();
    let (scheduler_counter, scheduler_waker) = counting_waker();
    let (lock_modify_counter, lock_modify_waker) = counting_waker();
    wakers.scheduler_waker.register(&scheduler_waker);
    wakers.lock_modify_waker.register(&lock_modify_waker);

    assert_eq!(
        wakers.notify_ordinary(BluetoothPrimaryOrdinaryPublication::Scheduler {
            scheduler: BluetoothSchedulerWakePublication::WakeWorker,
            lock_modify: BluetoothSchedulerLockModifyEventPublication::WakeWorker,
        }),
        BluetoothPrimaryOrdinaryPublication::Scheduler {
            scheduler: BluetoothSchedulerWakePublication::WakeWorker,
            lock_modify: BluetoothSchedulerLockModifyEventPublication::WakeWorker,
        }
    );
    assert_eq!(scheduler_counter.0.load(Ordering::Relaxed), 1);
    assert_eq!(lock_modify_counter.0.load(Ordering::Relaxed), 1);
}

#[test]
fn serialized_primary_notification_does_not_wake_coalesced_consumers() {
    let wakers = EmbassyBluetoothRuntimeWakers::<NoopRawMutex>::new();
    let (scheduler_counter, scheduler_waker) = counting_waker();
    let (lock_modify_counter, lock_modify_waker) = counting_waker();
    wakers.scheduler_waker.register(&scheduler_waker);
    wakers.lock_modify_waker.register(&lock_modify_waker);

    assert_eq!(
        wakers.notify_ordinary(BluetoothPrimaryOrdinaryPublication::Scheduler {
            scheduler: BluetoothSchedulerWakePublication::Coalesced,
            lock_modify: BluetoothSchedulerLockModifyEventPublication::Coalesced,
        }),
        BluetoothPrimaryOrdinaryPublication::Scheduler {
            scheduler: BluetoothSchedulerWakePublication::Coalesced,
            lock_modify: BluetoothSchedulerLockModifyEventPublication::Coalesced,
        }
    );
    assert_eq!(scheduler_counter.0.load(Ordering::Relaxed), 0);
    assert_eq!(lock_modify_counter.0.load(Ordering::Relaxed), 0);
}

#[test]
fn borrowed_scheduler_wait_observes_without_consuming() {
    let wakers = EmbassyBluetoothRuntimeWakers::<NoopRawMutex>::new();
    let wake = BluetoothSchedulerWakeCell::new();

    assert_eq!(
        wake.publish_from_interrupt(BluetoothSchedulerWorkerWakeClass::Ordinary),
        BluetoothSchedulerWakePublication::WakeWorker
    );
    block_on(wakers.wait_scheduler_ready(&wake));
    assert!(wakers.scheduler_pending(&wake));
    assert!(wake.take().is_some());
}

#[test]
fn cancelled_borrowed_scheduler_wait_preserves_replacement_readiness() {
    let wakers = EmbassyBluetoothRuntimeWakers::<NoopRawMutex>::new();
    let wake = BluetoothSchedulerWakeCell::new();
    let (_counter, task_waker) = counting_waker();
    let mut cancelled = Box::pin(wakers.wait_scheduler_ready(&wake));

    assert!(poll_once(cancelled.as_mut(), &task_waker).is_pending());
    drop(cancelled);
    assert_eq!(
        wake.publish_from_interrupt(BluetoothSchedulerWorkerWakeClass::Marked),
        BluetoothSchedulerWakePublication::WakeWorker
    );
    wakers.scheduler_waker.wake();

    block_on(wakers.wait_scheduler_ready(&wake));
    assert!(
        wake.take()
            .expect("published batch remains durable")
            .is_marked()
    );
}

#[test]
fn post_unlink_publication_before_wait_is_rechecked_without_loss() {
    let wakers = EmbassyBluetoothRuntimeWakers::<NoopRawMutex>::new();
    let ready = AtomicBool::new(true);

    assert_eq!(
        wakers.notify_post_unlink(BluetoothDtmPostUnlinkMailboxPublication::WakeConsumer),
        BluetoothDtmPostUnlinkMailboxPublication::WakeConsumer
    );
    block_on(poll_fn(|context| {
        poll_borrowed_ready(&wakers.post_unlink_waker, context, || {
            ready.load(Ordering::Acquire)
        })
    }));
}

#[test]
fn post_unlink_mailbox_wins_a_simultaneous_recheck_tie() {
    assert_eq!(
        block_on(select_post_unlink_first(ready(()), ready(()))),
        EmbassyBluetoothPostUnlinkSignal::Mailbox
    );
}

#[test]
fn absolute_recheck_advances_post_unlink_without_an_interrupt_edge() {
    assert_eq!(
        block_on(select_post_unlink_first(pending::<()>(), ready(()))),
        EmbassyBluetoothPostUnlinkSignal::Recheck
    );
}

#[test]
fn post_unlink_wait_wakes_once_while_ready_publications_coalesce() {
    let wakers = EmbassyBluetoothRuntimeWakers::<NoopRawMutex>::new();
    let ready = AtomicBool::new(false);
    let (counter, waker) = counting_waker();
    let mut wait = Box::pin(poll_fn(|context| {
        poll_borrowed_ready(&wakers.post_unlink_waker, context, || {
            ready.load(Ordering::Acquire)
        })
    }));

    assert!(poll_once(wait.as_mut(), &waker).is_pending());
    ready.store(true, Ordering::Release);
    wakers.notify_post_unlink(BluetoothDtmPostUnlinkMailboxPublication::WakeConsumer);
    assert_eq!(counter.0.load(Ordering::Relaxed), 1);
    assert_eq!(
        wakers.notify_post_unlink(BluetoothDtmPostUnlinkMailboxPublication::AlreadyReady),
        BluetoothDtmPostUnlinkMailboxPublication::AlreadyReady
    );
    assert_eq!(counter.0.load(Ordering::Relaxed), 1);
    assert!(poll_once(wait.as_mut(), &waker).is_ready());
}

#[test]
fn cancelled_post_unlink_wait_leaves_ready_epoch_for_replacement() {
    let wakers = EmbassyBluetoothRuntimeWakers::<NoopRawMutex>::new();
    let ready = AtomicBool::new(false);
    let (_counter, waker) = counting_waker();
    let mut cancelled = Box::pin(poll_fn(|context| {
        poll_borrowed_ready(&wakers.post_unlink_waker, context, || {
            ready.load(Ordering::Acquire)
        })
    }));

    assert!(poll_once(cancelled.as_mut(), &waker).is_pending());
    drop(cancelled);
    ready.store(true, Ordering::Release);
    wakers.notify_post_unlink(BluetoothDtmPostUnlinkMailboxPublication::WakeConsumer);

    block_on(poll_fn(|context| {
        poll_borrowed_ready(&wakers.post_unlink_waker, context, || {
            ready.load(Ordering::Acquire)
        })
    }));
}
