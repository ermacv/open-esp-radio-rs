use core::{
    cell::UnsafeCell,
    ffi::c_void,
    ptr,
    sync::atomic::{AtomicBool, AtomicU32, Ordering},
};

#[cfg(target_arch = "riscv32")]
use crate::tx_ampdu::BasicHtAmpduChain;
use crate::{adapter::schedule_internal_timer, timer::RawOsiTimer};

pub(crate) const TX_DISCARD_CONTINUATION: u32 = u32::MAX - 1;
pub(crate) const TX_AMPDU_COMPLETION_CONTINUATION: u32 = u32::MAX - 4;

const TX_DISABLE_SETTLE_US: u32 = 16;
const TXQ_INTERRUPT_CLEAR_REG: *mut u32 = 0x2010_4cb0 as *mut u32;
const TXQ_INTERRUPT_STATE_REG: *const u32 = 0x2010_4cb4 as *const u32;
const TXQ_COMPLETE_STATE_REG: *const u32 = 0x2010_4cbc as *const u32;
const MAC_CLOCK_REG: *const u32 = 0x2010_d800 as *const u32;
const TXQ_CONFIG_BASE_REG: usize = 0x2010_4d6c;
const TXQ_ENABLE_BASE_REG: usize = 0x2010_4d70;
const TXQ_PPDU_CONTROL_BASE_REG: usize = 0x2010_4d68;
const TXQ_PROTECTION_BASE_REG: usize = 0x2010_4d64;
const TXQ_PROTECTION_DURATION_BASE_REG: usize = 0x2010_54dc;
const TXQ_PTI_BASE_REG: usize = 0x2010_54e0;
const TXQ_PLCP1_BASE_REG: usize = 0x2010_54d8;
const TXQ_HTSIG_BASE_REG: usize = 0x2010_54e8;
const TXQ_HT_CONTROL_BASE_REG: usize = 0x2010_5504;
const TXQ_DATA_LENGTH_BASE_REG: usize = 0x2010_550c;
const TXQ_LENGTH_CONTROL_BASE_REG: usize = 0x2010_5510;
const TXQ_POWER_BASE_REG: usize = 0x2010_5500;
const TXQ_REGISTER_STRIDE: usize = 0x10;
const TXQ_POWER_STRIDE: usize = 0x7c;
const TX_QUEUE_STATE_SIZE: usize = 0x38;
const TX_QUEUE_HARDWARE_INDEX_OFFSET: usize = 0x04;
const TX_QUEUE_RATE_OFFSET: usize = 0x08;
const TX_QUEUE_SAVED_RATE_OFFSET: usize = 0x09;
const TX_QUEUE_RATE_LIMIT_OFFSET: usize = 0x0a;
const TX_QUEUE_SHORT_RETRY_OFFSET: usize = 0x0b;
const TX_QUEUE_LONG_RETRY_OFFSET: usize = 0x0c;
const TX_QUEUE_STATUS_OFFSET: usize = 0x12;
const TX_QUEUE_END_STATE_OFFSET: usize = 0x13;
const TX_QUEUE_TXOP_OUTSTANDING_OFFSET: usize = 0x1c;
const TX_QUEUE_KIND_OFFSET: usize = 0x1d;
const TX_QUEUE_SPECIAL_RETRY_OFFSET: usize = 0x2e;
const TX_QUEUE_TRIGGER_RETRY_OFFSET: usize = 0x30;
const TX_QUEUE_TRIGGER_STATE_OFFSET: usize = 0x34;
const TX_QUEUE_COMPLETED_COUNT_OFFSET: usize = 0x20;
const TX_QUEUE_DROP_COUNT_OFFSET: usize = 0x24;
const TX_FRAME_DESCRIPTOR_OFFSET: usize = 0x34;
const TX_FRAME_NEXT_OFFSET: usize = 0x30;
const TX_FRAME_SCHEDULER_OFFSET: usize = 0x04;
const TX_FRAME_LAYOUT_FLAGS_OFFSET: usize = 0x24;
const TX_FRAME_RATE_CONTEXT_OFFSET: usize = 0x2c;
const TX_DESCRIPTOR_REASON_OFFSET: usize = 0x13;
const TX_DESCRIPTOR_RESPONSE_OFFSET: usize = 0x0d;
const TX_DESCRIPTOR_QUEUE_WORD_OFFSET: usize = 0x10;
const TX_DESCRIPTOR_RATE_CONTROL_OFFSET: usize = 0x1c;
const TX_DESCRIPTOR_TIMESTAMP_OFFSET: usize = 0x18;
const TX_DESCRIPTOR_SELECTED_RATE_OFFSET: usize = 0x0c;
const TX_DESCRIPTOR_PHY_FLAGS_OFFSET: usize = 0x30;
const TX_DESCRIPTOR_LENGTH_LOW_OFFSET: usize = 0x40;
const TX_DESCRIPTOR_LENGTH_HIGH_OFFSET: usize = 0x44;
const TX_RATE_CONTEXT_MODE_OFFSET: usize = 0x0c;
const TX_RATE_CONTEXT_ALT_RATE_OFFSET: usize = 0x08;
const TX_RATE_CONTEXT_DEFAULT_RATE_OFFSET: usize = 0x09;
const TX_FRAME_ABORTED_BIT: u32 = 0x0002_0000;
const TX_FRAME_BAR_BIT: u32 = 0x0020_0000;
const TX_FRAME_AMPDU_BIT: u32 = 0x0040_0000;
const TX_FRAME_HE_BIT: u32 = 0x8000_0000;
const TX_FRAME_DEQUEUE_MASK: u32 = 0x0000_00c0;
const TX_FRAME_DEQUEUE_VALUE: u32 = 0x0000_0080;
const TX_FRAME_LONG_RETRY_BIT: u32 = 0x0000_0100;
const TX_FRAME_RETRY_SCHEDULER_MASK: u32 = 0x0060_0002;
const TX_FRAME_RETRY_RATE_TIME_BIT: u32 = 0x0000_0040;
const TX_FRAME_FORCE_SHORT_DISCARD_BIT: u32 = 0x1000_0000;
const TX_FRAME_RATE_LIMIT_BIT: u32 = 0x0800_0000;
const TX_FRAME_OFFCHANNEL_BIT: u32 = 0x0001_0000;
const TX_FRAME_FTM_BIT: u32 = 0x2000_0000;
const TX_SUCCESS_CLASSIFY_MASK: u32 = 0x0000_0402;
const TX_SUCCESS_AGGREGATE_STATE_MASK: u32 = 0x40c0_0000;
const AP_BEACON_SUCCESS_DESCRIPTOR: u32 = 0x0080_0412;

const DISCARD_IDLE: u8 = 0;
const DISCARD_FIND_TAIL: u8 = 1;
const DISCARD_FRAME: u8 = 2;
const DISCARD_WAIT_TX_DONE: u8 = 3;

unsafe extern "C" {
    static mut our_instances_ptr: *mut u8;
    static lmacConfMib: [u8; 48];
    static s_phy_get_max_pwr: i8;
    static mut coex_pti_tab: [u8; 48];

    fn hal_mac_tx_set_cca(value: u32);
    fn hal_mac_get_txq_state(kind: u32) -> u32;
    fn hal_mac_clr_txq_state(kind: u32, queue: u8);
    fn hal_mac_get_txq_in_trig_flow_state() -> u32;
    #[link_name = "hal_mac_get_txq_complete"]
    fn vendor_hal_mac_get_txq_complete(
        queue_state: *mut u8,
        queue: u8,
        completion: *mut u8,
        auxiliary: *mut u8,
    ) -> i32;
    fn hal_mac_is_txq_valid(queue: u8) -> u32;
    fn hal_mac_set_txq_invalid(queue: u8);
    fn hal_mac_txq_disable(queue: u8);
    fn lmacTxDone(frame: *mut c_void, mode: u32);
    fn pp_post(kind: u32, argument: *mut c_void) -> i32;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BasicRetryCause {
    CtsTimeout,
    AckTimeout,
    Collision,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LmacAsyncError {
    InstancesUnavailable,
    TimerUnavailable,
    InternalQueueFull,
    PreviousContinuationFailure,
    UnsupportedAggregatedFrame(u32),
    TxRxUnavailable,
    TxopQueue(crate::tx_queue::TxopQueueError),
    InvalidDiscardContinuation,
    TxDone(crate::txdone::TxDoneError),
    TxQueueSplitFailed,
    UnsupportedTxCompletionStatus(u8),
    UnsupportedTxSuccessQueueKind(u8),
    UnsupportedTxSuccessTxop(u8),
    UnsupportedTxSuccessChain,
    UnsupportedTxSuccessDescriptor(u32),
    UnsupportedTxRetryQueueKind(u8),
    UnsupportedTxRetryTxop(u8),
    UnsupportedTxRetryChain,
    UnsupportedTxRetryDescriptor(u32),
    UnsupportedTxRetryState(u32),
    UnsupportedTxCollisionMplen(u32),
    InvalidTxRetryRateControl,
    InvalidTxRetryScheduler,
    InvalidTxSubmissionPointer,
    UnsupportedTxSubmissionQueue(u8),
    UnsupportedTxSubmissionQueueStatus(u8),
    UnsupportedTxSubmissionDescriptor(u32),
    UnsupportedTxSubmissionMetadata(u32),
    UnsupportedTxSubmissionPti { priority: u8, count: u16 },
    UnsupportedTxAmpduChain { subframes: u8, length: u16 },
    TxAmpduOwnerBusy(u8),
    MissingTxAmpduOwner(u8),
    TxAmpduCompletionBusy,
    PreviousTxAmpduCompletionFailure,
    InvalidTxAmpduContinuation,
    TxAmpduRestore(crate::tx_ampdu::BasicHtAmpduRestoreError),
    TxAmpduFrameCompletion(crate::tx_ampdu::BasicHtAmpduFrameCompletionError),
}

#[derive(Clone, Copy)]
struct TxTimeoutState {
    active: bool,
    pending: bool,
    failed: bool,
    remaining: u32,
    current_queue: u8,
    discard_phase: u8,
    discard_reason: u8,
    queue_state: *mut u8,
    discard_frame: *mut u8,
    discard_tail: *mut u8,
    finish_timeout_queue: bool,
}

impl TxTimeoutState {
    const fn new() -> Self {
        Self {
            active: false,
            pending: false,
            failed: false,
            remaining: 0,
            current_queue: 0,
            discard_phase: DISCARD_IDLE,
            discard_reason: 0,
            queue_state: ptr::null_mut(),
            discard_frame: ptr::null_mut(),
            discard_tail: ptr::null_mut(),
            finish_timeout_queue: false,
        }
    }
}

struct StateCell(UnsafeCell<TxTimeoutState>);

unsafe impl Sync for StateCell {}

#[cfg(target_arch = "riscv32")]
struct AmpduOwnersCell(UnsafeCell<[Option<BasicHtAmpduChain>; 4]>);

#[cfg(target_arch = "riscv32")]
unsafe impl Sync for AmpduOwnersCell {}

#[cfg(target_arch = "riscv32")]
struct AmpduCompletionState {
    active: bool,
    failed: bool,
    chain: Option<BasicHtAmpduChain>,
    block_ack: Option<crate::tx_ampdu::TxBlockAckBitmap>,
    response: u8,
    next: u8,
    resume_event: u8,
    retry_count: u8,
    retry_take: u8,
    retries: [*mut u8; crate::tx_ampdu::TX_AMPDU_SLOT_CAPACITY],
    retry_sequences: [u16; crate::tx_ampdu::TX_AMPDU_SLOT_CAPACITY],
}

#[cfg(target_arch = "riscv32")]
impl AmpduCompletionState {
    const fn new() -> Self {
        Self {
            active: false,
            failed: false,
            chain: None,
            block_ack: None,
            response: 0,
            next: 0,
            resume_event: 0,
            retry_count: 0,
            retry_take: 0,
            retries: [ptr::null_mut(); crate::tx_ampdu::TX_AMPDU_SLOT_CAPACITY],
            retry_sequences: [0; crate::tx_ampdu::TX_AMPDU_SLOT_CAPACITY],
        }
    }
}

#[cfg(target_arch = "riscv32")]
struct AmpduCompletionCell(UnsafeCell<AmpduCompletionState>);

#[cfg(target_arch = "riscv32")]
unsafe impl Sync for AmpduCompletionCell {}

struct TimerCell(UnsafeCell<RawOsiTimer>);

impl TimerCell {
    const fn new() -> Self {
        Self(UnsafeCell::new(RawOsiTimer {
            next: ptr::null_mut(),
            expire: 0,
            period: 0,
            callback: None,
            argument: ptr::null_mut(),
        }))
    }
}

unsafe impl Sync for TimerCell {}

static STATE: StateCell = StateCell(UnsafeCell::new(TxTimeoutState::new()));
#[cfg(target_arch = "riscv32")]
#[link_section = ".critical.bss.wifi_strict.tx_ampdu_owners"]
static AMPDU_OWNERS: AmpduOwnersCell = AmpduOwnersCell(UnsafeCell::new([const { None }; 4]));
#[cfg(target_arch = "riscv32")]
#[link_section = ".critical.bss.wifi_strict.tx_ampdu_completion_state"]
static AMPDU_COMPLETION: AmpduCompletionCell =
    AmpduCompletionCell(UnsafeCell::new(AmpduCompletionState::new()));
static TIMER: TimerCell = TimerCell::new();
static TXQ_SPLIT_FAILED: AtomicBool = AtomicBool::new(false);
#[cfg_attr(
    target_arch = "riscv32",
    link_section = ".critical.data.wifi_strict.tx_backoff"
)]
static BACKOFF_SEQUENCE: AtomicU32 = AtomicU32::new(0x6d2b_79f5);

/// HIL-only observations captured immediately before the selected completion
/// outcome runs. They let us prove the narrow basic-HT success invariants
/// before replacing the remaining vendor success/recycle bodies.
#[cfg(feature = "hil-vendor-tx")]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct LmacTxCompleteSnapshot {
    pub completions: u32,
    pub stale: u32,
    pub success: u32,
    pub rts_error: u32,
    pub cts_timeout: u32,
    pub tx_error: u32,
    pub ack_timeout: u32,
    pub collisions: u32,
    pub unexpected_status: u32,
    pub success_queue_mask: u32,
    pub success_queue_kind_mask: u32,
    pub success_txop_nonzero: u32,
    pub success_txop_max: u8,
    pub success_next_nonnull: u32,
    pub success_descriptor_flags_or: u32,
    pub last_queue: u8,
    pub last_queue_kind: u8,
    pub last_txop_outstanding: u8,
    pub last_response: u8,
    pub last_descriptor_flags: u32,
}

/// HIL-only before/after view of the vendor retry outcome bodies that remain
/// after the Rust-owned completion decoder.
#[cfg(feature = "hil-vendor-tx")]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct LmacRetrySnapshot {
    pub ack_timeout: u32,
    pub cts_timeout: u32,
    pub returned: u32,
    pub same_frame: u32,
    pub changed_frame: u32,
    pub detached_frame: u32,
    pub queue_kind_mask: u32,
    pub pre_status_mask: u32,
    pub post_status_mask: u32,
    pub descriptor_flags_or: u32,
    pub long_frame_flag: u32,
    pub next_nonnull: u32,
    pub txop_nonzero: u32,
    pub last_pre_queue_counters: u32,
    pub last_post_queue_counters: u32,
    pub last_pre_queue_state: u32,
    pub last_post_queue_state: u32,
    pub last_pre_descriptor_counters: u32,
    pub last_post_descriptor_counters: u32,
}

