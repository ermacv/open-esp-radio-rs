#![cfg_attr(not(target_arch = "riscv32"), allow(dead_code))]

use core::{
    cell::UnsafeCell,
    ffi::{c_char, c_void},
    ptr,
    sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering},
};

#[cfg(target_arch = "riscv32")]
use crate::{
    radio::{DispatchControl, PpDispatcher, RadioFuture},
    runtime::WifiRuntimeFuture,
    vendor::{VendorDispatchError, VendorPpDispatcher},
};
#[cfg(target_arch = "riscv32")]
use esp_wifi_sys_esp32s31::include::wifi_osi_funcs_t;

use crate::{
    context::{current_event, current_task_handle},
    diagnostics::{BlockingCall, BlockingCallProbe},
    event::PpEvent,
    osi::OsiPpQueue,
    queue::RadioQueue,
    task::{VirtualPpTask, PP_TASK_HANDLE},
    timer::{record_timer_failure, RuntimeTimerPool},
};

pub const PP_QUEUE_CAPACITY: usize = 256;
pub const INTERNAL_EVENT_QUEUE_CAPACITY: usize = 64;
pub const DEFAULT_EVENT_BUDGET: usize = 16;
const SEMAPHORE_CAPACITY: usize = 32;
// All storage is BSS-only. Ordinary vendor identities cannot consume the tail
// reserved for channel/TX executor continuations.
pub const TIMER_CAPACITY: usize = 128;
const INTERNAL_TIMER_RESERVE: usize = 8;
const MUTEX_CAPACITY: usize = 64;
const EVENT_GROUP_CAPACITY: usize = 32;
const NO_SEMAPHORE: usize = usize::MAX;

static STATE: RadioResources = RadioResources::new();
static TIME_SOURCE: AtomicUsize = AtomicUsize::new(0);
static TASK_DELAY_CALLER: AtomicUsize = AtomicUsize::new(0);
static TASK_DELAY_TICKS: AtomicU32 = AtomicU32::new(0);
static TASK_DELAY_CALLS: AtomicUsize = AtomicUsize::new(0);
#[cfg(target_arch = "riscv32")]
static INVALID_PP_POST_CALLER: AtomicUsize = AtomicUsize::new(0);
#[cfg(target_arch = "riscv32")]
static INVALID_PP_POST_ARGUMENT: AtomicUsize = AtomicUsize::new(0);
#[cfg(target_arch = "riscv32")]
static INVALID_PP_POST_KIND: AtomicU32 = AtomicU32::new(u32::MAX);
#[cfg(target_arch = "riscv32")]
static INVALID_PP_POST_CALLS: AtomicUsize = AtomicUsize::new(0);

#[cfg(target_arch = "riscv32")]
unsafe extern "C" {
    static mut g_osi_funcs_p: *const wifi_osi_funcs_t;
    static mut g_intr_lock_mux: *mut c_void;
    static mut g_wifi_global_lock: *mut c_void;
    static mut mac_list_lock: *mut c_void;
    static mut pp_sig_cnt: [u8; 36];
    static mut pp_task_hdl: *mut c_void;
    static mut s_pp_task_create_sem: *mut c_void;
    static mut s_pp_task_del_sem: *mut c_void;
    static mut s_wifi_queue: *mut c_void;
    static mut xphyQueue: *mut c_void;
    fn pp_post(kind: u32, argument: *mut c_void) -> i32;
    fn __real_pp_post(kind: u32, argument: *mut c_void) -> i32;
}

#[cfg(target_arch = "riscv32")]
type WifiIntRestore = unsafe extern "C" fn(*mut c_void, u32);

#[cfg(target_arch = "riscv32")]
struct PpCounterCritical {
    state: u32,
    initialization_restore: Option<WifiIntRestore>,
    mux: *mut c_void,
}

#[cfg(target_arch = "riscv32")]
unsafe fn enter_pp_counter_critical(strict: bool) -> Option<PpCounterCritical> {
    if strict {
        return Some(PpCounterCritical {
            state: crate::critical::strict_wifi_int_disable(),
            initialization_restore: None,
            mux: ptr::null_mut(),
        });
    }

    let table = ptr::addr_of!(g_osi_funcs_p).read().as_ref()?;
    let disable = table._wifi_int_disable?;
    let restore = table._wifi_int_restore?;
    let mux = ptr::addr_of!(g_intr_lock_mux).read();
    Some(PpCounterCritical {
        state: disable(mux),
        initialization_restore: Some(restore),
        mux,
    })
}

#[cfg(target_arch = "riscv32")]
unsafe fn leave_pp_counter_critical(critical: PpCounterCritical) {
    if let Some(restore) = critical.initialization_restore {
        restore(critical.mux, critical.state);
    } else {
        crate::critical::strict_wifi_int_restore(critical.state);
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ShutdownQueueFull;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TaskDelaySnapshot {
    pub calls: usize,
    pub ticks: u32,
    pub caller: usize,
}

/// Last producer that submitted an event routed to the vendor's fatal/default
/// `ppTask` arm. This is observation-only BSS state; recording it never changes
/// queue ownership or retries a producer.
#[cfg(target_arch = "riscv32")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidPpPostSnapshot {
    pub calls: usize,
    pub kind: u32,
    pub argument: usize,
    pub caller: usize,
}

#[cfg(target_arch = "riscv32")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InitializationDrainError {
    BudgetExhausted {
        remaining: usize,
    },
    UnexpectedShutdown {
        processed: usize,
    },
    Dispatch {
        processed: usize,
        event: PpEvent,
        error: VendorDispatchError,
    },
}

/// Fixed storage shared with the vendor ABI and exclusively driven by one
/// Rust radio owner after handoff.
///
/// The storage must have a stable address because C callbacks cannot carry a
/// Rust context pointer. `owner` is therefore the ownership boundary: ABI
/// callbacks may publish finite work into these queues, but only the future
/// returned by `take_radio_future` or `take_wifi_runtime` may consume and
/// mutate the logical radio state. A claim is deliberately never recycled
/// yet; a complete async stop transition must be implemented before reuse can
/// be sound.
struct RadioResources {
    queue: RadioQueue<PP_QUEUE_CAPACITY>,
    internal_queue: RadioQueue<INTERNAL_EVENT_QUEUE_CAPACITY>,
    probe: BlockingCallProbe,
    virtual_task: VirtualPpTask,
    semaphores: SemaphorePool<SEMAPHORE_CAPACITY>,
    mutexes: MutexPool<MUTEX_CAPACITY>,
    event_groups: EventGroupPool<EVENT_GROUP_CAPACITY>,
    timers: RuntimeTimerPool<TIMER_CAPACITY>,
    callbacks_patched: AtomicBool,
    owner: RadioOwnerClaim,
    shutdown_processed: AtomicBool,
    queue_descriptor: QueueDescriptor,
}

impl RadioResources {
    const fn new() -> Self {
        Self {
            queue: RadioQueue::new(),
            internal_queue: RadioQueue::new(),
            probe: BlockingCallProbe::new(),
            virtual_task: VirtualPpTask::new(),
            semaphores: SemaphorePool::new(),
            mutexes: MutexPool::new(),
            event_groups: EventGroupPool::new(),
            timers: RuntimeTimerPool::new(),
            callbacks_patched: AtomicBool::new(false),
            owner: RadioOwnerClaim::new(),
            shutdown_processed: AtomicBool::new(false),
            queue_descriptor: QueueDescriptor::new(),
        }
    }

    fn queue_handle(&self) -> *mut c_void {
        ptr::addr_of!(self.queue).cast_mut().cast()
    }

    fn is_pp_queue(&self, queue: *mut c_void) -> bool {
        queue == self.queue_handle()
    }

    fn queue_bridge(&self) -> OsiPpQueue<'_, PP_QUEUE_CAPACITY> {
        OsiPpQueue::new(&self.queue, &self.probe)
    }

    /// Atomically transfer all executor-side radio authority to one owner.
    ///
    /// The capability is deliberately constructed only after the one-way
    /// claim succeeds. It is then moved into `VendorPpDispatcher`; ISR code
    /// has no access to it and can only publish work into the fixed queues.
    fn try_take_executor(&self) -> Option<RxExecutorCapability> {
        self.owner.try_take_executor()
    }
}

