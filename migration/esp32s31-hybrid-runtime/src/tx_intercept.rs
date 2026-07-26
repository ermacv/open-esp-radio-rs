//! HIL-only bridge from the prepared vendor MPDU boundary to Rust A-MPDU.
//!
//! This module deliberately remains behind `hil-ampdu-intercept`. Post-ADDBA
//! QoS MPDUs use the recovered bounded Rust mapper preparation; large MPDUs
//! continue into Rust A-MPDU while short MPDUs retain the proven one-frame
//! submit path. The guarded pre-ADDBA management, EAPOL, Action and QoS states
//! use the recovered stateless Rust mapper; an unknown state traps instead of
//! entering the stateful vendor mapper.

use core::{
    cell::UnsafeCell,
    ffi::c_void,
    ptr,
    sync::atomic::{compiler_fence, AtomicBool, AtomicU32, Ordering},
};

use crate::tx_ampdu::TX_AMPDU_SLOT_CAPACITY;

pub(crate) const HIL_AMPDU_INTERCEPT_EVENT: u32 = u32::MAX - 5;

const HIL_HARDWARE_QUEUE: u8 = 2;
const MAX_HIL_SUBFRAMES: usize = 20;
const MAX_HIL_AGGREGATE_LENGTH: u16 = 0x7fff;
const TX_QUEUE_STATE_SIZE: usize = 0x38;
const TX_QUEUE_HARDWARE_INDEX_OFFSET: usize = 0x04;
const TX_QUEUE_STATUS_OFFSET: usize = 0x12;
#[cfg(feature = "hil-tx-deep-telemetry")]
const TX_QUEUE_KIND_OFFSET: usize = 0x1d;
const FRAME_FIRST_BUFFER_OFFSET: usize = 0x04;
const FRAME_LAYOUT_FLAGS_OFFSET: usize = 0x24;
const FRAME_DESCRIPTOR_OFFSET: usize = 0x34;
const BUFFER_DATA_OFFSET: usize = 0x04;
const DESCRIPTOR_RATE_OFFSET: usize = 0x0c;
const DESCRIPTOR_UNSUPPORTED_MASK: u32 = 0x8060_0000;
const MIN_HIL_MPDU_LENGTH: u32 = 1_200;
const HIL_COALESCE_DELAY_US: u32 = 250;
pub const HIL_PRE_ENABLE_MAPPER_RECORD_CAPACITY: usize = 16;
pub const HIL_AMPDU_SIZE_HISTOGRAM_CAPACITY: usize = MAX_HIL_SUBFRAMES + 1;

unsafe extern "C" {
    static mut our_instances_ptr: *mut u8;
}

struct InterceptState {
    event_pending: bool,
    waiting_hardware: bool,
    window: u8,
    count: u8,
    retry_prefix: u8,
    coalesce_armed: bool,
    coalesce_due: bool,
    direct_queue: u8,
    direct_frame: *mut u8,
    frames: [*mut u8; TX_AMPDU_SLOT_CAPACITY],
}

impl InterceptState {
    const fn new() -> Self {
        Self {
            event_pending: false,
            waiting_hardware: false,
            window: 0,
            count: 0,
            retry_prefix: 0,
            coalesce_armed: false,
            coalesce_due: false,
            direct_queue: u8::MAX,
            direct_frame: ptr::null_mut(),
            frames: [ptr::null_mut(); TX_AMPDU_SLOT_CAPACITY],
        }
    }
}

struct InterceptCell(UnsafeCell<InterceptState>);

unsafe impl Sync for InterceptCell {}

struct TimerCell(UnsafeCell<crate::timer::RawOsiTimer>);