#[cfg(feature = "hil-vendor-tx")]
struct TxCompleteCounters {
    completions: AtomicU32,
    stale: AtomicU32,
    outcomes: [AtomicU32; 6],
    collisions: AtomicU32,
    unexpected_status: AtomicU32,
    success_queue_mask: AtomicU32,
    success_queue_kind_mask: AtomicU32,
    success_txop_nonzero: AtomicU32,
    success_txop_max: AtomicU32,
    success_next_nonnull: AtomicU32,
    success_descriptor_flags_or: AtomicU32,
    last_queue: AtomicU32,
    last_queue_kind: AtomicU32,
    last_txop_outstanding: AtomicU32,
    last_response: AtomicU32,
    last_descriptor_flags: AtomicU32,
}

#[cfg(feature = "hil-vendor-tx")]
struct RetryCounters {
    ack_timeout: AtomicU32,
    cts_timeout: AtomicU32,
    returned: AtomicU32,
    same_frame: AtomicU32,
    changed_frame: AtomicU32,
    detached_frame: AtomicU32,
    queue_kind_mask: AtomicU32,
    pre_status_mask: AtomicU32,
    post_status_mask: AtomicU32,
    descriptor_flags_or: AtomicU32,
    long_frame_flag: AtomicU32,
    next_nonnull: AtomicU32,
    txop_nonzero: AtomicU32,
    last_pre_queue_counters: AtomicU32,
    last_post_queue_counters: AtomicU32,
    last_pre_queue_state: AtomicU32,
    last_post_queue_state: AtomicU32,
    last_pre_descriptor_counters: AtomicU32,
    last_post_descriptor_counters: AtomicU32,
}

#[cfg(feature = "hil-vendor-tx")]
impl RetryCounters {
    const fn new() -> Self {
        Self {
            ack_timeout: AtomicU32::new(0),
            cts_timeout: AtomicU32::new(0),
            returned: AtomicU32::new(0),
            same_frame: AtomicU32::new(0),
            changed_frame: AtomicU32::new(0),
            detached_frame: AtomicU32::new(0),
            queue_kind_mask: AtomicU32::new(0),
            pre_status_mask: AtomicU32::new(0),
            post_status_mask: AtomicU32::new(0),
            descriptor_flags_or: AtomicU32::new(0),
            long_frame_flag: AtomicU32::new(0),
            next_nonnull: AtomicU32::new(0),
            txop_nonzero: AtomicU32::new(0),
            last_pre_queue_counters: AtomicU32::new(0),
            last_post_queue_counters: AtomicU32::new(0),
            last_pre_queue_state: AtomicU32::new(0),
            last_post_queue_state: AtomicU32::new(0),
            last_pre_descriptor_counters: AtomicU32::new(0),
            last_post_descriptor_counters: AtomicU32::new(0),
        }
    }
}

#[cfg(feature = "hil-vendor-tx")]
impl TxCompleteCounters {
    const fn new() -> Self {
        Self {
            completions: AtomicU32::new(0),
            stale: AtomicU32::new(0),
            outcomes: [const { AtomicU32::new(0) }; 6],
            collisions: AtomicU32::new(0),
            unexpected_status: AtomicU32::new(0),
            success_queue_mask: AtomicU32::new(0),
            success_queue_kind_mask: AtomicU32::new(0),
            success_txop_nonzero: AtomicU32::new(0),
            success_txop_max: AtomicU32::new(0),
            success_next_nonnull: AtomicU32::new(0),
            success_descriptor_flags_or: AtomicU32::new(0),
            last_queue: AtomicU32::new(0),
            last_queue_kind: AtomicU32::new(0),
            last_txop_outstanding: AtomicU32::new(0),
            last_response: AtomicU32::new(0),
            last_descriptor_flags: AtomicU32::new(0),
        }
    }
}

#[cfg(feature = "hil-vendor-tx")]
static TX_COMPLETE_COUNTERS: TxCompleteCounters = TxCompleteCounters::new();
#[cfg(feature = "hil-vendor-tx")]
static RETRY_COUNTERS: RetryCounters = RetryCounters::new();

#[cfg(feature = "hil-vendor-tx")]
pub fn lmac_tx_complete_snapshot() -> LmacTxCompleteSnapshot {
    let counters = &TX_COMPLETE_COUNTERS;
    LmacTxCompleteSnapshot {
        completions: counters.completions.load(Ordering::Acquire),
        stale: counters.stale.load(Ordering::Acquire),
        success: counters.outcomes[0].load(Ordering::Acquire),
        rts_error: counters.outcomes[1].load(Ordering::Acquire),
        cts_timeout: counters.outcomes[2].load(Ordering::Acquire),
        tx_error: counters.outcomes[4].load(Ordering::Acquire),
        ack_timeout: counters.outcomes[5].load(Ordering::Acquire),
        collisions: counters.collisions.load(Ordering::Acquire),
        unexpected_status: counters.unexpected_status.load(Ordering::Acquire),
        success_queue_mask: counters.success_queue_mask.load(Ordering::Acquire),
        success_queue_kind_mask: counters.success_queue_kind_mask.load(Ordering::Acquire),
        success_txop_nonzero: counters.success_txop_nonzero.load(Ordering::Acquire),
        success_txop_max: counters.success_txop_max.load(Ordering::Acquire) as u8,
        success_next_nonnull: counters.success_next_nonnull.load(Ordering::Acquire),
        success_descriptor_flags_or: counters.success_descriptor_flags_or.load(Ordering::Acquire),
        last_queue: counters.last_queue.load(Ordering::Acquire) as u8,
        last_queue_kind: counters.last_queue_kind.load(Ordering::Acquire) as u8,
        last_txop_outstanding: counters.last_txop_outstanding.load(Ordering::Acquire) as u8,
        last_response: counters.last_response.load(Ordering::Acquire) as u8,
        last_descriptor_flags: counters.last_descriptor_flags.load(Ordering::Acquire),
    }
}

#[cfg(feature = "hil-vendor-tx")]
pub fn lmac_retry_snapshot() -> LmacRetrySnapshot {
    let counters = &RETRY_COUNTERS;
    LmacRetrySnapshot {
        ack_timeout: counters.ack_timeout.load(Ordering::Acquire),
        cts_timeout: counters.cts_timeout.load(Ordering::Acquire),
        returned: counters.returned.load(Ordering::Acquire),
        same_frame: counters.same_frame.load(Ordering::Acquire),
        changed_frame: counters.changed_frame.load(Ordering::Acquire),
        detached_frame: counters.detached_frame.load(Ordering::Acquire),
        queue_kind_mask: counters.queue_kind_mask.load(Ordering::Acquire),
        pre_status_mask: counters.pre_status_mask.load(Ordering::Acquire),
        post_status_mask: counters.post_status_mask.load(Ordering::Acquire),
        descriptor_flags_or: counters.descriptor_flags_or.load(Ordering::Acquire),
        long_frame_flag: counters.long_frame_flag.load(Ordering::Acquire),
        next_nonnull: counters.next_nonnull.load(Ordering::Acquire),
        txop_nonzero: counters.txop_nonzero.load(Ordering::Acquire),
        last_pre_queue_counters: counters.last_pre_queue_counters.load(Ordering::Acquire),
        last_post_queue_counters: counters.last_post_queue_counters.load(Ordering::Acquire),
        last_pre_queue_state: counters.last_pre_queue_state.load(Ordering::Acquire),
        last_post_queue_state: counters.last_post_queue_state.load(Ordering::Acquire),
        last_pre_descriptor_counters: counters
            .last_pre_descriptor_counters
            .load(Ordering::Acquire),
        last_post_descriptor_counters: counters
            .last_post_descriptor_counters
            .load(Ordering::Acquire),
    }
}

/// Final-link replacement for `hal_mac_get_txq_state`. The vendor complete
/// and collision handlers consume every returned bitmap bit in one call. The
/// strict wrapper exposes exactly one bit and posts another PP event for the
/// remainder, turning that loop into executor-visible continuations. It also
/// bypasses the original statistics/logging hooks.
#[no_mangle]
pub unsafe extern "C" fn __wrap_hal_mac_get_txq_state(kind: u32) -> u32 {
    let (bits, continuation) = match kind {
        0 => (TXQ_INTERRUPT_STATE_REG.read_volatile() & 0x0f, 24),
        1 => return (TXQ_INTERRUPT_STATE_REG.read_volatile() >> 16) & 0x0f,
        2 => (TXQ_COMPLETE_STATE_REG.read_volatile() & 0x0f, 23),
        _ => return 0,
    };
    if bits == 0 {
        return 0;
    }
    let one = 1_u32 << bits.trailing_zeros();
    if bits & !one != 0 && pp_post(continuation, ptr::null_mut()) != 0 {
        TXQ_SPLIT_FAILED.store(true, Ordering::Release);
        return 0;
    }
    one
}

/// Strict basic-HT replacement for the 0x81e-byte vendor completion reader.
///
/// The stock body starts with these fixed register decodes, then enters HE
/// MPLEN maintenance, connection-state queries, formatters, and debug logs.
/// Strict STA advertises HT rather than HE. Ordinary MPDUs and Rust-owned HT
/// A-MPDUs share this fixed register prefix; aggregate BlockAck registers are
/// read separately by the completion continuation. HE/BAR tails remain
/// forbidden invariants.
#[no_mangle]
#[link_section = ".rwtext.wifi_strict.txq_complete"]
pub unsafe extern "C" fn __wrap_hal_mac_get_txq_complete(
    queue_state: *mut u8,
    queue: u8,
    completion: *mut u8,
    auxiliary: *mut u8,
) -> i32 {
    if queue >= 4 || queue_state.is_null() || completion.is_null() {
        reject_txq_completion();
    }

    // Match the stock ABI even though every byte is overwritten below. This
    // also makes future extensions deterministic if another completion field
    // is recovered from the pinned body.
    completion.write(0);
    completion.add(1).write(0);
    completion.add(2).write(0);
    completion.add(3).write(0);
    completion.add(4).write(0);
    completion.add(5).write(0);
    if !auxiliary.is_null() {
        auxiliary.cast::<u32>().write(0);
        auxiliary.add(4).cast::<u32>().write(0);
    }

    let frame = queue_state.cast::<*mut u8>().read();
    if frame.is_null() {
        reject_txq_completion();
    }
    let descriptor = frame
        .add(TX_FRAME_DESCRIPTOR_OFFSET)
        .cast::<*mut u32>()
        .read();
    if descriptor.is_null() || descriptor.read() & (TX_FRAME_HE_BIT | TX_FRAME_BAR_BIT) != 0 {
        reject_txq_completion();
    }

    // `hal_mac_tx_clr_mplen` is a no-op unless this per-queue bit is set. A
    // set bit would make its linked-list walk and HE callbacks reachable.
    let mplen_state = (0x2010_4d68_usize - usize::from(queue) * 0x10) as *const u32;
    if mplen_state.read_volatile() & 0x08 != 0 {
        reject_txq_completion();
    }

    let queue_offset = usize::from(queue) * 0x7c;
    let primary = (0x2010_553c_usize - queue_offset) as *const u32;
    let secondary = (0x2010_5540_usize - queue_offset) as *const u32;
    let primary_word = primary.read_volatile();
    let secondary_word = secondary.read_volatile();

    let use_secondary = if auxiliary.is_null() {
        false
    } else {
        let completion_aux = decode_txq_completion_auxiliary(queue_offset);
        auxiliary.cast::<u32>().write(completion_aux.0);
        auxiliary.add(4).cast::<u32>().write(completion_aux.1);
        completion_aux.0 & 0x0010_0000 != 0
    };
    let status = if use_secondary {
        secondary_word
    } else {
        primary_word
    };

    completion.write(status as u8);
    completion.add(1).write((status >> 8) as u8);
    completion.add(2).write((primary_word >> 16) as u8);
    completion.add(3).write(((primary_word >> 25) & 0x03) as u8);
    let signed_metric = ((secondary_word >> 24) & 0x7f) as u8;
    completion.add(4).write(if signed_metric & 0x40 != 0 {
        signed_metric.wrapping_sub(0x80)
    } else {
        signed_metric
    });
    completion.add(5).write((secondary_word >> 16) as u8);

    0
}

#[inline(always)]
unsafe fn reject_txq_completion() -> ! {
    // The vendor caller discards this function's return value and immediately
    // interprets the completion bytes. Returning an error could therefore
    // turn an unsupported descriptor into a false success. Record the fault
    // and stop at the exact boundary instead.
    TXQ_SPLIT_FAILED.store(true, Ordering::Release);
    core::arch::asm!("ebreak", options(noreturn))
}

unsafe fn decode_txq_completion_auxiliary(queue_offset: usize) -> (u32, u32) {
    let status_534 = ((0x2010_5534_usize - queue_offset) as *const u32).read_volatile();
    let status_524 = ((0x2010_5524_usize - queue_offset) as *const u32).read_volatile();
    let status_54c = ((0x2010_554c_usize - queue_offset) as *const u32).read_volatile();

    let mut word0 = (status_534 & 0x000f_0000) << 12;
    word0 |= status_524 & 0x000f_e000;
    word0 |= status_524 & 0x0010_0000;
    word0 |= (status_524 >> 25) << 21;
    let mut word1 = (status_534 >> 20) & 0x03;
    word1 |= (status_54c >> 5) & 0x01fc;
    (word0, word1)
}

pub(crate) fn txq_split_failed() -> bool {
    TXQ_SPLIT_FAILED.load(Ordering::Acquire)
}

/// Replace the outer event-23 queue loop and indirect outcome jump table.
///
/// `__wrap_hal_mac_get_txq_state(2)` exposes at most one queue and posts a new
/// event for a captured remainder. This function therefore performs exactly
/// one fixed completion decode and one statically selected outcome call.
#[link_section = ".rwtext.wifi_strict.tx_complete_dispatch"]
#[inline(never)]
#[export_name = "__esp_wifi_strict_process_tx_complete"]
pub(crate) unsafe fn process_tx_complete() -> Result<(), LmacAsyncError> {
    let bits = __wrap_hal_mac_get_txq_state(2);
    if txq_split_failed() {
        return Err(LmacAsyncError::TxQueueSplitFailed);
    }
    if bits == 0 {
        return Ok(());
    }

    let queue = bits.trailing_zeros() as u8;
    let instances = ptr::addr_of!(our_instances_ptr).read();
    if instances.is_null() {
        return Err(LmacAsyncError::InstancesUnavailable);
    }
    let queue_state = instances.add(usize::from(queue) * TX_QUEUE_STATE_SIZE);
    if queue_state.add(TX_QUEUE_STATUS_OFFSET).read() != 1 {
        // Match the stock stale-completion branch without its formatter/log.
        #[cfg(feature = "hil-vendor-tx")]
        TX_COMPLETE_COUNTERS.stale.fetch_add(1, Ordering::Relaxed);
        hal_mac_clr_txq_state(2, queue);
        return Ok(());
    }

    let completed_frame = queue_state.cast::<*mut u8>().read();
    if completed_frame.is_null() {
        return Err(LmacAsyncError::InvalidTxSubmissionPointer);
    }
    let completed_descriptor = completed_frame
        .add(TX_FRAME_DESCRIPTOR_OFFSET)
        .cast::<*mut u8>()
        .read();
    if completed_descriptor.is_null() {
        return Err(LmacAsyncError::InvalidTxSubmissionPointer);
    }
    let aggregate = completed_descriptor.cast::<u32>().read() & TX_FRAME_AMPDU_BIT != 0;

    let mut completion = [0_u8; 6];
    let mut auxiliary = [0_u32; 2];
    __wrap_hal_mac_get_txq_complete(
        queue_state,
        queue,
        completion.as_mut_ptr(),
        auxiliary.as_mut_ptr().cast(),
    );

    let trigger_state = hal_mac_get_txq_in_trig_flow_state();
    queue_state
        .add(0x2d)
        .write(u8::from(completion[1] & 0xf0 == 0));
    queue_state
        .add(0x34)
        .write(((trigger_state >> queue) & 1) as u8);
    queue_state
        .add(0x2e)
        .write(((auxiliary[0] >> 20) & 1) as u8);
    queue_state.add(0x2f).write((auxiliary[1] >> 2) as u8);
    queue_state
        .add(0x30)
        .write(((auxiliary[0] >> 13) & 0x7f) as u8);
    queue_state
        .add(0x31)
        .write(((auxiliary[0] >> 21) & 0x7f) as u8);

    let status = completion[1] >> 4;
    #[cfg(feature = "hil-vendor-tx")]
    {
        record_tx_complete(queue_state, queue, status, completion[2]);
        #[cfg(feature = "hil-tx-deep-telemetry")]
        crate::tx_trace::record_descriptor_transition(
            crate::tx_trace::TxTraceEvent::CompletionInterrupt,
            completed_frame,
            completed_descriptor,
            tx_trace_frame_control(completed_frame),
            queue,
            completion[2],
            u32::from_le_bytes([completion[0], completion[1], completion[2], completion[3]]),
            auxiliary[0],
            auxiliary[1],
        );
    }
    #[cfg(feature = "hil-ampdu-intercept")]
    if aggregate || crate::tx_intercept::owns_direct_hardware_frame(completed_frame) {
        crate::tx_intercept::record_hardware_completion_edge();
    }

    let block_ack = if aggregate && status == 0 {
        Some(
            crate::tx_ampdu::read_ht_block_ack(queue)
                .map_err(|_| LmacAsyncError::UnsupportedTxSubmissionQueue(queue))?
                .block_ack,
        )
    } else {
        None
    };

    // `esp_test_tx_tb_complete` is diagnostic-only. The strict path omits it
    // and clears the hardware completion bit before entering the outcome.
    hal_mac_clr_txq_state(2, queue);
    if aggregate {
        if !matches!(status, 0 | 1 | 2 | 4 | 5) {
            return Err(LmacAsyncError::UnsupportedTxCompletionStatus(status));
        }
        let chain =
            take_basic_ht_ampdu_owner(queue).ok_or(LmacAsyncError::MissingTxAmpduOwner(queue))?;
        return begin_basic_ht_ampdu_completion(queue_state, chain, block_ack, completion[2]);
    }
    match status {
        0 => process_tx_success(queue_state, completion[2])?,
        1 => process_tx_rts_error(queue_state, completion[0])?,
        2 => {
            #[cfg(feature = "hil-vendor-tx")]
            let retry_frame = record_retry_before(queue_state, false);
            process_tx_retry(queue_state, BasicRetryCause::CtsTimeout)?;
            #[cfg(feature = "hil-vendor-tx")]
            record_retry_after(queue_state, retry_frame);
        }
        4 => process_tx_error(queue_state, completion[0])?,
        5 => {
            #[cfg(feature = "hil-vendor-tx")]
            let retry_frame = record_retry_before(queue_state, true);
            process_tx_retry(queue_state, BasicRetryCause::AckTimeout)?;
            #[cfg(feature = "hil-vendor-tx")]
            record_retry_after(queue_state, retry_frame);
        }
        status => return Err(LmacAsyncError::UnsupportedTxCompletionStatus(status)),
    }
    Ok(())
}