/// Non-cloneable proof that the caller is the sole executor-side RX consumer.
///
/// This is a zero-sized ownership token, not storage. Its private field and
/// the private `RadioResources::try_take_executor` constructor tie creation to
/// the same one-way claim that protects the complete radio future.
pub(crate) struct RxExecutorCapability {
    _private: (),
}

/// One-way ownership handoff from cold initialization to a Rust radio future.
///
/// Dropping the future does not release the claim. At present the vendor cold
/// state and interrupt publications cannot be proven reset merely because a
/// future was dropped. Keeping this transition one-way makes accidental
/// construction of two mutable radio owners impossible.
struct RadioOwnerClaim(AtomicBool);

impl RadioOwnerClaim {
    const fn new() -> Self {
        Self(AtomicBool::new(false))
    }

    fn try_take_executor(&self) -> Option<RxExecutorCapability> {
        self.0
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
            .then_some(RxExecutorCapability { _private: () })
    }

    fn is_taken(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

struct EventGroupSlot {
    allocated: AtomicBool,
    bits: AtomicU32,
}

impl EventGroupSlot {
    const fn new() -> Self {
        Self {
            allocated: AtomicBool::new(false),
            bits: AtomicU32::new(0),
        }
    }
}

struct EventGroupPool<const N: usize> {
    slots: [EventGroupSlot; N],
}

impl<const N: usize> EventGroupPool<N> {
    const fn new() -> Self {
        Self {
            slots: [const { EventGroupSlot::new() }; N],
        }
    }

    fn create(&self) -> *mut c_void {
        for slot in &self.slots {
            if slot
                .allocated
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                slot.bits.store(0, Ordering::Release);
                return ptr::from_ref(slot).cast_mut().cast();
            }
        }
        ptr::null_mut()
    }

    fn slot(&self, handle: *mut c_void) -> Option<&EventGroupSlot> {
        self.slots
            .iter()
            .find(|slot| ptr::from_ref(*slot).cast::<c_void>() == handle.cast_const())
            .filter(|slot| slot.allocated.load(Ordering::Acquire))
    }

    fn delete(&self, handle: *mut c_void) -> bool {
        let Some(slot) = self.slot(handle) else {
            return false;
        };
        slot.bits.store(0, Ordering::Release);
        slot.allocated.store(false, Ordering::Release);
        true
    }
}

struct MutexSlot {
    allocated: AtomicBool,
    recursive: AtomicBool,
    owner: AtomicUsize,
    depth: AtomicU32,
}

impl MutexSlot {
    const fn new() -> Self {
        Self {
            allocated: AtomicBool::new(false),
            recursive: AtomicBool::new(false),
            owner: AtomicUsize::new(0),
            depth: AtomicU32::new(0),
        }
    }

    const fn new_reserved(recursive: bool) -> Self {
        Self {
            allocated: AtomicBool::new(true),
            recursive: AtomicBool::new(recursive),
            owner: AtomicUsize::new(0),
            depth: AtomicU32::new(0),
        }
    }

    fn reset_reserved(&self, recursive: bool) -> bool {
        if self.owner.load(Ordering::Acquire) != 0 {
            return false;
        }
        self.recursive.store(recursive, Ordering::Release);
        self.depth.store(0, Ordering::Release);
        self.allocated.store(true, Ordering::Release);
        true
    }
}

#[link_section = ".critical.data.wifi_strict.init_global_lock"]
static INIT_GLOBAL_LOCK: MutexSlot = MutexSlot::new_reserved(true);
#[link_section = ".critical.data.wifi_strict.init_mac_list_lock"]
static INIT_MAC_LIST_LOCK: MutexSlot = MutexSlot::new_reserved(false);
#[link_section = ".critical.bss.wifi_strict.init_interrupt_lock"]
static mut INIT_INTERRUPT_LOCK: u32 = 0;

struct MutexPool<const N: usize> {
    slots: [MutexSlot; N],
}

impl<const N: usize> MutexPool<N> {
    const fn new() -> Self {
        Self {
            slots: [const { MutexSlot::new() }; N],
        }
    }

    fn create(&self, recursive: bool) -> *mut c_void {
        for slot in &self.slots {
            if slot
                .allocated
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                slot.recursive.store(recursive, Ordering::Release);
                slot.owner.store(0, Ordering::Release);
                slot.depth.store(0, Ordering::Release);
                return ptr::from_ref(slot).cast_mut().cast();
            }
        }
        ptr::null_mut()
    }

    fn slot(&self, handle: *mut c_void) -> Option<&MutexSlot> {
        if ptr::from_ref(&INIT_GLOBAL_LOCK).cast::<c_void>() == handle.cast_const() {
            return Some(&INIT_GLOBAL_LOCK);
        }
        if ptr::from_ref(&INIT_MAC_LIST_LOCK).cast::<c_void>() == handle.cast_const() {
            return Some(&INIT_MAC_LIST_LOCK);
        }
        self.slots
            .iter()
            .find(|slot| ptr::from_ref(*slot).cast::<c_void>() == handle.cast_const())
            .filter(|slot| slot.allocated.load(Ordering::Acquire))
    }

    fn delete(&self, handle: *mut c_void) -> bool {
        let Some(slot) = self.slot(handle) else {
            return false;
        };
        if slot.owner.load(Ordering::Acquire) != 0 {
            return false;
        }
        if ptr::eq(slot, &INIT_GLOBAL_LOCK) {
            return slot.reset_reserved(true);
        }
        if ptr::eq(slot, &INIT_MAC_LIST_LOCK) {
            return slot.reset_reserved(false);
        }
        slot.allocated.store(false, Ordering::Release);
        true
    }

    fn lock(&self, handle: *mut c_void, owner: usize) -> bool {
        let Some(slot) = self.slot(handle) else {
            return false;
        };
        if slot
            .owner
            .compare_exchange(0, owner, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            slot.depth.store(1, Ordering::Release);
            return true;
        }
        if slot.owner.load(Ordering::Acquire) == owner && slot.recursive.load(Ordering::Acquire) {
            slot.depth.fetch_add(1, Ordering::AcqRel);
            return true;
        }
        false
    }

    fn unlock(&self, handle: *mut c_void, owner: usize) -> bool {
        let Some(slot) = self.slot(handle) else {
            return false;
        };
        if slot.owner.load(Ordering::Acquire) != owner {
            return false;
        }
        let depth = slot.depth.load(Ordering::Acquire);
        if depth > 1 {
            slot.depth.store(depth - 1, Ordering::Release);
        } else {
            slot.depth.store(0, Ordering::Release);
            slot.owner.store(0, Ordering::Release);
        }
        true
    }
}

#[repr(C)]
struct QueueDescriptor {
    // `pp_create_task` reads the first word and stores it in `xphyQueue`.
    queue: UnsafeCell<*mut c_void>,
}

impl QueueDescriptor {
    const fn new() -> Self {
        Self {
            queue: UnsafeCell::new(ptr::null_mut()),
        }
    }

    unsafe fn initialize(&self, queue: *mut c_void) -> *mut c_void {
        self.queue.get().write(queue);
        self.queue.get().cast()
    }
}

unsafe impl Sync for QueueDescriptor {}

struct SemaphoreSlot {
    allocated: AtomicBool,
    count: AtomicU32,
    maximum: AtomicU32,
}

impl SemaphoreSlot {
    const fn new() -> Self {
        Self {
            allocated: AtomicBool::new(false),
            count: AtomicU32::new(0),
            maximum: AtomicU32::new(0),
        }
    }

