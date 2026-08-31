//! Durable first-fault storage shared by hard-handler composition.

use core::{cell::RefCell, future::poll_fn, task::Poll};

use embassy_sync::{
    blocking_mutex::Mutex, blocking_mutex::raw::RawMutex, waitqueue::GenericAtomicWaker,
};

/// A sticky first-fault cell with cancellation-safe borrowed observation.
///
/// Publication never replaces the first fault. Waiting registers its waker
/// before rechecking the durable cell, and successful observation does not
/// clear it. The fatal condition therefore survives both later IRQ entries and
/// cancellation of an executor wait.
pub(crate) struct DurableFirstFault<M: RawMutex, T: Copy> {
    value: Mutex<M, RefCell<Option<T>>>,
    waker: GenericAtomicWaker<M>,
}

impl<M: RawMutex, T: Copy> DurableFirstFault<M, T> {
    pub(crate) const fn new() -> Self {
        Self {
            value: Mutex::new(RefCell::new(None)),
            waker: GenericAtomicWaker::new(M::INIT),
        }
    }

    pub(crate) fn publish(&self, fault: T) {
        let stored = self.value.lock(|value| {
            let mut value = value.borrow_mut();
            if value.is_some() {
                false
            } else {
                *value = Some(fault);
                true
            }
        });
        if stored {
            self.waker.wake();
        }
    }

    pub(crate) fn get(&self) -> Option<T> {
        self.value.lock(|value| *value.borrow())
    }

    pub(crate) async fn wait(&self) -> T {
        poll_fn(|context| {
            self.waker.register(context.waker());
            self.get().map_or(Poll::Pending, Poll::Ready)
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use core::{
        future::Future,
        pin::pin,
        task::{Context, Poll, Waker},
    };

    use embassy_sync::blocking_mutex::raw::NoopRawMutex;

    use super::DurableFirstFault;

    #[test]
    fn first_fault_is_durable_and_later_faults_cannot_replace_it() {
        let fault = DurableFirstFault::<NoopRawMutex, u8>::new();

        fault.publish(3);
        fault.publish(7);

        assert_eq!(fault.get(), Some(3));
    }

    #[test]
    fn wait_rechecks_durable_state_after_registration() {
        let fault = DurableFirstFault::<NoopRawMutex, u8>::new();
        let mut wait = pin!(fault.wait());
        let waker = Waker::noop();
        let mut context = Context::from_waker(waker);

        assert_eq!(wait.as_mut().poll(&mut context), Poll::Pending);
        fault.publish(11);
        assert_eq!(wait.as_mut().poll(&mut context), Poll::Ready(11));
        assert_eq!(fault.get(), Some(11));
    }

    #[test]
    fn cancelling_a_wait_does_not_lose_a_later_fault() {
        let fault = DurableFirstFault::<NoopRawMutex, u8>::new();
        let waker = Waker::noop();
        let mut context = Context::from_waker(waker);

        {
            let mut cancelled = pin!(fault.wait());
            assert_eq!(cancelled.as_mut().poll(&mut context), Poll::Pending);
        }

        fault.publish(19);
        let mut replacement = pin!(fault.wait());
        assert_eq!(replacement.as_mut().poll(&mut context), Poll::Ready(19));
        assert_eq!(fault.get(), Some(19));
    }
}
