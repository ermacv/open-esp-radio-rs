use core::{
    ffi::c_void,
    mem, ptr,
    sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, AtomicUsize, Ordering},
    task::Waker,
};

use crate::{
    context::RadioContextGuard,
    diagnostics::{BlockingCall, BlockingCallProbe},
    queue::WakerCell,
};

pub const TIMER_CONTEXT_EVENT: u32 = u32::MAX - 1;

type TimerCallback = unsafe extern "C" fn(*mut c_void);

#[repr(C)]
pub struct RawOsiTimer {
    pub next: *mut c_void,
    pub expire: u32,
    pub period: u32,
    pub callback: Option<TimerCallback>,
    pub argument: *mut c_void,
}

struct TimerSlot {
    timer: AtomicPtr<c_void>,
    callback: AtomicUsize,
    argument: AtomicPtr<c_void>,
    deadline: AtomicU32,
    period: AtomicU32,
    armed: AtomicBool,
}

impl TimerSlot {
    const fn new() -> Self {
        Self {
            timer: AtomicPtr::new(ptr::null_mut()),
            callback: AtomicUsize::new(0),
            argument: AtomicPtr::new(ptr::null_mut()),
            deadline: AtomicU32::new(0),
            period: AtomicU32::new(0),
            armed: AtomicBool::new(false),
        }
    }

    fn is_due(&self, now: u32) -> bool {
        self.armed.load(Ordering::Acquire)
            && now.wrapping_sub(self.deadline.load(Ordering::Acquire)) < 0x8000_0000
    }
}

/// Fixed, allocation-free replacement for the OSI `ets_timer` adapter.
///
/// The hardware alarm must only call [`RuntimeTimerPool::alarm_interrupt`].
/// Vendor callbacks are executed later by `WifiRuntimeFuture`, in logical PP
/// context, so callbacks may safely take the vendor's inline Wi-Fi path.
pub struct RuntimeTimerPool<const N: usize> {
    slots: [TimerSlot; N],
    waker: WakerCell,
    set_callback_attempts: AtomicUsize,
    set_callback_calls: AtomicUsize,
    set_callback_rejections: AtomicUsize,
    last_set_callback: AtomicUsize,
    arm_attempts: AtomicUsize,
    arm_calls: AtomicUsize,
    arm_rejections: AtomicUsize,
    last_arm_timeout_us: AtomicUsize,
    disarm_calls: AtomicUsize,
    done_calls: AtomicUsize,
    callbacks_dispatched: AtomicUsize,
    last_callback: AtomicUsize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RuntimeTimerSnapshot {
    pub set_callback_attempts: usize,
    pub set_callback_calls: usize,
    pub set_callback_rejections: usize,
    pub last_set_callback: usize,
    pub arm_attempts: usize,
    pub arm_calls: usize,
    pub arm_rejections: usize,
    pub last_arm_timeout_us: usize,
    pub disarm_calls: usize,
    pub done_calls: usize,
    pub callbacks_dispatched: usize,
    pub last_callback: usize,
}

impl<const N: usize> RuntimeTimerPool<N> {
    pub const fn new() -> Self {
        Self {
            slots: [const { TimerSlot::new() }; N],
            waker: WakerCell::new(),
            set_callback_attempts: AtomicUsize::new(0),
            set_callback_calls: AtomicUsize::new(0),
            set_callback_rejections: AtomicUsize::new(0),
            last_set_callback: AtomicUsize::new(0),
            arm_attempts: AtomicUsize::new(0),
            arm_calls: AtomicUsize::new(0),
            arm_rejections: AtomicUsize::new(0),
            last_arm_timeout_us: AtomicUsize::new(0),
            disarm_calls: AtomicUsize::new(0),
            done_calls: AtomicUsize::new(0),
            callbacks_dispatched: AtomicUsize::new(0),
            last_callback: AtomicUsize::new(0),
        }
    }