    fn try_take(&self) -> bool {
        let count = self.count.load(Ordering::Acquire);
        if count == 0 {
            return false;
        }
        self.count
            .compare_exchange(count, count - 1, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    fn give(&self) -> bool {
        let maximum = self.maximum.load(Ordering::Acquire);
        let count = self.count.load(Ordering::Acquire);
        if count >= maximum {
            return false;
        }
        self.count
            .compare_exchange(count, count + 1, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }
}

struct SemaphorePool<const N: usize> {
    slots: [SemaphoreSlot; N],
    last_created: AtomicUsize,
    thread_semaphore: AtomicUsize,
}

impl<const N: usize> SemaphorePool<N> {
    const fn new() -> Self {
        Self {
            slots: [const { SemaphoreSlot::new() }; N],
            last_created: AtomicUsize::new(NO_SEMAPHORE),
            thread_semaphore: AtomicUsize::new(NO_SEMAPHORE),
        }
    }

    fn create(&self, maximum: u32, initial: u32) -> *mut c_void {
        if maximum == 0 || initial > maximum {
            return ptr::null_mut();
        }

        for (index, slot) in self.slots.iter().enumerate() {
            if slot
                .allocated
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                slot.maximum.store(maximum, Ordering::Release);
                slot.count.store(initial, Ordering::Release);
                self.last_created.store(index, Ordering::Release);
                return ptr::from_ref(slot).cast_mut().cast();
            }
        }
        ptr::null_mut()
    }

    fn slot(&self, handle: *mut c_void) -> Option<&SemaphoreSlot> {
        self.slots
            .iter()
            .find(|slot| ptr::from_ref(*slot).cast::<c_void>() == handle.cast_const())
            .filter(|slot| slot.allocated.load(Ordering::Acquire))
    }

    fn delete(&self, handle: *mut c_void) {
        if let Some(slot) = self.slot(handle) {
            slot.count.store(0, Ordering::Release);
            slot.maximum.store(0, Ordering::Release);
            slot.allocated.store(false, Ordering::Release);
        }
    }

    fn signal_last_created(&self) -> bool {
        let index = self.last_created.load(Ordering::Acquire);
        self.slots.get(index).is_some_and(SemaphoreSlot::give)
    }

    fn thread_semaphore(&self) -> *mut c_void {
        let existing = self.thread_semaphore.load(Ordering::Acquire);
        if let Some(slot) = self.slots.get(existing) {
            if slot.allocated.load(Ordering::Acquire) {
                return ptr::from_ref(slot).cast_mut().cast();
            }
        }

        let handle = self.create(1, 0);
        if handle.is_null() {
            return handle;
        }
        let index = self
            .slots
            .iter()
            .position(|slot| ptr::from_ref(slot).cast::<c_void>() == handle.cast_const())
            .unwrap_or(NO_SEMAPHORE);
        self.thread_semaphore.store(index, Ordering::Release);
        handle
    }
}

pub fn radio_queue() -> &'static RadioQueue<PP_QUEUE_CAPACITY> {
    &STATE.queue
}

pub fn internal_event_queue_snapshot() -> crate::queue::RadioQueueSnapshot {
    STATE.internal_queue.snapshot()
}

pub fn blocking_probe() -> &'static BlockingCallProbe {
    &STATE.probe
}

pub fn task_delay_snapshot() -> TaskDelaySnapshot {
    TaskDelaySnapshot {
        calls: TASK_DELAY_CALLS.load(Ordering::Acquire),
        ticks: TASK_DELAY_TICKS.load(Ordering::Relaxed),
        caller: TASK_DELAY_CALLER.load(Ordering::Relaxed),
    }
}

#[cfg(target_arch = "riscv32")]
pub fn invalid_pp_post_snapshot() -> InvalidPpPostSnapshot {
    InvalidPpPostSnapshot {
        calls: INVALID_PP_POST_CALLS.load(Ordering::Acquire),
        kind: INVALID_PP_POST_KIND.load(Ordering::Relaxed),
        argument: INVALID_PP_POST_ARGUMENT.load(Ordering::Relaxed),
        caller: INVALID_PP_POST_CALLER.load(Ordering::Relaxed),
    }
}

pub(crate) fn clear_task_delay_snapshot() {
    TASK_DELAY_CALLER.store(0, Ordering::Relaxed);
    TASK_DELAY_TICKS.store(0, Ordering::Relaxed);
    TASK_DELAY_CALLS.store(0, Ordering::Release);
}

pub fn timer_alarm_interrupt() {
    STATE.timers.alarm_interrupt();
}

pub fn next_timer_deadline_us() -> Option<u64> {
    configured_now().and_then(|now| STATE.timers.next_deadline_at(now()))
}

pub fn timer_snapshot() -> crate::timer::RuntimeTimerSnapshot {
    STATE.timers.snapshot()
}

/// Publish the monotonic clock before vendor initialization can arm an OSI
/// timer. This does not start the radio future or program an alarm.
#[cfg(target_arch = "riscv32")]
pub fn configure_wifi_runtime_clock(now: fn() -> u64) -> bool {
    let address = now as usize;
    TIME_SOURCE
        .compare_exchange(0, address, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
        || TIME_SOURCE.load(Ordering::Acquire) == address
}

#[cfg(target_arch = "riscv32")]
pub(crate) fn virtual_pp_task_started() -> bool {
    STATE.virtual_task.is_started()
}

fn init_interrupt_lock_handle() -> *mut c_void {
    ptr::addr_of_mut!(INIT_INTERRUPT_LOCK).cast()
}

fn init_global_lock_handle() -> *mut c_void {
    ptr::from_ref(&INIT_GLOBAL_LOCK).cast_mut().cast()
}

fn init_mac_list_lock_handle() -> *mut c_void {
    ptr::from_ref(&INIT_MAC_LIST_LOCK).cast_mut().cast()
}

/// Verify the three fixed lock publications used by cold initialization.
///
/// # Safety
///
/// Wi-Fi initialization/deinitialization must not mutate the publication
/// cells concurrently.
#[cfg(target_arch = "riscv32")]
pub unsafe fn static_wifi_init_locks_bound() -> bool {
    ptr::addr_of!(g_intr_lock_mux).read_volatile() == init_interrupt_lock_handle()
        && ptr::addr_of!(g_wifi_global_lock).read_volatile() == init_global_lock_handle()
        && ptr::addr_of!(mac_list_lock).read_volatile() == init_mac_list_lock_handle()
        && INIT_GLOBAL_LOCK.allocated.load(Ordering::Acquire)
        && INIT_MAC_LIST_LOCK.allocated.load(Ordering::Acquire)
}

#[cfg(target_arch = "riscv32")]
pub(crate) unsafe fn bind_static_wifi_init_locks() -> bool {
    let interrupt = init_interrupt_lock_handle();
    let global = init_global_lock_handle();
    let mac_list = init_mac_list_lock_handle();
    let current_interrupt = ptr::addr_of!(g_intr_lock_mux).read_volatile();
    let current_global = ptr::addr_of!(g_wifi_global_lock).read_volatile();
    let current_mac_list = ptr::addr_of!(mac_list_lock).read_volatile();
    if (!current_interrupt.is_null() && current_interrupt != interrupt)
        || (!current_global.is_null() && current_global != global)
        || (!current_mac_list.is_null() && current_mac_list != mac_list)
        || !INIT_GLOBAL_LOCK.reset_reserved(true)
        || !INIT_MAC_LIST_LOCK.reset_reserved(false)
    {
        return false;
    }
    ptr::addr_of_mut!(g_intr_lock_mux).write_volatile(interrupt);
    ptr::addr_of_mut!(g_wifi_global_lock).write_volatile(global);
    ptr::addr_of_mut!(mac_list_lock).write_volatile(mac_list);
    true
}

#[cfg(target_arch = "riscv32")]
pub(crate) unsafe fn unbind_static_wifi_init_locks() -> bool {
    if !static_wifi_init_locks_bound()
        || !INIT_GLOBAL_LOCK.reset_reserved(true)
        || !INIT_MAC_LIST_LOCK.reset_reserved(false)
    {
        return false;
    }
    ptr::addr_of_mut!(g_intr_lock_mux).write_volatile(ptr::null_mut());
    ptr::addr_of_mut!(g_wifi_global_lock).write_volatile(ptr::null_mut());
    ptr::addr_of_mut!(mac_list_lock).write_volatile(ptr::null_mut());
    true
}

/// Verify the exact fixed queue and logical task publications used by the
/// taskless cold-init wrapper.
///
/// # Safety
///
/// Wi-Fi initialization/deinitialization must not mutate the five vendor
/// publication cells concurrently.
#[cfg(target_arch = "riscv32")]
pub unsafe fn static_pp_task_bound() -> bool {
    let queue = STATE.queue_handle();
    let descriptor = ptr::addr_of!(STATE.queue_descriptor).cast_mut().cast();
    STATE.virtual_task.is_started()
        && ptr::addr_of!(pp_task_hdl).read_volatile() == PP_TASK_HANDLE
        && ptr::addr_of!(s_wifi_queue).read_volatile() == descriptor
        && ptr::addr_of!(xphyQueue).read_volatile() == queue
        && ptr::addr_of!(s_pp_task_create_sem)
            .read_volatile()
            .is_null()
        && ptr::addr_of!(s_pp_task_del_sem)
            .read_volatile()
            .is_null()
}

/// Replace the RTOS-style `pp_create_task` envelope with direct fixed-state
/// publication. No task entry, semaphore, queue-create callback, delay, or
/// scheduler primitive is entered.
#[cfg(all(
    target_arch = "riscv32",
    feature = "rust-static-pp-task-init-interpose"
))]
#[no_mangle]
pub unsafe extern "C" fn __wrap_pp_create_task() -> i32 {
    if !ptr::addr_of!(pp_task_hdl).read_volatile().is_null()
        || !ptr::addr_of!(s_wifi_queue).read_volatile().is_null()
        || !STATE.virtual_task.try_start_static()
    {
        return 0x101;
    }

    let queue = STATE.queue_handle();
    let descriptor = STATE.queue_descriptor.initialize(queue);
    ptr::addr_of_mut!(xphyQueue).write_volatile(queue);
    ptr::addr_of_mut!(s_wifi_queue).write_volatile(descriptor);
    ptr::addr_of_mut!(s_pp_task_create_sem).write_volatile(ptr::null_mut());
    ptr::addr_of_mut!(s_pp_task_del_sem).write_volatile(ptr::null_mut());
    ptr::addr_of_mut!(pp_task_hdl).write_volatile(PP_TASK_HANDLE);
    STATE.shutdown_processed.store(false, Ordering::Release);
    0
}

