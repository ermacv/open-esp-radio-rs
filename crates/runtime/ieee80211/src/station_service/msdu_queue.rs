//! Runtime wake policy around the audited affine SPSC queue.

use atomic_waker::AtomicWaker;
use core::future::poll_fn;
use core::task::Poll;
use oer_memory::{AffineSpscQueue, AffineSpscReceiver, AffineSpscSender, AffineSpscTrySendError};

/// Queue storage and atomic wake state. The composition splits it once and
/// moves the two non-cloneable endpoints to the application and the service.
/// The service uses it for both network MSDUs and application commands.
pub struct MsduQueue<T, const DEPTH: usize> {
    queue: AffineSpscQueue<T, DEPTH>,
    consumer: AtomicWaker,
    producer: AtomicWaker,
}

type Endpoints<'a, T, const DEPTH: usize> = (MsduSender<'a, T, DEPTH>, MsduReceiver<'a, T, DEPTH>);

impl<T, const DEPTH: usize> MsduQueue<T, DEPTH> {
    pub const fn new() -> Self {
        Self {
            queue: const { AffineSpscQueue::new() },
            consumer: AtomicWaker::new(),
            producer: AtomicWaker::new(),
        }
    }

    /// Exclusively borrow the storage to grant its sole endpoint pair.
    /// A later pair receives any values left in the queue by its predecessor.
    pub fn split(&mut self) -> Endpoints<'_, T, DEPTH> {
        let (sender, receiver) = self.queue.split();
        (
            MsduSender {
                sender,
                consumer: &self.consumer,
                producer: &self.producer,
            },
            MsduReceiver {
                receiver,
                consumer: &self.consumer,
                producer: &self.producer,
            },
        )
    }
}

impl<T, const DEPTH: usize> Default for MsduQueue<T, DEPTH> {
    fn default() -> Self {
        Self::new()
    }
}

pub struct MsduSender<'a, T, const DEPTH: usize> {
    sender: AffineSpscSender<'a, T, DEPTH>,
    consumer: &'a AtomicWaker,
    producer: &'a AtomicWaker,
}

impl<T, const DEPTH: usize> MsduSender<'_, T, DEPTH> {
    /// A full queue returns the original value and all its ownership.
    pub fn try_send(&self, value: T) -> Result<(), AffineSpscTrySendError<T>> {
        self.sender.try_send(value)?;
        self.consumer.wake();
        Ok(())
    }

    /// Wait for capacity while retaining the sole producer wake registration.
    /// The caller keeps its value until `try_send`; cancellation owns no value
    /// and consumes no credit. With one sender, a free cell stays free until
    /// that sender publishes.
    ///
    /// ```compile_fail,E0499
    /// use oer_ieee80211_runtime::station_service::MsduQueue;
    /// let mut queue = MsduQueue::<u8, 1>::new();
    /// let (mut sender, receiver) = queue.split();
    /// let first = sender.ready();
    /// let second = sender.ready();
    /// drop((first, second));
    /// ```
    ///
    /// ```
    /// use oer_ieee80211_runtime::station_service::MsduQueue;
    /// async fn sequential() {
    ///     let mut queue = MsduQueue::<u8, 1>::new();
    ///     let (mut sender, receiver) = queue.split();
    ///     sender.ready().await;
    ///     sender.try_send(1).unwrap();
    ///     assert_eq!(receiver.try_receive(), Some(1));
    ///     sender.ready().await;
    /// }
    /// ```
    pub async fn ready(&mut self) {
        poll_fn(|cx| {
            self.producer.register(cx.waker());
            if self.sender.free_capacity() == 0 {
                Poll::Pending
            } else {
                Poll::Ready(())
            }
        })
        .await;
    }
}

pub struct MsduReceiver<'a, T, const DEPTH: usize> {
    receiver: AffineSpscReceiver<'a, T, DEPTH>,
    consumer: &'a AtomicWaker,
    producer: &'a AtomicWaker,
}