    pub fn snapshot(&self) -> RuntimeTimerSnapshot {
        RuntimeTimerSnapshot {
            set_callback_attempts: self.set_callback_attempts.load(Ordering::Acquire),
            set_callback_calls: self.set_callback_calls.load(Ordering::Acquire),
            set_callback_rejections: self.set_callback_rejections.load(Ordering::Acquire),
            last_set_callback: self.last_set_callback.load(Ordering::Acquire),
            arm_attempts: self.arm_attempts.load(Ordering::Acquire),
            arm_calls: self.arm_calls.load(Ordering::Acquire),
            arm_rejections: self.arm_rejections.load(Ordering::Acquire),
            last_arm_timeout_us: self.last_arm_timeout_us.load(Ordering::Acquire),
            disarm_calls: self.disarm_calls.load(Ordering::Acquire),
            done_calls: self.done_calls.load(Ordering::Acquire),
            callbacks_dispatched: self.callbacks_dispatched.load(Ordering::Acquire),
            last_callback: self.last_callback.load(Ordering::Acquire),
        }
    }

    pub fn register_waker(&self, waker: &Waker) {
        self.waker.register(waker);
    }

    pub fn alarm_interrupt(&self) {
        self.waker.wake();
    }

    /// # Safety
    /// `timer` must point to writable storage for one OSI `ets_timer`.
    pub unsafe fn set_callback(
        &self,
        timer: *mut c_void,
        callback: *mut c_void,
        argument: *mut c_void,
    ) -> bool {
        let Some(slot) = self.find_or_claim(timer) else {
            return false;
        };

        self.configure_slot(slot, timer, callback, argument)
    }

    pub unsafe fn set_callback_with_reserved_tail(
        &self,
        timer: *mut c_void,
        callback: *mut c_void,
        argument: *mut c_void,
        reserved_tail: usize,
    ) -> bool {
        self.set_callback_attempts.fetch_add(1, Ordering::Relaxed);
        self.last_set_callback
            .store(callback as usize, Ordering::Release);
        let end = N.saturating_sub(reserved_tail.min(N));
        let Some(slot) = self
            .find(timer)
            .or_else(|| self.find_or_claim_in(timer, 0, end))
        else {
            self.set_callback_rejections.fetch_add(1, Ordering::Relaxed);
            return false;
        };
        self.configure_slot(slot, timer, callback, argument)
    }

    pub unsafe fn set_internal_callback(
        &self,
        timer: *mut c_void,
        callback: *mut c_void,
        argument: *mut c_void,
        reserved_tail: usize,
    ) -> bool {
        let start = N.saturating_sub(reserved_tail.min(N));
        let Some(slot) = self
            .find(timer)
            .or_else(|| self.find_or_claim_in(timer, start, N))
        else {
            return false;
        };
        self.configure_slot(slot, timer, callback, argument)
    }

    unsafe fn configure_slot(
        &self,
        slot: &TimerSlot,
        timer: *mut c_void,
        callback: *mut c_void,
        argument: *mut c_void,
    ) -> bool {
        self.set_callback_calls.fetch_add(1, Ordering::Relaxed);
        slot.armed.store(false, Ordering::Release);
        slot.argument.store(argument, Ordering::Release);
        slot.callback.store(callback as usize, Ordering::Release);

        unsafe {
            let raw = timer.cast::<RawOsiTimer>();
            (*raw).next = ptr::null_mut();
            (*raw).expire = 0;
            (*raw).period = 0;
            (*raw).callback = if callback.is_null() {
                None
            } else {
                Some(mem::transmute::<*mut c_void, TimerCallback>(callback))
            };
            (*raw).argument = argument;
        }
        self.waker.wake();
        true
    }