/// Replace event 24 with one collision queue per executor action.
///
/// `__wrap_hal_mac_get_txq_state(0)` exposes one bitmap bit and reposts a
/// captured remainder. Strict basic-HT keeps MPLEN disabled, so the stock
/// linked-list clear is a proven no-op; an unexpected live MPLEN state fails
/// before the queue or frame is mutated.
#[link_section = ".rwtext.wifi_strict.tx_collision_dispatch"]
#[inline(never)]
#[export_name = "__esp_wifi_strict_process_tx_collision"]
pub(crate) unsafe fn process_tx_collision() -> Result<(), LmacAsyncError> {
    let bits = __wrap_hal_mac_get_txq_state(0);
    if txq_split_failed() {
        return Err(LmacAsyncError::TxQueueSplitFailed);
    }
    if bits == 0 {
        return Ok(());
    }

    let queue = bits.trailing_zeros() as u8;
    let instances = ptr::addr_of!(our_instances_ptr).read();
    if instances.is_null() {
        return Err(LmacAsyncError::InstancesUnavailable);
    }
    let queue_state = instances.add(usize::from(queue) * TX_QUEUE_STATE_SIZE);
    if queue_state.add(TX_QUEUE_STATUS_OFFSET).read() != 1 {
        hal_mac_clr_txq_state(0, queue);
        return Ok(());
    }
    let frame = queue_state.cast::<*mut u8>().read();
    if frame.is_null() {
        return Err(LmacAsyncError::InvalidTxSubmissionPointer);
    }
    let descriptor = frame
        .add(TX_FRAME_DESCRIPTOR_OFFSET)
        .cast::<*mut u8>()
        .read();
    if descriptor.is_null() {
        return Err(LmacAsyncError::InvalidTxSubmissionPointer);
    }

    let ppdu_control =
        (TXQ_PPDU_CONTROL_BASE_REG - usize::from(queue) * TXQ_REGISTER_STRIDE) as *const u32;
    let ppdu_state = ppdu_control.read_volatile();
    if ppdu_state & 0x08 != 0 {
        return Err(LmacAsyncError::UnsupportedTxCollisionMplen(ppdu_state));
    }

    hal_mac_txq_disable(queue);
    hal_mac_clr_txq_state(0, queue);
    #[cfg(feature = "hil-vendor-tx")]
    TX_COMPLETE_COUNTERS
        .collisions
        .fetch_add(1, Ordering::Relaxed);
    process_tx_retry(queue_state, BasicRetryCause::Collision)
}

/// Recovered basic-HT ACK/CTS retry path.
///
/// The stock timeout bodies combine retry accounting, rate fallback, retry
/// limit/lifetime decisions, aggregate handling, test hooks, and the next TX
/// submission. Strict mode admits one unlinked, non-aggregate queue-kind-3
/// frame. This reproduces the accounting and decisions in Rust, sends at most
/// one frame, and routes a terminal failure through the existing one-step
/// discard continuation.
unsafe fn process_tx_retry(
    queue_state: *mut u8,
    cause: BasicRetryCause,
) -> Result<(), LmacAsyncError> {
    let queue_kind = queue_state.add(TX_QUEUE_KIND_OFFSET).read();
    if queue_kind != 3 {
        return Err(LmacAsyncError::UnsupportedTxRetryQueueKind(queue_kind));
    }
    let txop_outstanding = queue_state.add(TX_QUEUE_TXOP_OUTSTANDING_OFFSET).read();
    if txop_outstanding != 0 {
        return Err(LmacAsyncError::UnsupportedTxRetryTxop(txop_outstanding));
    }

    let frame = queue_state.cast::<*mut u8>().read();
    if frame.is_null()
        || !frame
            .add(TX_FRAME_NEXT_OFFSET)
            .cast::<*mut u8>()
            .read()
            .is_null()
    {
        return Err(LmacAsyncError::UnsupportedTxRetryChain);
    }
    let descriptor = descriptor(frame)?;
    let flags = descriptor.cast::<u32>().read();
    #[cfg(feature = "hil-tx-deep-telemetry")]
    crate::tx_trace::record_descriptor_transition(
        crate::tx_trace::TxTraceEvent::RetryDecision,
        frame,
        descriptor,
        tx_trace_frame_control(frame),
        queue_state.add(TX_QUEUE_HARDWARE_INDEX_OFFSET).read(),
        match cause {
            BasicRetryCause::CtsTimeout => 2,
            BasicRetryCause::AckTimeout => 5,
            BasicRetryCause::Collision => 1,
        },
        MAC_CLOCK_REG.read_volatile(),
        pack_four_bytes_unconditional(
            queue_state,
            TX_QUEUE_RATE_OFFSET,
            TX_QUEUE_SAVED_RATE_OFFSET,
            TX_QUEUE_SHORT_RETRY_OFFSET,
            TX_QUEUE_LONG_RETRY_OFFSET,
        ),
        u32::from(queue_state.add(TX_QUEUE_STATUS_OFFSET).read())
            | (u32::from(queue_state.add(TX_QUEUE_END_STATE_OFFSET).read()) << 8),
    );
    if flags == AP_BEACON_SUCCESS_DESCRIPTOR {
        // A beacon is a persistent broadcast object, so ACK/CTS retry has no
        // useful peer semantics. Treat a hardware-error edge as completion of
        // this one transmission: release the retained buffer and arm the next
        // Rust async TBTT instead of resubmitting a descriptor that the beacon
        // producer may already refresh in place.
        return process_tx_success(queue_state, 0x7f);
    }
    if crate::tx_proto::is_ap_group_ccmp_descriptor(flags) {
        // AP group frames are broadcast and therefore have no meaningful
        // ACK retry. A hardware error is local to this descriptor: complete
        // the existing bounded discard continuation instead of terminating
        // the radio owner or entering the unicast scheduler state.
        return discard_tx_hardware_error(queue_state, 0x7f);
    }
    if flags
        & (TX_FRAME_HE_BIT
            | TX_FRAME_BAR_BIT
            | TX_FRAME_AMPDU_BIT
            | TX_FRAME_ABORTED_BIT
            | TX_FRAME_RETRY_SCHEDULER_MASK
            | TX_FRAME_RETRY_RATE_TIME_BIT)
        != 0
    {
        return Err(LmacAsyncError::UnsupportedTxRetryDescriptor(flags));
    }
    let retry_state = pack_four_bytes_unconditional(
        queue_state,
        0x0d,
        0x0f,
        TX_QUEUE_SPECIAL_RETRY_OFFSET,
        TX_QUEUE_TRIGGER_RETRY_OFFSET,
    );
    if retry_state != 0 || queue_state.add(TX_QUEUE_TRIGGER_STATE_OFFSET).read() != 0 {
        return Err(LmacAsyncError::UnsupportedTxRetryState(retry_state));
    }

    let long_retry = match cause {
        BasicRetryCause::CtsTimeout => false,
        BasicRetryCause::AckTimeout => flags & TX_FRAME_LONG_RETRY_BIT != 0,
        BasicRetryCause::Collision => {
            flags & 0x0000_0300 == 0 && basic_frame_is_long(frame, descriptor)
        }
    };
    if long_retry {
        // `lmacProcessAckTimeout` first accounts for the successful short
        // exchange and then records a long retry failure. A collision enters
        // the long-retry body directly and therefore preserves both fields.
        if cause == BasicRetryCause::AckTimeout {
            queue_state
                .add(TX_QUEUE_RATE_OFFSET)
                .write(queue_state.add(TX_QUEUE_SAVED_RATE_OFFSET).read());
            queue_state.add(TX_QUEUE_SHORT_RETRY_OFFSET).write(0);
        }
        update_retry_rate(queue_state, TX_QUEUE_LONG_RETRY_OFFSET, lmacConfMib[0x14]);
        descriptor
            .add(7)
            .write(descriptor.add(7).read().wrapping_add(1));
        if cause == BasicRetryCause::AckTimeout {
            descriptor
                .add(5)
                .write(descriptor.add(5).read().wrapping_add(1));
        }
    } else {
        update_retry_rate(queue_state, TX_QUEUE_SHORT_RETRY_OFFSET, lmacConfMib[0x15]);
        descriptor
            .add(6)
            .write(descriptor.add(6).read().wrapping_add(1));
        // CTS timeout is a short retry but does not consume the descriptor's
        // total ACK retry budget in the pinned implementation.
        if cause == BasicRetryCause::AckTimeout {
            descriptor
                .add(5)
                .write(descriptor.add(5).read().wrapping_add(1));
        }
    }

    let reached_rate_limit = retry_rate_limit_reached(descriptor)?;
    let reached_mib_limit = if long_retry {
        descriptor.add(7).read() >= lmacConfMib[0x14]
    } else {
        descriptor.add(6).read() >= lmacConfMib[0x15]
    };
    let aged = retry_frame_aged(descriptor, flags);
    let forced_short_discard =
        flags & TX_FRAME_LONG_RETRY_BIT == 0 && flags & TX_FRAME_FORCE_SHORT_DISCARD_BIT != 0;
    if reached_rate_limit || reached_mib_limit || aged || forced_short_discard {
        queue_state.add(TX_QUEUE_STATUS_OFFSET).write(6);
        queue_state.add(TX_QUEUE_END_STATE_OFFSET).write(9);
        return begin_retry_discard(queue_state, frame);
    }

    // Both collision retry bodies call `lmacRetryTxFrame` directly. CTS/ACK
    // first mark the recovered per-frame scheduler byte.
    if cause != BasicRetryCause::Collision {
        mark_retry_scheduler(frame)?;
    }
    queue_state.add(TX_QUEUE_STATUS_OFFSET).write(3);

    // Narrow non-aggregate body of `lmacRetryTxFrame`. The bounded basic-HT
    // rate fallback is Rust-owned; only the final hardware submission remains
    // as a vendor leaf.
    let rate_context = frame
        .add(TX_FRAME_RATE_CONTEXT_OFFSET)
        .cast::<*mut u8>()
        .read();
    if rate_context.is_null() {
        return Err(LmacAsyncError::InvalidTxRetryRateControl);
    }
    select_basic_retry_rate(rate_context, descriptor)?;

    let post_rate_flags = descriptor.cast::<u32>().read();
    if post_rate_flags
        & (TX_FRAME_HE_BIT
            | TX_FRAME_BAR_BIT
            | TX_FRAME_AMPDU_BIT
            | TX_FRAME_ABORTED_BIT
            | TX_FRAME_RETRY_SCHEDULER_MASK
            | TX_FRAME_RETRY_RATE_TIME_BIT)
        != 0
    {
        return Err(LmacAsyncError::UnsupportedTxRetryDescriptor(
            post_rate_flags,
        ));
    }
    submit_basic_retry(queue_state, frame, descriptor)?;
    queue_state.add(TX_QUEUE_END_STATE_OFFSET).write(7);
    Ok(())
}

/// Bounded status-one routing recovered from `lmacProcessTxRtsError`.
///
/// Only its collision-class response values have a recoverable retry meaning.
/// The security-key error and all diagnostic/interface-specific values discard
/// exactly one frame instead of entering logging, interface callbacks, or the
/// stateful vendor completion graph.
unsafe fn process_tx_rts_error(queue_state: *mut u8, response: u8) -> Result<(), LmacAsyncError> {
    if response == 1 || (3..=5).contains(&response) || (0xa0..=0xad).contains(&response) {
        process_tx_retry(queue_state, BasicRetryCause::Collision)
    } else {
        discard_tx_hardware_error(queue_state, response)
    }
}

/// Bounded status-four routing recovered from `lmacProcessTxError`.
unsafe fn process_tx_error(queue_state: *mut u8, response: u8) -> Result<(), LmacAsyncError> {
    match response {
        0 => process_tx_retry(queue_state, BasicRetryCause::CtsTimeout),
        1 | 3..=5 => process_tx_retry(queue_state, BasicRetryCause::Collision),
        0xc0 => discard_tx_hardware_error(queue_state, response),
        _ => process_tx_retry(queue_state, BasicRetryCause::AckTimeout),
    }
}

unsafe fn discard_tx_hardware_error(
    queue_state: *mut u8,
    response: u8,
) -> Result<(), LmacAsyncError> {
    let frame = queue_state.cast::<*mut u8>().read();
    if frame.is_null()
        || !frame
            .add(TX_FRAME_NEXT_OFFSET)
            .cast::<*mut u8>()
            .read()
            .is_null()
    {
        return Err(LmacAsyncError::UnsupportedTxRetryChain);
    }
    let descriptor = descriptor(frame)?;
    let flags = descriptor.cast::<u32>().read();
    if queue_state.add(TX_QUEUE_KIND_OFFSET).read() != 3
        || queue_state.add(TX_QUEUE_TXOP_OUTSTANDING_OFFSET).read() != 0
        || flags & (TX_FRAME_HE_BIT | TX_FRAME_BAR_BIT | TX_FRAME_AMPDU_BIT) != 0
    {
        return Err(LmacAsyncError::UnsupportedTxRetryDescriptor(flags));
    }
    descriptor
        .add(TX_DESCRIPTOR_RESPONSE_OFFSET)
        .write(response);
    queue_state.add(TX_QUEUE_STATUS_OFFSET).write(6);
    queue_state.add(TX_QUEUE_END_STATE_OFFSET).write(9);
    begin_retry_discard(queue_state, frame)
}