/// Paired taskless deinitializer. A live future or queued work fails
/// immediately instead of being discarded or synchronously drained.
#[cfg(all(
    target_arch = "riscv32",
    feature = "rust-static-pp-task-init-interpose"
))]
#[no_mangle]
pub unsafe extern "C" fn __wrap_pp_delete_task() -> i32 {
    if STATE.owner.is_taken() || !STATE.queue.is_empty() || !STATE.internal_queue.is_empty() {
        return 0x101;
    }
    if !static_pp_task_bound() {
        return if ptr::addr_of!(pp_task_hdl).read_volatile().is_null()
            && ptr::addr_of!(s_wifi_queue).read_volatile().is_null()
        {
            0
        } else {
            0x101
        };
    }

    ptr::addr_of_mut!(pp_task_hdl).write_volatile(ptr::null_mut());
    ptr::addr_of_mut!(s_wifi_queue).write_volatile(ptr::null_mut());
    ptr::addr_of_mut!(xphyQueue).write_volatile(ptr::null_mut());
    ptr::addr_of_mut!(s_pp_task_create_sem).write_volatile(ptr::null_mut());
    ptr::addr_of_mut!(s_pp_task_del_sem).write_volatile(ptr::null_mut());
    STATE.virtual_task.stop();
    0
}

/// Run a finite cold-start batch after `esp_wifi_init_internal` returns.
///
/// This processes already-owned queue entries on the caller's stack. It does
/// not create a task, wait for producers, poll a status flag, or insert a
/// delay. The explicit budget makes a continuously producing initialization
/// path fail closed.
#[cfg(target_arch = "riscv32")]
pub fn drain_wifi_initialization_events(budget: usize) -> Result<usize, InitializationDrainError> {
    let mut dispatcher = VendorPpDispatcher::for_initialization();
    let mut processed = 0;
    while processed < budget {
        let Some(event) = STATE.queue.try_pop() else {
            return Ok(processed);
        };
        match dispatcher.dispatch(event) {
            Ok(DispatchControl::Continue) => processed += 1,
            Ok(DispatchControl::Stop) => {
                return Err(InitializationDrainError::UnexpectedShutdown { processed });
            }
            Err(error) => {
                return Err(InitializationDrainError::Dispatch {
                    processed,
                    event,
                    error,
                });
            }
        }
    }
    let remaining = STATE.queue.len();
    if remaining == 0 {
        Ok(processed)
    } else {
        Err(InitializationDrainError::BudgetExhausted { remaining })
    }
}

pub(crate) fn mark_shutdown_processed() {
    STATE.shutdown_processed.store(true, Ordering::Release);
    STATE.virtual_task.stop();
}

/// Schedule an internal vendor continuation on the same timer pool used by
/// `WifiRuntimeFuture`.
///
/// # Safety
/// `timer` must point to writable `RawOsiTimer` storage that remains live until
/// the callback fires or is explicitly rescheduled.
#[allow(dead_code)]
pub(crate) unsafe fn schedule_internal_timer(
    timer: *mut c_void,
    callback: unsafe extern "C" fn(*mut c_void),
    argument: *mut c_void,
    delay_us: u32,
) -> bool {
    let Some(now) = configured_now() else {
        STATE.probe.record(
            BlockingCall::TimerWithoutClock,
            current_event(),
            timer as usize,
        );
        return false;
    };
    let registered = STATE.timers.set_internal_callback(
        timer,
        callback as *const () as *mut c_void,
        argument,
        INTERNAL_TIMER_RESERVE,
    );
    if !registered {
        record_timer_failure(&STATE.probe, BlockingCall::TimerSetCallbackRejected, timer);
        return false;
    }
    if !STATE.timers.arm_at(timer, delay_us, false, now() as u32) {
        record_timer_failure(&STATE.probe, BlockingCall::TimerArmRejected, timer);
        return false;
    }
    true
}

#[allow(dead_code)]
pub(crate) unsafe fn cancel_internal_timer(timer: *mut c_void) -> bool {
    STATE.timers.done(timer)
}

#[cfg(feature = "wpa-async-eap")]
pub(crate) fn try_lock_internal_mutex(handle: *mut c_void) -> bool {
    STATE.mutexes.lock(handle, current_task_handle() as usize)
}

#[cfg(feature = "wpa-async-eap")]
pub(crate) fn unlock_internal_mutex(handle: *mut c_void) -> bool {
    STATE.mutexes.unlock(handle, current_task_handle() as usize)
}

#[cfg(feature = "wpa-async-eap")]
pub(crate) fn give_internal_semaphore(handle: *mut c_void) -> bool {
    STATE
        .semaphores
        .slot(handle)
        .is_some_and(SemaphoreSlot::give)
}

#[cfg(target_arch = "riscv32")]
#[allow(dead_code)]
#[link_section = ".rwtext.wifi_strict.internal_event"]
pub(crate) fn enqueue_internal_event(event: PpEvent) -> bool {
    // Strict internal callbacks share one radio hart, but an interrupt may
    // preempt an executor/callback producer. Their queue deliberately makes
    // one CAS attempt and reports producer contention rather than spinning,
    // so serialize that single publication with the same bounded local
    // interrupt mask used by pp_post. Vendor PP events use a separate queue.
    // This is not a cross-hart lock and contains no retry or wait.
    if crate::critical::strict_wifi_hart_armed() {
        if !crate::critical::on_strict_wifi_hart() {
            return false;
        }
        let interrupt_state = unsafe { crate::critical::strict_wifi_int_disable() };
        let queued = STATE.internal_queue.try_push_deferred_wake(event).is_ok();
        unsafe { crate::critical::strict_wifi_int_restore(interrupt_state) };
        if queued {
            wifi_strict_wake_internal_consumer();
        }
        queued
    } else {
        STATE.internal_queue.try_push(event).is_ok()
    }
}