impl<T, const DEPTH: usize> MsduReceiver<'_, T, DEPTH> {
    #[must_use]
    pub fn try_receive(&self) -> Option<T> {
        let value = self.receiver.try_receive().ok()?;
        self.producer.wake();
        Some(value)
    }

    /// Wait for a value without consuming it. The exclusive borrow lasts for
    /// the entire future, so the queue's sole consumer waker cannot be replaced
    /// by another outstanding wait. Cancellation leaves the value in the queue.
    ///
    /// ```compile_fail,E0499
    /// use oer_ieee80211_runtime::station_service::MsduQueue;
    /// let mut queue = MsduQueue::<u8, 1>::new();
    /// let (sender, mut receiver) = queue.split();
    /// let first = receiver.ready();
    /// let second = receiver.ready();
    /// drop((first, second));
    /// ```
    ///
    /// ```
    /// use oer_ieee80211_runtime::station_service::MsduQueue;
    /// async fn sequential() {
    ///     let mut queue = MsduQueue::<u8, 1>::new();
    ///     let (sender, mut receiver) = queue.split();
    ///     sender.try_send(1).unwrap();
    ///     receiver.ready().await;
    ///     assert_eq!(receiver.try_receive(), Some(1));
    /// }
    /// ```
    pub async fn ready(&mut self) {
        // Register before testing the queue: a publication before registration
        // is found by the load; a later one wakes this registration.
        poll_fn(|cx| {
            self.consumer.register(cx.waker());
            if self.receiver.is_empty() {
                Poll::Pending
            } else {
                Poll::Ready(())
            }
        })
        .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::future::Future;
    use core::sync::atomic::{AtomicUsize, Ordering};
    use core::task::Context;
    use std::sync::Arc;
    use std::task::{Wake, Waker};

    struct Wakes(AtomicUsize);

    impl Wake for Wakes {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
        fn wake_by_ref(self: &Arc<Self>) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn full_queue_returns_the_same_non_clone_owner_and_keeps_fifo() {
        struct Owner(u8);
        let mut queue = MsduQueue::<Owner, 2>::new();
        let (sender, receiver) = queue.split();
        assert!(sender.try_send(Owner(1)).is_ok());
        assert!(sender.try_send(Owner(2)).is_ok());
        assert_eq!(sender.try_send(Owner(3)).err().unwrap().0.0, 3);
        assert_eq!(receiver.try_receive().unwrap().0, 1);
        assert!(sender.try_send(Owner(4)).is_ok());
        assert_eq!(receiver.try_receive().unwrap().0, 2);
        assert_eq!(receiver.try_receive().unwrap().0, 4);
        assert!(receiver.try_receive().is_none());
    }

    #[test]
    fn publication_wakes_the_consumer_and_reclaim_wakes_the_blocked_producer() {
        struct Owner(u8);
        let mut queue = MsduQueue::<Owner, 1>::new();
        let (mut sender, mut receiver) = queue.split();
        let wakes = Arc::new(Wakes(AtomicUsize::new(0)));
        let waker = Waker::from(Arc::clone(&wakes));
        let mut cx = Context::from_waker(&waker);
        {
            let mut ready = core::pin::pin!(receiver.ready());
            assert!(ready.as_mut().poll(&mut cx).is_pending());
            assert!(sender.try_send(Owner(1)).is_ok());
            assert_eq!(wakes.0.load(Ordering::Relaxed), 1);
            assert!(ready.as_mut().poll(&mut cx).is_ready());
        }
        let owner = Owner(2);
        {
            let mut ready = core::pin::pin!(sender.ready());
            assert!(ready.as_mut().poll(&mut cx).is_pending());
            assert_eq!(receiver.try_receive().unwrap().0, 1);
            assert_eq!(wakes.0.load(Ordering::Relaxed), 2);
            assert!(ready.as_mut().poll(&mut cx).is_ready());
        }
        assert!(sender.try_send(owner).is_ok());
        assert_eq!(receiver.try_receive().unwrap().0, 2);
        assert!(receiver.try_receive().is_none());
    }

    #[test]
    fn cancellation_keeps_the_owner_and_both_endpoints_can_wait_again() {
        struct Owner(u8);
        let mut queue = MsduQueue::<Owner, 1>::new();
        let (mut sender, mut receiver) = queue.split();
        let wakes = Arc::new(Wakes(AtomicUsize::new(0)));
        let waker = Waker::from(wakes);
        let mut cx = Context::from_waker(&waker);
        assert!(sender.try_send(Owner(1)).is_ok());
        let owner = Owner(2);
        {
            let mut wait = core::pin::pin!(sender.ready());
            assert!(wait.as_mut().poll(&mut cx).is_pending());
        }
        assert_eq!(owner.0, 2);
        assert_eq!(receiver.try_receive().unwrap().0, 1);
        {
            let mut wait = core::pin::pin!(sender.ready());
            // Reclamation before registration is seen without another wake.
            assert!(wait.as_mut().poll(&mut cx).is_ready());
        }
        {
            let mut wait = core::pin::pin!(receiver.ready());
            assert!(wait.as_mut().poll(&mut cx).is_pending());
        }
        assert!(sender.try_send(owner).is_ok());
        {
            let mut wait = core::pin::pin!(receiver.ready());
            // Publication before registration is seen without another wake.
            assert!(wait.as_mut().poll(&mut cx).is_ready());
        }
        assert_eq!(receiver.try_receive().unwrap().0, 2);
    }

    #[test]
    fn resplitting_keeps_the_queued_owner_until_the_next_receiver_takes_it() {
        struct Owner(Arc<AtomicUsize>);
        impl Drop for Owner {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::Relaxed);
            }
        }
        let drops = Arc::new(AtomicUsize::new(0));
        {
            let mut queue = MsduQueue::<Owner, 1>::new();
            {
                let (sender, _receiver) = queue.split();
                assert!(sender.try_send(Owner(Arc::clone(&drops))).is_ok());
            }
            let (_next_sender, next_receiver) = queue.split();
            assert_eq!(drops.load(Ordering::Relaxed), 0);
            let owner = next_receiver.try_receive().unwrap();
            assert!(next_receiver.try_receive().is_none());
            assert_eq!(drops.load(Ordering::Relaxed), 0);
            drop(owner);
            assert_eq!(drops.load(Ordering::Relaxed), 1);
        }
        assert_eq!(drops.load(Ordering::Relaxed), 1);
    }
}