/// Submit one strict basic-HT retry without the stock `lmacTxFrame` wrapper.
///
/// The rejected branches cover off-channel/NAN/FTM, TXOP, HE and test-only
/// paths which can log, assert, discard synchronously, or call an indirect
/// callback. The admitted path performs fixed descriptor updates and MMIO
/// writes before invoking the remaining finite PLCP/HTSIG and PHY/MMIO leaves.
unsafe fn submit_basic_retry(
    queue_state: *mut u8,
    frame: *mut u8,
    descriptor: *mut u8,
) -> Result<(), LmacAsyncError> {
    let queue_status = queue_state.add(TX_QUEUE_STATUS_OFFSET).read();
    if queue_status != 3 {
        return Err(LmacAsyncError::UnsupportedTxSubmissionQueueStatus(
            queue_status,
        ));
    }
    let hardware_queue = queue_state.add(TX_QUEUE_HARDWARE_INDEX_OFFSET).read();
    if hardware_queue > 3 {
        return Err(LmacAsyncError::UnsupportedTxSubmissionQueue(hardware_queue));
    }

    let mut flags = descriptor.cast::<u32>().read();
    let descriptor_word = descriptor.add(0x10).cast::<u32>().read();
    if flags & (TX_FRAME_OFFCHANNEL_BIT | TX_FRAME_FTM_BIT) != 0
        || !crate::tx_proto::admitted_basic_packet_kind(hardware_queue, descriptor_word, flags)
    {
        return Err(LmacAsyncError::UnsupportedTxSubmissionDescriptor(flags));
    }

    // Recovered status-three branch: status four is the pre-existing-frame
    // assertion path and is deliberately outside the strict profile.
    queue_state.cast::<*mut u8>().write(frame);

    if flags & 0x0000_2102 == 0x0000_2000 {
        flags |= 0x0000_1000;
    }
    if basic_frame_is_long(frame, descriptor) && flags & 0x02 == 0 {
        flags = (flags & !0x0000_1000) | TX_FRAME_LONG_RETRY_BIT;
    }
    if flags & 0x0000_1000 != 0 && descriptor.add(5).read() >= lmacConfMib[42] {
        flags = (flags & !0x0000_1000) | TX_FRAME_LONG_RETRY_BIT;
        descriptor.add(7).write(descriptor.add(6).read());
        descriptor.add(6).write(0);
    }
    descriptor.cast::<u32>().write(flags);

    apply_basic_rate_override(descriptor);
    configure_basic_timeout(queue_state, descriptor);
    guard_basic_ppdu_inputs(frame, descriptor)?;
    format_basic_non_he_ppdu(queue_state, frame, descriptor)?;

    configure_basic_edca(queue_state, descriptor);
    #[cfg(feature = "hil-tx-deep-telemetry")]
    crate::tx_trace::record_descriptor_transition(
        crate::tx_trace::TxTraceEvent::RetrySubmit,
        frame,
        descriptor,
        tx_trace_frame_control(frame),
        hardware_queue,
        descriptor.add(TX_DESCRIPTOR_RESPONSE_OFFSET).read(),
        txq_config_register(queue_state).read_volatile(),
        MAC_CLOCK_REG.read_volatile(),
        u32::from(descriptor.add(5).read())
            | (u32::from(descriptor.add(6).read()) << 8)
            | (u32::from(descriptor.add(7).read()) << 16),
    );
    enable_basic_tx_queue(queue_state, descriptor)?;
    Ok(())
}

/// Recovered basic-HT branch of `hal_mac_tx_set_ppdu`.
///
/// The stock wrapper contains diagnostic branches and ends in
/// `mac_tx_set_pti`, which calls the coexistence OSI table indirectly. The
/// strict branch admits only rates 16 through 35, invokes the finite PLCP and
/// HTSIG hardware-formatting leaves, reproduces the two bounded power-table
/// lookups, and programs PTI through the terminal SRAM/MMIO leaf directly.
unsafe fn format_basic_non_he_ppdu(
    queue_state: *mut u8,
    frame: *mut u8,
    descriptor: *mut u8,
) -> Result<(), LmacAsyncError> {
    let queue = queue_state.add(TX_QUEUE_HARDWARE_INDEX_OFFSET).read();
    debug_assert!(queue <= 3);
    if frame
        .add(TX_FRAME_DESCRIPTOR_OFFSET)
        .cast::<*mut u8>()
        .read()
        != descriptor
    {
        return Err(LmacAsyncError::InvalidTxSubmissionPointer);
    }
    let rate = descriptor.add(TX_DESCRIPTOR_SELECTED_RATE_OFFSET).read();
    let Some(rts_rate) = crate::tx_rate::basic_non_he_rts_rate(rate) else {
        return Err(LmacAsyncError::UnsupportedTxSubmissionDescriptor(
            descriptor.cast::<u32>().read(),
        ));
    };

    program_basic_plcp0(queue, frame, descriptor);
    program_basic_plcp1(queue, frame, descriptor, rate);

    let ppdu_control =
        (TXQ_PPDU_CONTROL_BASE_REG - usize::from(queue) * TXQ_REGISTER_STRIDE) as *mut u32;
    ppdu_control.write_volatile(ppdu_control.read_volatile() & !0x08);

    let power_table = ptr::addr_of!(s_phy_get_max_pwr).cast::<i8>();
    let rts_rate_index = usize::from(rts_rate);
    let rts_power = (power_table.add(rts_rate_index * 2).read() as i32 as u32) << 16
        | (power_table.add(rts_rate_index * 2 + 1).read() as i32 as u32) << 24;

    let data_power = if rate < 16 {
        // The queue kinds at or below two enter the vendor HW-TXOP linked-list
        // formatter. Strict one-frame completion is qualified only for the
        // ordinary kind-three queue, whose TXOP leaf merely clears fields that
        // are already zero in `basic_length_control_word`.
        let queue_kind = queue_state.add(TX_QUEUE_KIND_OFFSET).read();
        if queue_kind <= 2 {
            return Err(LmacAsyncError::UnsupportedTxSubmissionDescriptor(
                descriptor.cast::<u32>().read(),
            ));
        }
        program_basic_legacy_length(queue, descriptor, rts_rate);
        power_table.add(usize::from(rate) * 2).read() as i32 as u32
    } else {
        // HE and FTM are rejected before entering this function, so rates
        // 16..=35 are exactly the finite HTSIG path.
        program_htsig(queue, frame, descriptor, rate, rts_rate, None)?;
        let data_rate = if rate <= 25 { rate } else { rate - 10 };
        let data_rate = usize::from(data_rate);
        power_table.add(data_rate * 2).read() as i32 as u32
            | (power_table.add(data_rate * 2 + 1).read() as i32 as u32) << 8
    };
    let power_register = (TXQ_POWER_BASE_REG - usize::from(queue) * TXQ_POWER_STRIDE) as *mut u32;
    power_register.write_volatile(data_power | rts_power);

    program_basic_tx_pti(queue, descriptor)?;

    Ok(())
}

/// Recovered non-HE hardware-programming branch of `mac_tx_set_plcp0`.
///
/// The stock function also contains HE/test diagnostics and four logging
/// calls. Strict retry submission rejects HE and aggregate descriptors before
/// this point; the remaining post-protection branches only log or decode the
/// programmed PPDU and therefore have no hardware effect.
unsafe fn program_basic_plcp0(queue: u8, frame: *mut u8, descriptor: *mut u8) {
    let metadata = frame.add(4).cast::<*mut u8>().read();
    debug_assert!(!metadata.is_null());
    let flags = descriptor.cast::<u32>().read();
    debug_assert_eq!(flags & TX_FRAME_HE_BIT, 0);

    let plcp0 = crate::tx_plcp::basic_plcp0_word(metadata as usize, flags);
    let plcp0_register =
        (TXQ_ENABLE_BASE_REG - usize::from(queue) * TXQ_REGISTER_STRIDE) as *mut u32;
    plcp0_register.write_volatile(plcp0);

    // Exact finite body of `hal_he_set_tx_protection`. Its third argument is
    // unused by the pinned implementation.
    let protection_register =
        (TXQ_PROTECTION_BASE_REG - usize::from(queue) * TXQ_REGISTER_STRIDE) as *mut u32;
    let protection = protection_register.read_volatile();
    let protection = if flags & 0x0000_0100 != 0 {
        protection | 0x8000_0000
    } else {
        protection & 0x7fff_ffff
    };
    protection_register.write_volatile(protection);

    let phy_flags = descriptor
        .add(TX_DESCRIPTOR_PHY_FLAGS_OFFSET)
        .cast::<u32>()
        .read();
    if (phy_flags >> 3) & 0x03ff != 0 {
        let duration = descriptor.add(0x34).cast::<u32>().read();
        let duration_register =
            (TXQ_PROTECTION_DURATION_BASE_REG - usize::from(queue) * TXQ_POWER_STRIDE) as *mut u32;
        duration_register.write_volatile((duration & 0x0000_ffff) | 0x0001_0000);
    }
}

/// Recovered guarded non-HE body of `mac_tx_set_plcp1`.
unsafe fn program_basic_plcp1(queue: u8, frame: *mut u8, descriptor: *mut u8, rate: u8) {
    debug_assert!(rate <= 35);
    let flags = descriptor.cast::<u32>().read();
    debug_assert_eq!(flags & TX_FRAME_HE_BIT, 0);
    let queue_word_low = descriptor.add(TX_DESCRIPTOR_QUEUE_WORD_OFFSET).read();
    let protection = descriptor.add(8).cast::<u32>().read();
    let legacy_signal = if rate < 16 {
        let metadata = frame.add(4).cast::<*mut u8>().read();
        let data = metadata.add(4).cast::<*const u32>().read();
        data.read_unaligned()
    } else {
        0
    };
    let plcp1 = crate::tx_plcp::basic_non_he_plcp1_word(
        rate,
        flags,
        queue_word_low,
        protection,
        legacy_signal,
    );

    let register = (TXQ_PLCP1_BASE_REG - usize::from(queue) * TXQ_POWER_STRIDE) as *mut u32;
    register.write_volatile(plcp1);
}

/// Legacy branch of `mac_tx_set_len`. Unlike HT, it uses the fixed entry flag
/// one and does not program the HT data-length register.
unsafe fn program_basic_legacy_length(queue: u8, descriptor: *mut u8, rts_rate: u8) {
    debug_assert!(descriptor.add(TX_DESCRIPTOR_SELECTED_RATE_OFFSET).read() < 16);
    let queue_word = descriptor
        .add(TX_DESCRIPTOR_QUEUE_WORD_OFFSET)
        .cast::<u32>()
        .read();
    let length_control = crate::tx_plcp::basic_length_control_word(rts_rate, 1, queue_word);
    let register =
        (TXQ_LENGTH_CONTROL_BASE_REG - usize::from(queue) * TXQ_POWER_STRIDE) as *mut u32;
    register.write_volatile(length_control);
}

/// Recovered non-aggregate body of `mac_tx_set_htsig` and its terminal
/// `mac_tx_set_len` leaf.
#[cfg_attr(target_arch = "riscv32", link_section = ".rwtext.wifi_strict.tx_htsig")]
unsafe fn program_htsig(
    queue: u8,
    frame: *mut u8,
    descriptor: *mut u8,
    rate: u8,
    rts_rate: u8,
    aggregate_length: Option<u16>,
) -> Result<(), LmacAsyncError> {
    let flags = descriptor.cast::<u32>().read();
    let aggregate = aggregate_length.is_some();
    debug_assert_eq!(flags & TX_FRAME_HE_BIT, 0);
    debug_assert_eq!(flags & TX_FRAME_AMPDU_BIT != 0, aggregate);

    let peer = frame
        .add(TX_FRAME_RATE_CONTEXT_OFFSET)
        .cast::<*mut u8>()
        .read();
    let mut extension = false;
    let mut peer_state = 0_u16;
    if !peer.is_null() {
        let kind = peer.add(0x86).read();
        extension = kind.wrapping_sub(4) < 2;
        peer_state = peer.add(0x82).cast::<u16>().read();
    }
    if flags & 0x0000_4000 != 0 {
        extension = descriptor.add(8).cast::<u32>().read() & 0x0000_8000 != 0;
    }

    let metadata = frame.add(4).cast::<*mut u8>().read();
    debug_assert!(!metadata.is_null());
    let length_source = metadata.add(4).cast::<*const u32>().read();
    debug_assert!(!length_source.is_null());
    let length = aggregate_length
        .map(u32::from)
        .unwrap_or_else(|| length_source.read() & 0x3fff);

    let htsig = crate::tx_plcp::ht_htsig_word(rate, extension, length, aggregate);
    let htsig_register = (TXQ_HTSIG_BASE_REG - usize::from(queue) * TXQ_POWER_STRIDE) as *mut u32;
    htsig_register.write_volatile(htsig);

    let control_register =
        (TXQ_HT_CONTROL_BASE_REG - usize::from(queue) * TXQ_POWER_STRIDE) as *mut u32;
    let spatial = descriptor.add(0x2a).read();
    let coding = descriptor.add(0x2e).read();
    let mut control = control_register.read_volatile();
    control = (control & 0xffff_ff80) | u32::from(spatial & 0x7f);
    control_register.write_volatile(control);
    control = control_register.read_volatile();
    control = (control & 0xffff_c07f) | ((u32::from(coding) << 7) & 0x0000_3f80);
    control_register.write_volatile(control);
    control = control_register.read_volatile();
    control = (control & 0xffe0_3fff) | ((u32::from(spatial) << 14) & 0x01fc_0000);
    control_register.write_volatile(control);

    let protection_register =
        (TXQ_PROTECTION_BASE_REG - usize::from(queue) * TXQ_REGISTER_STRIDE) as *mut u32;
    let peer_state = u32::from(peer_state & 0x03ff);
    let mut protection = protection_register.read_volatile();
    protection = (protection & 0xffff_fc00) | peer_state;
    protection_register.write_volatile(protection);
    protection = protection_register.read_volatile();
    protection = (protection & 0xfff0_03ff) | (peer_state << 10);
    protection_register.write_volatile(protection);
    protection = protection_register.read_volatile();
    protection = (protection & 0xc00f_ffff) | (peer_state << 20);
    protection_register.write_volatile(protection);

    let queue_word = descriptor
        .add(TX_DESCRIPTOR_QUEUE_WORD_OFFSET)
        .cast::<u32>()
        .read();
    let logical_queue = ((queue_word >> 20) & 0x0f) as u8;
    // `ppCalTxAMPDULength` initializes these adjacent bytes to 0x01/0x01
    // before assembly. Carry those two values explicitly for the Rust-owned
    // aggregate instead of changing the adopted per-queue formatter policy.
    let (length_flags, data_flags) = if aggregate {
        (1, 1)
    } else {
        crate::tx_queue::ppdu_format_flags(logical_queue).ok_or(LmacAsyncError::TxRxUnavailable)?
    };
    let length_control =
        crate::tx_plcp::basic_length_control_word(rts_rate, length_flags, queue_word);
    let length_control_register =
        (TXQ_LENGTH_CONTROL_BASE_REG - usize::from(queue) * TXQ_POWER_STRIDE) as *mut u32;
    length_control_register.write_volatile(length_control);

    if flags & 0x0100_0000 == 0 {
        let data_length = crate::tx_plcp::basic_data_length_word(rate, length, data_flags);
        let data_length_register =
            (TXQ_DATA_LENGTH_BASE_REG - usize::from(queue) * TXQ_POWER_STRIDE) as *mut u32;
        data_length_register.write_volatile(data_length);
    }
    Ok(())
}