unsafe impl Sync for TimerCell {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HilPreEnableMapperRecord {
    pub calls: u32,
    pub mapped: i32,
    pub rate: u8,
    pub layout: u16,
    pub frame_control: u16,
    pub pre: [u32; 5],
    pub post: [u32; 5],
}

impl HilPreEnableMapperRecord {
    const EMPTY: Self = Self {
        calls: 0,
        mapped: i32::MIN,
        rate: u8::MAX,
        layout: u16::MAX,
        frame_control: u16::MAX,
        pre: [0; 5],
        post: [0; 5],
    };
}

struct PreEnableMapperOracle {
    calls: u32,
    count: u8,
    overflow: u32,
    records: [HilPreEnableMapperRecord; HIL_PRE_ENABLE_MAPPER_RECORD_CAPACITY],
}

impl PreEnableMapperOracle {
    const fn new() -> Self {
        Self {
            calls: 0,
            count: 0,
            overflow: 0,
            records: [HilPreEnableMapperRecord::EMPTY; HIL_PRE_ENABLE_MAPPER_RECORD_CAPACITY],
        }
    }
}

struct PreEnableMapperOracleCell(UnsafeCell<PreEnableMapperOracle>);

unsafe impl Sync for PreEnableMapperOracleCell {}

#[link_section = ".critical.bss.wifi_strict.hil_ampdu_intercept"]
static STATE: InterceptCell = InterceptCell(UnsafeCell::new(InterceptState::new()));
#[link_section = ".critical.bss.wifi_strict.hil_ampdu_intercept"]
static COALESCE_TIMER: TimerCell = TimerCell(UnsafeCell::new(crate::timer::RawOsiTimer {
    next: ptr::null_mut(),
    expire: 0,
    period: 0,
    callback: None,
    argument: ptr::null_mut(),
}));
#[link_section = ".critical.bss.wifi_strict.hil_ampdu_intercept"]
static PRE_ENABLE_MAPPER_ORACLE: PreEnableMapperOracleCell =
    PreEnableMapperOracleCell(UnsafeCell::new(PreEnableMapperOracle::new()));
// Activation crosses from the Rust RX/management path into the vendor TX
// callback. Keep it atomic even on the single radio hart: interrupts are an
// independent execution context, and an ordinary private bool can otherwise
// be proven permanently false by whole-program LTO.
static ENABLED: AtomicBool = AtomicBool::new(false);
static FAILED: AtomicBool = AtomicBool::new(false);
static RETAINED: AtomicU32 = AtomicU32::new(0);
static SUBMITTED: AtomicU32 = AtomicU32::new(0);
static COMPLETED: AtomicU32 = AtomicU32::new(0);
static DIRECT_SUBMITTED: AtomicU32 = AtomicU32::new(0);
static DIRECT_COMPLETED: AtomicU32 = AtomicU32::new(0);
static LEGACY_DIRECT_SUBMITTED: AtomicU32 = AtomicU32::new(0);
static LEGACY_DIRECT_COMPLETED: AtomicU32 = AtomicU32::new(0);
static COALESCE_ARMED: AtomicU32 = AtomicU32::new(0);
static COALESCE_EXPIRED: AtomicU32 = AtomicU32::new(0);
static SUBFRAMES: AtomicU32 = AtomicU32::new(0);
static READY: AtomicU32 = AtomicU32::new(0);
static READY_HIGH_WATER: AtomicU32 = AtomicU32::new(0);
static AGGREGATE_BYTES: AtomicU32 = AtomicU32::new(0);
static AGGREGATE_SIZE_HISTOGRAM: [AtomicU32; HIL_AMPDU_SIZE_HISTOGRAM_CAPACITY] =
    [const { AtomicU32::new(0) }; HIL_AMPDU_SIZE_HISTOGRAM_CAPACITY];
static LAST_SUBMIT_CYCLE: AtomicU32 = AtomicU32::new(0);
static LAST_COMPLETION_EDGE_CYCLE: AtomicU32 = AtomicU32::new(0);
static LAST_COMPLETION_HANDOFF_CYCLE: AtomicU32 = AtomicU32::new(0);
static HARDWARE_SERVICE_SAMPLES: AtomicU32 = AtomicU32::new(0);
static HARDWARE_SERVICE_TICKS_SUM: AtomicU32 = AtomicU32::new(0);
static HARDWARE_SERVICE_CYCLES_MAX: AtomicU32 = AtomicU32::new(0);
static COMPLETION_DISPATCH_SAMPLES: AtomicU32 = AtomicU32::new(0);
static COMPLETION_DISPATCH_TICKS_SUM: AtomicU32 = AtomicU32::new(0);
static COMPLETION_DISPATCH_CYCLES_MAX: AtomicU32 = AtomicU32::new(0);
static REFILL_GAP_SAMPLES: AtomicU32 = AtomicU32::new(0);
static REFILL_GAP_TICKS_SUM: AtomicU32 = AtomicU32::new(0);
static REFILL_GAP_CYCLES_MAX: AtomicU32 = AtomicU32::new(0);
static RETRY_SUBMITS: AtomicU32 = AtomicU32::new(0);
static RETRY_SUBFRAMES: AtomicU32 = AtomicU32::new(0);
static MISSING_BLOCK_ACK_SUBFRAMES: AtomicU32 = AtomicU32::new(0);
static ENABLED_CALLS: AtomicU32 = AtomicU32::new(0);
static MAPPER_BYPASSED: AtomicU32 = AtomicU32::new(0);
static MAPPER_ALREADY_PREPARED: AtomicU32 = AtomicU32::new(0);
static MAPPER_FALLBACKS: AtomicU32 = AtomicU32::new(0);
static NULL_BUFFER_QUARANTINE_CALLS: AtomicU32 = AtomicU32::new(0);
static NULL_BUFFER_QUARANTINE_FRAME: AtomicU32 = AtomicU32::new(0);
static CLASSIFICATION_REJECT_REASON: AtomicU32 = AtomicU32::new(0);
static LAST_FALLBACK_REASON: AtomicU32 = AtomicU32::new(0);
static LAST_FALLBACK_DESCRIPTOR: AtomicU32 = AtomicU32::new(0);
static LAST_FALLBACK_RATE: AtomicU32 = AtomicU32::new(0);
static LAST_FALLBACK_LAYOUT: AtomicU32 = AtomicU32::new(0);
static LAST_FALLBACK_FRAME_CONTROL: AtomicU32 = AtomicU32::new(0);
static LAST_FALLBACK_PRE: [AtomicU32; 5] = [const { AtomicU32::new(0) }; 5];
static LAST_FALLBACK_POST: [AtomicU32; 5] = [const { AtomicU32::new(0) }; 5];
static NONZERO_FALLBACKS: AtomicU32 = AtomicU32::new(0);
static LAST_NONZERO_FALLBACK_PRE: [AtomicU32; 5] = [const { AtomicU32::new(0) }; 5];
static LAST_NONZERO_FALLBACK_POST: [AtomicU32; 5] = [const { AtomicU32::new(0) }; 5];
static MAPPED_ZERO: AtomicU32 = AtomicU32::new(0);
static MAPPED_ONE: AtomicU32 = AtomicU32::new(0);
static MAPPED_TWO: AtomicU32 = AtomicU32::new(0);
static MAPPED_OTHER: AtomicU32 = AtomicU32::new(0);
static RATE0_STATE_A: AtomicU32 = AtomicU32::new(0);
static RATE0_STATE_B: AtomicU32 = AtomicU32::new(0);
static LAST_RATE0_A: [AtomicU32; 8] = [const { AtomicU32::new(0) }; 8];
static LAST_RATE0_B: [AtomicU32; 8] = [const { AtomicU32::new(0) }; 8];
static ELIGIBLE: AtomicU32 = AtomicU32::new(0);
static BELOW_MIN_LENGTH: AtomicU32 = AtomicU32::new(0);
static LAST_MAPPED: AtomicU32 = AtomicU32::new(u32::MAX);
static LAST_DESCRIPTOR: AtomicU32 = AtomicU32::new(0);
static LAST_RATE: AtomicU32 = AtomicU32::new(0);
static LAST_LAYOUT: AtomicU32 = AtomicU32::new(0);
static LAST_FRAME_CONTROL: AtomicU32 = AtomicU32::new(0);
static LAST_MAPPER_PRE: [AtomicU32; 5] = [const { AtomicU32::new(0) }; 5];
static LAST_MAPPER_POST: [AtomicU32; 5] = [const { AtomicU32::new(0) }; 5];
static SUBMIT_QUEUE_STATE: AtomicU32 = AtomicU32::new(0);
static SUBMIT_FRAME: AtomicU32 = AtomicU32::new(0);
static SUBMIT_DESCRIPTOR: AtomicU32 = AtomicU32::new(0);
static SUBMIT_REGISTERS: [AtomicU32; 11] = [const { AtomicU32::new(0) }; 11];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HilAmpduInterceptSnapshot {
    pub enabled: bool,
    pub enabled_calls: u32,
    pub mapper_bypassed: u32,
    pub mapper_already_prepared: u32,
    pub mapper_fallbacks: u32,
    pub null_buffer_quarantine_calls: u32,
    pub null_buffer_quarantine_frame: u32,
    pub last_fallback_reason: u8,
    pub last_fallback_descriptor: u32,
    pub last_fallback_rate: u8,
    pub last_fallback_layout: u16,
    pub last_fallback_frame_control: u16,
    pub last_fallback_pre: [u32; 5],
    pub last_fallback_post: [u32; 5],
    pub nonzero_fallbacks: u32,
    pub last_nonzero_fallback_pre: [u32; 5],
    pub last_nonzero_fallback_post: [u32; 5],
    pub mapped_zero: u32,
    pub mapped_one: u32,
    pub mapped_two: u32,
    pub mapped_other: u32,
    pub rate0_state_a: u32,
    pub rate0_state_b: u32,
    /// Layout/frame words, buffer words and the first three payload words for
    /// the two exact rate-zero identity states.
    pub last_rate0_a: [u32; 8],
    pub last_rate0_b: [u32; 8],
    pub eligible: u32,
    pub below_min_length: u32,
    pub last_mapped: u32,
    pub last_descriptor: u32,
    pub last_rate: u8,
    pub last_layout: u16,
    pub last_frame_control: u16,
    /// Descriptor flags/word1/queue word followed by peer flags/queue selector
    /// immediately before and after the recovered mapper transformation.
    pub last_mapper_pre: [u32; 5],
    pub last_mapper_post: [u32; 5],
    pub retained: u32,
    pub submitted: u32,
    pub completed: u32,
    pub direct_submitted: u32,
    pub direct_completed: u32,
    pub legacy_direct_submitted: u32,
    pub legacy_direct_completed: u32,
    pub coalesce_armed: u32,
    pub coalesce_expired: u32,
    pub subframes: u32,
    pub ready: u32,
    pub ready_high_water: u32,
    pub aggregate_bytes: u32,
    /// Index is the number of MPDUs in one submitted A-MPDU. Slots 0 and 1
    /// remain zero because direct submissions have dedicated counters.
    pub aggregate_size_histogram: [u32; HIL_AMPDU_SIZE_HISTOGRAM_CAPACITY],
    /// One tick is 256 CPU cycles. S31 runs at 320 MHz in this profile.
    pub hardware_service_samples: u32,
    pub hardware_service_ticks_sum: u32,
    pub hardware_service_cycles_max: u32,
    pub completion_dispatch_samples: u32,
    pub completion_dispatch_ticks_sum: u32,
    pub completion_dispatch_cycles_max: u32,
    pub refill_gap_samples: u32,
    pub refill_gap_ticks_sum: u32,
    pub refill_gap_cycles_max: u32,
    pub retry_submits: u32,
    pub retry_subframes: u32,
    pub missing_block_ack_subframes: u32,
    pub failed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HilPreEnableMapperSnapshot {
    pub calls: u32,
    pub count: u8,
    pub overflow: u32,
    pub records: [HilPreEnableMapperRecord; HIL_PRE_ENABLE_MAPPER_RECORD_CAPACITY],
}

/// Snapshot the bounded mapper oracle with local interrupt exclusion.
///
/// Before A-MPDU activation the TX callback can still append records, so the
/// old "read only after enabled" rule hid precisely the evidence needed when
/// a pre-ADDBA frame failed. Strict Wi-Fi callbacks and this diagnostic run on
/// the configured radio hart; masking local MIE makes the finite copy
/// race-free without a lock, spin, wait, or other-core stall. After activation
/// the table is immutable and the same operation remains harmless.
pub fn hil_pre_enable_mapper_snapshot() -> HilPreEnableMapperSnapshot {
    #[cfg(target_arch = "riscv32")]
    let interrupt_state = unsafe { crate::critical::strict_wifi_int_disable() };
    compiler_fence(Ordering::Acquire);
    let oracle = unsafe { &*PRE_ENABLE_MAPPER_ORACLE.0.get() };
    let snapshot = HilPreEnableMapperSnapshot {
        calls: oracle.calls,
        count: oracle.count,
        overflow: oracle.overflow,
        records: oracle.records,
    };
    compiler_fence(Ordering::Release);
    #[cfg(target_arch = "riscv32")]
    unsafe {
        crate::critical::strict_wifi_int_restore(interrupt_state);
    }
    snapshot
}

pub fn hil_ampdu_intercept_snapshot() -> HilAmpduInterceptSnapshot {
    let mut last_mapper_pre = [0_u32; 5];
    let mut last_mapper_post = [0_u32; 5];
    let mut last_fallback_pre = [0_u32; 5];
    let mut last_fallback_post = [0_u32; 5];
    let mut last_nonzero_fallback_pre = [0_u32; 5];
    let mut last_nonzero_fallback_post = [0_u32; 5];
    let mut last_rate0_a = [0_u32; 8];
    let mut last_rate0_b = [0_u32; 8];
    let mut aggregate_size_histogram = [0_u32; HIL_AMPDU_SIZE_HISTOGRAM_CAPACITY];
    let mut index = 0_usize;
    while index < last_mapper_pre.len() {
        last_mapper_pre[index] = LAST_MAPPER_PRE[index].load(Ordering::Acquire);
        last_mapper_post[index] = LAST_MAPPER_POST[index].load(Ordering::Acquire);
        last_fallback_pre[index] = LAST_FALLBACK_PRE[index].load(Ordering::Acquire);
        last_fallback_post[index] = LAST_FALLBACK_POST[index].load(Ordering::Acquire);
        last_nonzero_fallback_pre[index] = LAST_NONZERO_FALLBACK_PRE[index].load(Ordering::Acquire);
        last_nonzero_fallback_post[index] =
            LAST_NONZERO_FALLBACK_POST[index].load(Ordering::Acquire);
        index += 1;
    }
    index = 0;
    while index < last_rate0_a.len() {
        last_rate0_a[index] = LAST_RATE0_A[index].load(Ordering::Acquire);
        last_rate0_b[index] = LAST_RATE0_B[index].load(Ordering::Acquire);
        index += 1;
    }
    index = 0;
    while index < aggregate_size_histogram.len() {
        aggregate_size_histogram[index] = AGGREGATE_SIZE_HISTOGRAM[index].load(Ordering::Acquire);
        index += 1;
    }
    HilAmpduInterceptSnapshot {
        enabled: unsafe { load_enabled_from_callback_context() },
        enabled_calls: ENABLED_CALLS.load(Ordering::Acquire),
        mapper_bypassed: MAPPER_BYPASSED.load(Ordering::Acquire),
        mapper_already_prepared: MAPPER_ALREADY_PREPARED.load(Ordering::Acquire),
        mapper_fallbacks: MAPPER_FALLBACKS.load(Ordering::Acquire),
        null_buffer_quarantine_calls: NULL_BUFFER_QUARANTINE_CALLS.load(Ordering::Acquire),
        null_buffer_quarantine_frame: NULL_BUFFER_QUARANTINE_FRAME.load(Ordering::Acquire),
        last_fallback_reason: LAST_FALLBACK_REASON.load(Ordering::Acquire) as u8,
        last_fallback_descriptor: LAST_FALLBACK_DESCRIPTOR.load(Ordering::Acquire),
        last_fallback_rate: LAST_FALLBACK_RATE.load(Ordering::Acquire) as u8,
        last_fallback_layout: LAST_FALLBACK_LAYOUT.load(Ordering::Acquire) as u16,
        last_fallback_frame_control: LAST_FALLBACK_FRAME_CONTROL.load(Ordering::Acquire) as u16,
        last_fallback_pre,
        last_fallback_post,
        nonzero_fallbacks: NONZERO_FALLBACKS.load(Ordering::Acquire),
        last_nonzero_fallback_pre,
        last_nonzero_fallback_post,
        mapped_zero: MAPPED_ZERO.load(Ordering::Acquire),
        mapped_one: MAPPED_ONE.load(Ordering::Acquire),
        mapped_two: MAPPED_TWO.load(Ordering::Acquire),
        mapped_other: MAPPED_OTHER.load(Ordering::Acquire),
        rate0_state_a: RATE0_STATE_A.load(Ordering::Acquire),
        rate0_state_b: RATE0_STATE_B.load(Ordering::Acquire),
        last_rate0_a,
        last_rate0_b,
        eligible: ELIGIBLE.load(Ordering::Acquire),
        below_min_length: BELOW_MIN_LENGTH.load(Ordering::Acquire),
        last_mapped: LAST_MAPPED.load(Ordering::Acquire),
        last_descriptor: LAST_DESCRIPTOR.load(Ordering::Acquire),
        last_rate: LAST_RATE.load(Ordering::Acquire) as u8,
        last_layout: LAST_LAYOUT.load(Ordering::Acquire) as u16,
        last_frame_control: LAST_FRAME_CONTROL.load(Ordering::Acquire) as u16,
        last_mapper_pre,
        last_mapper_post,
        retained: RETAINED.load(Ordering::Acquire),
        submitted: SUBMITTED.load(Ordering::Acquire),
        completed: COMPLETED.load(Ordering::Acquire),
        direct_submitted: DIRECT_SUBMITTED.load(Ordering::Acquire),
        direct_completed: DIRECT_COMPLETED.load(Ordering::Acquire),
        legacy_direct_submitted: LEGACY_DIRECT_SUBMITTED.load(Ordering::Acquire),
        legacy_direct_completed: LEGACY_DIRECT_COMPLETED.load(Ordering::Acquire),
        coalesce_armed: COALESCE_ARMED.load(Ordering::Acquire),
        coalesce_expired: COALESCE_EXPIRED.load(Ordering::Acquire),
        subframes: SUBFRAMES.load(Ordering::Acquire),
        ready: READY.load(Ordering::Acquire),
        ready_high_water: READY_HIGH_WATER.load(Ordering::Acquire),
        aggregate_bytes: AGGREGATE_BYTES.load(Ordering::Acquire),
        aggregate_size_histogram,
        hardware_service_samples: HARDWARE_SERVICE_SAMPLES.load(Ordering::Acquire),
        hardware_service_ticks_sum: HARDWARE_SERVICE_TICKS_SUM.load(Ordering::Acquire),
        hardware_service_cycles_max: HARDWARE_SERVICE_CYCLES_MAX.load(Ordering::Acquire),
        completion_dispatch_samples: COMPLETION_DISPATCH_SAMPLES.load(Ordering::Acquire),
        completion_dispatch_ticks_sum: COMPLETION_DISPATCH_TICKS_SUM.load(Ordering::Acquire),
        completion_dispatch_cycles_max: COMPLETION_DISPATCH_CYCLES_MAX.load(Ordering::Acquire),
        refill_gap_samples: REFILL_GAP_SAMPLES.load(Ordering::Acquire),
        refill_gap_ticks_sum: REFILL_GAP_TICKS_SUM.load(Ordering::Acquire),
        refill_gap_cycles_max: REFILL_GAP_CYCLES_MAX.load(Ordering::Acquire),
        retry_submits: RETRY_SUBMITS.load(Ordering::Acquire),
        retry_subframes: RETRY_SUBFRAMES.load(Ordering::Acquire),
        missing_block_ack_subframes: MISSING_BLOCK_ACK_SUBFRAMES.load(Ordering::Acquire),
        failed: FAILED.load(Ordering::Acquire),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HilAmpduHardwareSnapshot {
    /// Packed bytes: hardware queue, software status, queue kind, byte 0x28.
    pub submit_queue_state: u32,
    pub submit_frame: u32,
    pub submit_descriptor: u32,
    /// protection, PPDU control, config, PLCP0, PLCP1, PTI, HTSIG, power,
    /// HT control, data length, and length control captured after enable.
    pub submit_registers: [u32; 11],
    pub live_interrupt_state: u32,
    pub live_complete_state: u32,
    pub live_registers: [u32; 11],
    /// Primary/secondary completion words followed by the three BlockAck
    /// words and the two adjacent completion auxiliaries for TXQ2.
    pub live_completion_registers: [u32; 7],
}

/// Read-only HIL evidence. Mutable queue SRAM is copied to atomics by the
/// radio owner at submission; this accessor reads only those atomics and MMIO,
/// so diagnostics on the network hart cannot race Rust-owned queue state.
pub fn hil_ampdu_hardware_snapshot() -> HilAmpduHardwareSnapshot {
    let mut submit_registers = [0_u32; 11];
    let mut index = 0_usize;
    while index < submit_registers.len() {
        submit_registers[index] = SUBMIT_REGISTERS[index].load(Ordering::Acquire);
        index += 1;
    }
    let live_registers = unsafe { read_hardware_registers() };
    const QUEUE_OFFSET: usize = HIL_HARDWARE_QUEUE as usize * 0x7c;
    HilAmpduHardwareSnapshot {
        submit_queue_state: SUBMIT_QUEUE_STATE.load(Ordering::Acquire),
        submit_frame: SUBMIT_FRAME.load(Ordering::Acquire),
        submit_descriptor: SUBMIT_DESCRIPTOR.load(Ordering::Acquire),
        submit_registers,
        live_interrupt_state: unsafe { (0x2010_4cb4_usize as *const u32).read_volatile() },
        live_complete_state: unsafe { (0x2010_4cbc_usize as *const u32).read_volatile() },
        live_registers,
        live_completion_registers: unsafe {
            [
                ((0x2010_553c_usize - QUEUE_OFFSET) as *const u32).read_volatile(),
                ((0x2010_5540_usize - QUEUE_OFFSET) as *const u32).read_volatile(),
                ((0x2010_5530_usize - QUEUE_OFFSET) as *const u32).read_volatile(),
                ((0x2010_552c_usize - QUEUE_OFFSET) as *const u32).read_volatile(),
                ((0x2010_5528_usize - QUEUE_OFFSET) as *const u32).read_volatile(),
                ((0x2010_5534_usize - QUEUE_OFFSET) as *const u32).read_volatile(),
                ((0x2010_5524_usize - QUEUE_OFFSET) as *const u32).read_volatile(),
            ]
        },
    }
}

unsafe fn read_hardware_registers() -> [u32; 11] {
    const QUEUE_16: usize = HIL_HARDWARE_QUEUE as usize * 0x10;
    const QUEUE_124: usize = HIL_HARDWARE_QUEUE as usize * 0x7c;
    [
        ((0x2010_4d64_usize - QUEUE_16) as *const u32).read_volatile(),
        ((0x2010_4d68_usize - QUEUE_16) as *const u32).read_volatile(),
        ((0x2010_4d6c_usize - QUEUE_16) as *const u32).read_volatile(),
        ((0x2010_4d70_usize - QUEUE_16) as *const u32).read_volatile(),
        ((0x2010_54d8_usize - QUEUE_124) as *const u32).read_volatile(),
        ((0x2010_54e0_usize - QUEUE_124) as *const u32).read_volatile(),
        ((0x2010_54e8_usize - QUEUE_124) as *const u32).read_volatile(),
        ((0x2010_5500_usize - QUEUE_124) as *const u32).read_volatile(),
        ((0x2010_5504_usize - QUEUE_124) as *const u32).read_volatile(),
        ((0x2010_550c_usize - QUEUE_124) as *const u32).read_volatile(),
        ((0x2010_5510_usize - QUEUE_124) as *const u32).read_volatile(),
    ]
}

#[cfg(feature = "hil-tx-deep-telemetry")]
unsafe fn record_hardware_submit(queue_state: *mut u8) {
    let frame = queue_state.cast::<*mut u8>().read();
    let descriptor = if frame.is_null() {
        ptr::null_mut()
    } else {
        frame.add(FRAME_DESCRIPTOR_OFFSET).cast::<*mut u8>().read()
    };
    let packed = u32::from(queue_state.add(TX_QUEUE_HARDWARE_INDEX_OFFSET).read())
        | (u32::from(queue_state.add(TX_QUEUE_STATUS_OFFSET).read()) << 8)
        | (u32::from(queue_state.add(TX_QUEUE_KIND_OFFSET).read()) << 16)
        | (u32::from(queue_state.add(0x28).read()) << 24);
    SUBMIT_QUEUE_STATE.store(packed, Ordering::Release);
    SUBMIT_FRAME.store(frame as usize as u32, Ordering::Release);
    SUBMIT_DESCRIPTOR.store(descriptor as usize as u32, Ordering::Release);
    let registers = read_hardware_registers();
    let mut index = 0_usize;
    while index < registers.len() {
        SUBMIT_REGISTERS[index].store(registers[index], Ordering::Release);
        index += 1;
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TxInterceptError {
    PreviousFailure,
    InternalQueueFull,
    ReadyQueueFull,
    InstancesUnavailable,
    InvalidHardwareQueue(u8),
    MissingDirectOwner,
    CoalesceTimerSchedule,
    CoalesceTimerCancel,
    Aggregate(crate::tx_ampdu::BasicHtAmpduChainError),
    Submit(crate::lmac::LmacAsyncError),
}

/// Enable the laboratory bridge only after Rust has accepted the peer's
/// ADDBA response. No vendor aggregation operational bit is changed.
pub(crate) unsafe fn enable(window: u16) {
    let state = &mut *STATE.0.get();
    state.window = window.clamp(2, TX_AMPDU_SLOT_CAPACITY as u16) as u8;
    ENABLED.store(true, Ordering::Release);
}

pub(crate) unsafe fn can_reset_sta_link() -> bool {
    let state = &*STATE.0.get();
    state.count == 0
        && state.retry_prefix == 0
        && !state.event_pending
        && !state.waiting_hardware
        && !state.coalesce_armed
        && !state.coalesce_due
        && state.direct_frame.is_null()
        && state.frames.iter().all(|frame| frame.is_null())
}

/// Disable the HIL aggregation bridge after every owned TX frame has returned.
///
/// This is part of the strict STA teardown transaction. Diagnostic counters
/// intentionally remain cumulative across associations.
pub(crate) unsafe fn reset_sta_link() {
    debug_assert!(can_reset_sta_link());
    let _ = crate::adapter::cancel_internal_timer(COALESCE_TIMER.0.get().cast());
    *STATE.0.get() = InterceptState::new();
    ENABLED.store(false, Ordering::Release);
}

/// GNU-ld wrapper around the last vendor preparation leaf used by `ppTxPkt`.
/// Returning a value other than 0/1/2 makes `ppTxPkt` return without inserting
/// the frame into any vendor PP list or recycling it.
#[link_section = ".rwtext.wifi_strict.hil_ampdu_intercept"]
pub unsafe extern "C" fn hil_ampdu_intercept_pp_map_tx_queue(frame: *mut u8) -> i32 {
    // The activation edge is delivered by a management RX callback that is
    // outside LLVM's ordinary call graph. An explicit RISC-V atomic byte load
    // keeps that external edge visible under fat whole-program LTO.
    let enabled = load_enabled_from_callback_context();
    if !enabled {
        return bypass_pre_enable_mapper(frame);
    }
    let state = &mut *STATE.0.get();
    ENABLED_CALLS.fetch_add(1, Ordering::Relaxed);
    if let Some(aggregate_eligible) = strict_qos_data(frame) {
        let mapper_pre = read_mapper_state(frame);
        // For the guarded strict STA QoS state, the recovered mapper oracle
        // leaves the already selected logical queue and every frame/peer word
        // unchanged. A fresh frame needs the descriptor treatment byte
        // 0x20 -> 0x07; a frame that crossed the preparation boundary before
        // ADDBA activation can already contain 0x07. Omit ppProcessWaitingQueue,
        // power-management and dynamic queue-search calls entirely.
        if mapper_pre[0] != 0x0000_2009
            || (mapper_pre[1] != 0x0000_0020 && mapper_pre[1] != 0x0000_0007)
            || mapper_pre[2] != 0x0000_0304
            || mapper_pre[3] & 0x80 == 0
            || mapper_pre[4] != 0
        {
            fail_and_trap();
        }
        if mapper_pre[1] == 0x0000_0007 {
            MAPPER_ALREADY_PREPARED.fetch_add(1, Ordering::Relaxed);
        }
        let descriptor = frame.add(FRAME_DESCRIPTOR_OFFSET).cast::<*mut u8>().read();
        descriptor.add(4).write(7);
        record_mapper_state(&LAST_MAPPER_PRE, &mapper_pre);
        let mapper_post = read_mapper_state(frame);
        record_mapper_state(&LAST_MAPPER_POST, &mapper_post);
        MAPPER_BYPASSED.fetch_add(1, Ordering::Relaxed);
        LAST_MAPPED.store(0, Ordering::Release);
        MAPPED_ZERO.fetch_add(1, Ordering::Relaxed);
        if !aggregate_eligible {
            if push_ready(state, frame).is_err()
                || reconcile_coalesce_deadline(state).is_err()
                || schedule(state).is_err()
            {
                fail_and_trap();
            }
            RETAINED.fetch_add(1, Ordering::Relaxed);
            // Ownership is now in the Rust queue. The executor submits this
            // frame through the bounded one-frame LMAC leaf.
            return 3;
        }
        ELIGIBLE.fetch_add(1, Ordering::Relaxed);
        if push_ready(state, frame).is_err() {
            fail_and_trap();
        }
        RETAINED.fetch_add(1, Ordering::Relaxed);
        if reconcile_coalesce_deadline(state).is_err() {
            fail_and_trap();
        }
        if state.count >= 2 && schedule(state).is_err() {
            fail_and_trap();
        }
        // `ppTxPkt` treats all values except 0, 1 and 2 as already consumed.
        return 3;
    }

    let reject_reason = CLASSIFICATION_REJECT_REASON.load(Ordering::Relaxed);
    if reject_reason == 5 && !frame.is_null() {
        // A completion/recycle edge can leave one late ppTxPkt invocation with
        // its frame object still addressable but its first buffer already
        // detached. It cannot be inspected, queued or safely recycled here.
        // Quarantine exactly one pointer and report it as consumed; a second
        // distinct pointer still traps so pool corruption cannot be hidden.
        let frame_address = frame as usize as u32;
        let quarantined = NULL_BUFFER_QUARANTINE_FRAME.compare_exchange(
            0,
            frame_address,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
        if quarantined.is_ok() || quarantined == Err(frame_address) {
            NULL_BUFFER_QUARANTINE_CALLS.fetch_add(1, Ordering::Relaxed);
            return 3;
        }
    }

    let fallback_pre = read_mapper_state(frame);
    if reject_reason == 4
        && (fallback_pre == [0x0200_2009, 0x0000_0007, 0x0000_0304, 0x0000_0081, 0]
            || fallback_pre == [0, 0x0000_0007, 0, 0x0000_0081, 0])
    {
        // The recovered rate-zero oracle is an identity operation for these
        // two exact post-ADDBA states and returns logical queue zero. Preserve
        // that result without entering ppProcessWaitingQueue or PM branches.
        MAPPER_BYPASSED.fetch_add(1, Ordering::Relaxed);
        LAST_MAPPED.store(0, Ordering::Release);
        MAPPED_ZERO.fetch_add(1, Ordering::Relaxed);
        let diagnostic = read_rate0_diagnostic(frame);
        if fallback_pre[0] == 0x0200_2009 {
            RATE0_STATE_A.fetch_add(1, Ordering::Relaxed);
            record_rate0_diagnostic(&LAST_RATE0_A, &diagnostic);
        } else {
            RATE0_STATE_B.fetch_add(1, Ordering::Relaxed);
            record_rate0_diagnostic(&LAST_RATE0_B, &diagnostic);
        }
        let layout = diagnostic[0] as u16;
        let length = diagnostic[5] & 0x3fff;
        let frame_control = diagnostic[7] as u16;
        let qos_data = fallback_pre[0] == 0x0200_2009
            && layout & 0x2000 != 0
            && length < MIN_HIL_MPDU_LENGTH
            && frame_control & 0x008c == 0x0088;
        let action = fallback_pre[0] == 0
            && layout & 0x2000 != 0
            && length < MIN_HIL_MPDU_LENGTH
            && frame_control & 0x00fc == 0x00d0;
        if qos_data || action {
            if push_ready(state, frame).is_err()
                || reconcile_coalesce_deadline(state).is_err()
                || schedule(state).is_err()
            {
                fail_and_trap();
            }
            RETAINED.fetch_add(1, Ordering::Relaxed);
            return 3;
        }
        return 0;
    }
    MAPPER_FALLBACKS.fetch_add(1, Ordering::Relaxed);
    LAST_FALLBACK_REASON.store(
        CLASSIFICATION_REJECT_REASON.load(Ordering::Relaxed),
        Ordering::Release,
    );
    LAST_FALLBACK_DESCRIPTOR.store(LAST_DESCRIPTOR.load(Ordering::Relaxed), Ordering::Release);
    LAST_FALLBACK_RATE.store(LAST_RATE.load(Ordering::Relaxed), Ordering::Release);
    LAST_FALLBACK_LAYOUT.store(LAST_LAYOUT.load(Ordering::Relaxed), Ordering::Release);
    LAST_FALLBACK_FRAME_CONTROL.store(
        LAST_FRAME_CONTROL.load(Ordering::Relaxed),
        Ordering::Release,
    );
    record_mapper_state(&LAST_FALLBACK_PRE, &fallback_pre);
    record_mapper_state(&LAST_FALLBACK_POST, &fallback_pre);
    if fallback_pre[0] != 0 {
        NONZERO_FALLBACKS.fetch_add(1, Ordering::Relaxed);
        record_mapper_state(&LAST_NONZERO_FALLBACK_PRE, &fallback_pre);
        record_mapper_state(&LAST_NONZERO_FALLBACK_POST, &fallback_pre);
    }
    fail_and_trap()
}

#[inline(always)]
unsafe fn bypass_pre_enable_mapper(frame: *mut u8) -> i32 {
    let pre = read_mapper_state(frame);
    let (rate, layout, frame_control) = read_mapper_identity(frame);
    let Some(treatment) =
        crate::tx_mapper::strict_sta_ap_treatment(rate, layout, frame_control, pre)
    else {
        fail_and_trap();
    };
    let descriptor = frame.add(FRAME_DESCRIPTOR_OFFSET).cast::<*mut u8>().read();
    descriptor.add(4).write(treatment);
    let mapped = 0;
    let post = read_mapper_state(frame);
    let oracle = &mut *PRE_ENABLE_MAPPER_ORACLE.0.get();
    oracle.calls = oracle.calls.wrapping_add(1);

    let mut index = 0_usize;
    while index < usize::from(oracle.count) {
        let record = &mut oracle.records[index];
        if record.mapped == mapped
            && record.rate == rate
            && record.layout == layout
            && record.frame_control == frame_control
            && record.pre == pre
            && record.post == post
        {
            record.calls = record.calls.wrapping_add(1);
            return mapped;
        }
        index += 1;
    }
    if index == HIL_PRE_ENABLE_MAPPER_RECORD_CAPACITY {
        oracle.overflow = oracle.overflow.wrapping_add(1);
        return mapped;
    }
    oracle.records[index] = HilPreEnableMapperRecord {
        calls: 1,
        mapped,
        rate,
        layout,
        frame_control,
        pre,
        post,
    };
    oracle.count = oracle.count.wrapping_add(1);
    mapped
}

#[inline(always)]
unsafe fn read_mapper_identity(frame: *mut u8) -> (u8, u16, u16) {
    if frame.is_null() {
        return (u8::MAX, u16::MAX, u16::MAX);
    }
    let descriptor = frame.add(FRAME_DESCRIPTOR_OFFSET).cast::<*mut u8>().read();
    let rate = if descriptor.is_null() {
        u8::MAX
    } else {
        descriptor.add(DESCRIPTOR_RATE_OFFSET).read()
    };
    let layout = frame.add(FRAME_LAYOUT_FLAGS_OFFSET).cast::<u16>().read();
    let first_buffer = frame
        .add(FRAME_FIRST_BUFFER_OFFSET)
        .cast::<*mut u8>()
        .read();
    if first_buffer.is_null() {
        return (rate, layout, u16::MAX);
    }
    let mut header = first_buffer
        .add(BUFFER_DATA_OFFSET)
        .cast::<*mut u8>()
        .read();
    if header.is_null() {
        return (rate, layout, u16::MAX);
    }
    if layout & 0x2000 != 0 {
        header = header.add(8);
    }
    (rate, layout, header.cast::<u16>().read_unaligned())
}

#[inline(always)]
unsafe fn read_mapper_state(frame: *mut u8) -> [u32; 5] {
    let mut words = [0_u32; 5];
    if !frame.is_null() {
        let descriptor = frame.add(FRAME_DESCRIPTOR_OFFSET).cast::<*mut u8>().read();
        if !descriptor.is_null() {
            words[0] = descriptor.cast::<u32>().read();
            words[1] = descriptor.add(4).cast::<u32>().read();
            words[2] = descriptor.add(0x10).cast::<u32>().read();
        }
        let peer = frame.add(0x2c).cast::<*mut u8>().read();
        if !peer.is_null() {
            words[3] = peer.add(0x0c).cast::<u32>().read();
            words[4] = u32::from(peer.add(0x84).read());
        }
    }
    words
}

#[inline(always)]
fn record_mapper_state(destination: &[AtomicU32; 5], words: &[u32; 5]) {
    let mut index = 0_usize;
    while index < words.len() {
        destination[index].store(words[index], Ordering::Release);
        index += 1;
    }
}

#[inline(always)]
unsafe fn read_rate0_diagnostic(frame: *mut u8) -> [u32; 8] {
    let first_buffer = frame
        .add(FRAME_FIRST_BUFFER_OFFSET)
        .cast::<*mut u8>()
        .read();
    let header = if first_buffer.is_null() {
        ptr::null_mut()
    } else {
        first_buffer
            .add(BUFFER_DATA_OFFSET)
            .cast::<*mut u8>()
            .read()
    };
    [
        u32::from(frame.add(FRAME_LAYOUT_FLAGS_OFFSET).cast::<u16>().read()),
        frame.add(0x20).cast::<u32>().read_unaligned(),
        frame.add(0x28).cast::<u32>().read_unaligned(),
        if first_buffer.is_null() {
            0
        } else {
            first_buffer.cast::<u32>().read_unaligned()
        },
        if first_buffer.is_null() {
            0
        } else {
            first_buffer.add(8).cast::<u32>().read_unaligned()
        },
        if header.is_null() {
            0
        } else {
            header.cast::<u32>().read_unaligned()
        },
        if header.is_null() {
            0
        } else {
            header.add(4).cast::<u32>().read_unaligned()
        },
        if header.is_null() {
            0
        } else {
            header.add(8).cast::<u32>().read_unaligned()
        },
    ]
}

#[inline(always)]
fn record_rate0_diagnostic(destination: &[AtomicU32; 8], words: &[u32; 8]) {
    let mut index = 0_usize;
    while index < words.len() {
        destination[index].store(words[index], Ordering::Release);
        index += 1;
    }
}

#[inline(always)]
unsafe fn load_enabled_from_callback_context() -> bool {
    let value: usize;
    core::arch::asm!(
        "lbu {value}, 0({address})",
        value = out(reg) value,
        address = in(reg) ENABLED.as_ptr(),
        options(nostack, readonly),
    );
    compiler_fence(Ordering::Acquire);
    value != 0
}

/// Return whether a strict QoS data frame is large enough for the qualified
/// A-MPDU path. `Some(false)` remains a valid mapper-bypass candidate.
unsafe fn strict_qos_data(frame: *mut u8) -> Option<bool> {
    CLASSIFICATION_REJECT_REASON.store(0, Ordering::Relaxed);
    LAST_DESCRIPTOR.store(u32::MAX, Ordering::Relaxed);
    LAST_RATE.store(u32::MAX, Ordering::Relaxed);
    LAST_LAYOUT.store(u32::MAX, Ordering::Relaxed);
    LAST_FRAME_CONTROL.store(u32::MAX, Ordering::Relaxed);
    if frame.is_null() {
        return reject_qos(1);
    }
    let descriptor = frame.add(FRAME_DESCRIPTOR_OFFSET).cast::<*mut u8>().read();
    if descriptor.is_null() {
        return reject_qos(2);
    }
    let descriptor_word = descriptor.cast::<u32>().read();
    LAST_DESCRIPTOR.store(descriptor_word, Ordering::Release);
    if descriptor_word & DESCRIPTOR_UNSUPPORTED_MASK != 0 {
        return reject_qos(3);
    }
    let rate = descriptor.add(DESCRIPTOR_RATE_OFFSET).read();
    LAST_RATE.store(u32::from(rate), Ordering::Release);
    if !(16..=35).contains(&rate) {
        return reject_qos(4);
    }
    let first_buffer = frame
        .add(FRAME_FIRST_BUFFER_OFFSET)
        .cast::<*mut u8>()
        .read();
    if first_buffer.is_null() {
        return reject_qos(5);
    }
    let mut header = first_buffer
        .add(BUFFER_DATA_OFFSET)
        .cast::<*mut u8>()
        .read();
    if header.is_null() {
        return reject_qos(6);
    }
    let layout = frame.add(FRAME_LAYOUT_FLAGS_OFFSET).cast::<u16>().read();
    LAST_LAYOUT.store(u32::from(layout), Ordering::Release);
    // The qualified strict CCMP layout has an eight-byte PP prefix. Both the
    // large throughput MPDUs and short post-link QoS MPDUs expose the same
    // guarded mapper state; retain the size counter to keep the two classes
    // visible in HIL diagnostics.
    if layout & 0x2000 == 0 {
        return reject_qos(7);
    }
    let mpdu_length = header.cast::<u32>().read() & 0x3fff;
    let aggregate_eligible = mpdu_length >= MIN_HIL_MPDU_LENGTH;
    header = header.add(8);
    let frame_control = header.cast::<u16>().read_unaligned();
    LAST_FRAME_CONTROL.store(u32::from(frame_control), Ordering::Release);
    if frame_control & 0x008c != 0x0088 {
        return reject_qos(8);
    }
    if !aggregate_eligible {
        BELOW_MIN_LENGTH.fetch_add(1, Ordering::Relaxed);
    }
    Some(aggregate_eligible)
}

#[inline(always)]
fn reject_qos(reason: u32) -> Option<bool> {
    CLASSIFICATION_REJECT_REASON.store(reason, Ordering::Release);
    None
}

unsafe fn push_ready(state: &mut InterceptState, frame: *mut u8) -> Result<(), TxInterceptError> {
    let index = usize::from(state.count);
    if index >= TX_AMPDU_SLOT_CAPACITY {
        return Err(TxInterceptError::ReadyQueueFull);
    }
    state.frames[index] = frame;
    state.count = state.count.wrapping_add(1);
    record_ready_depth(state.count);
    Ok(())
}

unsafe fn push_retry_front(
    state: &mut InterceptState,
    frame: *mut u8,
) -> Result<(), TxInterceptError> {
    let count = usize::from(state.count);
    if count >= TX_AMPDU_SLOT_CAPACITY {
        return Err(TxInterceptError::ReadyQueueFull);
    }
    let insertion = usize::from(state.retry_prefix);
    let mut index = count;
    while index != insertion {
        state.frames[index] = state.frames[index - 1];
        index -= 1;
    }
    state.frames[insertion] = frame;
    state.count = state.count.wrapping_add(1);
    state.retry_prefix = state.retry_prefix.wrapping_add(1);
    record_ready_depth(state.count);
    Ok(())
}

#[inline(always)]
fn record_ready_depth(count: u8) {
    let count = u32::from(count);
    READY.store(count, Ordering::Release);
    let observed = READY_HIGH_WATER.load(Ordering::Relaxed);
    if count > observed {
        // Diagnostics never retry. A racing observation may conservatively
        // retain the larger value already published by the radio/IRQ owner.
        let _ = READY_HIGH_WATER.compare_exchange(
            observed,
            count,
            Ordering::Relaxed,
            Ordering::Relaxed,
        );
    }
}

#[inline(always)]
fn cycle_count() -> u32 {
    let value: u32;
    unsafe {
        core::arch::asm!(
            "csrr {value}, mcycle",
            value = out(reg) value,
            options(nomem, nostack)
        )
    };
    value
}

#[inline(always)]
fn record_cycle_max(counter: &AtomicU32, value: u32) {
    let observed = counter.load(Ordering::Relaxed);
    if value > observed {
        // Telemetry gets one attempt and never introduces a CAS retry loop.
        let _ = counter.compare_exchange(observed, value, Ordering::Relaxed, Ordering::Relaxed);
    }
}

#[link_section = ".rwtext.wifi_strict.hil_ampdu_cadence"]
pub(crate) fn record_hardware_completion_edge() {
    let now = cycle_count();
    let submitted = LAST_SUBMIT_CYCLE.swap(0, Ordering::AcqRel);
    if submitted != 0 {
        let cycles = now.wrapping_sub(submitted);
        HARDWARE_SERVICE_SAMPLES.fetch_add(1, Ordering::Relaxed);
        HARDWARE_SERVICE_TICKS_SUM.fetch_add(cycles >> 8, Ordering::Relaxed);
        record_cycle_max(&HARDWARE_SERVICE_CYCLES_MAX, cycles);
    }
    LAST_COMPLETION_EDGE_CYCLE.store(now, Ordering::Release);
}

#[inline(always)]
fn record_completion_handoff(retry_count: u8) {
    let now = cycle_count();
    let completed = LAST_COMPLETION_EDGE_CYCLE.swap(0, Ordering::AcqRel);
    if completed != 0 {
        let cycles = now.wrapping_sub(completed);
        COMPLETION_DISPATCH_SAMPLES.fetch_add(1, Ordering::Relaxed);
        COMPLETION_DISPATCH_TICKS_SUM.fetch_add(cycles >> 8, Ordering::Relaxed);
        record_cycle_max(&COMPLETION_DISPATCH_CYCLES_MAX, cycles);
    }
    MISSING_BLOCK_ACK_SUBFRAMES.fetch_add(u32::from(retry_count), Ordering::Relaxed);
    LAST_COMPLETION_HANDOFF_CYCLE.store(now, Ordering::Release);
}

#[inline(always)]
fn record_submit_cadence(retry_subframes: u8) {
    let now = cycle_count();
    let completed = LAST_COMPLETION_HANDOFF_CYCLE.swap(0, Ordering::AcqRel);
    if completed != 0 {
        let cycles = now.wrapping_sub(completed);
        REFILL_GAP_SAMPLES.fetch_add(1, Ordering::Relaxed);
        REFILL_GAP_TICKS_SUM.fetch_add(cycles >> 8, Ordering::Relaxed);
        record_cycle_max(&REFILL_GAP_CYCLES_MAX, cycles);
    }
    if retry_subframes != 0 {
        RETRY_SUBMITS.fetch_add(1, Ordering::Relaxed);
        RETRY_SUBFRAMES.fetch_add(u32::from(retry_subframes), Ordering::Relaxed);
    }
    LAST_SUBMIT_CYCLE.store(now, Ordering::Release);
}

fn schedule(state: &mut InterceptState) -> Result<(), TxInterceptError> {
    if state.event_pending {
        return Ok(());
    }
    if !crate::adapter::enqueue_internal_event(crate::event::PpEvent {
        kind: HIL_AMPDU_INTERCEPT_EVENT,
        argument: ptr::null_mut::<c_void>(),
    }) {
        return Err(TxInterceptError::InternalQueueFull);
    }
    state.event_pending = true;
    Ok(())
}

#[link_section = ".rwtext.wifi_strict.hil_ampdu_intercept"]
unsafe extern "C" fn coalesce_timeout(_argument: *mut c_void) {
    let state = &mut *STATE.0.get();
    if !state.coalesce_armed {
        return;
    }
    state.coalesce_armed = false;
    if state.count == 0 {
        state.coalesce_due = false;
        return;
    }
    state.coalesce_due = true;
    COALESCE_EXPIRED.fetch_add(1, Ordering::Relaxed);
    if schedule(state).is_err() {
        fail_and_trap();
    }
}

unsafe fn reconcile_coalesce_deadline(state: &mut InterceptState) -> Result<(), TxInterceptError> {
    let needs_deadline = state.count == 1
        && !state.frames[0].is_null()
        && aggregate_eligible_prepared(state.frames[0]);
    if !needs_deadline {
        if state.coalesce_armed {
            if !crate::adapter::cancel_internal_timer(COALESCE_TIMER.0.get().cast()) {
                return Err(TxInterceptError::CoalesceTimerCancel);
            }
            state.coalesce_armed = false;
        }
        state.coalesce_due = false;
        return Ok(());
    }
    if state.coalesce_due || state.coalesce_armed {
        return Ok(());
    }
    if !crate::adapter::schedule_internal_timer(
        COALESCE_TIMER.0.get().cast(),
        coalesce_timeout,
        ptr::null_mut(),
        HIL_COALESCE_DELAY_US,
    ) {
        return Err(TxInterceptError::CoalesceTimerSchedule);
    }
    state.coalesce_armed = true;
    COALESCE_ARMED.fetch_add(1, Ordering::Relaxed);
    Ok(())
}

pub(crate) const fn is_event(kind: u32) -> bool {
    kind == HIL_AMPDU_INTERCEPT_EVENT
}

#[link_section = ".rwtext.wifi_strict.hil_ampdu_intercept"]
pub(crate) unsafe fn dispatch() -> Result<(), TxInterceptError> {
    if FAILED.load(Ordering::Acquire) {
        return Err(TxInterceptError::PreviousFailure);
    }
    let state = &mut *STATE.0.get();
    state.event_pending = false;

    // Move a fixed prefix of detached retries per executor action. This keeps
    // the action finite while avoiding one private event for every missing
    // BlockAck bit in a large partial-ACK aggregate.
    const RETRY_TRANSFER_QUANTUM: u8 = 4;
    let mut transferred = 0_u8;
    while transferred < RETRY_TRANSFER_QUANTUM {
        let Some(retry) = crate::lmac::take_basic_ht_ampdu_retry() else {
            break;
        };
        let _sequence = retry.sequence;
        push_retry_front(state, retry.frame)?;
        transferred = transferred.wrapping_add(1);
    }
    if transferred != 0 {
        reconcile_coalesce_deadline(state)?;
        schedule(state)?;
        return Ok(());
    }
    if state.count == 0 || state.waiting_hardware {
        return Ok(());
    }

    let first_descriptor = state.frames[0]
        .add(FRAME_DESCRIPTOR_OFFSET)
        .cast::<*mut u8>()
        .read();
    if first_descriptor.is_null() {
        return fail(TxInterceptError::Submit(
            crate::lmac::LmacAsyncError::InvalidTxSubmissionPointer,
        ));
    }
    let first_rate = first_descriptor.add(DESCRIPTOR_RATE_OFFSET).read();
    let submit_queue = if first_rate < 16 {
        0
    } else {
        HIL_HARDWARE_QUEUE
    };
    let instances = ptr::addr_of!(our_instances_ptr).read();
    if instances.is_null() {
        return fail(TxInterceptError::InstancesUnavailable);
    }
    let queue_state = instances.add(usize::from(submit_queue) * TX_QUEUE_STATE_SIZE);
    let hardware_queue = queue_state.add(TX_QUEUE_HARDWARE_INDEX_OFFSET).read();
    if hardware_queue != submit_queue {
        return fail(TxInterceptError::InvalidHardwareQueue(hardware_queue));
    }
    if queue_state.add(TX_QUEUE_STATUS_OFFSET).read() != 0 {
        state.waiting_hardware = true;
        return Ok(());
    }

    let count = usize::from(state.count);
    if !aggregate_eligible_prepared(state.frames[0]) {
        return submit_one(state, queue_state, count);
    }
    if count < 2 && !state.coalesce_due {
        return Ok(());
    }
    if count < 2 {
        return submit_one(state, queue_state, count);
    }
    let mut selected = count.min(usize::from(state.window)).min(MAX_HIL_SUBFRAMES);
    let mut index = 1_usize;
    while index < selected {
        let descriptor = state.frames[index]
            .add(FRAME_DESCRIPTOR_OFFSET)
            .cast::<*mut u8>()
            .read();
        if descriptor.is_null()
            || descriptor.add(DESCRIPTOR_RATE_OFFSET).read() != first_rate
            || !aggregate_eligible_prepared(state.frames[index])
        {
            selected = index;
            break;
        }
        index += 1;
    }
    if selected < 2 {
        return submit_one(state, queue_state, count);
    }

    let chain = crate::tx_ampdu::prepare_basic_ht_ampdu_chain(
        &state.frames[..selected],
        MAX_HIL_AGGREGATE_LENGTH,
    )
    .map_err(TxInterceptError::Aggregate)?;
    let aggregate_length = chain.aggregate_length;
    let retry_subframes = state.retry_prefix.min(selected as u8);
    if let Err(error) = crate::lmac::submit_basic_ht_ampdu(queue_state, chain) {
        // Submission validates queue/descriptor state before ownership
        // transfer. A failure after preparation is fatal for this laboratory
        // bridge; silently falling back would duplicate frame ownership.
        return fail(TxInterceptError::Submit(error));
    }
    record_submit_cadence(retry_subframes);
    #[cfg(feature = "hil-tx-deep-telemetry")]
    record_hardware_submit(queue_state);

    let remaining = count - selected;
    let mut source = selected;
    while source < count {
        state.frames[source - selected] = state.frames[source];
        source += 1;
    }
    let mut clear = remaining;
    while clear < count {
        state.frames[clear] = ptr::null_mut();
        clear += 1;
    }
    state.count = remaining as u8;
    state.retry_prefix = state.retry_prefix.saturating_sub(selected as u8);
    state.waiting_hardware = true;
    READY.store(remaining as u32, Ordering::Release);
    reconcile_coalesce_deadline(state)?;
    SUBMITTED.fetch_add(1, Ordering::Relaxed);
    SUBFRAMES.fetch_add(selected as u32, Ordering::Relaxed);
    AGGREGATE_BYTES.fetch_add(u32::from(aggregate_length), Ordering::Relaxed);
    AGGREGATE_SIZE_HISTOGRAM[selected].fetch_add(1, Ordering::Relaxed);
    Ok(())
}

/// Called by the Rust BlockAck completion after it has retained every missing
/// MPDU. It only posts a private executor event; it never submits recursively.
pub(crate) fn on_hardware_completion(retry_count: u8) -> Result<(), TxInterceptError> {
    let state = unsafe { &mut *STATE.0.get() };
    record_completion_handoff(retry_count);
    state.waiting_hardware = false;
    COMPLETED.fetch_add(1, Ordering::Relaxed);
    schedule(state)
}

pub(crate) fn owns_direct_hardware_frame(frame: *mut u8) -> bool {
    !frame.is_null() && unsafe { (*STATE.0.get()).direct_frame == frame }
}

pub(crate) fn on_direct_hardware_completion() -> Result<(), TxInterceptError> {
    let state = unsafe { &mut *STATE.0.get() };
    if state.direct_frame.is_null() {
        return fail(TxInterceptError::MissingDirectOwner);
    }
    let instances = unsafe { ptr::addr_of!(our_instances_ptr).read() };
    if instances.is_null() {
        return fail(TxInterceptError::InstancesUnavailable);
    }
    let direct_queue = state.direct_queue;
    if direct_queue > 3 {
        return fail(TxInterceptError::MissingDirectOwner);
    }
    let queue_state = unsafe { instances.add(usize::from(direct_queue) * TX_QUEUE_STATE_SIZE) };
    let queue_frame = unsafe { queue_state.cast::<*mut u8>().read() };
    if !queue_frame.is_null() && queue_frame != state.direct_frame {
        return fail(TxInterceptError::MissingDirectOwner);
    }
    unsafe {
        queue_state.cast::<*mut u8>().write(ptr::null_mut());
    }
    state.direct_frame = ptr::null_mut();
    state.direct_queue = u8::MAX;
    DIRECT_COMPLETED.fetch_add(1, Ordering::Relaxed);
    if direct_queue == 0 {
        LEGACY_DIRECT_COMPLETED.fetch_add(1, Ordering::Relaxed);
    }
    on_hardware_completion(0)
}

unsafe fn submit_one(
    state: &mut InterceptState,
    queue_state: *mut u8,
    count: usize,
) -> Result<(), TxInterceptError> {
    let frame = state.frames[0];
    let hardware_queue = queue_state.add(TX_QUEUE_HARDWARE_INDEX_OFFSET).read();
    state.direct_queue = hardware_queue;
    state.direct_frame = frame;
    if let Err(error) = crate::lmac::submit_basic_non_he_frame(queue_state, frame) {
        state.direct_frame = ptr::null_mut();
        state.direct_queue = u8::MAX;
        return Err(TxInterceptError::Submit(error));
    }
    record_submit_cadence(state.retry_prefix.min(1));
    #[cfg(feature = "hil-tx-deep-telemetry")]
    record_hardware_submit(queue_state);

    let mut source = 1_usize;
    while source < count {
        state.frames[source - 1] = state.frames[source];
        source += 1;
    }
    state.frames[count - 1] = ptr::null_mut();
    state.count = state.count.wrapping_sub(1);
    state.retry_prefix = state.retry_prefix.saturating_sub(1);
    state.waiting_hardware = true;
    READY.store(u32::from(state.count), Ordering::Release);
    reconcile_coalesce_deadline(state)?;
    SUBMITTED.fetch_add(1, Ordering::Relaxed);
    SUBFRAMES.fetch_add(1, Ordering::Relaxed);
    DIRECT_SUBMITTED.fetch_add(1, Ordering::Relaxed);
    if hardware_queue == 0 {
        LEGACY_DIRECT_SUBMITTED.fetch_add(1, Ordering::Relaxed);
    }
    Ok(())
}

unsafe fn aggregate_eligible_prepared(frame: *mut u8) -> bool {
    let first_buffer = frame
        .add(FRAME_FIRST_BUFFER_OFFSET)
        .cast::<*mut u8>()
        .read();
    let header = first_buffer
        .add(BUFFER_DATA_OFFSET)
        .cast::<*mut u8>()
        .read();
    !header.is_null() && header.cast::<u32>().read() & 0x3fff >= MIN_HIL_MPDU_LENGTH
}

fn fail<T>(error: TxInterceptError) -> Result<T, TxInterceptError> {
    FAILED.store(true, Ordering::Release);
    Err(error)
}

#[inline(always)]
unsafe fn fail_and_trap() -> ! {
    FAILED.store(true, Ordering::Release);
    core::arch::asm!("ebreak", options(noreturn))
}
