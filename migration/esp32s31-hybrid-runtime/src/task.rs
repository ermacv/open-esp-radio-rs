use core::{
    ffi::c_void,
    sync::atomic::{AtomicBool, Ordering},
};

#[cfg(target_arch = "riscv32")]
extern "C" {
    fn ppTask(argument: *mut c_void);
    fn pp_create_task();
}

// Return address immediately after the pinned indirect `_task_delay(1)` call
// in the 0x1e8-byte `pp_create_task` body.
#[cfg(target_arch = "riscv32")]
const PP_CREATE_TASK_STARTUP_DELAY_RETURN_OFFSET: usize = 0x17c;

/// Logical task handle returned to the blob for the virtualized `ppTask`.
pub const PP_TASK_HANDLE: *mut c_void = core::ptr::dangling_mut::<c_void>();

/// State used by the OSI `_task_create*` adapter to suppress creation of the
/// stackful vendor `ppTask`.
pub struct VirtualPpTask {
    started: AtomicBool,
    startup_signal: AtomicBool,
    startup_delay_pending: AtomicBool,
}

impl VirtualPpTask {
    pub const fn new() -> Self {
        Self {
            started: AtomicBool::new(false),
            startup_signal: AtomicBool::new(false),
            startup_delay_pending: AtomicBool::new(false),
        }
    }

    pub fn is_pp_entry(entry: *const c_void) -> bool {
        #[cfg(target_arch = "riscv32")]
        {
            entry == ppTask as *const () as *const c_void
        }
        #[cfg(not(target_arch = "riscv32"))]
        {
            let _ = entry;
            false
        }
    }

    /// Register `ppTask` as a virtual async component and write the task handle
    /// expected by `pp_create_task`.
    ///
    /// Returns `false` for all other task entry points so the caller can log or
    /// reject them independently (notably the WPA event loop).
    ///
    /// # Safety
    /// `out_handle` must be the writable handle pointer supplied to the OSI
    /// task-create callback.
    pub unsafe fn try_start(&self, entry: *const c_void, out_handle: *mut *mut c_void) -> bool {
        if !Self::is_pp_entry(entry) {
            return false;
        }

        if !out_handle.is_null() {
            out_handle.write(PP_TASK_HANDLE);
        }
        self.started.store(true, Ordering::Release);
        self.startup_signal.store(true, Ordering::Release);
        self.startup_delay_pending.store(true, Ordering::Release);
        true
    }

    pub fn is_started(&self) -> bool {
        self.started.load(Ordering::Acquire)
    }

    /// Start the logical PP identity without entering the vendor
    /// `pp_create_task` RTOS-style envelope.
    pub fn try_start_static(&self) -> bool {
        // One AMO, not an LR/SC retry loop. Cold initialization is serialized;
        // the old value only detects an accidental second owner.
        if self.started.swap(true, Ordering::AcqRel) {
            return false;
        }
        self.startup_signal.store(false, Ordering::Release);
        self.startup_delay_pending.store(false, Ordering::Release);
        true
    }

    /// Consume the virtual startup signal that replaces the first
    /// `sem_give(s_pp_task_create_sem)` performed by the real `ppTask`.
    pub fn take_startup_signal(&self) -> bool {
        self.startup_signal.swap(false, Ordering::AcqRel)
    }

    /// Consume the one-tick yield following the startup latch in the pinned
    /// `pp_create_task`. A real task needs that yield to begin running; the
    /// virtual task has already published its state synchronously.
    pub fn take_redundant_startup_delay(&self, caller: usize, ticks: u32) -> bool {
        #[cfg(target_arch = "riscv32")]
        let expected_caller =
            pp_create_task as *const () as usize + PP_CREATE_TASK_STARTUP_DELAY_RETURN_OFFSET;
        #[cfg(not(target_arch = "riscv32"))]
        let expected_caller = usize::MAX;

        ticks == 1
            && caller == expected_caller
            && self.startup_delay_pending.swap(false, Ordering::AcqRel)
    }

    pub fn stop(&self) {
        self.started.store(false, Ordering::Release);
        self.startup_signal.store(false, Ordering::Release);
        self.startup_delay_pending.store(false, Ordering::Release);
    }
}

impl Default for VirtualPpTask {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::VirtualPpTask;

    #[test]
    fn static_start_has_no_synthetic_latch_or_delay() {
        let task = VirtualPpTask::new();
        assert!(task.try_start_static());
        assert!(task.is_started());
        assert!(!task.take_startup_signal());
        assert!(!task.take_redundant_startup_delay(usize::MAX, 1));
        assert!(!task.try_start_static());
        task.stop();
        assert!(task.try_start_static());
    }
}