/// Submit one already assembled basic-HT A-MPDU to an idle hardware queue.
///
/// This is the finite initial-transmit branch recovered from `lmacTxFrame`,
/// `lmacSetTxFrame`, `hal_mac_tx_set_ppdu`, and `hal_mac_txq_enable`. It does
/// not enter `GetAccess`, allocate, wait, post an event, invoke a callback, or
/// traverse the vendor scheduler. The caller must have acquired coexistence
/// ownership and installed aggregate-aware completion state before calling.
///
/// This leaf is intentionally not wired into the PP dispatcher yet: the
/// ordinary completion path accepts exactly one descriptor and must never see
/// the linked chain produced by `prepare_basic_ht_ampdu_chain`.
///
/// # Safety
///
/// `queue_state` must be the SRAM state of an idle hardware queue exclusively
/// owned by the radio executor. `chain` must be the unchanged result of
/// `prepare_basic_ht_ampdu_chain`; all of its frame, descriptor, buffer, peer,
/// and metadata pointers must remain valid in SRAM through completion.
#[cfg(target_arch = "riscv32")]
#[link_section = ".rwtext.wifi_strict.tx_ampdu_submit"]
pub unsafe fn submit_basic_ht_ampdu(
    queue_state: *mut u8,
    chain: BasicHtAmpduChain,
) -> Result<(), LmacAsyncError> {
    let completion = &*AMPDU_COMPLETION.0.get();
    if completion.failed {
        return Err(LmacAsyncError::PreviousTxAmpduCompletionFailure);
    }
    if completion.active || completion.retry_count != 0 {
        return Err(LmacAsyncError::TxAmpduCompletionBusy);
    }
    if chain.subframes < 2 || chain.subframes > 32 || chain.aggregate_length == 0 {
        return Err(LmacAsyncError::UnsupportedTxAmpduChain {
            subframes: chain.subframes,
            length: chain.aggregate_length,
        });
    }
    let queue_status = queue_state.add(TX_QUEUE_STATUS_OFFSET).read();
    if queue_status != 0 {
        return Err(LmacAsyncError::UnsupportedTxSubmissionQueueStatus(
            queue_status,
        ));
    }
    let hardware_queue = queue_state.add(TX_QUEUE_HARDWARE_INDEX_OFFSET).read();
    if hardware_queue > 3 {
        return Err(LmacAsyncError::UnsupportedTxSubmissionQueue(hardware_queue));
    }
    if chain.first.is_null() || chain.last.is_null() {
        return Err(LmacAsyncError::InvalidTxSubmissionPointer);
    }

    let descriptor = chain
        .first
        .add(TX_FRAME_DESCRIPTOR_OFFSET)
        .cast::<*mut u8>()
        .read();
    if descriptor.is_null() {
        return Err(LmacAsyncError::InvalidTxSubmissionPointer);
    }
    let mut flags = descriptor.cast::<u32>().read();
    if flags & TX_FRAME_AMPDU_BIT == 0
        || flags & (TX_FRAME_HE_BIT | TX_FRAME_BAR_BIT | TX_FRAME_OFFCHANNEL_BIT | TX_FRAME_FTM_BIT)
            != 0
    {
        return Err(LmacAsyncError::UnsupportedTxSubmissionDescriptor(flags));
    }
    let descriptor_word = descriptor
        .add(TX_DESCRIPTOR_QUEUE_WORD_OFFSET)
        .cast::<u32>()
        .read();
    if !crate::tx_proto::admitted_basic_packet_kind(hardware_queue, descriptor_word, flags) {
        return Err(LmacAsyncError::UnsupportedTxSubmissionDescriptor(flags));
    }

    // Initial status-zero branch of lmacTxFrame. Aggregate length always
    // exceeds the long-frame threshold for the admitted two-or-more MPDUs.
    if flags & 0x0000_2102 == 0x0000_2000 {
        flags |= 0x0000_1000;
    }
    if basic_frame_is_long(chain.first, descriptor) && flags & 0x02 == 0 {
        flags = (flags & !0x0000_1000) | TX_FRAME_LONG_RETRY_BIT;
    }
    descriptor.cast::<u32>().write(flags);
    apply_basic_rate_override(descriptor);

    queue_state.cast::<*mut u8>().write(chain.first);
    configure_basic_timeout(queue_state, descriptor);
    guard_basic_ampdu_ppdu_inputs(&chain, descriptor)?;
    format_basic_ht_ampdu_ppdu(queue_state, &chain, descriptor)?;
    configure_basic_edca(queue_state, descriptor);
    #[cfg(feature = "hil-tx-deep-telemetry")]
    let trace_chain = (chain.first, chain.subframes, chain.aggregate_length);
    install_basic_ht_ampdu_owner(hardware_queue, chain)?;
    #[cfg(feature = "hil-tx-deep-telemetry")]
    crate::tx_trace::record_descriptor_transition(
        crate::tx_trace::TxTraceEvent::Submit,
        trace_chain.0,
        descriptor,
        tx_trace_frame_control(trace_chain.0),
        hardware_queue,
        descriptor.add(TX_DESCRIPTOR_RESPONSE_OFFSET).read(),
        txq_config_register(queue_state).read_volatile(),
        MAC_CLOCK_REG.read_volatile(),
        u32::from(trace_chain.1) | (u32::from(trace_chain.2) << 8),
    );
    match enable_basic_tx_queue(queue_state, descriptor) {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = take_basic_ht_ampdu_owner(hardware_queue);
            Err(error)
        }
    }
}

/// Submit one already prepared strict non-HE MPDU directly to an idle hardware
/// queue. Rates 0..=15 use the finite legacy PPDU branch; rates 16..=35 use HT.
///
/// This is the non-aggregate sibling of `submit_basic_ht_ampdu`. It performs
/// only bounded descriptor updates and finite SRAM/MMIO leaves; it does not
/// insert the frame into a PP list, post an event, allocate, or wait.
///
/// # Safety
///
/// `queue_state` and every pointer reachable from `frame` must remain valid
/// writable SRAM under the single radio owner until the completion edge.
#[cfg(target_arch = "riscv32")]
#[link_section = ".rwtext.wifi_strict.tx_single_submit"]
pub unsafe fn submit_basic_non_he_frame(
    queue_state: *mut u8,
    frame: *mut u8,
) -> Result<(), LmacAsyncError> {
    if frame.is_null() {
        return Err(LmacAsyncError::InvalidTxSubmissionPointer);
    }
    let queue_status = queue_state.add(TX_QUEUE_STATUS_OFFSET).read();
    if queue_status != 0 {
        return Err(LmacAsyncError::UnsupportedTxSubmissionQueueStatus(
            queue_status,
        ));
    }
    let hardware_queue = queue_state.add(TX_QUEUE_HARDWARE_INDEX_OFFSET).read();
    if hardware_queue > 3 {
        return Err(LmacAsyncError::UnsupportedTxSubmissionQueue(hardware_queue));
    }
    if !frame
        .add(TX_FRAME_NEXT_OFFSET)
        .cast::<*mut u8>()
        .read()
        .is_null()
    {
        return Err(LmacAsyncError::UnsupportedTxSubmissionDescriptor(0));
    }
    let descriptor = frame
        .add(TX_FRAME_DESCRIPTOR_OFFSET)
        .cast::<*mut u8>()
        .read();
    if descriptor.is_null() {
        return Err(LmacAsyncError::InvalidTxSubmissionPointer);
    }
    let mut flags = descriptor.cast::<u32>().read();
    if flags
        & (TX_FRAME_AMPDU_BIT
            | TX_FRAME_HE_BIT
            | TX_FRAME_BAR_BIT
            | TX_FRAME_OFFCHANNEL_BIT
            | TX_FRAME_FTM_BIT)
        != 0
    {
        return Err(LmacAsyncError::UnsupportedTxSubmissionDescriptor(flags));
    }
    let descriptor_word = descriptor
        .add(TX_DESCRIPTOR_QUEUE_WORD_OFFSET)
        .cast::<u32>()
        .read();
    if !crate::tx_proto::admitted_basic_packet_kind(hardware_queue, descriptor_word, flags) {
        return Err(LmacAsyncError::UnsupportedTxSubmissionDescriptor(flags));
    }

    if flags & 0x0000_2102 == 0x0000_2000 {
        flags |= 0x0000_1000;
    }
    if basic_frame_is_long(frame, descriptor) && flags & 0x02 == 0 {
        flags = (flags & !0x0000_1000) | TX_FRAME_LONG_RETRY_BIT;
    }
    descriptor.cast::<u32>().write(flags);
    apply_basic_rate_override(descriptor);

    queue_state.cast::<*mut u8>().write(frame);
    configure_basic_timeout(queue_state, descriptor);
    guard_basic_ppdu_inputs(frame, descriptor)?;
    format_basic_non_he_ppdu(queue_state, frame, descriptor)?;
    configure_basic_edca(queue_state, descriptor);
    #[cfg(feature = "hil-tx-deep-telemetry")]
    crate::tx_trace::record_descriptor_transition(
        crate::tx_trace::TxTraceEvent::Submit,
        frame,
        descriptor,
        tx_trace_frame_control(frame),
        hardware_queue,
        descriptor.add(TX_DESCRIPTOR_RESPONSE_OFFSET).read(),
        txq_config_register(queue_state).read_volatile(),
        MAC_CLOCK_REG.read_volatile(),
        u32::from(queue_state.add(TX_QUEUE_STATUS_OFFSET).read())
            | (u32::from(queue_state.add(TX_QUEUE_KIND_OFFSET).read()) << 8),
    );
    enable_basic_tx_queue(queue_state, descriptor)
}

/// Transfer the prepared chain into the fixed owner slot before hardware can
/// expose a completion edge. The radio executor is the sole writer; no lock,
/// allocation, compare/retry loop, or scheduler primitive is involved.
#[cfg(target_arch = "riscv32")]
#[link_section = ".rwtext.wifi_strict.tx_ampdu_owner"]
unsafe fn install_basic_ht_ampdu_owner(
    queue: u8,
    chain: BasicHtAmpduChain,
) -> Result<(), LmacAsyncError> {
    let owners = &mut *AMPDU_OWNERS.0.get();
    let owner = &mut owners[usize::from(queue)];
    if owner.is_some() {
        return Err(LmacAsyncError::TxAmpduOwnerBusy(queue));
    }
    *owner = Some(chain);
    Ok(())
}

/// Take the exact aggregate token associated with one hardware completion.
#[cfg(target_arch = "riscv32")]
#[link_section = ".rwtext.wifi_strict.tx_ampdu_owner"]
unsafe fn take_basic_ht_ampdu_owner(queue: u8) -> Option<BasicHtAmpduChain> {
    (&mut *AMPDU_OWNERS.0.get())[usize::from(queue)].take()
}

/// Start disposition of one hardware A-MPDU. The complete pointer topology is
/// validated and detached before the first MPDU can reach TX-done or retry
/// ownership. A non-success hardware outcome deliberately supplies no
/// BlockAck, making every MPDU retryable.
#[cfg(target_arch = "riscv32")]
#[link_section = ".rwtext.wifi_strict.tx_ampdu_completion"]
unsafe fn begin_basic_ht_ampdu_completion(
    queue_state: *mut u8,
    chain: BasicHtAmpduChain,
    block_ack: Option<crate::tx_ampdu::TxBlockAckBitmap>,
    response: u8,
) -> Result<(), LmacAsyncError> {
    let state = &mut *AMPDU_COMPLETION.0.get();
    if state.failed {
        return Err(LmacAsyncError::PreviousTxAmpduCompletionFailure);
    }
    if state.active || state.retry_count != 0 {
        return Err(LmacAsyncError::TxAmpduCompletionBusy);
    }

    let descriptor = chain
        .first
        .add(TX_FRAME_DESCRIPTOR_OFFSET)
        .cast::<*mut u8>()
        .read();
    if descriptor.is_null() {
        return Err(LmacAsyncError::InvalidTxSubmissionPointer);
    }
    let resume_event = queue_state.add(TX_QUEUE_HARDWARE_INDEX_OFFSET).read();
    crate::tx_ampdu::restore_basic_ht_ampdu_chain(&chain)
        .map_err(LmacAsyncError::TxAmpduRestore)?;

    queue_state.add(TX_QUEUE_STATUS_OFFSET).write(0);
    queue_state.add(TX_QUEUE_END_STATE_OFFSET).write(3);
    let completed = queue_state
        .add(TX_QUEUE_COMPLETED_COUNT_OFFSET)
        .cast::<u32>();
    completed.write(completed.read().wrapping_add(1));

    state.active = true;
    state.chain = Some(chain);
    state.block_ack = block_ack;
    state.response = response;
    state.next = 0;
    state.resume_event = resume_event;
    state.retry_take = 0;
    if let Err(error) = enqueue_ampdu_completion() {
        state.failed = true;
        state.active = false;
        return Err(error);
    }
    Ok(())
}

fn enqueue_ampdu_completion() -> Result<(), LmacAsyncError> {
    if crate::adapter::enqueue_internal_event(crate::event::PpEvent {
        kind: TX_AMPDU_COMPLETION_CONTINUATION,
        argument: ptr::null_mut(),
    }) {
        Ok(())
    } else {
        Err(LmacAsyncError::InternalQueueFull)
    }
}

pub(crate) const fn is_ampdu_completion_continuation(kind: u32) -> bool {
    kind == TX_AMPDU_COMPLETION_CONTINUATION
}

/// Advance a fixed prefix of callback-free data MPDU dispositions, or perform
/// the constant-size final ownership transition.
///
/// Four frames keep the radio-owner dispatch finite while avoiding one A-MPDU
/// continuation and one PP event-16 publication per acknowledged subframe.
/// The TX-done leaf rejects any callback-bearing descriptor before admitting
/// it to this batch.
#[cfg(target_arch = "riscv32")]
#[link_section = ".rwtext.wifi_strict.tx_ampdu_completion"]
pub(crate) unsafe fn dispatch_ampdu_completion() -> Result<(), LmacAsyncError> {
    let state = &mut *AMPDU_COMPLETION.0.get();
    if state.failed {
        return Err(LmacAsyncError::PreviousTxAmpduCompletionFailure);
    }
    if !state.active {
        return Err(LmacAsyncError::InvalidTxAmpduContinuation);
    }
    let result = dispatch_ampdu_completion_step(state);
    if result.is_err() {
        state.failed = true;
        state.active = false;
    }
    result
}

#[cfg(target_arch = "riscv32")]
unsafe fn dispatch_ampdu_completion_step(
    state: &mut AmpduCompletionState,
) -> Result<(), LmacAsyncError> {
    const COMPLETION_QUANTUM: u8 = 4;
    let subframes = state
        .chain
        .as_ref()
        .ok_or(LmacAsyncError::InvalidTxAmpduContinuation)?
        .subframes;
    let mut processed = 0_u8;
    let mut committed = false;

    while state.next < subframes && processed < COMPLETION_QUANTUM {
        let chain = state
            .chain
            .as_ref()
            .ok_or(LmacAsyncError::InvalidTxAmpduContinuation)?;
        let index = state.next;
        let frame = chain
            .frame(index)
            .ok_or(LmacAsyncError::InvalidTxAmpduContinuation)?;
        let sequence = chain
            .sequence(index)
            .ok_or(LmacAsyncError::InvalidTxAmpduContinuation)?;
        let acknowledged = state
            .block_ack
            .is_some_and(|block_ack| block_ack.acknowledges(sequence));
        if let Err(error) =
            crate::tx_ampdu::apply_basic_ht_ampdu_completion(frame, state.response, acknowledged)
        {
            if committed {
                crate::txdone::publish_callback_free_ampdu_batch()
                    .map_err(LmacAsyncError::TxDone)?;
            }
            return Err(LmacAsyncError::TxAmpduFrameCompletion(error));
        }
        state.next = state.next.wrapping_add(1);
        processed = processed.wrapping_add(1);

        if acknowledged {
            if let Err(error) = crate::txdone::commit_callback_free_ampdu_success(frame) {
                if committed {
                    crate::txdone::publish_callback_free_ampdu_batch()
                        .map_err(LmacAsyncError::TxDone)?;
                }
                return Err(LmacAsyncError::TxDone(error));
            }
            committed = true;
        } else {
            let retry_index = usize::from(state.retry_count);
            if retry_index >= crate::tx_ampdu::TX_AMPDU_SLOT_CAPACITY {
                if committed {
                    crate::txdone::publish_callback_free_ampdu_batch()
                        .map_err(LmacAsyncError::TxDone)?;
                }
                return Err(LmacAsyncError::InvalidTxAmpduContinuation);
            }
            state.retries[retry_index] = frame;
            state.retry_sequences[retry_index] = sequence;
            state.retry_count = state.retry_count.wrapping_add(1);
        }
    }

    if committed {
        crate::txdone::publish_callback_free_ampdu_batch().map_err(LmacAsyncError::TxDone)?;
    }
    if state.next >= subframes {
        let resume_event = state.resume_event;
        let retry_count = state.retry_count;
        state.chain = None;
        state.block_ack = None;
        state.active = false;
        state.next = 0;
        #[cfg(feature = "hil-ampdu-intercept")]
        crate::tx_intercept::on_hardware_completion(retry_count)
            .map_err(|_| LmacAsyncError::InternalQueueFull)?;
        if retry_count == 0 && pp_post(u32::from(resume_event), ptr::null_mut()) != 0 {
            return Err(LmacAsyncError::InternalQueueFull);
        }
        return Ok(());
    }
    enqueue_ampdu_completion()
}