    /// # Safety
    /// `timer` must be the live timer previously passed to `set_callback`.
    pub unsafe fn arm_at(
        &self,
        timer: *mut c_void,
        timeout_us: u32,
        repeat: bool,
        now: u32,
    ) -> bool {
        self.arm_attempts.fetch_add(1, Ordering::Relaxed);
        self.last_arm_timeout_us
            .store(timeout_us as usize, Ordering::Release);
        let Some(slot) = self.find(timer) else {
            self.arm_rejections.fetch_add(1, Ordering::Relaxed);
            return false;
        };
        if slot.callback.load(Ordering::Acquire) == 0 {
            self.arm_rejections.fetch_add(1, Ordering::Relaxed);
            return false;
        }

        self.arm_calls.fetch_add(1, Ordering::Relaxed);

        let period = if repeat { timeout_us.max(1) } else { 0 };
        let deadline = now.wrapping_add(timeout_us);
        slot.period.store(period, Ordering::Release);
        slot.deadline.store(deadline, Ordering::Release);
        slot.armed.store(true, Ordering::Release);

        unsafe {
            let raw = timer.cast::<RawOsiTimer>();
            (*raw).expire = deadline;
            (*raw).period = period;
        }
        self.waker.wake();
        true
    }

    /// # Safety
    /// `timer` must be the live timer previously passed to `set_callback`.
    pub unsafe fn disarm(&self, timer: *mut c_void) -> bool {
        if timer.is_null() {
            return false;
        }
        self.disarm_calls.fetch_add(1, Ordering::Relaxed);
        if let Some(slot) = self.find(timer) {
            slot.armed.store(false, Ordering::Release);
            unsafe {
                (*timer.cast::<RawOsiTimer>()).period = 0;
            }
        } else if !unsafe { Self::raw_is_empty(timer.cast()) } {
            // The stock adapter treats disarming a deleted timer as a no-op.
            // Accept only our fully cleared tombstone, never an unknown live
            // object which could belong to another timer backend.
            return false;
        }
        self.waker.wake();
        true
    }

    /// # Safety
    /// `timer` must be the live timer previously passed to `set_callback`.
    pub unsafe fn done(&self, timer: *mut c_void) -> bool {
        if timer.is_null() {
            return false;
        }
        self.done_calls.fetch_add(1, Ordering::Relaxed);

        let raw = timer.cast::<RawOsiTimer>();
        if let Some(slot) = self.find(timer) {
            slot.armed.store(false, Ordering::Release);
            slot.callback.store(0, Ordering::Release);
            slot.argument.store(ptr::null_mut(), Ordering::Release);
            slot.timer.store(ptr::null_mut(), Ordering::Release);

            // Match the OSI timer contract: `done` deletes the timer backend and
            // leaves the embedded handle empty.  Clearing the complete public
            // object also gives a later duplicate `done` an unambiguous,
            // allocation-free tombstone to validate.
            unsafe {
                (*raw).next = ptr::null_mut();
                (*raw).expire = 0;
                (*raw).period = 0;
                (*raw).callback = None;
                (*raw).argument = ptr::null_mut();
            }
        } else {
            // The stock adapter makes deleting an empty timer a no-op. Preserve
            // that required idempotence, but reject an unregistered object if
            // any field still describes a live or unknown timer.
            let already_done = unsafe { Self::raw_is_empty(raw) };
            if !already_done {
                return false;
            }
        }
        self.waker.wake();
        true
    }

    unsafe fn raw_is_empty(raw: *const RawOsiTimer) -> bool {
        unsafe {
            (*raw).next.is_null()
                && (*raw).expire == 0
                && (*raw).period == 0
                && (*raw).callback.is_none()
                && (*raw).argument.is_null()
        }
    }