/// Wake the single Rust radio consumer without inserting a finite queue item.
///
/// RX uses this after publishing an empty-to-non-empty transition into its
/// durable intrusive queue. Keeping this leaf in internal SRAM makes the
/// callback's direct call graph independent of flash cache availability.
#[cfg(target_arch = "riscv32")]
#[no_mangle]
#[inline(never)]
#[link_section = ".rwtext.wifi_strict.internal_event"]
pub(crate) extern "C" fn wifi_strict_wake_internal_consumer() {
    STATE.internal_queue.wake_consumer();
}

/// Queue the vendor's shutdown event without entering `pp_delete_task`, whose
/// synchronous semaphore handshake cannot be used by a stackless executor.
#[cfg(target_arch = "riscv32")]
pub fn request_shutdown() -> Result<(), ShutdownQueueFull> {
    let result = unsafe { pp_post(15, ptr::null_mut()) };
    if result == 0 {
        Ok(())
    } else {
        Err(ShutdownQueueFull)
    }
}

#[cfg(target_arch = "riscv32")]
pub fn take_radio_future(
    event_budget: usize,
) -> Option<
    RadioFuture<'static, VendorPpDispatcher, PP_QUEUE_CAPACITY, INTERNAL_EVENT_QUEUE_CAPACITY>,
> {
    let rx_executor = STATE.try_take_executor()?;
    Some(RadioFuture::new(
        &STATE.queue,
        &STATE.internal_queue,
        VendorPpDispatcher::new(rx_executor),
        event_budget,
    ))
}

/// Take the complete PP + OS-timer future. `rearm_alarm` must program one
/// non-blocking hardware/executor alarm and that alarm's ISR must call
/// [`timer_alarm_interrupt`].
#[cfg(target_arch = "riscv32")]
pub fn take_wifi_runtime(
    event_budget: usize,
    timer_budget: usize,
    now: fn() -> u64,
    rearm_alarm: fn(Option<u64>),
) -> Option<
    WifiRuntimeFuture<
        'static,
        VendorPpDispatcher,
        PP_QUEUE_CAPACITY,
        INTERNAL_EVENT_QUEUE_CAPACITY,
        TIMER_CAPACITY,
    >,
> {
    let rx_executor = STATE.try_take_executor()?;
    TIME_SOURCE.store(now as usize, Ordering::Release);
    Some(WifiRuntimeFuture::new(
        &STATE.queue,
        &STATE.internal_queue,
        VendorPpDispatcher::new(rx_executor),
        &STATE.timers,
        now,
        rearm_alarm,
        event_budget,
        timer_budget,
    ))
}

/// Replace only scheduling-related callbacks in an otherwise complete S31 OSI
/// table. Hardware interrupts, PHY, clocks, allocator, NVS, and coexistence
/// hooks remain owned by the caller's base adapter.
///
/// `_env_is_chip` is also pinned here because the vendor RX-success path calls
/// OSI slot 1 indirectly. The strict runtime supports only real ESP32-S31
/// silicon, so this leaf has one constant, finite answer and needs no adapter
/// state.
#[cfg(target_arch = "riscv32")]
#[no_mangle]
#[inline(never)]
#[link_section = ".rwtext.wifi_strict.env_is_chip"]
pub unsafe extern "C" fn wifi_strict_env_is_chip() -> bool {
    true
}

#[cfg(target_arch = "riscv32")]
pub fn patch_pp_runtime_callbacks(table: &mut wifi_osi_funcs_t) {
    table._env_is_chip = Some(wifi_strict_env_is_chip);
    table._task_yield_from_isr = Some(task_yield_from_isr);
    table._semphr_create = Some(semphr_create);
    table._semphr_delete = Some(semphr_delete);
    table._semphr_take = Some(semphr_take);
    table._semphr_give = Some(semphr_give);
    table._wifi_thread_semphr_get = Some(wifi_thread_semphr_get);
    table._mutex_create = Some(mutex_create);
    table._recursive_mutex_create = Some(recursive_mutex_create);
    table._mutex_delete = Some(mutex_delete);
    table._mutex_lock = Some(mutex_lock);
    table._mutex_unlock = Some(mutex_unlock);
    table._queue_create = Some(queue_create);
    table._queue_delete = Some(queue_delete);
    table._queue_send = Some(queue_send);
    table._queue_send_from_isr = Some(queue_send_from_isr);
    table._queue_send_to_back = Some(queue_send);
    table._queue_send_to_front = Some(queue_send);
    table._queue_recv = Some(queue_receive);
    table._queue_msg_waiting = Some(queue_messages_waiting);
    table._event_group_create = Some(event_group_create);
    table._event_group_delete = Some(event_group_delete);
    table._event_group_set_bits = Some(event_group_set_bits);
    table._event_group_clear_bits = Some(event_group_clear_bits);
    table._event_group_wait_bits = Some(event_group_wait_bits);
    table._task_create_pinned_to_core = Some(task_create_pinned);
    table._task_create = Some(task_create);
    table._task_delete = Some(task_delete);
    table._task_delay = Some(task_delay);
    table._task_ms_to_tick = Some(task_ms_to_tick);
    table._task_get_current_task = Some(task_get_current);
    table._task_get_max_priority = Some(task_max_priority);
    table._wifi_create_queue = Some(wifi_create_queue);
    table._wifi_delete_queue = Some(wifi_delete_queue);
    table._timer_arm = Some(timer_arm_ms);
    table._timer_disarm = Some(timer_disarm);
    table._timer_done = Some(timer_done);
    table._timer_setfn = Some(timer_setfn);
    table._timer_arm_us = Some(timer_arm_us);
    table._esp_timer_get_time = Some(esp_timer_get_time);
    STATE.callbacks_patched.store(true, Ordering::Release);
}

#[cfg(target_arch = "riscv32")]
pub(crate) fn pp_runtime_callbacks_patched() -> bool {
    if !STATE.callbacks_patched.load(Ordering::Acquire) {
        return false;
    }
    let table = unsafe { ptr::addr_of!(g_osi_funcs_p).read().as_ref() };
    let Some(table) = table else {
        return false;
    };
    macro_rules! callback_is {
        ($field:ident, $callback:expr) => {
            table.$field.is_some_and(|registered| {
                registered as *const () as usize == $callback as *const () as usize
            })
        };
    }
    callback_is!(_env_is_chip, wifi_strict_env_is_chip)
        && callback_is!(_task_yield_from_isr, task_yield_from_isr)
        && callback_is!(_semphr_create, semphr_create)
        && callback_is!(_semphr_delete, semphr_delete)
        && callback_is!(_semphr_take, semphr_take)
        && callback_is!(_semphr_give, semphr_give)
        && callback_is!(_wifi_thread_semphr_get, wifi_thread_semphr_get)
        && callback_is!(_mutex_create, mutex_create)
        && callback_is!(_recursive_mutex_create, recursive_mutex_create)
        && callback_is!(_mutex_delete, mutex_delete)
        && callback_is!(_mutex_lock, mutex_lock)
        && callback_is!(_mutex_unlock, mutex_unlock)
        && callback_is!(_queue_create, queue_create)
        && callback_is!(_queue_delete, queue_delete)
        && callback_is!(_queue_send, queue_send)
        && callback_is!(_queue_send_from_isr, queue_send_from_isr)
        && callback_is!(_queue_send_to_back, queue_send)
        && callback_is!(_queue_send_to_front, queue_send)
        && callback_is!(_queue_recv, queue_receive)
        && callback_is!(_queue_msg_waiting, queue_messages_waiting)
        && callback_is!(_event_group_create, event_group_create)
        && callback_is!(_event_group_delete, event_group_delete)
        && callback_is!(_event_group_set_bits, event_group_set_bits)
        && callback_is!(_event_group_clear_bits, event_group_clear_bits)
        && callback_is!(_event_group_wait_bits, event_group_wait_bits)
        && callback_is!(_task_create_pinned_to_core, task_create_pinned)
        && callback_is!(_task_create, task_create)
        && callback_is!(_task_delete, task_delete)
        && callback_is!(_task_delay, task_delay)
        && callback_is!(_task_ms_to_tick, task_ms_to_tick)
        && callback_is!(_task_get_current_task, task_get_current)
        && callback_is!(_task_get_max_priority, task_max_priority)
        && callback_is!(_wifi_create_queue, wifi_create_queue)
        && callback_is!(_wifi_delete_queue, wifi_delete_queue)
        && callback_is!(_timer_arm, timer_arm_ms)
        && callback_is!(_timer_disarm, timer_disarm)
        && callback_is!(_timer_done, timer_done)
        && callback_is!(_timer_setfn, timer_setfn)
        && callback_is!(_timer_arm_us, timer_arm_us)
        && callback_is!(_esp_timer_get_time, esp_timer_get_time)
}