/// One detached, CCMP-ready MPDU retained after a missing BlockAck bit.
/// Sequence ownership is explicit because retry aggregates need not be
/// consecutive.
pub(crate) struct BasicHtAmpduRetryFrame {
    pub(crate) frame: *mut u8,
    pub(crate) sequence: u16,
}

/// Transfer at most one pending retry MPDU to the future Rust aggregation
/// scheduler. The caller becomes the sole owner of the returned SRAM frame.
///
/// # Safety
///
/// This is a radio-executor-only ownership operation. It must not race an
/// aggregate completion or another retry consumer.
pub(crate) unsafe fn take_basic_ht_ampdu_retry() -> Option<BasicHtAmpduRetryFrame> {
    let state = &mut *AMPDU_COMPLETION.0.get();
    if state.active || state.failed || state.retry_take >= state.retry_count {
        return None;
    }
    let index = usize::from(state.retry_take);
    let retry = BasicHtAmpduRetryFrame {
        frame: state.retries[index],
        sequence: state.retry_sequences[index],
    };
    state.retries[index] = ptr::null_mut();
    state.retry_sequences[index] = 0;
    state.retry_take = state.retry_take.wrapping_add(1);
    if state.retry_take == state.retry_count {
        state.retry_take = 0;
        state.retry_count = 0;
    }
    Some(retry)
}

#[cfg(target_arch = "riscv32")]
#[link_section = ".rwtext.wifi_strict.tx_ampdu_guard"]
unsafe fn guard_basic_ampdu_ppdu_inputs(
    chain: &BasicHtAmpduChain,
    descriptor: *mut u8,
) -> Result<(), LmacAsyncError> {
    if chain
        .first
        .add(TX_FRAME_NEXT_OFFSET)
        .cast::<*mut u8>()
        .read()
        .is_null()
        || !chain
            .last
            .add(TX_FRAME_NEXT_OFFSET)
            .cast::<*mut u8>()
            .read()
            .is_null()
    {
        return Err(LmacAsyncError::InvalidTxSubmissionPointer);
    }
    let metadata = chain.first.add(4).cast::<*mut u8>().read();
    if metadata.is_null() {
        return Err(LmacAsyncError::InvalidTxSubmissionPointer);
    }
    let length_source = metadata.add(4).cast::<*mut u8>().read();
    if length_source.is_null() {
        return Err(LmacAsyncError::InvalidTxSubmissionPointer);
    }
    let metadata_flags = length_source.cast::<u32>().read();
    // Bits 0..13 are the MPDU byte length, not fixed format bits. The first
    // oracle happened to contain 1,554-byte frames (`...0612`), whose low two
    // bits are 2; real short frames legitimately exercise every residue.
    // `prepare_basic_ht_ampdu_chain` has already accounted for the required
    // four-byte padding, so only a zero encoded length is invalid here.
    if metadata_flags & 0x3fff == 0 {
        return Err(LmacAsyncError::UnsupportedTxSubmissionMetadata(
            metadata_flags,
        ));
    }

    let peer = chain
        .first
        .add(TX_FRAME_RATE_CONTEXT_OFFSET)
        .cast::<*mut u8>()
        .read();
    let flags = descriptor.cast::<u32>().read();
    if peer.is_null() && flags & 0x0000_4000 == 0 {
        return Err(LmacAsyncError::UnsupportedTxSubmissionDescriptor(flags));
    }
    Ok(())
}

#[cfg(target_arch = "riscv32")]
#[link_section = ".rwtext.wifi_strict.tx_ampdu_format"]
unsafe fn format_basic_ht_ampdu_ppdu(
    queue_state: *mut u8,
    chain: &BasicHtAmpduChain,
    descriptor: *mut u8,
) -> Result<(), LmacAsyncError> {
    let queue = queue_state.add(TX_QUEUE_HARDWARE_INDEX_OFFSET).read();
    let rate = descriptor.add(TX_DESCRIPTOR_SELECTED_RATE_OFFSET).read();
    let Some(rts_rate) = crate::tx_rate::basic_non_he_rts_rate(rate) else {
        return Err(LmacAsyncError::UnsupportedTxSubmissionDescriptor(
            descriptor.cast::<u32>().read(),
        ));
    };

    program_basic_plcp0(queue, chain.first, descriptor);
    program_basic_plcp1(queue, chain.first, descriptor, rate);
    let ppdu_control =
        (TXQ_PPDU_CONTROL_BASE_REG - usize::from(queue) * TXQ_REGISTER_STRIDE) as *mut u32;
    ppdu_control.write_volatile(ppdu_control.read_volatile() & !0x08);

    let power_table = ptr::addr_of!(s_phy_get_max_pwr).cast::<i8>();
    let rts_index = usize::from(rts_rate);
    let rts_power = (power_table.add(rts_index * 2).read() as i32 as u32) << 16
        | (power_table.add(rts_index * 2 + 1).read() as i32 as u32) << 24;
    program_htsig(
        queue,
        chain.first,
        descriptor,
        rate,
        rts_rate,
        Some(chain.aggregate_length),
    )?;
    let data_rate = usize::from(if rate <= 25 { rate } else { rate - 10 });
    let data_power = power_table.add(data_rate * 2).read() as i32 as u32
        | (power_table.add(data_rate * 2 + 1).read() as i32 as u32) << 8;
    let power_register = (TXQ_POWER_BASE_REG - usize::from(queue) * TXQ_POWER_STRIDE) as *mut u32;
    power_register.write_volatile(data_power | rts_power);
    program_basic_tx_pti(queue, descriptor)
}

/// Finite success branch of `mac_tx_set_pti` and `hal_set_tx_pti` without the
/// OSI-table callback or the remaining binary MMIO leaf.
unsafe fn program_basic_tx_pti(queue: u8, descriptor: *mut u8) -> Result<(), LmacAsyncError> {
    let original = descriptor.add(0x20).read();
    // The removed callback is `coex_core_pti_get(1, &adjusted)`. Its pinned
    // success branch is one bounded byte read from the exported 48-byte table.
    let adjusted = ptr::read_volatile(ptr::addr_of_mut!(coex_pti_tab).cast::<u8>().add(1));
    let active = original.min(adjusted);
    let count = descriptor.add(0x22).cast::<u16>().read();
    if original > 0x0f || count > 0x0fff {
        return Err(LmacAsyncError::UnsupportedTxSubmissionPti {
            priority: original,
            count,
        });
    }

    let queue_control =
        (TXQ_CONFIG_BASE_REG - usize::from(queue) * TXQ_REGISTER_STRIDE) as *mut u32;
    let mut value = queue_control.read_volatile();
    value = (value & 0x0fff_ffff) | (u32::from(active) << 28);
    queue_control.write_volatile(value);

    let pti_register = (TXQ_PTI_BASE_REG - usize::from(queue) * TXQ_POWER_STRIDE) as *mut u32;
    value = pti_register.read_volatile();
    value = (value & 0xffff_0fff) | (u32::from(original) << 12);
    pti_register.write_volatile(value);

    value = pti_register.read_volatile();
    value = (value & 0xffff_f0ff) | (u32::from(original) << 8);
    pti_register.write_volatile(value);

    value = pti_register.read_volatile();
    value = (value & 0xffff_ff0f) | (u32::from(original) << 4);
    pti_register.write_volatile(value);

    value = pti_register.read_volatile();
    value = (value & 0xfff0_ffff) | (u32::from(original) << 16);
    pti_register.write_volatile(value);

    value = pti_register.read_volatile();
    value = (value & 0x000f_ffff) | (u32::from(count) << 20);
    pti_register.write_volatile(value);
    Ok(())
}

unsafe fn basic_frame_is_long(frame: *mut u8, descriptor: *mut u8) -> bool {
    debug_assert_eq!(descriptor.cast::<u32>().read() & TX_FRAME_HE_BIT, 0);
    let length = u32::from(frame.add(20).cast::<u16>().read())
        .wrapping_add(u32::from(frame.add(22).cast::<u16>().read()));
    let threshold = u32::from(
        ptr::addr_of!(lmacConfMib)
            .cast::<u8>()
            .add(22)
            .cast::<u16>()
            .read_unaligned(),
    );
    length > threshold
}

unsafe fn apply_basic_rate_override(descriptor: *mut u8) {
    if lmacConfMib[46] == 0 {
        return;
    }
    let rate = descriptor.add(TX_DESCRIPTOR_SELECTED_RATE_OFFSET).read();
    if lmacConfMib[45] != 0 {
        let adjusted = rate.wrapping_sub(4);
        if adjusted <= 3 {
            descriptor
                .add(TX_DESCRIPTOR_SELECTED_RATE_OFFSET)
                .write(adjusted);
        }
    } else if rate.wrapping_sub(1) <= 2 {
        descriptor
            .add(TX_DESCRIPTOR_SELECTED_RATE_OFFSET)
            .write(rate.wrapping_add(4));
    }
}

#[cfg_attr(
    target_arch = "riscv32",
    link_section = ".rwtext.wifi_strict.tx_timeout_format"
)]
unsafe fn configure_basic_timeout(queue_state: *mut u8, descriptor: *mut u8) {
    let lifetime = ptr::addr_of!(lmacConfMib)
        .cast::<u8>()
        .add(8)
        .cast::<u32>()
        .read_unaligned()
        << 10;
    let now = MAC_CLOCK_REG.read_volatile();
    let timestamp = descriptor
        .add(TX_DESCRIPTOR_TIMESTAMP_OFFSET)
        .cast::<u32>()
        .read();
    let remaining = lifetime.wrapping_sub(now).wrapping_add(timestamp);
    let timeout = if remaining >= lifetime {
        10
    } else {
        (remaining >> 10).max(1)
    };

    let high = descriptor
        .add(TX_DESCRIPTOR_LENGTH_HIGH_OFFSET)
        .cast::<u32>()
        .read();
    let low = descriptor
        .add(TX_DESCRIPTOR_LENGTH_LOW_OFFSET)
        .cast::<u32>()
        .read();
    let mut hardware_timeout = (low >> 10) | (high << 22);
    if high >> 10 != 0 || hardware_timeout >= 0x1000 {
        hardware_timeout = 0x0fff;
    }
    hardware_timeout = hardware_timeout.max(timeout);

    let register = txq_config_register(queue_state);
    let current = register.read_volatile();
    register.write_volatile((current & 0xffff_f000) | hardware_timeout);
}

unsafe fn guard_basic_ppdu_inputs(
    frame: *mut u8,
    descriptor: *mut u8,
) -> Result<(), LmacAsyncError> {
    let metadata = frame.add(4).cast::<*mut u8>().read();
    if metadata.is_null() {
        return Err(LmacAsyncError::InvalidTxSubmissionPointer);
    }
    let metadata_flags = metadata.add(4).cast::<u32>().read();
    if metadata_flags == 0 || metadata_flags & 0x03 != 0 {
        return Err(LmacAsyncError::UnsupportedTxSubmissionMetadata(
            metadata_flags,
        ));
    }

    let peer = frame.add(44).cast::<*mut u8>().read();
    let flags = descriptor.cast::<u32>().read();
    if peer.is_null() && flags & 0x0000_4000 == 0 {
        return Err(LmacAsyncError::UnsupportedTxSubmissionDescriptor(flags));
    }
    if !peer.is_null()
        && peer.add(148).cast::<u32>().read() == 2
        && descriptor.add(TX_DESCRIPTOR_SELECTED_RATE_OFFSET).read() <= 7
    {
        return Err(LmacAsyncError::UnsupportedTxSubmissionDescriptor(flags));
    }
    Ok(())
}

#[cfg_attr(
    target_arch = "riscv32",
    link_section = ".rwtext.wifi_strict.tx_edca_format"
)]
unsafe fn configure_basic_edca(queue_state: *mut u8, descriptor: *mut u8) {
    let contention_window = next_backoff_random();
    let exponent = u32::from(queue_state.add(8).read());
    let mask = !u32::MAX.wrapping_shl(exponent);
    queue_state
        .add(6)
        .cast::<u16>()
        .write((contention_window & mask) as u16);

    let register = txq_config_register(queue_state);
    let mut value = register.read_volatile();
    value = (value & 0xf0ff_ffff) | (u32::from(queue_state.add(5).read() & 0x0f) << 24);
    register.write_volatile(value);

    value = register.read_volatile();
    value = (value & 0xffc0_0fff)
        | ((u32::from(queue_state.add(6).cast::<u16>().read()) & 0x03ff) << 12);
    register.write_volatile(value);

    value = register.read_volatile();
    let phy = (descriptor.add(0x10).cast::<u32>().read() >> 18) & 0x03;
    register.write_volatile((value & 0xff3f_ffff) | (phy << 22));
}

fn next_backoff_random() -> u32 {
    let mut value = BACKOFF_SEQUENCE
        .fetch_add(0x9e37_79b9, Ordering::Relaxed)
        .wrapping_add(unsafe { MAC_CLOCK_REG.read_volatile() });
    value ^= value << 13;
    value ^= value >> 17;
    value ^ (value << 5)
}

unsafe fn enable_basic_tx_queue(
    queue_state: *mut u8,
    descriptor: *mut u8,
) -> Result<(), LmacAsyncError> {
    let he_tb = descriptor.add(47).read() & 0x70;
    if he_tb == 0x30 {
        return Err(LmacAsyncError::UnsupportedTxSubmissionDescriptor(
            descriptor.cast::<u32>().read(),
        ));
    }

    let queue = queue_state.add(TX_QUEUE_HARDWARE_INDEX_OFFSET).read();
    queue_state.add(TX_QUEUE_STATUS_OFFSET).write(1);
    let register = (TXQ_ENABLE_BASE_REG - usize::from(queue) * 16) as *mut u32;
    register.write_volatile(register.read_volatile() | 0xc000_0000);
    queue_state
        .add(40)
        .write(queue_state.add(40).read() & !0x02);
    Ok(())
}

unsafe fn txq_config_register(queue_state: *mut u8) -> *mut u32 {
    let queue = queue_state.add(TX_QUEUE_HARDWARE_INDEX_OFFSET).read();
    (TXQ_CONFIG_BASE_REG - usize::from(queue) * 16) as *mut u32
}