    pub fn dispatch_due_at(&self, now: u32, budget: usize) -> usize {
        let mut dispatched = 0;
        for slot in &self.slots {
            if dispatched == budget || !slot.is_due(now) {
                continue;
            }
            if slot
                .armed
                .compare_exchange(true, false, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
            {
                continue;
            }

            let period = slot.period.load(Ordering::Acquire);
            if period != 0 {
                let previous = slot.deadline.load(Ordering::Acquire);
                let mut next = previous.wrapping_add(period);
                if now.wrapping_sub(next) < 0x8000_0000 {
                    next = now.wrapping_add(period);
                }
                slot.deadline.store(next, Ordering::Release);
                slot.armed.store(true, Ordering::Release);
                let timer = slot.timer.load(Ordering::Acquire);
                if !timer.is_null() {
                    unsafe { (*timer.cast::<RawOsiTimer>()).expire = next };
                }
            }

            let callback = slot.callback.load(Ordering::Acquire);
            if callback != 0 {
                self.last_callback.store(callback, Ordering::Release);
                self.callbacks_dispatched.fetch_add(1, Ordering::Relaxed);
                let callback = unsafe { mem::transmute::<usize, TimerCallback>(callback) };
                let argument = slot.argument.load(Ordering::Acquire);
                let _context = RadioContextGuard::enter(TIMER_CONTEXT_EVENT);
                unsafe { callback(argument) };
                dispatched += 1;
            }
        }
        dispatched
    }

    pub fn has_due_at(&self, now: u32) -> bool {
        self.slots.iter().any(|slot| slot.is_due(now))
    }

    pub fn next_deadline_at(&self, now: u64) -> Option<u64> {
        let now_low = now as u32;
        self.slots
            .iter()
            .filter(|slot| slot.armed.load(Ordering::Acquire))
            .map(|slot| {
                let deadline = slot.deadline.load(Ordering::Acquire);
                let delta = deadline.wrapping_sub(now_low);
                if delta < 0x8000_0000 {
                    now.saturating_add(delta as u64)
                } else {
                    now
                }
            })
            .min()
    }

    fn find(&self, timer: *mut c_void) -> Option<&TimerSlot> {
        if timer.is_null() {
            return None;
        }
        self.slots
            .iter()
            .find(|slot| slot.timer.load(Ordering::Acquire) == timer)
    }

    fn find_or_claim(&self, timer: *mut c_void) -> Option<&TimerSlot> {
        self.find(timer)
            .or_else(|| self.find_or_claim_in(timer, 0, N))
    }

    fn find_or_claim_in(&self, timer: *mut c_void, start: usize, end: usize) -> Option<&TimerSlot> {
        if timer.is_null() {
            return None;
        }
        self.slots[start.min(N)..end.min(N)].iter().find(|slot| {
            slot.timer
                .compare_exchange(ptr::null_mut(), timer, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
        })
    }
}

impl<const N: usize> Default for RuntimeTimerPool<N> {
    fn default() -> Self {
        Self::new()
    }
}

pub(crate) fn record_timer_failure(
    probe: &BlockingCallProbe,
    call: BlockingCall,
    timer: *mut c_void,
) {
    probe.record(call, TIMER_CONTEXT_EVENT, timer as usize);
}

#[cfg(test)]
mod tests {
    use core::sync::atomic::{AtomicU32, Ordering};

    use super::{RawOsiTimer, RuntimeTimerPool};

    static ONE_SHOT_CALLS: AtomicU32 = AtomicU32::new(0);
    static PERIODIC_CALLS: AtomicU32 = AtomicU32::new(0);

    unsafe extern "C" fn one_shot_callback(_argument: *mut core::ffi::c_void) {
        ONE_SHOT_CALLS.fetch_add(1, Ordering::Relaxed);
    }

    unsafe extern "C" fn periodic_callback(_argument: *mut core::ffi::c_void) {
        PERIODIC_CALLS.fetch_add(1, Ordering::Relaxed);
    }

    fn raw_timer() -> RawOsiTimer {
        RawOsiTimer {
            next: core::ptr::null_mut(),
            expire: 0,
            period: 0,
            callback: None,
            argument: core::ptr::null_mut(),
        }
    }

    #[test]
    fn callbacks_only_run_when_polled_and_due() {
        ONE_SHOT_CALLS.store(0, Ordering::Relaxed);
        let pool = RuntimeTimerPool::<2>::new();
        let mut timer = raw_timer();
        let timer_ptr = core::ptr::from_mut(&mut timer).cast();

        assert!(unsafe {
            pool.set_callback(
                timer_ptr,
                one_shot_callback as *const () as *mut _,
                core::ptr::null_mut(),
            )
        });
        assert!(unsafe { pool.arm_at(timer_ptr, 10, false, 100) });
        assert_eq!(pool.dispatch_due_at(109, 1), 0);
        assert_eq!(pool.dispatch_due_at(110, 1), 1);
        assert_eq!(ONE_SHOT_CALLS.load(Ordering::Relaxed), 1);
        assert_eq!(pool.dispatch_due_at(200, 1), 0);
    }

    #[test]
    fn periodic_timer_rearms_without_drift_after_late_poll() {
        PERIODIC_CALLS.store(0, Ordering::Relaxed);
        let pool = RuntimeTimerPool::<1>::new();
        let mut timer = raw_timer();
        let timer_ptr = core::ptr::from_mut(&mut timer).cast();

        assert!(unsafe {
            pool.set_callback(
                timer_ptr,
                periodic_callback as *const () as *mut _,
                core::ptr::null_mut(),
            )
        });
        assert!(unsafe { pool.arm_at(timer_ptr, 10, true, 100) });
        assert_eq!(pool.dispatch_due_at(135, 1), 1);
        assert_eq!(timer.expire, 145);
        assert_eq!(pool.next_deadline_at(135), Some(145));
    }

    #[test]
    fn internal_tail_is_reserved_from_vendor_timers() {
        let pool = RuntimeTimerPool::<3>::new();
        let mut vendor_a = raw_timer();
        let mut vendor_b = raw_timer();
        let mut internal = raw_timer();

        unsafe {
            for timer in [&mut vendor_a, &mut vendor_b] {
                assert!(pool.set_callback_with_reserved_tail(
                    core::ptr::from_mut(timer).cast(),
                    one_shot_callback as *const () as *mut _,
                    core::ptr::null_mut(),
                    1,
                ));
            }
            assert!(pool.set_internal_callback(
                core::ptr::from_mut(&mut internal).cast(),
                one_shot_callback as *const () as *mut _,
                core::ptr::null_mut(),
                1,
            ));
        }
    }

    #[test]
    fn done_is_idempotent_only_for_an_empty_timer() {
        let pool = RuntimeTimerPool::<1>::new();
        let mut timer = raw_timer();
        let timer_ptr = core::ptr::from_mut(&mut timer).cast();

        unsafe {
            assert!(pool.set_callback(
                timer_ptr,
                one_shot_callback as *const () as *mut _,
                1usize as *mut _,
            ));
            assert!(pool.arm_at(timer_ptr, 10, false, 100));
            assert!(pool.done(timer_ptr));
            assert!(pool.done(timer_ptr));
        }
        assert!(timer.next.is_null());
        assert_eq!(timer.expire, 0);
        assert_eq!(timer.period, 0);
        assert!(timer.callback.is_none());
        assert!(timer.argument.is_null());

        let mut unknown_timer = RawOsiTimer {
            callback: Some(one_shot_callback),
            ..raw_timer()
        };
        assert!(!unsafe { pool.done(core::ptr::from_mut(&mut unknown_timer).cast()) });
    }

    #[test]
    fn done_accepts_a_never_initialized_empty_timer() {
        let pool = RuntimeTimerPool::<1>::new();
        let mut timer = raw_timer();

        assert!(unsafe { pool.done(core::ptr::from_mut(&mut timer).cast()) });
    }

    #[test]
    fn disarm_is_idempotent_only_for_an_empty_timer() {
        let pool = RuntimeTimerPool::<1>::new();
        let mut timer = raw_timer();
        let timer_ptr = core::ptr::from_mut(&mut timer).cast();

        assert!(unsafe { pool.disarm(timer_ptr) });

        let mut unknown_timer = RawOsiTimer {
            callback: Some(one_shot_callback),
            ..raw_timer()
        };
        assert!(!unsafe { pool.disarm(core::ptr::from_mut(&mut unknown_timer).cast()) });
    }
}