#[cfg(target_arch = "riscv32")]
pub(crate) fn pp_post_link_wrapper_active() -> bool {
    core::ptr::eq(pp_post as *const (), __wrap_pp_post as *const ())
}

/// Strict final-link replacement for the vendor `pp_post` dispatcher.
///
/// Initialization and teardown still delegate to the original implementation.
/// Once the strict hart is armed, this preserves the recovered signal-counter
/// coalescing rules and writes directly to the fixed Rust queue. There is no
/// OSI queue callback, scheduler yield, retry, or cross-core spinlock.
///
/// # Safety
///
/// Must be called with the vendor `pp_post` ABI. Before strict mode, `argument`
/// must satisfy the original function's requirements because it is forwarded
/// unchanged. In strict mode its ownership is transferred to the selected
/// one-action event handler.
#[cfg(target_arch = "riscv32")]
#[no_mangle]
#[inline(never)]
pub unsafe extern "C" fn __wrap_pp_post(kind: u32, argument: *mut c_void) -> i32 {
    // Read `ra` before the wrapper makes any call. Events 9..=12 and 28 are
    // deliberately routed to the stock fatal/default arm, so retaining their
    // producer is the only safe way to distinguish a malformed TX queue ID
    // from a timer or power-management producer after async dispatch.
    let caller: usize;
    core::arch::asm!("mv {caller}, ra", caller = out(reg) caller, options(nomem, nostack));
    if matches!(kind, 9..=12 | 28 | 34..=u32::MAX) {
        INVALID_PP_POST_CALLER.store(caller, Ordering::Relaxed);
        INVALID_PP_POST_ARGUMENT.store(argument as usize, Ordering::Relaxed);
        INVALID_PP_POST_KIND.store(kind, Ordering::Relaxed);
        INVALID_PP_POST_CALLS.fetch_add(1, Ordering::Release);
    }

    let strict = crate::critical::strict_wifi_hart_armed();
    let draining = crate::handoff::pp_task_handoff_draining();
    if !strict && !draining {
        return __real_pp_post(kind, argument);
    }
    if (strict && !crate::critical::on_strict_wifi_hart()) || kind > 35 || kind == 13 {
        return 1;
    }

    let Some(interrupt_state) = enter_pp_counter_critical(strict) else {
        return 1;
    };
    let counter = ptr::addr_of_mut!(pp_sig_cnt)
        .cast::<u8>()
        .add(kind as usize);
    let previous = counter.read();
    if !(6..=8).contains(&kind) && previous != 0 {
        leave_pp_counter_critical(interrupt_state);
        return 0;
    }
    let Some(next) = previous.checked_add(1) else {
        leave_pp_counter_critical(interrupt_state);
        return 1;
    };
    let event = PpEvent { kind, argument };
    if STATE.queue.try_push_deferred_wake(event).is_ok() {
        counter.write(next);
        leave_pp_counter_critical(interrupt_state);
        STATE.queue.wake_consumer();
        return 0;
    }
    leave_pp_counter_critical(interrupt_state);
    1
}

unsafe extern "C" fn task_yield_from_isr() {
    // Queue/timer producers wake the Rust executor directly. There is no
    // higher-priority RTOS task to switch to on ISR exit.
}

#[cfg(not(target_arch = "riscv32"))]
pub fn patch_pp_runtime_callbacks<T>(_table: &mut T) {}

unsafe extern "C" fn semphr_create(maximum: u32, initial: u32) -> *mut c_void {
    let handle = STATE.semaphores.create(maximum, initial);
    if handle.is_null() {
        STATE.probe.record(
            BlockingCall::SemaphorePoolExhausted,
            current_event(),
            maximum as usize,
        );
    }
    handle
}

unsafe extern "C" fn semphr_delete(handle: *mut c_void) {
    STATE.semaphores.delete(handle);
}

unsafe extern "C" fn semphr_take(handle: *mut c_void, timeout: u32) -> i32 {
    let Some(slot) = STATE.semaphores.slot(handle) else {
        return 0;
    };
    if slot.try_take() {
        return 1;
    }
    #[cfg(target_arch = "riscv32")]
    if timeout != 0 && STATE.virtual_task.is_started() && !crate::critical::strict_wifi_hart_armed()
    {
        // The pinned initialization path posts PP work and immediately waits
        // for its completion semaphore. There is no worker task in the cold
        // runtime, so consume only work that is already ready on this stack.
        // Queue exhaustion or a missing token fails immediately; this loop
        // never waits for a producer and has an explicit finite budget.
        let mut dispatcher = VendorPpDispatcher::for_initialization();
        for _ in 0..DEFAULT_EVENT_BUDGET {
            let Some(event) = STATE.queue.try_pop() else {
                break;
            };
            if !matches!(dispatcher.dispatch(event), Ok(DispatchControl::Continue)) {
                break;
            }
            if slot.try_take() {
                return 1;
            }
        }
    }
    #[cfg(all(target_arch = "riscv32", feature = "wpa-async-eap"))]
    if crate::eap::is_sync_semaphore(handle) {
        // `wpa2_post` ignores the take result and used this semaphore only to
        // wait for the worker task. Queue acceptance is the async boundary;
        // teardown still gives a real token before returning from queue_send.
        return 1;
    }
    if timeout != 0 {
        STATE.probe.record(
            BlockingCall::SemaphoreTake,
            current_event(),
            timeout as usize,
        );
    }
    0
}

unsafe extern "C" fn semphr_give(handle: *mut c_void) -> i32 {
    STATE
        .semaphores
        .slot(handle)
        .is_some_and(SemaphoreSlot::give) as i32
}

unsafe extern "C" fn wifi_thread_semphr_get() -> *mut c_void {
    STATE.semaphores.thread_semaphore()
}

unsafe extern "C" fn mutex_create() -> *mut c_void {
    create_mutex(false)
}

unsafe extern "C" fn recursive_mutex_create() -> *mut c_void {
    create_mutex(true)
}

fn create_mutex(recursive: bool) -> *mut c_void {
    let handle = STATE.mutexes.create(recursive);
    if handle.is_null() {
        STATE.probe.record(
            BlockingCall::MutexPoolExhausted,
            current_event(),
            recursive as usize,
        );
    }
    handle
}

unsafe extern "C" fn mutex_delete(handle: *mut c_void) {
    if !STATE.mutexes.delete(handle) {
        STATE
            .probe
            .record(BlockingCall::MutexLock, current_event(), handle as usize);
    }
}

unsafe extern "C" fn mutex_lock(handle: *mut c_void) -> i32 {
    let owner = current_task_handle() as usize;
    let success = STATE.mutexes.lock(handle, owner);
    if !success {
        STATE
            .probe
            .record(BlockingCall::MutexLock, current_event(), handle as usize);
    }
    success as i32
}

unsafe extern "C" fn mutex_unlock(handle: *mut c_void) -> i32 {
    let owner = current_task_handle() as usize;
    STATE.mutexes.unlock(handle, owner) as i32
}