/// Recovered non-HE body of the pinned `rcGetRate` implementation.
///
/// Strict mode has already rejected HE and aggregate descriptors. The nested
/// `rcGetSMPDURate` helper is therefore a no-op, leaving either a direct
/// per-peer rate choice or a four-entry cumulative retry table. Keeping the
/// fixed bound explicit also removes the vendor `wifi_assert` failure path.
unsafe fn select_basic_retry_rate(
    rate_context: *mut u8,
    descriptor: *mut u8,
) -> Result<(), LmacAsyncError> {
    let flags = descriptor.cast::<u32>().read();
    debug_assert_eq!(flags & (TX_FRAME_HE_BIT | TX_FRAME_AMPDU_BIT), 0);

    let mode = rate_context
        .add(TX_RATE_CONTEXT_MODE_OFFSET)
        .cast::<u16>()
        .read_unaligned();
    if mode & 0x03 != 0 {
        let offset = if flags & 0x08 != 0 {
            TX_RATE_CONTEXT_ALT_RATE_OFFSET
        } else {
            TX_RATE_CONTEXT_DEFAULT_RATE_OFFSET
        };
        descriptor
            .add(TX_DESCRIPTOR_SELECTED_RATE_OFFSET)
            .write(rate_context.add(offset).read());
        return Ok(());
    }

    let schedule = descriptor
        .add(TX_DESCRIPTOR_RATE_CONTROL_OFFSET)
        .cast::<*mut u8>()
        .read();
    if schedule.is_null() {
        return Err(LmacAsyncError::InvalidTxRetryRateControl);
    }

    let attempts = descriptor.add(5).read().max(descriptor.add(6).read());
    let mut cumulative = 0_u8;
    for index in 0..4 {
        let entry = schedule.add(index * 2);
        cumulative = cumulative.wrapping_add(entry.add(1).read());
        if attempts < cumulative {
            let mut rate = entry.read();
            let phy_flags = descriptor
                .add(TX_DESCRIPTOR_PHY_FLAGS_OFFSET)
                .cast::<u32>()
                .read();
            if phy_flags & 0x0001_0000 != 0 && rate > 35 {
                rate = 16;
            }
            descriptor
                .add(TX_DESCRIPTOR_SELECTED_RATE_OFFSET)
                .write(rate);
            break;
        }
    }
    Ok(())
}

unsafe fn update_retry_rate(queue_state: *mut u8, retry_offset: usize, limit: u8) {
    let retry = queue_state.add(retry_offset).read();
    let retry = if retry < limit {
        retry.wrapping_add(1)
    } else {
        retry
    };
    queue_state.add(retry_offset).write(retry);

    if retry >= limit {
        queue_state
            .add(TX_QUEUE_RATE_OFFSET)
            .write(queue_state.add(TX_QUEUE_SAVED_RATE_OFFSET).read());
        return;
    }
    let rate = queue_state.add(TX_QUEUE_RATE_OFFSET).read();
    let rate_limit = queue_state.add(TX_QUEUE_RATE_LIMIT_OFFSET).read();
    if rate < rate_limit {
        queue_state
            .add(TX_QUEUE_RATE_OFFSET)
            .write(rate.wrapping_add(1));
    }
}

unsafe fn retry_rate_limit_reached(descriptor: *mut u8) -> Result<bool, LmacAsyncError> {
    let attempts = descriptor.add(5).read();
    let flags = descriptor.cast::<u32>().read();
    if attempts > 4 && flags & TX_FRAME_RATE_LIMIT_BIT != 0 {
        return Ok(true);
    }
    let rate_control = descriptor
        .add(TX_DESCRIPTOR_RATE_CONTROL_OFFSET)
        .cast::<*mut u8>()
        .read();
    if rate_control.is_null() {
        return Err(LmacAsyncError::InvalidTxRetryRateControl);
    }
    // Mesh is disabled by strict configuration. The stock non-mesh branch is
    // exactly `attempts >= rate_control[8]`.
    Ok(attempts >= rate_control.add(8).read())
}

unsafe fn retry_frame_aged(descriptor: *mut u8, flags: u32) -> bool {
    debug_assert_eq!(flags & TX_FRAME_AMPDU_BIT, 0);
    let lifetime = ptr::addr_of!(lmacConfMib)
        .cast::<u8>()
        .add(8)
        .cast::<u32>()
        .read_unaligned()
        << 10;
    let now = (0x2010_d800 as *const u32).read_volatile();
    let timestamp = descriptor
        .add(TX_DESCRIPTOR_TIMESTAMP_OFFSET)
        .cast::<u32>()
        .read();
    let elapsed = now.wrapping_sub(timestamp);
    elapsed > lifetime || elapsed > lifetime.wrapping_sub(0x1400)
}

unsafe fn mark_retry_scheduler(frame: *mut u8) -> Result<(), LmacAsyncError> {
    let state = retry_scheduler_state(frame)?;
    state.add(1).write(state.add(1).read() | 0x08);
    Ok(())
}

unsafe fn retry_scheduler_state(frame: *mut u8) -> Result<*mut u8, LmacAsyncError> {
    let scheduler = frame
        .add(TX_FRAME_SCHEDULER_OFFSET)
        .cast::<*mut u8>()
        .read();
    if scheduler.is_null() {
        return Err(LmacAsyncError::InvalidTxRetryScheduler);
    }
    let mut state = scheduler.add(4).cast::<*mut u8>().read();
    if state.is_null() {
        return Err(LmacAsyncError::InvalidTxRetryScheduler);
    }
    if frame.add(TX_FRAME_LAYOUT_FLAGS_OFFSET).cast::<u16>().read() & 0x2000 != 0 {
        state = state.add(8);
    }
    Ok(state)
}

unsafe fn begin_retry_discard(queue_state: *mut u8, frame: *mut u8) -> Result<(), LmacAsyncError> {
    let state = &mut *STATE.0.get();
    if state.discard_phase != DISCARD_IDLE {
        return Err(LmacAsyncError::PreviousContinuationFailure);
    }
    state.finish_timeout_queue = false;
    begin_discard(state, queue_state, frame)
}

#[cfg(feature = "hil-vendor-tx")]
unsafe fn record_retry_before(queue_state: *mut u8, ack_timeout: bool) -> *mut u8 {
    let counters = &RETRY_COUNTERS;
    if ack_timeout {
        counters.ack_timeout.fetch_add(1, Ordering::Relaxed);
    } else {
        counters.cts_timeout.fetch_add(1, Ordering::Relaxed);
    }
    let frame = queue_state.cast::<*mut u8>().read();
    let descriptor = frame
        .add(TX_FRAME_DESCRIPTOR_OFFSET)
        .cast::<*mut u8>()
        .read();
    let flags = descriptor.cast::<u32>().read();
    let kind = queue_state.add(TX_QUEUE_KIND_OFFSET).read();
    let status = queue_state.add(TX_QUEUE_STATUS_OFFSET).read();
    if kind < 32 {
        counters
            .queue_kind_mask
            .fetch_or(1_u32 << kind, Ordering::Relaxed);
    }
    if status < 32 {
        counters
            .pre_status_mask
            .fetch_or(1_u32 << status, Ordering::Relaxed);
    }
    counters
        .descriptor_flags_or
        .fetch_or(flags, Ordering::Relaxed);
    if flags & 0x0000_0100 != 0 {
        counters.long_frame_flag.fetch_add(1, Ordering::Relaxed);
    }
    if !frame
        .add(TX_FRAME_NEXT_OFFSET)
        .cast::<*mut u8>()
        .read()
        .is_null()
    {
        counters.next_nonnull.fetch_add(1, Ordering::Relaxed);
    }
    if queue_state.add(TX_QUEUE_TXOP_OUTSTANDING_OFFSET).read() != 0 {
        counters.txop_nonzero.fetch_add(1, Ordering::Relaxed);
    }
    counters.last_pre_queue_counters.store(
        pack_four_bytes(queue_state, 8, 9, 10, 11),
        Ordering::Relaxed,
    );
    counters.last_pre_queue_state.store(
        pack_four_bytes(
            queue_state,
            12,
            TX_QUEUE_STATUS_OFFSET,
            TX_QUEUE_KIND_OFFSET,
            0x34,
        ),
        Ordering::Relaxed,
    );
    counters.last_pre_descriptor_counters.store(
        pack_four_bytes(descriptor, 5, 6, 7, TX_DESCRIPTOR_REASON_OFFSET),
        Ordering::Release,
    );
    frame
}

#[cfg(feature = "hil-vendor-tx")]
unsafe fn record_retry_after(queue_state: *mut u8, previous_frame: *mut u8) {
    let counters = &RETRY_COUNTERS;
    counters.returned.fetch_add(1, Ordering::Relaxed);
    let frame = queue_state.cast::<*mut u8>().read();
    if frame.is_null() {
        counters.detached_frame.fetch_add(1, Ordering::Relaxed);
    } else if frame == previous_frame {
        counters.same_frame.fetch_add(1, Ordering::Relaxed);
    } else {
        counters.changed_frame.fetch_add(1, Ordering::Relaxed);
    }
    let status = queue_state.add(TX_QUEUE_STATUS_OFFSET).read();
    if status < 32 {
        counters
            .post_status_mask
            .fetch_or(1_u32 << status, Ordering::Relaxed);
    }
    counters.last_post_queue_counters.store(
        pack_four_bytes(queue_state, 8, 9, 10, 11),
        Ordering::Relaxed,
    );
    counters.last_post_queue_state.store(
        pack_four_bytes(
            queue_state,
            12,
            TX_QUEUE_STATUS_OFFSET,
            TX_QUEUE_KIND_OFFSET,
            0x34,
        ),
        Ordering::Relaxed,
    );
    if !previous_frame.is_null() {
        let descriptor = previous_frame
            .add(TX_FRAME_DESCRIPTOR_OFFSET)
            .cast::<*mut u8>()
            .read();
        if !descriptor.is_null() {
            counters.last_post_descriptor_counters.store(
                pack_four_bytes(descriptor, 5, 6, 7, TX_DESCRIPTOR_REASON_OFFSET),
                Ordering::Release,
            );
        }
    }
}

#[cfg(feature = "hil-vendor-tx")]
unsafe fn pack_four_bytes(pointer: *mut u8, a: usize, b: usize, c: usize, d: usize) -> u32 {
    pack_four_bytes_unconditional(pointer, a, b, c, d)
}

unsafe fn pack_four_bytes_unconditional(
    pointer: *mut u8,
    a: usize,
    b: usize,
    c: usize,
    d: usize,
) -> u32 {
    u32::from(pointer.add(a).read())
        | (u32::from(pointer.add(b).read()) << 8)
        | (u32::from(pointer.add(c).read()) << 16)
        | (u32::from(pointer.add(d).read()) << 24)
}

/// Recovered basic-HT success path for the strict one-descriptor profile.
///
/// The stock `lmacProcessTxSuccess` first selects optional TXOP/list handling,
/// then converges through `lmacEndFrameExchangeSequence`, `lmacRecycleMPDU`,
/// and `lmacTxDone`. Hardware stress proved that ordinary STA traffic uses
/// queue kind 3 with no TXOP ownership, linked MPDU, or aggregate descriptor
/// state. Rejecting those invariants before mutation keeps the remaining path
/// finite and hands the frame directly to the existing Rust TX-done steps.
/// WPA2 AP group data uses the measured `0x200b` classify state: the same
/// finite RTS-threshold test selects one retry-byte clear, and the vendor leaf
/// publishes response `0x7f` before converging on the common completion path.
unsafe fn process_tx_success(queue_state: *mut u8, response: u8) -> Result<(), LmacAsyncError> {
    let queue_kind = queue_state.add(TX_QUEUE_KIND_OFFSET).read();
    if queue_kind != 3 {
        return Err(LmacAsyncError::UnsupportedTxSuccessQueueKind(queue_kind));
    }
    let txop_outstanding = queue_state.add(TX_QUEUE_TXOP_OUTSTANDING_OFFSET).read();
    if txop_outstanding != 0 {
        return Err(LmacAsyncError::UnsupportedTxSuccessTxop(txop_outstanding));
    }

    let frame = queue_state.cast::<*mut u8>().read();
    if !frame
        .add(TX_FRAME_NEXT_OFFSET)
        .cast::<*mut u8>()
        .read()
        .is_null()
    {
        return Err(LmacAsyncError::UnsupportedTxSuccessChain);
    }
    let descriptor = frame
        .add(TX_FRAME_DESCRIPTOR_OFFSET)
        .cast::<*mut u8>()
        .read();
    let flags = descriptor.cast::<u32>().read();
    let ap_beacon = flags == AP_BEACON_SUCCESS_DESCRIPTOR;
    let classified_ap_group = crate::tx_proto::is_ap_group_ccmp_descriptor(flags);
    if !ap_beacon
        && !classified_ap_group
        && flags & (TX_SUCCESS_CLASSIFY_MASK | TX_SUCCESS_AGGREGATE_STATE_MASK) != 0
    {
        return Err(LmacAsyncError::UnsupportedTxSuccessDescriptor(flags));
    }
    if classified_ap_group && flags & TX_SUCCESS_AGGREGATE_STATE_MASK != 0 {
        return Err(LmacAsyncError::UnsupportedTxSuccessDescriptor(flags));
    }

    if !ap_beacon {
        // Non-HE `lmacProcessShortFrameSuccess`: copy the saved retry/rate
        // byte and clear the short-frame state. Bit 8 additionally runs the
        // matching long-frame success leaf, which only clears the adjacent
        // state byte. Broadcast beacons have no ACK/retry state to update.
        queue_state.add(8).write(queue_state.add(9).read());
        if classified_ap_group && basic_frame_is_long(frame, descriptor) {
            queue_state.add(0x0c).write(0);
        } else {
            queue_state.add(0x0b).write(0);
            if flags & 0x0000_0100 != 0 {
                queue_state.add(0x0c).write(0);
            }
        }
    }
    descriptor
        .add(TX_DESCRIPTOR_RESPONSE_OFFSET)
        .write(if classified_ap_group { 0x7f } else { response });

    // Basic non-aggregate convergence from `lmacEndFrameExchangeSequence`
    // and `lmacRecycleMPDU`.
    queue_state.add(TX_QUEUE_STATUS_OFFSET).write(0);
    queue_state.add(TX_QUEUE_END_STATE_OFFSET).write(3);
    let completed = queue_state
        .add(TX_QUEUE_COMPLETED_COUNT_OFFSET)
        .cast::<u32>();
    completed.write(completed.read().wrapping_add(1));
    descriptor.add(TX_DESCRIPTOR_REASON_OFFSET).write(1);
    if ap_beacon {
        return crate::txdone::complete_ap_beacon_success(frame).map_err(LmacAsyncError::TxDone);
    }
    #[cfg(feature = "hil-ampdu-intercept")]
    if crate::tx_intercept::owns_direct_hardware_frame(frame) {
        return crate::txdone::begin_from_intercept_success(frame).map_err(LmacAsyncError::TxDone);
    }
    crate::txdone::begin_from_tx_success(
        frame,
        queue_state.add(TX_QUEUE_HARDWARE_INDEX_OFFSET).read(),
    )
    .map_err(LmacAsyncError::TxDone)
}

#[cfg(feature = "hil-vendor-tx")]
unsafe fn record_tx_complete(queue_state: *mut u8, queue: u8, status: u8, response: u8) {
    let counters = &TX_COMPLETE_COUNTERS;
    counters.completions.fetch_add(1, Ordering::Relaxed);
    match status {
        0 | 1 | 2 | 4 | 5 => {
            counters.outcomes[usize::from(status)].fetch_add(1, Ordering::Relaxed);
        }
        _ => {
            counters.unexpected_status.fetch_add(1, Ordering::Relaxed);
        }
    }
    if status != 0 {
        return;
    }

    #[cfg(not(feature = "hil-tx-deep-telemetry"))]
    {
        let _ = (queue_state, queue, response);
        return;
    }

    #[cfg(feature = "hil-tx-deep-telemetry")]
    record_tx_complete_details(counters, queue_state, queue, response);
}

