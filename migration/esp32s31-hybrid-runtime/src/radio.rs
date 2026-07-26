use core::{
    future::Future,
    pin::Pin,
    sync::atomic::{AtomicUsize, Ordering},
    task::{Context, Poll},
};

use crate::{event::PpEvent, queue::RadioQueue};

static POLLS: AtomicUsize = AtomicUsize::new(0);
static VENDOR_EVENTS: AtomicUsize = AtomicUsize::new(0);
static INTERNAL_EVENTS: AtomicUsize = AtomicUsize::new(0);
static RX_CONTINUATIONS: AtomicUsize = AtomicUsize::new(0);
static SELF_WAKES_VENDOR: AtomicUsize = AtomicUsize::new(0);
static SELF_WAKES_INTERNAL: AtomicUsize = AtomicUsize::new(0);
static SELF_WAKES_RX: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RadioFutureSnapshot {
    pub polls: usize,
    pub vendor_events: usize,
    pub internal_events: usize,
    pub rx_continuations: usize,
    pub self_wakes_vendor: usize,
    pub self_wakes_internal: usize,
    pub self_wakes_rx: usize,
}

pub fn radio_future_snapshot() -> RadioFutureSnapshot {
    RadioFutureSnapshot {
        polls: POLLS.load(Ordering::Acquire),
        vendor_events: VENDOR_EVENTS.load(Ordering::Acquire),
        internal_events: INTERNAL_EVENTS.load(Ordering::Acquire),
        rx_continuations: RX_CONTINUATIONS.load(Ordering::Acquire),
        self_wakes_vendor: SELF_WAKES_VENDOR.load(Ordering::Acquire),
        self_wakes_internal: SELF_WAKES_INTERNAL.load(Ordering::Acquire),
        self_wakes_rx: SELF_WAKES_RX.load(Ordering::Acquire),
    }
}

#[cfg(all(target_arch = "riscv32", feature = "strict-no-wait"))]
fn pending_rx_continuation() -> Option<PpEvent> {
    crate::rx::pending_continuation()
}

#[cfg(not(all(target_arch = "riscv32", feature = "strict-no-wait")))]
fn pending_rx_continuation() -> Option<PpEvent> {
    None
}

#[cfg(all(target_arch = "riscv32", feature = "strict-no-wait"))]
fn pending_net80211_power_save(cx: &mut Context<'_>) -> Option<PpEvent> {
    crate::net80211_tx::pending_power_save_continuation(cx)
}

#[cfg(not(all(target_arch = "riscv32", feature = "strict-no-wait")))]
fn pending_net80211_power_save(_cx: &mut Context<'_>) -> Option<PpEvent> {
    None
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DispatchControl {
    Continue,
    Stop,
}

/// Run-to-completion PP event dispatcher.
///
/// Implementations must not wait for queues, semaphores, timers, or task
/// notifications. Returning from this method is the only scheduling boundary.
pub trait PpDispatcher {
    type Error;

    fn dispatch(&mut self, event: PpEvent) -> Result<DispatchControl, Self::Error>;
}

/// Wake-driven replacement for the vendor `ppTask` loop.
///
/// Vendor PP events and Rust-owned continuations have independent static
/// queues. One executor drains both with alternating preference, so neither
/// source needs an RTOS task and neither can continuously starve the other.
pub struct RadioFuture<'a, D, const N: usize, const I: usize> {
    queue: &'a RadioQueue<N>,
    internal_queue: &'a RadioQueue<I>,
    dispatcher: D,
    event_budget: usize,
    stop_requested: bool,
    next_source: u8,
}

impl<'a, D, const N: usize, const I: usize> RadioFuture<'a, D, N, I> {
    pub fn new(
        queue: &'a RadioQueue<N>,
        internal_queue: &'a RadioQueue<I>,
        dispatcher: D,
        event_budget: usize,
    ) -> Self {
        assert!(event_budget > 0);
        Self {
            queue,
            internal_queue,
            dispatcher,
            event_budget,
            stop_requested: false,
            // Preserve the historical first preference for Rust-owned work.
            next_source: 1,
        }
    }

    pub fn dispatcher(&self) -> &D {
        &self.dispatcher
    }

    pub fn dispatcher_mut(&mut self) -> &mut D {
        &mut self.dispatcher
    }
}

