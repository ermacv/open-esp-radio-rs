//! One-shot handoff from the initialization-only RTOS `ppTask` to Rust.
//!
//! The pinned S31 `ppTask` handles event 15 by deleting its queue, clearing
//! `pp_task_hdl` and `s_wifi_queue`, and finally calling `_task_delete(NULL)`.
//! `pp_delete_task` must not be used for this transition: it waits forever on
//! an RTOS semaphore immediately after posting that event.

use core::{
    ffi::c_void,
    future::Future,
    pin::Pin,
    ptr,
    sync::atomic::{AtomicU8, AtomicUsize, Ordering},
    task::{Context, Poll},
};

use esp_wifi_sys_esp32s31::include::wifi_osi_funcs_t;

use crate::interrupt::{InterruptSignal, WaitForInterrupt};

const UNINSTALLED: u8 = 0;
const INSTALLED: u8 = 1;
const ARMED: u8 = 2;
const DELETE_HOOK_REGISTERED: u8 = 3;
const TABLE_SWITCHED: u8 = 4;
const COMPLETE: u8 = 5;
const POST_FAILED: u8 = 6;
const DELETE_HOOK_FAILED: u8 = 7;

type TaskDelete = unsafe extern "C" fn(*mut c_void);
type TaskGetCurrent = unsafe extern "C" fn() -> *mut c_void;
pub type TaskDeleteCompletionRegistrar = unsafe fn(unsafe extern "C" fn()) -> bool;

static PHASE: AtomicU8 = AtomicU8::new(UNINSTALLED);
static BASE_TABLE: AtomicUsize = AtomicUsize::new(0);
static STRICT_TABLE: AtomicUsize = AtomicUsize::new(0);
static ORIGINAL_TASK_DELETE: AtomicUsize = AtomicUsize::new(0);
static ORIGINAL_TASK_GET_CURRENT: AtomicUsize = AtomicUsize::new(0);
static TASK_DELETE_COMPLETION_REGISTRAR: AtomicUsize = AtomicUsize::new(0);
static EXPECTED_PP_TASK: AtomicUsize = AtomicUsize::new(0);
static HANDOFF_REQUESTED: AtomicU8 = AtomicU8::new(0);
static HANDOFF_SIGNAL: InterruptSignal = InterruptSignal::new();