#[cfg(all(feature = "hil-vendor-tx", feature = "hil-tx-deep-telemetry"))]
unsafe fn record_tx_complete_details(
    counters: &TxCompleteCounters,
    queue_state: *mut u8,
    queue: u8,
    response: u8,
) {
    let queue_kind = queue_state.add(TX_QUEUE_KIND_OFFSET).read();
    let txop_outstanding = queue_state.add(TX_QUEUE_TXOP_OUTSTANDING_OFFSET).read();
    let frame = queue_state.cast::<*mut u8>().read();
    let next = frame.add(TX_FRAME_NEXT_OFFSET).cast::<*mut u8>().read();
    let descriptor = frame
        .add(TX_FRAME_DESCRIPTOR_OFFSET)
        .cast::<*mut u32>()
        .read();
    let descriptor_flags = descriptor.read();

    counters
        .success_queue_mask
        .fetch_or(1_u32 << queue, Ordering::Relaxed);
    if queue_kind < 32 {
        counters
            .success_queue_kind_mask
            .fetch_or(1_u32 << queue_kind, Ordering::Relaxed);
    }
    if txop_outstanding != 0 {
        counters
            .success_txop_nonzero
            .fetch_add(1, Ordering::Relaxed);
    }
    counters
        .success_txop_max
        .fetch_max(u32::from(txop_outstanding), Ordering::Relaxed);
    if !next.is_null() {
        counters
            .success_next_nonnull
            .fetch_add(1, Ordering::Relaxed);
    }
    counters
        .success_descriptor_flags_or
        .fetch_or(descriptor_flags, Ordering::Relaxed);
    counters
        .last_queue
        .store(u32::from(queue), Ordering::Relaxed);
    counters
        .last_queue_kind
        .store(u32::from(queue_kind), Ordering::Relaxed);
    counters
        .last_txop_outstanding
        .store(u32::from(txop_outstanding), Ordering::Relaxed);
    counters
        .last_response
        .store(u32::from(response), Ordering::Relaxed);
    counters
        .last_descriptor_flags
        .store(descriptor_flags, Ordering::Release);
}

pub(crate) fn runtime_tx_link_wrappers_active() -> bool {
    core::ptr::eq(
        hal_mac_get_txq_state as *const (),
        __wrap_hal_mac_get_txq_state as *const (),
    ) && core::ptr::eq(
        vendor_hal_mac_get_txq_complete as *const (),
        __wrap_hal_mac_get_txq_complete as *const (),
    ) && core::ptr::eq(
        lmacTxDone as *const (),
        crate::txdone::__wrap_lmacTxDone as *const (),
    ) && crate::txdone::runtime_callback_link_wrappers_active()
}

/// Start the async replacement for PP event 22 (`lmacProcessTxTimeout`).
///
/// Each active TX queue becomes a separate two-phase continuation around the
/// original 16-us hardware settling interval. Repeated timeout events are
/// coalesced and re-sample hardware state after the active pass completes.
///
/// # Safety
/// Must run under the single radio owner with the pinned S31 archive. The
/// hardware adapter must have installed the executor-driven timer clock.
pub unsafe fn begin_tx_timeout() -> Result<(), LmacAsyncError> {
    let state = &mut *STATE.0.get();
    if state.failed {
        return Err(LmacAsyncError::PreviousContinuationFailure);
    }
    if state.active {
        state.pending = true;
        return Ok(());
    }
    start_pass(state)
}

unsafe fn start_pass(state: &mut TxTimeoutState) -> Result<(), LmacAsyncError> {
    // `hal_mac_get_txq_state(1)` is just this field plus optional test/log
    // hooks. Calling it would make logging callbacks reachable in strict mode.
    state.remaining = (TXQ_INTERRUPT_STATE_REG.read_volatile() >> 16) & 0x0f;
    if state.remaining == 0 {
        state.active = false;
        state.pending = false;
        return Ok(());
    }
    state.active = true;
    arm_next_queue(state)
}

unsafe fn arm_next_queue(state: &mut TxTimeoutState) -> Result<(), LmacAsyncError> {
    let queue = state.remaining.trailing_zeros() as u8;
    state.remaining &= !(1u32 << queue);
    state.current_queue = queue;

    hal_mac_tx_set_cca(3);
    if !schedule_internal_timer(
        TIMER.0.get().cast(),
        tx_disable_settled,
        ptr::null_mut(),
        TX_DISABLE_SETTLE_US,
    ) {
        state.failed = true;
        state.active = false;
        hal_mac_tx_set_cca(0);
        return Err(LmacAsyncError::TimerUnavailable);
    }
    Ok(())
}

unsafe extern "C" fn tx_disable_settled(_argument: *mut c_void) {
    let state = &mut *STATE.0.get();
    match finish_queue(state, state.current_queue) {
        Ok(true) => finish_current_queue(state),
        Ok(false) => {}
        Err(_) => fail(state),
    }
}

unsafe fn finish_current_queue(state: &mut TxTimeoutState) {
    TXQ_INTERRUPT_CLEAR_REG.write_volatile(1u32 << (state.current_queue + 16));

    let result = if state.remaining != 0 {
        arm_next_queue(state)
    } else if state.pending {
        state.pending = false;
        start_pass(state)
    } else {
        state.active = false;
        Ok(())
    };
    if result.is_err() {
        fail(state);
    }
}

unsafe fn fail(state: &mut TxTimeoutState) {
    state.failed = true;
    state.active = false;
    state.discard_phase = DISCARD_IDLE;
    state.finish_timeout_queue = false;
}

unsafe fn finish_queue(state: &mut TxTimeoutState, queue: u8) -> Result<bool, LmacAsyncError> {
    let instances = ptr::addr_of!(our_instances_ptr).read();
    if instances.is_null() {
        hal_mac_tx_set_cca(0);
        return Err(LmacAsyncError::InstancesUnavailable);
    }

    let queue_state = instances.add(usize::from(queue) * TX_QUEUE_STATE_SIZE);
    let frame = queue_state.cast::<*mut u8>().read();
    let was_valid = hal_mac_is_txq_valid(queue) != 0;
    hal_mac_set_txq_invalid(queue);
    hal_mac_tx_set_cca(0);

    if was_valid {
        hal_mac_txq_disable(queue);
        queue_state.add(TX_QUEUE_STATUS_OFFSET).write(6);
        if !frame.is_null() {
            let descriptor = frame
                .add(TX_FRAME_DESCRIPTOR_OFFSET)
                .cast::<*mut u8>()
                .read();
            if descriptor.is_null() {
                return Err(LmacAsyncError::InvalidDiscardContinuation);
            }
            if descriptor.cast::<u32>().read() & TX_FRAME_AMPDU_BIT != 0 {
                // A hardware timeout has no BlockAck. Restore the statically
                // owned chain and feed every MPDU into the same one-frame-per-
                // event retry continuation used by a missing BlockAck bit.
                // Returning `true` lets `finish_current_queue` clear the
                // timeout interrupt and advance the finite queue bitmap.
                let chain = take_basic_ht_ampdu_owner(queue)
                    .ok_or(LmacAsyncError::MissingTxAmpduOwner(queue))?;
                begin_basic_ht_ampdu_completion(queue_state, chain, None, 0)?;
                return Ok(true);
            }
            state.finish_timeout_queue = true;
            begin_discard(state, queue_state, frame)?;
            return Ok(false);
        }
    } else if !frame.is_null() {
        let descriptor = frame
            .add(TX_FRAME_DESCRIPTOR_OFFSET)
            .cast::<*mut u32>()
            .read();
        if !descriptor.is_null() {
            descriptor.write(descriptor.read() | TX_FRAME_ABORTED_BIT);
        }
    }
    Ok(true)
}

unsafe fn begin_discard(
    state: &mut TxTimeoutState,
    queue_state: *mut u8,
    frame: *mut u8,
) -> Result<(), LmacAsyncError> {
    queue_state.add(TX_QUEUE_STATUS_OFFSET).write(0);
    queue_state.cast::<*mut u8>().write(ptr::null_mut());

    let descriptor = frame
        .add(TX_FRAME_DESCRIPTOR_OFFSET)
        .cast::<*mut u8>()
        .read();
    if descriptor.is_null() {
        return Err(LmacAsyncError::InvalidDiscardContinuation);
    }
    let flags = descriptor.cast::<u32>().read();
    if flags & (TX_FRAME_BAR_BIT | TX_FRAME_AMPDU_BIT) != 0 {
        return Err(LmacAsyncError::UnsupportedAggregatedFrame(flags));
    }

    let retry_count = descriptor.add(6).read();
    let ack_count = descriptor.add(7).read();
    state.discard_reason = if retry_count >= lmacConfMib[0x15] {
        2
    } else if ack_count >= lmacConfMib[0x14] {
        3
    } else {
        4
    };
    state.queue_state = queue_state;
    state.discard_frame = frame;

    let next = frame.add(TX_FRAME_NEXT_OFFSET).cast::<*mut u8>().read();
    if queue_state.add(TX_QUEUE_KIND_OFFSET).read() <= 2 && !next.is_null() {
        state.discard_tail = next;
        state.discard_phase = DISCARD_FIND_TAIL;
    } else {
        state.discard_tail = ptr::null_mut();
        state.discard_phase = DISCARD_FRAME;
    }
    enqueue_discard_continuation()
}

fn enqueue_discard_continuation() -> Result<(), LmacAsyncError> {
    if crate::adapter::enqueue_internal_event(crate::event::PpEvent {
        kind: TX_DISCARD_CONTINUATION,
        argument: ptr::null_mut(),
    }) {
        Ok(())
    } else {
        Err(LmacAsyncError::InternalQueueFull)
    }
}

pub(crate) const fn is_continuation(kind: u32) -> bool {
    kind == TX_DISCARD_CONTINUATION
}

/// Advance at most one pointer or one discarded MSDU. This turns both loops
/// recovered from `lmacDiscardMSDU` into executor-visible continuations.
pub(crate) unsafe fn dispatch_continuation() -> Result<(), LmacAsyncError> {
    let state = &mut *STATE.0.get();
    let result = match state.discard_phase {
        DISCARD_FIND_TAIL => find_tail_step(state),
        DISCARD_FRAME => discard_frame_step(state),
        DISCARD_WAIT_TX_DONE => finish_discard_frame_step(state),
        _ => Err(LmacAsyncError::InvalidDiscardContinuation),
    };
    if result.is_err() {
        fail(state);
    }
    result
}

unsafe fn find_tail_step(state: &mut TxTimeoutState) -> Result<(), LmacAsyncError> {
    let next = state
        .discard_tail
        .add(TX_FRAME_NEXT_OFFSET)
        .cast::<*mut u8>()
        .read();
    if !next.is_null() {
        state.discard_tail = next;
        return enqueue_discard_continuation();
    }

    let descriptor = descriptor(state.discard_frame)?;
    let queue = descriptor_queue(descriptor);
    let chain_head = state
        .discard_frame
        .add(TX_FRAME_NEXT_OFFSET)
        .cast::<*mut u8>()
        .read();
    crate::tx_queue::requeue_logical_chain_front(queue, chain_head, state.discard_tail)
        .map_err(|_| LmacAsyncError::TxRxUnavailable)?;

    state.discard_phase = DISCARD_FRAME;
    enqueue_discard_continuation()
}

unsafe fn discard_frame_step(state: &mut TxTimeoutState) -> Result<(), LmacAsyncError> {
    let frame = state.discard_frame;
    let queue_state = state.queue_state;
    let count = queue_state.add(TX_QUEUE_DROP_COUNT_OFFSET).cast::<u32>();
    count.write(count.read().wrapping_add(1));

    let descriptor = descriptor(frame)?;
    descriptor
        .add(TX_DESCRIPTOR_REASON_OFFSET)
        .write(state.discard_reason);
    state.discard_phase = DISCARD_WAIT_TX_DONE;
    crate::txdone::begin_from_lmac(frame).map_err(LmacAsyncError::TxDone)
}

pub(crate) fn resume_after_tx_done() -> Result<(), LmacAsyncError> {
    enqueue_discard_continuation()
}

unsafe fn finish_discard_frame_step(state: &mut TxTimeoutState) -> Result<(), LmacAsyncError> {
    let frame = state.discard_frame;
    let queue_state = state.queue_state;
    #[cfg(feature = "hil-ampdu-intercept")]
    if crate::tx_intercept::owns_direct_hardware_frame(frame) {
        state.discard_phase = DISCARD_IDLE;
        state.queue_state = ptr::null_mut();
        state.discard_frame = ptr::null_mut();
        state.discard_tail = ptr::null_mut();
        let finish_timeout_queue = state.finish_timeout_queue;
        state.finish_timeout_queue = false;
        crate::tx_intercept::on_direct_hardware_completion()
            .map_err(|_| LmacAsyncError::InternalQueueFull)?;
        if finish_timeout_queue {
            finish_current_queue(state);
        }
        return Ok(());
    }
    let descriptor = descriptor(frame)?;
    let flags = descriptor.cast::<u32>().read();
    let queue = descriptor_queue(descriptor);
    if flags & TX_FRAME_DEQUEUE_MASK == TX_FRAME_DEQUEUE_VALUE {
        let next = crate::tx_queue::dequeue_logical_queue(queue)
            .map_err(|_| LmacAsyncError::TxRxUnavailable)?;
        if !next.is_null() {
            state.discard_frame = next;
            state.discard_phase = DISCARD_FRAME;
            return enqueue_discard_continuation();
        }
    } else {
        // The strict TX-done prefix has already posted event 16. Release TXOP
        // and queue TX processing behind that event instead of tail-calling
        // the vendor dispatcher synchronously. The stock path posts the
        // descriptor's logical queue number here because its `ppProcessTxQ`
        // dispatcher owns the complete logical mapping. Our recovered async
        // dispatcher accepts hardware events 0..=3 and performs that mapping
        // in `tx_queue::select_logical_queue`, so posting logical WMM queue 10
        // would enter the fatal/default `ppTask` arm. Resume the hardware queue
        // which raised this timeout instead.
        if queue_state.add(TX_QUEUE_KIND_OFFSET).read() <= 2 {
            crate::tx_queue::release_txop_queue(queue).map_err(LmacAsyncError::TxopQueue)?;
        }
        if pp_post(u32::from(state.current_queue), ptr::null_mut()) != 0 {
            return Err(LmacAsyncError::InternalQueueFull);
        }
    }

    state.discard_phase = DISCARD_IDLE;
    state.queue_state = ptr::null_mut();
    state.discard_frame = ptr::null_mut();
    state.discard_tail = ptr::null_mut();
    let finish_timeout_queue = state.finish_timeout_queue;
    state.finish_timeout_queue = false;
    if finish_timeout_queue {
        finish_current_queue(state);
    }
    Ok(())
}

unsafe fn descriptor(frame: *mut u8) -> Result<*mut u8, LmacAsyncError> {
    let descriptor = frame
        .add(TX_FRAME_DESCRIPTOR_OFFSET)
        .cast::<*mut u8>()
        .read();
    if descriptor.is_null() {
        Err(LmacAsyncError::InvalidDiscardContinuation)
    } else {
        Ok(descriptor)
    }
}

#[cfg(feature = "hil-tx-deep-telemetry")]
#[link_section = ".rwtext.wifi_strict.tx_trace_frame_control"]
unsafe fn tx_trace_frame_control(frame: *mut u8) -> u16 {
    if frame.is_null() {
        return 0;
    }
    let buffer = frame.add(4).cast::<*mut u8>().read();
    if buffer.is_null() {
        return 0;
    }
    let mut data = buffer.add(4).cast::<*mut u8>().read();
    if data.is_null() {
        return 0;
    }
    if frame
        .add(TX_FRAME_LAYOUT_FLAGS_OFFSET)
        .cast::<u16>()
        .read_unaligned()
        & 0x2000
        != 0
    {
        data = data.add(8);
    }
    data.cast::<u16>().read_unaligned()
}

unsafe fn descriptor_queue(descriptor: *mut u8) -> u8 {
    ((descriptor
        .add(TX_DESCRIPTOR_QUEUE_WORD_OFFSET)
        .cast::<u32>()
        .read()
        >> 20)
        & 0x0f) as u8
}