impl<D: PpDispatcher + Unpin, const N: usize, const I: usize> Future for RadioFuture<'_, D, N, I> {
    type Output = Result<(), D::Error>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        POLLS.fetch_add(1, Ordering::Relaxed);
        // Register before inspecting the queue. A producer racing this poll
        // either observes the waker or leaves a pending wake for registration.
        self.queue.register_waker(cx.waker());
        self.internal_queue.register_waker(cx.waker());

        for _ in 0..self.event_budget {
            let mut selected = None;
            for offset in 0..4 {
                let source = (self.next_source + offset) % 4;
                let event = match source {
                    0 => self.queue.try_pop(),
                    1 => self.internal_queue.try_pop(),
                    2 => pending_rx_continuation(),
                    3 => pending_net80211_power_save(cx),
                    _ => unreachable!(),
                };
                if let Some(event) = event {
                    selected = Some((event, source));
                    break;
                }
            }
            let Some((event, source)) = selected else {
                return if self.stop_requested {
                    Poll::Ready(Ok(()))
                } else {
                    Poll::Pending
                };
            };
            self.next_source = (source + 1) % 4;
            match source {
                0 => VENDOR_EVENTS.fetch_add(1, Ordering::Relaxed),
                1 | 3 => INTERNAL_EVENTS.fetch_add(1, Ordering::Relaxed),
                2 => RX_CONTINUATIONS.fetch_add(1, Ordering::Relaxed),
                _ => unreachable!(),
            };

            match self.dispatcher.dispatch(event) {
                Ok(DispatchControl::Continue) => {}
                // The vendor loop drains messages already queued after event
                // 15 before deleting its task. Preserve that ownership rule.
                Ok(DispatchControl::Stop) => self.stop_requested = true,
                Err(error) => return Poll::Ready(Err(error)),
            }
        }

        // Preserve fairness if producers keep the radio queue continuously
        // non-empty. This schedules one additional executor poll, not a busy
        // loop or a stack/context switch.
        let vendor_pending = !self.queue.is_empty();
        let internal_pending = !self.internal_queue.is_empty();
        let rx_pending = pending_rx_continuation().is_some();
        let net80211_power_save_pending = pending_net80211_power_save(cx).is_some();
        if vendor_pending {
            SELF_WAKES_VENDOR.fetch_add(1, Ordering::Relaxed);
        }
        if internal_pending {
            SELF_WAKES_INTERNAL.fetch_add(1, Ordering::Relaxed);
        }
        if rx_pending {
            SELF_WAKES_RX.fetch_add(1, Ordering::Relaxed);
        }
        if self.stop_requested
            || vendor_pending
            || internal_pending
            || rx_pending
            || net80211_power_save_pending
        {
            cx.waker().wake_by_ref();
        }
        Poll::Pending
    }
}

#[cfg(test)]
mod tests {
    use core::{
        ffi::c_void,
        future::Future,
        pin::Pin,
        task::{Context, Poll, Waker},
    };

    use super::{DispatchControl, PpDispatcher, RadioFuture};
    use crate::{event::PpEvent, queue::RadioQueue};

    #[derive(Default)]
    struct Dispatcher {
        calls: usize,
        seen: [u32; 4],
    }

    impl PpDispatcher for Dispatcher {
        type Error = ();

        fn dispatch(&mut self, event: PpEvent) -> Result<DispatchControl, Self::Error> {
            if self.calls < self.seen.len() {
                self.seen[self.calls] = event.kind;
            }
            self.calls += 1;
            Ok(if event.kind == 15 {
                DispatchControl::Stop
            } else {
                DispatchControl::Continue
            })
        }
    }

    fn event(kind: u32) -> PpEvent {
        PpEvent {
            kind,
            argument: core::ptr::null_mut::<c_void>(),
        }
    }

    #[test]
    fn future_only_dispatches_ready_events() {
        let queue = RadioQueue::<4>::new();
        let internal_queue = RadioQueue::<4>::new();
        let mut future = RadioFuture::new(&queue, &internal_queue, Dispatcher::default(), 4);
        let waker = Waker::noop();
        let mut context = Context::from_waker(waker);

        assert_eq!(Pin::new(&mut future).poll(&mut context), Poll::Pending);
        queue.try_push(event(8)).unwrap();
        assert_eq!(Pin::new(&mut future).poll(&mut context), Poll::Pending);
        assert_eq!(future.dispatcher().calls, 1);

        internal_queue.try_push(event(15)).unwrap();
        assert_eq!(
            Pin::new(&mut future).poll(&mut context),
            Poll::Ready(Ok(()))
        );
        assert_eq!(future.dispatcher().calls, 2);
    }

    #[test]
    fn future_alternates_internal_and_vendor_sources() {
        let queue = RadioQueue::<2>::new();
        let internal_queue = RadioQueue::<2>::new();
        queue.try_push(event(10)).unwrap();
        queue.try_push(event(11)).unwrap();
        internal_queue.try_push(event(20)).unwrap();
        internal_queue.try_push(event(21)).unwrap();

        let mut future = RadioFuture::new(&queue, &internal_queue, Dispatcher::default(), 4);
        let waker = Waker::noop();
        let mut context = Context::from_waker(waker);

        assert_eq!(Pin::new(&mut future).poll(&mut context), Poll::Pending);
        assert_eq!(future.dispatcher().calls, 4);
        assert_eq!(future.dispatcher().seen, [20, 10, 21, 11]);
        assert!(queue.is_empty());
        assert!(internal_queue.is_empty());
    }
}
