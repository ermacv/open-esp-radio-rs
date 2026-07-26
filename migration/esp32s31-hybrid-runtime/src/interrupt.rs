use core::{
    future::Future,
    pin::Pin,
    sync::atomic::{AtomicUsize, Ordering},
    task::{Context, Poll},
};

use crate::queue::WakerCell;

/// Allocation-free edge counter for synchronizing an ISR with an async task.
///
/// The interrupt side only increments an atomic counter and wakes the task. It
/// never executes vendor code, waits for a lock, or performs a context switch.
pub struct InterruptSignal {
    generation: AtomicUsize,
    waker: WakerCell,
}

impl InterruptSignal {
    pub const fn new() -> Self {
        Self {
            generation: AtomicUsize::new(0),
            waker: WakerCell::new(),
        }
    }

    pub fn generation(&self) -> usize {
        self.generation.load(Ordering::Acquire)
    }

    /// Signal one or more pending edges from interrupt context.
    pub fn notify_from_isr(&self) -> usize {
        let next = self
            .generation
            .fetch_add(1, Ordering::Release)
            .wrapping_add(1);
        self.waker.wake();
        next
    }

    /// Wait until the counter differs from a previously observed generation.
    pub fn wait_after(&self, observed: usize) -> WaitForInterrupt<'_> {
        WaitForInterrupt {
            signal: self,
            observed,
        }
    }
}

impl Default for InterruptSignal {
    fn default() -> Self {
        Self::new()
    }
}

pub struct WaitForInterrupt<'a> {
    signal: &'a InterruptSignal,
    observed: usize,
}

impl Future for WaitForInterrupt<'_> {
    type Output = usize;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.signal.waker.register(cx.waker());
        let generation = self.signal.generation();
        if generation == self.observed {
            Poll::Pending
        } else {
            Poll::Ready(generation)
        }
    }
}

#[cfg(test)]
mod tests {
    use core::{
        future::Future,
        pin::Pin,
        task::{Context, Poll, Waker},
    };

    use super::InterruptSignal;

    #[test]
    fn signal_turns_an_interrupt_edge_into_a_future() {
        let signal = InterruptSignal::new();
        let mut wait = signal.wait_after(signal.generation());
        let waker = Waker::noop();
        let mut context = Context::from_waker(waker);
        assert_eq!(Pin::new(&mut wait).poll(&mut context), Poll::Pending);
        assert_eq!(signal.notify_from_isr(), 1);
        assert_eq!(Pin::new(&mut wait).poll(&mut context), Poll::Ready(1));
    }
}
