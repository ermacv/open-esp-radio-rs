use core::{
    cell::UnsafeCell,
    ffi::c_void,
    ptr,
    sync::atomic::{AtomicU32, Ordering},
};

use crate::{
    adapter::schedule_internal_timer,
    channel_state::{ChannelState, ChannelStateAdoptionError, CHANNEL_COUNT, CHANNEL_INFO_BYTES},
    timer::RawOsiTimer,
};

const MAC_CONTROL: *mut u32 = 0x2010_4cac as *mut u32;
const MAC_STOP_MASK: u32 = 0x00ff_1000;
const MAC_ACTIVE_MASK: u32 = 0x0000_e000;
const MAC_COMMAND_SETTLE_US: u32 = 20;
const MAC_IDLE_SETTLE_US: u32 = 5;

type ChannelCallback = unsafe extern "C" fn(*mut c_void, u32);

unsafe extern "C" {
    static mut g_chm: *mut u8;
    static mut g_mac_deinit_count: u32;
    static mut g_mac_deinit_rxing: u8;
    static mut g_mac_deinit_txing: u8;

    fn chm_start_op(
        channel: *const u8,
        first_dwell_ms: u32,
        final_dwell_ms: u32,
        start: Option<ChannelCallback>,
        end: Option<ChannelCallback>,
        context: *mut c_void,
    ) -> i32;
    fn __real_chm_start_op(
        channel: *const u8,
        first_dwell_ms: u32,
        final_dwell_ms: u32,
        start: Option<ChannelCallback>,
        end: Option<ChannelCallback>,
        context: *mut c_void,
    ) -> i32;
    fn chm_return_home_channel();
    fn __real_chm_return_home_channel();
    fn hal_mac_set_csi_cbw(cbw: u32);
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum ChannelSwitchError {
    None = 0,
    WrongHart = 1,
    StateUnavailable = 2,
    Busy = 3,
    InvalidChannel = 4,
    TimerUnavailable = 5,
    MacDidNotBecomeIdle = 6,
    LegacyDwellRejected = 7,
    PhyFunctionTableChanged = 8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ChannelSwitchSnapshot {
    pub started: u32,
    pub completed: u32,
    pub failed: ChannelSwitchError,
    pub mac_status: u32,
    pub phy_function_table_expected: usize,
    pub phy_function_table_current: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ChannelStateSnapshot {
    pub adopted: bool,
    pub home: Option<[u8; 2]>,
    pub current: Option<[u8; 2]>,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum Completion {
    Operation,
    Home,
}

#[derive(Clone, Copy)]
struct State {
    active: bool,
    waiting_for_mac_edge: bool,
    operation_active: bool,
    channel: [u8; 2],
    frequency_mhz: u16,
    cbw: u8,
    completion: Completion,
    first_dwell_ms: u32,
    final_dwell_ms: u32,
    start: Option<ChannelCallback>,
    end: Option<ChannelCallback>,
    context: *mut c_void,
    started: u32,
    completed: u32,
}

impl State {
    const fn new() -> Self {
        Self {
            active: false,
            waiting_for_mac_edge: false,
            operation_active: false,
            channel: [0; 2],
            frequency_mhz: 0,
            cbw: 0,
            completion: Completion::Operation,
            first_dwell_ms: 0,
            final_dwell_ms: 0,
            start: None,
            end: None,
            context: ptr::null_mut(),
            started: 0,
            completed: 0,
        }
    }

    fn clear_operation(&mut self) {
        self.operation_active = false;
        self.first_dwell_ms = 0;
        self.final_dwell_ms = 0;
        self.start = None;
        self.end = None;
        self.context = ptr::null_mut();
    }
}

struct ChannelResources {
    machine: UnsafeCell<State>,
    channels: UnsafeCell<ChannelState>,
    first_timer: UnsafeCell<RawOsiTimer>,
    final_timer: UnsafeCell<RawOsiTimer>,
}

impl ChannelResources {
    const fn new() -> Self {
        Self {
            machine: UnsafeCell::new(State::new()),
            channels: UnsafeCell::new(ChannelState::new()),
            first_timer: UnsafeCell::new(RawOsiTimer {
                next: ptr::null_mut(),
                expire: 0,
                period: 0,
                callback: None,
                argument: ptr::null_mut(),
            }),
            final_timer: UnsafeCell::new(RawOsiTimer {
                next: ptr::null_mut(),
                expire: 0,
                period: 0,
                callback: None,
                argument: ptr::null_mut(),
            }),
        }
    }
}

// Machine access is serialized by the single strict radio owner. Home/current
// selectors use atomic publication so synchronous readers need neither an
// interrupt mask nor a critical section. The timers are stable-address
// identities registered in Rust's fixed timer pool; they are not vendor
// `gChmCxt` timer objects.
unsafe impl Sync for ChannelResources {}

#[link_section = ".critical.bss.wifi_strict.channel_resources"]
static RESOURCES: ChannelResources = ChannelResources::new();
static FAILURE: AtomicU32 = AtomicU32::new(ChannelSwitchError::None as u32);
static MAC_FAILURE_STATUS: AtomicU32 = AtomicU32::new(0);

pub(crate) fn link_wrappers_active() -> bool {
    core::ptr::eq(chm_start_op as *const (), __wrap_chm_start_op as *const ())
        && core::ptr::eq(
            chm_return_home_channel as *const (),
            __wrap_chm_return_home_channel as *const (),
        )
}

pub fn channel_switch_snapshot() -> ChannelSwitchSnapshot {
    let state = unsafe { &*RESOURCES.machine.get() };
    ChannelSwitchSnapshot {
        started: state.started,
        completed: state.completed,
        failed: decode_error(FAILURE.load(Ordering::Acquire)),
        mac_status: MAC_FAILURE_STATUS.load(Ordering::Acquire),
        phy_function_table_expected: crate::phy_param::PHY_ROM_FUNCTION_TABLE_ADDRESS as usize,
        phy_function_table_current: crate::phy_param::PHY_ROM_FUNCTION_TABLE_ADDRESS as usize,
    }
}

pub fn channel_state_snapshot() -> ChannelStateSnapshot {
    let channels = unsafe { &*RESOURCES.channels.get() };
    ChannelStateSnapshot {
        adopted: channels.adopted(),
        home: channels.home(),
        current: channels.current(),
    }
}

pub(crate) fn home_channel() -> Option<[u8; 2]> {
    unsafe { (&*RESOURCES.channels.get()).home() }
}

pub(crate) fn is_at_home_channel() -> bool {
    let channels = unsafe { &*RESOURCES.channels.get() };
    matches!(
        (channels.home(), channels.current()),
        (Some(home), Some(current)) if home == current
    )
}

/// Copy the finite channel-manager state needed after strict handoff.
///
/// This is the only permitted read of the vendor `g_chm` pointer in the
/// strict implementation. The source is cold state initialized by pinned
/// `wl_chm.o`; all later transitions use [`RESOURCES`].
///
/// # Safety
/// The vendor Wi-Fi instance must be initialized, its channel operation must
/// be idle, and no radio handler may execute concurrently.
pub(crate) unsafe fn adopt_vendor_channel_state() -> Result<(), ChannelStateAdoptionError> {
    let source = g_chm;
    if source.is_null() {
        return Err(ChannelStateAdoptionError::StateUnavailable);
    }
    if source.add(4).read() != u8::MAX {
        return Err(ChannelStateAdoptionError::OperationInProgress);
    }

    let home = [source.add(80).read(), source.add(81).read()];
    let current = [source.add(82).read(), source.add(83).read()];
    let mut records = [[0_u8; CHANNEL_INFO_BYTES]; CHANNEL_COUNT];
    for (index, destination) in records.iter_mut().enumerate() {
        let record = source.add(84 + index * CHANNEL_INFO_BYTES);
        for (offset, byte) in destination.iter_mut().enumerate() {
            *byte = record.add(offset).read();
        }
    }
    (&mut *RESOURCES.channels.get()).adopt(home, current, records)
}

pub(crate) fn failure() -> Option<ChannelSwitchError> {
    let error = decode_error(FAILURE.load(Ordering::Acquire));
    (error != ChannelSwitchError::None).then_some(error)
}

const fn decode_error(raw: u32) -> ChannelSwitchError {
    match raw {
        1 => ChannelSwitchError::WrongHart,
        2 => ChannelSwitchError::StateUnavailable,
        3 => ChannelSwitchError::Busy,
        4 => ChannelSwitchError::InvalidChannel,
        5 => ChannelSwitchError::TimerUnavailable,
        6 => ChannelSwitchError::MacDidNotBecomeIdle,
        7 => ChannelSwitchError::LegacyDwellRejected,
        8 => ChannelSwitchError::PhyFunctionTableChanged,
        _ => ChannelSwitchError::None,
    }
}

/// A vendor-owned dwell cannot cross the ownership handoff. The cold adoption
/// step rejects a busy `gChmCxt`, so this compatibility entry always fails
/// closed and never executes a vendor callback.
pub(crate) unsafe fn complete_legacy_scan_dwell(which: usize) -> Result<(), ChannelSwitchError> {
    if which > 1 || !crate::critical::on_strict_wifi_hart() {
        return Err(ChannelSwitchError::LegacyDwellRejected);
    }
    Err(ChannelSwitchError::LegacyDwellRejected)
}

unsafe fn fail(error: ChannelSwitchError, detail: u32) {
    let state = &mut *RESOURCES.machine.get();
    state.active = false;
    state.waiting_for_mac_edge = false;
    state.clear_operation();
    MAC_FAILURE_STATUS.store(detail, Ordering::Relaxed);
    FAILURE.store(error as u32, Ordering::Release);
    crate::scan::channel_switch_failed(error as u32);
}

unsafe fn first_timer() -> *mut c_void {
    RESOURCES.first_timer.get().cast()
}

unsafe fn final_timer() -> *mut c_void {
    RESOURCES.final_timer.get().cast()
}

unsafe fn prepare_channel(channel: [u8; 2]) -> Option<(u16, u8)> {
    let prepared = (&*RESOURCES.channels.get()).prepare(channel)?;
    Some((prepared.frequency_mhz, prepared.cbw))
}

unsafe fn begin(channel: [u8; 2], completion: Completion) -> Result<(), ChannelSwitchError> {
    if !crate::critical::on_strict_wifi_hart() {
        return Err(ChannelSwitchError::WrongHart);
    }
    if failure().is_some() {
        return Err(ChannelSwitchError::Busy);
    }
    let state = &mut *RESOURCES.machine.get();
    if state.active {
        return Err(ChannelSwitchError::Busy);
    }
    let Some((frequency_mhz, cbw)) = prepare_channel(channel) else {
        return Err(ChannelSwitchError::InvalidChannel);
    };

    state.active = true;
    state.waiting_for_mac_edge = false;
    state.channel = channel;
    state.frequency_mhz = frequency_mhz;
    state.cbw = cbw;
    state.completion = completion;
    state.started = state.started.wrapping_add(1);

    // The pinned `ic_set_current_channel` wrapper only forwards to the
    // 26-byte `wDev_SetCurChannel` body, which copies these two bytes to
    // `wDevCtrl[0x2c..=0x2d]`. No strict-runtime reader consumes that legacy
    // cache. The requested selector already belongs to this state machine,
    // while `ChannelState::current` is published only after PHY/MAC
    // programming completes below; keep those two meanings explicit instead
    // of maintaining a second C-owned copy.
    g_mac_deinit_count = g_mac_deinit_count.wrapping_add(1);
    let status = MAC_CONTROL.read_volatile();
    g_mac_deinit_rxing = ((status >> 14) & 1) as u8;
    g_mac_deinit_txing = ((status >> 13) & 1) as u8;
    MAC_CONTROL.write_volatile(status | MAC_STOP_MASK);

    if !schedule_internal_timer(
        first_timer(),
        mac_command_settled,
        ptr::null_mut(),
        MAC_COMMAND_SETTLE_US,
    ) {
        fail(ChannelSwitchError::TimerUnavailable, 0);
        return Err(ChannelSwitchError::TimerUnavailable);
    }
    Ok(())
}

unsafe extern "C" fn mac_command_settled(_argument: *mut c_void) {
    try_finish_mac_stop();
}

unsafe fn try_finish_mac_stop() {
    let state = &mut *RESOURCES.machine.get();
    if !state.active {
        return;
    }
    let status = MAC_CONTROL.read_volatile();
    if status & MAC_ACTIVE_MASK != 0 {
        // Do not poll the register. The in-flight TX completion enters
        // `tx_done_edge`, which retries this single check from the real
        // hardware-driven completion path.
        state.waiting_for_mac_edge = true;
        MAC_FAILURE_STATUS.store(status, Ordering::Relaxed);
        return;
    }
    state.waiting_for_mac_edge = false;
    MAC_FAILURE_STATUS.store(0, Ordering::Relaxed);
    if !schedule_internal_timer(
        first_timer(),
        mac_idle_settled,
        ptr::null_mut(),
        MAC_IDLE_SETTLE_US,
    ) {
        fail(ChannelSwitchError::TimerUnavailable, 0);
    }
}

/// Continue a deferred channel transition from the real hardware TX-done
/// path. No retry loop or periodic timer is involved.
pub(crate) unsafe fn tx_done_edge() {
    if !crate::critical::strict_wifi_hart_armed() || !crate::critical::on_strict_wifi_hart() {
        return;
    }
    if (*RESOURCES.machine.get()).waiting_for_mac_edge {
        try_finish_mac_stop();
    }
}

unsafe extern "C" fn mac_idle_settled(_argument: *mut c_void) {
    let (frequency_mhz, cbw, channel) = {
        let state = &*RESOURCES.machine.get();
        (state.frequency_mhz, state.cbw, state.channel)
    };
    if !(*RESOURCES.machine.get()).active {
        fail(ChannelSwitchError::Busy, 0);
        return;
    }

    crate::phy_channel::program_channel(frequency_mhz, cbw);
    hal_mac_set_csi_cbw(u32::from(cbw));
    crate::radio_hal::restart_mac_without_power_save();

    let set_current = (&*RESOURCES.channels.get()).set_current(channel);
    if set_current.is_err() {
        fail(ChannelSwitchError::StateUnavailable, 0);
        return;
    }

    let completion = {
        let state = &mut *RESOURCES.machine.get();
        let completion = state.completion;
        state.active = false;
        state.waiting_for_mac_edge = false;
        state.completed = state.completed.wrapping_add(1);
        completion
    };
    if completion == Completion::Operation {
        finish_operation();
    }
}

unsafe fn finish_operation() {
    let (start, context, first, final_dwell) = {
        let state = &*RESOURCES.machine.get();
        if !state.operation_active {
            fail(ChannelSwitchError::StateUnavailable, 0);
            return;
        }
        (
            state.start,
            state.context,
            state.first_dwell_ms,
            state.final_dwell_ms,
        )
    };
    if let Some(start) = start {
        start(context, 0);
    }

    if first == 0 && final_dwell == 0 {
        (&mut *RESOURCES.machine.get()).clear_operation();
        return;
    }
    if first != 0 && first < final_dwell {
        if !schedule_internal_timer(
            first_timer(),
            first_dwell_elapsed,
            ptr::null_mut(),
            first.saturating_mul(1_000),
        ) {
            fail(ChannelSwitchError::TimerUnavailable, 0);
            return;
        }
    }
    if !schedule_internal_timer(
        final_timer(),
        final_dwell_elapsed,
        ptr::null_mut(),
        final_dwell.saturating_mul(1_000),
    ) {
        fail(ChannelSwitchError::TimerUnavailable, 0);
    }
}

/// Promote the physically selected channel to the fixed STA home channel.
/// Called only by the Rust tune completion while executing on the radio owner.
pub(crate) unsafe fn make_current_channel_home() -> Result<(), ChannelSwitchError> {
    if !crate::critical::on_strict_wifi_hart() {
        fail(ChannelSwitchError::StateUnavailable, 0);
        return Err(ChannelSwitchError::StateUnavailable);
    }
    let promoted = (&*RESOURCES.channels.get()).promote_current_to_home();
    promoted.map_err(|_| {
        fail(ChannelSwitchError::StateUnavailable, 0);
        ChannelSwitchError::StateUnavailable
    })
}

unsafe extern "C" fn first_dwell_elapsed(_argument: *mut c_void) {
    let _ = crate::adapter::cancel_internal_timer(final_timer());
    finish_strict_dwell();
}

unsafe extern "C" fn final_dwell_elapsed(_argument: *mut c_void) {
    finish_strict_dwell();
}

unsafe fn finish_strict_dwell() {
    let (end, context) = {
        let state = &*RESOURCES.machine.get();
        if !state.operation_active {
            fail(ChannelSwitchError::StateUnavailable, 0);
            return;
        }
        (state.end, state.context)
    };
    if end
        .is_none_or(|callback| callback as *const () != crate::scan::channel_complete as *const ())
    {
        fail(ChannelSwitchError::LegacyDwellRejected, 0);
        return;
    }

    // Release operation ownership before invoking the sole callback accepted
    // by the strict scan API. This permits the callback to enqueue its next
    // Rust-owned operation without aliasing this state.
    (&mut *RESOURCES.machine.get()).clear_operation();
    crate::scan::channel_complete(context, 0);
}

/// Strict final-link channel-operation boundary. The callback ABI is
/// preserved, while operation state and both timers are Rust-owned.
#[no_mangle]
pub unsafe extern "C" fn __wrap_chm_start_op(
    channel: *const u8,
    first_dwell_ms: u32,
    final_dwell_ms: u32,
    start: Option<ChannelCallback>,
    end: Option<ChannelCallback>,
    context: *mut c_void,
) -> i32 {
    if !crate::critical::strict_wifi_hart_armed() {
        return __real_chm_start_op(channel, first_dwell_ms, final_dwell_ms, start, end, context);
    }
    if channel.is_null() || !crate::critical::on_strict_wifi_hart() {
        return 3;
    }

    let selected = [channel.read(), channel.add(1).read()];
    {
        let state = &mut *RESOURCES.machine.get();
        if state.operation_active || !(&*RESOURCES.channels.get()).adopted() {
            return 3;
        }
        state.operation_active = true;
        state.first_dwell_ms = first_dwell_ms;
        state.final_dwell_ms = final_dwell_ms;
        state.start = start;
        state.end = end;
        state.context = context;
    }

    if let Err(error) = begin(selected, Completion::Operation) {
        fail(error, 0);
        return 3;
    }
    0
}

/// Return-to-home is also split at the final-link boundary. `scan_done`
/// continues its finite state cleanup, while the radio owner completes the
/// physical 25-us transition before processing another queued PP event.
#[no_mangle]
pub unsafe extern "C" fn __wrap_chm_return_home_channel() {
    if !crate::critical::strict_wifi_hart_armed() {
        __real_chm_return_home_channel();
        return;
    }
    let channels = &*RESOURCES.channels.get();
    let (Some(home), Some(current)) = (channels.home(), channels.current()) else {
        fail(ChannelSwitchError::StateUnavailable, 0);
        return;
    };
    if home != current {
        if let Err(error) = begin(home, Completion::Home) {
            fail(error, 0);
        }
    }
}