unsafe extern "C" {
    static mut g_osi_funcs_p: *const wifi_osi_funcs_t;
    static mut pp_task_hdl: *mut c_void;
    static mut s_wifi_queue: *mut c_void;
    fn __real_pp_post(kind: u32, argument: *mut c_void) -> i32;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PpTaskHandoffInstallError {
    AlreadyInstalled,
    MissingTaskDelete,
    MissingTaskGetCurrent,
    MissingTaskDeleteCompletionRegistrar,
    AliasedTables,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PpTaskHandoffError {
    NotInstalled,
    AlreadyStarted,
    LiveTableChanged,
    MissingPpTask,
    MissingPpQueue,
    QueueFull,
    TaskDeleteCompletionUnavailable,
    WrongExecutorHart,
    InvalidCompletionState,
}

/// Install a one-shot hook in the initialization OSI table.
///
/// `strict_table` must be a separately stored, fully configured copy of
/// `base_table`. It is not made live until the real `ppTask` reaches its final
/// `_task_delete(NULL)` call. This pointer swap avoids mutating a callback
/// table while the RTOS task is reading it.
///
/// Call this before `esp_wifi_init_internal`, after applying all strict OSI
/// patches to `strict_table`.
///
/// # Safety
/// Both tables must remain at stable addresses for the lifetime of the Wi-Fi
/// instance. No other code may replace `base_table._task_delete` or write
/// `g_osi_funcs_p` during the handoff.
pub unsafe fn install_pp_task_handoff(
    base_table: &mut wifi_osi_funcs_t,
    strict_table: &'static wifi_osi_funcs_t,
    register_task_delete_completion: TaskDeleteCompletionRegistrar,
) -> Result<(), PpTaskHandoffInstallError> {
    if ptr::eq(base_table, strict_table) {
        return Err(PpTaskHandoffInstallError::AliasedTables);
    }
    if PHASE
        .compare_exchange(UNINSTALLED, INSTALLED, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Err(PpTaskHandoffInstallError::AlreadyInstalled);
    }

    let Some(task_delete) = base_table._task_delete else {
        PHASE.store(UNINSTALLED, Ordering::Release);
        return Err(PpTaskHandoffInstallError::MissingTaskDelete);
    };
    let Some(task_get_current) = base_table._task_get_current_task else {
        PHASE.store(UNINSTALLED, Ordering::Release);
        return Err(PpTaskHandoffInstallError::MissingTaskGetCurrent);
    };

    BASE_TABLE.store(ptr::from_mut(base_table) as usize, Ordering::Release);
    STRICT_TABLE.store(ptr::from_ref(strict_table) as usize, Ordering::Release);
    ORIGINAL_TASK_DELETE.store(task_delete as *const () as usize, Ordering::Release);
    ORIGINAL_TASK_GET_CURRENT.store(task_get_current as *const () as usize, Ordering::Release);
    TASK_DELETE_COMPLETION_REGISTRAR.store(
        register_task_delete_completion as *const () as usize,
        Ordering::Release,
    );
    base_table._task_delete = Some(pp_task_delete_handoff);
    Ok(())
}

/// Post event 15 directly to the initialization-only RTOS `ppTask`.
///
/// The returned future is wake-driven. It never calls `pp_delete_task`, waits
/// on an RTOS primitive, retries, polls a status bit, or inserts a delay.
/// Cancellation does not cancel the already-posted handoff.
///
/// # Safety
/// This must be called and awaited by a thread-mode executor running on the
/// same hart that was selected as `wifi_task_core_id`. All callback producers
/// must be quiescent except for the initialization `ppTask` being retired.
pub unsafe fn begin_pp_task_handoff() -> Result<PpTaskHandoff, PpTaskHandoffError> {
    let handoff = arm_pp_task_handoff()?;
    request_armed_pp_task_handoff()?;
    Ok(handoff)
}

/// Arm the final `ppTask` hook without posting event 15 yet.
///
/// This form is used for WPA2 STA: the normal connect request runs during the
/// initialization phase, and the first owned EAPOL frame requests retirement.
/// Thus scanning and association still use the initialization task, while the
/// four-way handshake starts only after the strict Rust runtime is active.
///
/// # Safety
/// The requirements of [`begin_pp_task_handoff`] apply. The caller must ensure
/// that [`request_armed_pp_task_handoff`] is reached by a bounded event edge.
pub unsafe fn arm_pp_task_handoff() -> Result<PpTaskHandoff, PpTaskHandoffError> {
    if PHASE.load(Ordering::Acquire) == UNINSTALLED {
        return Err(PpTaskHandoffError::NotInstalled);
    }
    let base = BASE_TABLE.load(Ordering::Acquire) as *const wifi_osi_funcs_t;
    if ptr::addr_of!(g_osi_funcs_p).read_volatile() != base {
        return Err(PpTaskHandoffError::LiveTableChanged);
    }
    let task = ptr::addr_of!(pp_task_hdl).read_volatile();
    if task.is_null() {
        return Err(PpTaskHandoffError::MissingPpTask);
    }
    if ptr::addr_of!(s_wifi_queue).read_volatile().is_null() {
        return Err(PpTaskHandoffError::MissingPpQueue);
    }

    let observed = HANDOFF_SIGNAL.generation();
    EXPECTED_PP_TASK.store(task as usize, Ordering::Release);
    HANDOFF_REQUESTED.store(0, Ordering::Release);
    if PHASE
        .compare_exchange(INSTALLED, ARMED, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Err(PpTaskHandoffError::AlreadyStarted);
    }
    Ok(PpTaskHandoff {
        wait: HANDOFF_SIGNAL.wait_after(observed),
        executor_hart: crate::critical::current_hart(),
    })
}

/// Request an already-armed handoff from an event callback.
///
/// The first caller posts exactly one event. Duplicate ingress edges are
/// coalesced without touching the RTOS queue.
pub fn request_armed_pp_task_handoff() -> Result<(), PpTaskHandoffError> {
    if PHASE.load(Ordering::Acquire) != ARMED {
        return Err(PpTaskHandoffError::NotInstalled);
    }
    if HANDOFF_REQUESTED
        .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Ok(());
    }
    if unsafe { __real_pp_post(15, ptr::null_mut()) } != 0 {
        PHASE.store(POST_FAILED, Ordering::Release);
        HANDOFF_SIGNAL.notify_from_isr();
        return Err(PpTaskHandoffError::QueueFull);
    }
    Ok(())
}

pub(crate) fn request_handoff_on_wpa2_ingress() {
    if PHASE.load(Ordering::Acquire) == ARMED {
        let _ = request_armed_pp_task_handoff();
    }
}

/// True after the shutdown edge has been claimed but before the strict table
/// is live. During this short phase new `pp_post` producers must target the
/// Rust queue so the retiring `ppTask` can drain its finite backlog instead of
/// chasing continuously arriving radio events.
pub(crate) fn pp_task_handoff_draining() -> bool {
    PHASE.load(Ordering::Acquire) == ARMED && HANDOFF_REQUESTED.load(Ordering::Acquire) != 0
}

pub struct PpTaskHandoff {
    wait: WaitForInterrupt<'static>,
    executor_hart: usize,
}

impl Future for PpTaskHandoff {
    type Output = Result<(), PpTaskHandoffError>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if crate::critical::current_hart() != self.executor_hart {
            return Poll::Ready(Err(PpTaskHandoffError::WrongExecutorHart));
        }
        if Pin::new(&mut self.wait).poll(cx).is_pending() {
            return Poll::Pending;
        }
        if PHASE.load(Ordering::Acquire) == POST_FAILED {
            return Poll::Ready(Err(PpTaskHandoffError::QueueFull));
        }
        if PHASE.load(Ordering::Acquire) == DELETE_HOOK_FAILED {
            return Poll::Ready(Err(PpTaskHandoffError::TaskDeleteCompletionUnavailable));
        }
        let strict = STRICT_TABLE.load(Ordering::Acquire) as *const wifi_osi_funcs_t;
        let globals_cleared = unsafe {
            ptr::addr_of!(pp_task_hdl).read_volatile().is_null()
                && ptr::addr_of!(s_wifi_queue).read_volatile().is_null()
        };
        if PHASE.load(Ordering::Acquire) != TABLE_SWITCHED
            || strict.is_null()
            || unsafe { ptr::addr_of!(g_osi_funcs_p).read_volatile() } != strict
            || !globals_cleared
        {
            return Poll::Ready(Err(PpTaskHandoffError::InvalidCompletionState));
        }
        PHASE.store(COMPLETE, Ordering::Release);
        Poll::Ready(Ok(()))
    }
}