unsafe extern "C" fn timer_setfn(timer: *mut c_void, callback: *mut c_void, argument: *mut c_void) {
    if !STATE.timers.set_callback_with_reserved_tail(
        timer,
        callback,
        argument,
        INTERNAL_TIMER_RESERVE,
    ) {
        record_timer_failure(&STATE.probe, BlockingCall::TimerSetCallbackRejected, timer);
    }
}

unsafe extern "C" fn timer_arm_ms(timer: *mut c_void, timeout_ms: u32, repeat: bool) {
    timer_arm_at(timer, timeout_ms.saturating_mul(1_000), repeat);
}

unsafe extern "C" fn timer_arm_us(timer: *mut c_void, timeout_us: u32, repeat: bool) {
    timer_arm_at(timer, timeout_us, repeat);
}

fn timer_arm_at(timer: *mut c_void, timeout_us: u32, repeat: bool) {
    let Some(now) = configured_now() else {
        STATE.probe.record(
            BlockingCall::TimerWithoutClock,
            current_event(),
            timer as usize,
        );
        return;
    };
    if !unsafe { STATE.timers.arm_at(timer, timeout_us, repeat, now() as u32) } {
        record_timer_failure(&STATE.probe, BlockingCall::TimerArmRejected, timer);
    }
}

unsafe extern "C" fn timer_disarm(timer: *mut c_void) {
    if !STATE.timers.disarm(timer) {
        record_timer_failure(&STATE.probe, BlockingCall::TimerDisarmRejected, timer);
    }
}

unsafe extern "C" fn timer_done(timer: *mut c_void) {
    if !STATE.timers.done(timer) {
        record_timer_failure(&STATE.probe, BlockingCall::TimerDoneRejected, timer);
    }
}

unsafe extern "C" fn esp_timer_get_time() -> i64 {
    configured_now().map_or(0, |now| now().min(i64::MAX as u64) as i64)
}

fn configured_now() -> Option<fn() -> u64> {
    let address = TIME_SOURCE.load(Ordering::Acquire);
    if address == 0 {
        None
    } else {
        Some(unsafe { core::mem::transmute::<usize, fn() -> u64>(address) })
    }
}

/// Read the monotonic microsecond clock owned by the Rust executor.
///
/// Strict radio leaves use this instead of the S31 ROM TSF export, which is
/// observed returning zero even while the AP MAC is active.
pub(crate) fn runtime_now_us() -> Option<u64> {
    configured_now().map(|now| now())
}

unsafe extern "C" fn wifi_create_queue(queue_len: i32, item_size: i32) -> *mut c_void {
    if queue_len <= 0
        || queue_len as usize > PP_QUEUE_CAPACITY
        || item_size as usize != core::mem::size_of::<PpEvent>()
    {
        STATE.probe.record(
            BlockingCall::UnsupportedQueue,
            current_event(),
            ((queue_len as u32 as usize) << 16) | item_size as u32 as usize,
        );
        return ptr::null_mut();
    }
    STATE.queue_descriptor.initialize(STATE.queue_handle())
}

unsafe extern "C" fn wifi_delete_queue(queue: *mut c_void) {
    if queue != ptr::addr_of!(STATE.queue_descriptor).cast_mut().cast() {
        STATE.probe.record(
            BlockingCall::UnsupportedQueue,
            current_event(),
            queue as usize,
        );
    }
}

unsafe extern "C" fn queue_create(length: u32, item_size: u32) -> *mut c_void {
    #[cfg(all(target_arch = "riscv32", feature = "wpa-async-eap"))]
    if let Some(queue) = crate::eap::try_create_queue(length, item_size) {
        return queue;
    }
    STATE.probe.record(
        BlockingCall::UnsupportedQueue,
        current_event(),
        ((length as usize) << 16) | item_size as usize,
    );
    ptr::null_mut()
}

unsafe extern "C" fn queue_delete(queue: *mut c_void) {
    #[cfg(all(target_arch = "riscv32", feature = "wpa-async-eap"))]
    if crate::eap::delete_queue(queue) {
        return;
    }
    if !STATE.is_pp_queue(queue) {
        STATE.probe.record(
            BlockingCall::UnsupportedQueue,
            current_event(),
            queue as usize,
        );
    }
}

unsafe extern "C" fn queue_send(queue: *mut c_void, item: *mut c_void, _timeout: u32) -> i32 {
    #[cfg(all(target_arch = "riscv32", feature = "wpa-async-eap"))]
    if crate::eap::is_queue(queue) {
        if item.is_null() {
            return 0;
        }
        let signal = item.cast::<PpEvent>().read().kind;
        let Some(kind) = crate::eap::encode_vendor_signal(signal) else {
            return 0;
        };
        if kind == crate::eap::EAP_STOP_EVENT {
            return matches!(
                crate::eap::dispatch(kind),
                crate::eap::DispatchResult::Complete
            ) as i32;
        }
        return enqueue_internal_event(PpEvent {
            kind,
            argument: ptr::null_mut(),
        }) as i32;
    }
    if !STATE.is_pp_queue(queue) {
        STATE.probe.record(
            BlockingCall::UnsupportedQueue,
            current_event(),
            queue as usize,
        );
        return 0;
    }
    if !item.is_null()
        && item.cast::<PpEvent>().read().kind == 15
        && STATE.shutdown_processed.load(Ordering::Acquire)
    {
        // Force a later `pp_delete_task` down its non-waiting manual cleanup
        // path after the async runtime has already drained event 15.
        return 0;
    }
    STATE.queue_bridge().send_osi(item)
}

unsafe extern "C" fn queue_send_from_isr(
    queue: *mut c_void,
    item: *mut c_void,
    higher_priority_task_woken: *mut c_void,
) -> i32 {
    if !higher_priority_task_woken.is_null() {
        higher_priority_task_woken.cast::<u32>().write(0);
    }
    queue_send(queue, item, 0)
}

unsafe extern "C" fn queue_receive(queue: *mut c_void, _item: *mut c_void, timeout: u32) -> i32 {
    if STATE.is_pp_queue(queue) {
        return STATE
            .queue_bridge()
            .reject_receive(timeout, current_event());
    }
    #[cfg(all(target_arch = "riscv32", feature = "wpa-async-eap"))]
    if crate::eap::is_queue(queue) {
        STATE.probe.record(
            BlockingCall::QueueReceive,
            current_event(),
            timeout as usize,
        );
        return 0;
    }
    STATE.probe.record(
        BlockingCall::UnsupportedQueue,
        current_event(),
        queue as usize,
    );
    0
}

unsafe extern "C" fn queue_messages_waiting(queue: *mut c_void) -> u32 {
    if STATE.is_pp_queue(queue) {
        STATE.queue_bridge().messages_waiting()
    } else {
        0
    }
}

unsafe extern "C" fn event_group_create() -> *mut c_void {
    let handle = STATE.event_groups.create();
    if handle.is_null() {
        STATE.probe.record(
            BlockingCall::EventGroupWait,
            current_event(),
            EVENT_GROUP_CAPACITY,
        );
    }
    handle
}

unsafe extern "C" fn event_group_delete(handle: *mut c_void) {
    if !STATE.event_groups.delete(handle) {
        STATE.probe.record(
            BlockingCall::EventGroupWait,
            current_event(),
            handle as usize,
        );
    }
}

unsafe extern "C" fn event_group_set_bits(handle: *mut c_void, bits: u32) -> u32 {
    STATE
        .event_groups
        .slot(handle)
        .map_or(0, |slot| slot.bits.fetch_or(bits, Ordering::AcqRel) | bits)
}

unsafe extern "C" fn event_group_clear_bits(handle: *mut c_void, bits: u32) -> u32 {
    STATE
        .event_groups
        .slot(handle)
        .map_or(0, |slot| slot.bits.fetch_and(!bits, Ordering::AcqRel))
}

