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
mod tests;
