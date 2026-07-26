#[cfg(test)]
use core::cell::Cell;
use core::ffi::c_void;
#[cfg(not(test))]
use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use crate::task::PP_TASK_HANDLE;

const APPLICATION_CONTEXT: usize = 2;
const NO_EVENT: u32 = u32::MAX;

#[cfg(not(test))]
static CURRENT_CONTEXT: AtomicUsize = AtomicUsize::new(APPLICATION_CONTEXT);
#[cfg(not(test))]
static CURRENT_EVENT: AtomicU32 = AtomicU32::new(NO_EVENT);

// Unit tests execute concurrently on host threads, unlike the single radio
// executor on target. Keep their virtual task identity thread-local so one
// test cannot make another appear to be inside the Wi-Fi callback.
#[cfg(test)]
std::thread_local! {
    static CURRENT_CONTEXT: Cell<usize> = const { Cell::new(APPLICATION_CONTEXT) };
    static CURRENT_EVENT: Cell<u32> = const { Cell::new(NO_EVENT) };
}

#[cfg(not(test))]
fn swap_context(value: usize) -> usize {
    CURRENT_CONTEXT.swap(value, Ordering::AcqRel)
}

#[cfg(test)]
fn swap_context(value: usize) -> usize {
    CURRENT_CONTEXT.with(|current| current.replace(value))
}

#[cfg(not(test))]
fn swap_event(value: u32) -> u32 {
    CURRENT_EVENT.swap(value, Ordering::AcqRel)
}

#[cfg(test)]
fn swap_event(value: u32) -> u32 {
    CURRENT_EVENT.with(|current| current.replace(value))
}

#[cfg(not(test))]
fn store_context(value: usize) {
    CURRENT_CONTEXT.store(value, Ordering::Release);
}

#[cfg(test)]
fn store_context(value: usize) {
    CURRENT_CONTEXT.with(|current| current.set(value));
}

#[cfg(not(test))]
fn store_event(value: u32) {
    CURRENT_EVENT.store(value, Ordering::Release);
}

#[cfg(test)]
fn store_event(value: u32) {
    CURRENT_EVENT.with(|current| current.set(value));
}

#[cfg(not(test))]
fn load_context() -> usize {
    CURRENT_CONTEXT.load(Ordering::Acquire)
}

#[cfg(test)]
fn load_context() -> usize {
    CURRENT_CONTEXT.with(Cell::get)
}

#[cfg(not(test))]
fn load_event() -> u32 {
    CURRENT_EVENT.load(Ordering::Acquire)
}

#[cfg(test)]
fn load_event() -> u32 {
    CURRENT_EVENT.with(Cell::get)
}

/// Marks a bounded vendor handler invocation as the logical Wi-Fi task.
/// There is no task switch: this only preserves blob identity checks such as
/// `current_task_is_wifi_task()`.
pub struct RadioContextGuard {
    _task: TaskContextGuard,
}

pub(crate) struct TaskContextGuard {
    previous_context: usize,
    previous_event: u32,
}

impl RadioContextGuard {
    pub fn enter(event: u32) -> Self {
        Self {
            _task: TaskContextGuard::enter(PP_TASK_HANDLE, event),
        }
    }
}

impl TaskContextGuard {
    pub(crate) fn enter(task: *mut c_void, event: u32) -> Self {
        let previous_context = swap_context(task as usize);
        let previous_event = swap_event(event);
        Self {
            previous_context,
            previous_event,
        }
    }
}

impl Drop for TaskContextGuard {
    fn drop(&mut self) {
        store_event(self.previous_event);
        store_context(self.previous_context);
    }
}

pub fn in_radio_context() -> bool {
    load_context() == PP_TASK_HANDLE as usize
}

pub fn current_event() -> u32 {
    load_event()
}

pub(crate) fn current_task_handle() -> *mut c_void {
    load_context() as *mut c_void
}

#[cfg(test)]
mod tests {
    use super::{current_event, in_radio_context, RadioContextGuard, NO_EVENT};

    #[test]
    fn radio_identity_is_scoped_and_nestable() {
        assert!(!in_radio_context());
        assert_eq!(current_event(), NO_EVENT);
        {
            let _outer = RadioContextGuard::enter(8);
            assert!(in_radio_context());
            assert_eq!(current_event(), 8);
            {
                let _inner = RadioContextGuard::enter(16);
                assert!(in_radio_context());
                assert_eq!(current_event(), 16);
            }
            assert_eq!(current_event(), 8);
        }
        assert!(!in_radio_context());
        assert_eq!(current_event(), NO_EVENT);
    }
}
