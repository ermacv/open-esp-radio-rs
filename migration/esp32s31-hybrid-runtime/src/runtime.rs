use core::{
    future::Future,
    pin::Pin,
    sync::atomic::{AtomicUsize, Ordering},
    task::{Context, Poll},
};

use crate::{
    queue::RadioQueue,
    radio::{PpDispatcher, RadioFuture},
    timer::RuntimeTimerPool,
};

static TIMER_BUDGET_SELF_WAKES: AtomicUsize = AtomicUsize::new(0);

pub fn timer_budget_self_wakes() -> usize {
    TIMER_BUDGET_SELF_WAKES.load(Ordering::Acquire)
}

/// Combined PP and OS-timer runtime. All vendor callbacks run to completion on
/// one Rust executor stack; the hardware alarm only wakes this future.
pub struct WifiRuntimeFuture<'a, D, const Q: usize, const I: usize, const T: usize> {
    radio: RadioFuture<'a, D, Q, I>,
    timers: &'a RuntimeTimerPool<T>,
    now: fn() -> u64,
    rearm_alarm: fn(Option<u64>),
    timer_budget: usize,
}

impl<'a, D, const Q: usize, const I: usize, const T: usize> WifiRuntimeFuture<'a, D, Q, I, T> {
    pub fn new(
        queue: &'a RadioQueue<Q>,
        internal_queue: &'a RadioQueue<I>,
        dispatcher: D,
        timers: &'a RuntimeTimerPool<T>,
        now: fn() -> u64,
        rearm_alarm: fn(Option<u64>),
        event_budget: usize,
        timer_budget: usize,
    ) -> Self {
        assert!(timer_budget > 0);
        Self {
            radio: RadioFuture::new(queue, internal_queue, dispatcher, event_budget),
            timers,
            now,
            rearm_alarm,
            timer_budget,
        }
    }

    pub fn radio(&self) -> &RadioFuture<'a, D, Q, I> {
        &self.radio
    }
}

impl<D: PpDispatcher + Unpin, const Q: usize, const I: usize, const T: usize> Future
    for WifiRuntimeFuture<'_, D, Q, I, T>
{
    type Output = Result<(), D::Error>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.timers.register_waker(cx.waker());

        let now = (self.now)();
        let dispatched = self.timers.dispatch_due_at(now as u32, self.timer_budget);
        if dispatched == self.timer_budget && self.timers.has_due_at(now as u32) {
            TIMER_BUDGET_SELF_WAKES.fetch_add(1, Ordering::Relaxed);
            cx.waker().wake_by_ref();
        }

        let result = Pin::new(&mut self.radio).poll(cx);
        if result.is_ready() {
            (self.rearm_alarm)(None);
        } else {
            (self.rearm_alarm)(self.timers.next_deadline_at((self.now)()));
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use core::{
        ffi::c_void,
        future::Future,
        pin::Pin,
        sync::atomic::{AtomicBool, Ordering},
        task::{Context, Poll, Waker},
    };

    use super::WifiRuntimeFuture;
    use crate::{
        context::in_radio_context,
        event::PpEvent,
        queue::RadioQueue,
        radio::{DispatchControl, PpDispatcher},
        timer::{RawOsiTimer, RuntimeTimerPool},
    };

    static RAN_IN_RADIO_CONTEXT: AtomicBool = AtomicBool::new(false);

    unsafe extern "C" fn timer_callback(_argument: *mut c_void) {
        RAN_IN_RADIO_CONTEXT.store(in_radio_context(), Ordering::Relaxed);
    }

    fn now() -> u64 {
        10
    }

    fn rearm(_deadline: Option<u64>) {}

    struct Dispatcher;

    impl PpDispatcher for Dispatcher {
        type Error = ();

        fn dispatch(&mut self, _event: PpEvent) -> Result<DispatchControl, Self::Error> {
            Ok(DispatchControl::Continue)
        }
    }

    #[test]
    fn timer_callback_runs_on_the_runtime_stack() {
        RAN_IN_RADIO_CONTEXT.store(false, Ordering::Relaxed);
        let queue = RadioQueue::<2>::new();
        let internal_queue = RadioQueue::<2>::new();
        let timers = RuntimeTimerPool::<1>::new();
        let mut raw = RawOsiTimer {
            next: core::ptr::null_mut(),
            expire: 0,
            period: 0,
            callback: None,
            argument: core::ptr::null_mut(),
        };
        let timer = core::ptr::from_mut(&mut raw).cast();
        assert!(unsafe {
            timers.set_callback(
                timer,
                timer_callback as *const () as *mut _,
                core::ptr::null_mut(),
            )
        });
        assert!(unsafe { timers.arm_at(timer, 10, false, 0) });

        let mut runtime = WifiRuntimeFuture::new(
            &queue,
            &internal_queue,
            Dispatcher,
            &timers,
            now,
            rearm,
            2,
            2,
        );
        let waker = Waker::noop();
        let mut context = Context::from_waker(waker);
        assert_eq!(Pin::new(&mut runtime).poll(&mut context), Poll::Pending);
        assert!(RAN_IN_RADIO_CONTEXT.load(Ordering::Relaxed));
    }
}