pub(crate) fn pp_task_handoff_complete() -> bool {
    PHASE.load(Ordering::Acquire) == COMPLETE
}

#[cfg(target_arch = "riscv32")]
#[unsafe(no_mangle)]
pub extern "C" fn esp_wifi_async_handoff_debug_phase() -> u8 {
    PHASE.load(Ordering::Acquire)
}

unsafe extern "C" fn pp_task_delete_handoff(handle: *mut c_void) {
    let original_address = ORIGINAL_TASK_DELETE.load(Ordering::Acquire);
    if original_address == 0 {
        return;
    }
    let original: TaskDelete = core::mem::transmute(original_address);

    if handle.is_null() && PHASE.load(Ordering::Acquire) == ARMED {
        let current_address = ORIGINAL_TASK_GET_CURRENT.load(Ordering::Acquire);
        let current = if current_address == 0 {
            ptr::null_mut()
        } else {
            let get_current: TaskGetCurrent = core::mem::transmute(current_address);
            get_current()
        };
        if current as usize == EXPECTED_PP_TASK.load(Ordering::Acquire) {
            let registrar_address = TASK_DELETE_COMPLETION_REGISTRAR.load(Ordering::Acquire);
            if registrar_address == 0 {
                PHASE.store(DELETE_HOOK_FAILED, Ordering::Release);
                HANDOFF_SIGNAL.notify_from_isr();
            } else {
                let registrar: TaskDeleteCompletionRegistrar =
                    core::mem::transmute(registrar_address);
                if registrar(pp_task_delete_complete) {
                    PHASE.store(DELETE_HOOK_REGISTERED, Ordering::Release);
                } else {
                    PHASE.store(DELETE_HOOK_FAILED, Ordering::Release);
                    HANDOFF_SIGNAL.notify_from_isr();
                }
            }
        }
    }

    original(handle);
}

unsafe extern "C" fn pp_task_delete_complete() {
    if PHASE.load(Ordering::Acquire) != DELETE_HOOK_REGISTERED {
        PHASE.store(DELETE_HOOK_FAILED, Ordering::Release);
        HANDOFF_SIGNAL.notify_from_isr();
        return;
    }
    let strict = STRICT_TABLE.load(Ordering::Acquire) as *const wifi_osi_funcs_t;
    if strict.is_null() {
        PHASE.store(DELETE_HOOK_FAILED, Ordering::Release);
        HANDOFF_SIGNAL.notify_from_isr();
        return;
    }

    // The scheduler calls this hook only after the current ppTask has been
    // removed from every run/wait queue and marked Deleted. It therefore
    // cannot race the strict executor or observe the table after this swap.
    ptr::addr_of_mut!(g_osi_funcs_p).write_volatile(strict);
    PHASE.store(TABLE_SWITCHED, Ordering::Release);
    HANDOFF_SIGNAL.notify_from_isr();
}