unsafe extern "C" fn event_group_wait_bits(
    handle: *mut c_void,
    bits_to_wait_for: u32,
    clear_on_exit: i32,
    wait_for_all_bits: i32,
    timeout: u32,
) -> u32 {
    let Some(slot) = STATE.event_groups.slot(handle) else {
        return 0;
    };
    let observed = slot.bits.load(Ordering::Acquire);
    let selected = observed & bits_to_wait_for;
    let ready = if wait_for_all_bits != 0 {
        selected == bits_to_wait_for
    } else {
        selected != 0
    };
    if ready {
        if clear_on_exit != 0 {
            slot.bits.fetch_and(!bits_to_wait_for, Ordering::AcqRel);
        }
        observed
    } else {
        if timeout != 0 {
            STATE.probe.record(
                BlockingCall::EventGroupWait,
                current_event(),
                timeout as usize,
            );
        }
        observed
    }
}

unsafe extern "C" fn task_create_pinned(
    task_func: *mut c_void,
    _name: *const c_char,
    _stack_depth: u32,
    _parameter: *mut c_void,
    _priority: u32,
    task_handle: *mut c_void,
    _core: u32,
) -> i32 {
    task_create_common(task_func, task_handle)
}

unsafe extern "C" fn task_create(
    task_func: *mut c_void,
    _name: *const c_char,
    _stack_depth: u32,
    _parameter: *mut c_void,
    _priority: u32,
    task_handle: *mut c_void,
) -> i32 {
    task_create_common(task_func, task_handle)
}

unsafe fn task_create_common(task_func: *mut c_void, task_handle: *mut c_void) -> i32 {
    if STATE
        .virtual_task
        .try_start(task_func, task_handle.cast::<*mut c_void>())
    {
        STATE.shutdown_processed.store(false, Ordering::Release);
        // Reproduce the first operation in `ppTask`: give the startup latch.
        return if STATE.semaphores.signal_last_created() {
            1
        } else {
            0
        };
    }

    #[cfg(all(target_arch = "riscv32", feature = "wpa-async-eap"))]
    if crate::eap::try_start_task(task_func, task_handle.cast::<*mut c_void>()) {
        return 1;
    }

    STATE.probe.record(
        BlockingCall::UnknownTaskCreate,
        current_event(),
        task_func as usize,
    );
    0
}

unsafe extern "C" fn task_delete(handle: *mut c_void) {
    #[cfg(all(target_arch = "riscv32", feature = "wpa-async-eap"))]
    if crate::eap::is_task_handle(handle)
        || (handle.is_null() && crate::eap::is_task_handle(current_task_handle()))
    {
        crate::eap::stop_task();
        return;
    }
    if handle == PP_TASK_HANDLE || (handle.is_null() && current_task_handle() == PP_TASK_HANDLE) {
        STATE.virtual_task.stop();
    }
}

#[inline(never)]
unsafe extern "C" fn task_delay(ticks: u32) {
    #[cfg(target_arch = "riscv32")]
    let caller = {
        let caller: usize;
        unsafe {
            core::arch::asm!("mv {caller}, ra", caller = out(reg) caller, options(nomem, nostack))
        };
        caller
    };
    #[cfg(not(target_arch = "riscv32"))]
    let caller = 0;

    if STATE
        .virtual_task
        .take_redundant_startup_delay(caller, ticks)
    {
        return;
    }

    TASK_DELAY_CALLER.store(caller, Ordering::Relaxed);
    TASK_DELAY_TICKS.store(ticks, Ordering::Relaxed);
    TASK_DELAY_CALLS.fetch_add(1, Ordering::Release);
    STATE
        .probe
        .record(BlockingCall::TaskDelay, current_event(), ticks as usize);
    #[cfg(all(target_arch = "riscv32", feature = "strict-no-wait"))]
    if crate::critical::strict_wifi_hart_armed() {
        crate::delay::trap_blocking_delay(BlockingCall::TaskDelay, caller);
    }
}

unsafe extern "C" fn task_ms_to_tick(milliseconds: u32) -> i32 {
    milliseconds.min(i32::MAX as u32) as i32
}

unsafe extern "C" fn task_get_current() -> *mut c_void {
    #[cfg(target_arch = "riscv32")]
    if STATE.virtual_task.is_started() && !crate::critical::strict_wifi_hart_armed() {
        // There is one serialized composition-root caller between ppTask
        // virtualization and strict takeover. Give that caller the logical
        // Wi-Fi identity so `ieee80211_ioctl` executes finite control leaves
        // inline instead of posting work to the task that does not exist.
        return PP_TASK_HANDLE;
    }
    current_task_handle()
}

unsafe extern "C" fn task_max_priority() -> i32 {
    25
}

#[cfg(test)]
mod tests {
    use core::{ptr, sync::atomic::Ordering};

    use super::{queue_send, MutexPool, RadioOwnerClaim, SemaphorePool, NO_SEMAPHORE, STATE};
    use crate::event::PpEvent;

    #[test]
    fn counting_semaphore_never_blocks() {
        let pool = SemaphorePool::<2>::new();
        let handle = pool.create(1, 0);
        let slot = pool.slot(handle).unwrap();

        assert!(!slot.try_take());
        assert!(slot.give());
        assert!(slot.try_take());
        assert!(!slot.try_take());
        assert!(pool.signal_last_created());
        assert!(slot.try_take());
        pool.delete(handle);
        assert!(pool.slot(handle).is_none());
        assert_eq!(pool.last_created.load(Ordering::Relaxed), 0);
        assert_eq!(pool.thread_semaphore.load(Ordering::Relaxed), NO_SEMAPHORE);
    }

    #[test]
    fn mutex_contention_is_reported_without_waiting() {
        let pool = MutexPool::<2>::new();
        let normal = pool.create(false);
        let recursive = pool.create(true);

        assert!(pool.lock(normal, 1));
        assert!(!pool.lock(normal, 1));
        assert!(!pool.lock(normal, 2));
        assert!(pool.unlock(normal, 1));
        assert!(pool.lock(normal, 2));
        assert!(pool.unlock(normal, 2));

        assert!(pool.lock(recursive, 1));
        assert!(pool.lock(recursive, 1));
        assert!(pool.unlock(recursive, 1));
        assert!(!pool.lock(recursive, 2));
        assert!(pool.unlock(recursive, 1));
        assert!(pool.lock(recursive, 2));
    }

    #[test]
    fn radio_owner_claim_is_one_way_and_single_consumer() {
        let owner = RadioOwnerClaim::new();

        assert!(!owner.is_taken());
        let executor = owner.try_take_executor();
        assert!(executor.is_some());
        assert_eq!(core::mem::size_of_val(executor.as_ref().unwrap()), 0);
        assert!(owner.is_taken());
        assert!(owner.try_take_executor().is_none());
    }

    #[test]
    fn fixed_init_mutexes_are_directly_addressable_and_fail_fast() {
        let pool = MutexPool::<0>::new();
        let global = super::init_global_lock_handle();
        let mac_list = super::init_mac_list_lock_handle();

        assert!(pool.lock(global, 1));
        assert!(pool.lock(global, 1));
        assert!(!pool.lock(global, 2));
        assert!(pool.unlock(global, 1));
        assert!(pool.unlock(global, 1));
        assert!(pool.delete(global));

        assert!(pool.lock(mac_list, 1));
        assert!(!pool.lock(mac_list, 1));
        assert!(!pool.lock(mac_list, 2));
        assert!(pool.unlock(mac_list, 1));
        assert!(pool.delete(mac_list));
    }

    #[test]
    fn duplicate_shutdown_selects_manual_blob_cleanup() {
        let mut event = PpEvent {
            kind: 15,
            argument: ptr::null_mut(),
        };
        STATE.shutdown_processed.store(true, Ordering::Release);
        let result =
            unsafe { queue_send(STATE.queue_handle(), ptr::from_mut(&mut event).cast(), 0) };
        STATE.shutdown_processed.store(false, Ordering::Release);

        assert_eq!(result, 0);
        assert!(STATE.queue.is_empty());
    }
}
