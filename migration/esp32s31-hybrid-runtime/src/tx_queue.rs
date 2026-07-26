//! Strict TX-queue processing boundary.
//!
//! Hardware observation first narrowed the pinned `ppProcessTxQ` state machine
//! to one basic MPDU. Reverse engineering then established that its event is a
//! hardware queue while the descriptor contains one of sixteen logical queues.
//! Per-hardware-queue bitmaps and cursors map between the two. Strict handoff
//! adopts the four fixed vendor masks once, then all sixteen intrusive queue
//! heads, tails, and rotation cursors live in one Rust-owned single-hart state.

use crate::tx_queue_state::{
    select_ready_logical_queue, LogicalQueue, TxopQueueState, TX_FRAME_NEXT_OFFSET,
};
#[cfg(feature = "hil-vendor-tx")]
use core::sync::atomic::{AtomicU32, Ordering};
use core::{
    cell::UnsafeCell,
    ptr,
    sync::atomic::{AtomicBool, Ordering as AtomicOrdering},
};

#[cfg(feature = "hil-vendor-tx")]
const TX_QUEUE_HARDWARE_INDEX_OFFSET: usize = 0x04;
const TX_QUEUE_STATE_SIZE: usize = 0x38;
const TX_QUEUE_STATUS_OFFSET: usize = 0x12;
const TX_QUEUE_KIND_OFFSET: usize = 0x1d;
const TX_FRAME_DESCRIPTOR_OFFSET: usize = 0x34;
const TX_FRAME_LAYOUT_FLAGS_OFFSET: usize = 0x24;
#[cfg(feature = "hil-vendor-tx")]
const TX_DESCRIPTOR_SELECTED_RATE_OFFSET: usize = 0x0c;
const TX_DESCRIPTOR_QUEUE_WORD_OFFSET: usize = 0x10;
const TXRX_QUEUE_SIZE: usize = 0x34;
const TXRX_QUEUE_HEAD_OFFSET: usize = 0x20;
const TXRX_QUEUE_TAIL_LINK_OFFSET: usize = 0x24;
const TXRX_QUEUE_BUSY_OFFSET: usize = 0x29;
const TXRX_HARDWARE_MASKS_OFFSET: usize = 0x04;
const TXRX_HARDWARE_CURSORS_OFFSET: usize = 0x18;
// The pinned HT formatter reads these bytes relative to the selected logical
// entry. Their individual bit meanings are not yet known; preserve the exact
// initialized values as opaque PLCP inputs instead of naming inferred fields.
const TXRX_PPDU_LENGTH_FLAGS_OFFSET: usize = 0x40;
const TXRX_PPDU_DATA_FLAGS_OFFSET: usize = 0x41;
const LOGICAL_QUEUE_COUNT: usize = 16;
const HARDWARE_QUEUE_COUNT: usize = 4;

unsafe extern "C" {
    static mut our_instances_ptr: *mut u8;
    static mut pTxRx: *mut u8;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TxQueueStateAdoptionError {
    TxRxUnavailable,
    InstancesUnavailable,
    QueueNotEmpty(u8),
    InvalidEmptyTailLink(u8),
    QueueBusy(u8),
    InvalidTxopClass { queue: u8, class: u8 },
}

struct StrictTxQueueState {
    hardware_masks: [u16; HARDWARE_QUEUE_COUNT],
    cursors: [u8; HARDWARE_QUEUE_COUNT],
    ppdu_length_flags: [u8; LOGICAL_QUEUE_COUNT],
    ppdu_data_flags: [u8; LOGICAL_QUEUE_COUNT],
    queues: [LogicalQueue; LOGICAL_QUEUE_COUNT],
}

impl StrictTxQueueState {
    const fn empty() -> Self {
        Self {
            hardware_masks: [0; HARDWARE_QUEUE_COUNT],
            cursors: [0; HARDWARE_QUEUE_COUNT],
            ppdu_length_flags: [0; LOGICAL_QUEUE_COUNT],
            ppdu_data_flags: [0; LOGICAL_QUEUE_COUNT],
            queues: [LogicalQueue::empty(); LOGICAL_QUEUE_COUNT],
        }
    }
}

struct StrictTxQueueStateCell(UnsafeCell<StrictTxQueueState>);

// All mutable access is restricted to the adopted strict Wi-Fi hart. The
// release/acquire adoption edge publishes the initialized state to that hart.
unsafe impl Sync for StrictTxQueueStateCell {}

#[link_section = ".critical.bss.wifi_strict.tx_queue_state"]
static STRICT_TX_QUEUE_STATE: StrictTxQueueStateCell =
    StrictTxQueueStateCell(UnsafeCell::new(StrictTxQueueState::empty()));
static STRICT_TX_QUEUE_STATE_ADOPTED: AtomicBool = AtomicBool::new(false);

#[repr(transparent)]
struct TxopQueueStateCell(UnsafeCell<TxopQueueState>);

// The three bytes are both the typed Rust allocator state and the object
// published through `g_txop_queue_status_ptr`. The pinned vendor object used
// the same `[1, 1, 1]` representation, so no duplicate compatibility mirror
// exists and no synchronization between C and Rust state is required.
unsafe impl Sync for TxopQueueStateCell {}

#[no_mangle]
#[used]
#[link_section = ".critical.data.wifi_strict.txop_queue_status"]
static wifi_strict_txop_queue_status: TxopQueueStateCell =
    TxopQueueStateCell(UnsafeCell::new(TxopQueueState::all_available()));

pub(crate) fn txop_queue_status_abi_ptr() -> *mut u8 {
    wifi_strict_txop_queue_status.0.get().cast::<u8>()
}

/// Adopt only the finite TX scheduler policy from the vendor `pTxRx` object.
///
/// The handoff is intentionally fail-closed: no frame or busy queue may cross
/// the ownership edge. The initialized scheduler masks, cursors, and two PPDU
/// format bytes per logical queue are copied into explicit Rust state.
///
/// # Safety
///
/// Wi-Fi initialization must be quiescent and no TX producer may run until
/// this function returns and the strict radio owner is armed.
#[cfg(target_arch = "riscv32")]
pub(crate) unsafe fn adopt_vendor_tx_queue_state() -> Result<(), TxQueueStateAdoptionError> {
    if STRICT_TX_QUEUE_STATE_ADOPTED.load(AtomicOrdering::Acquire) {
        return Ok(());
    }
    let txrx = ptr::addr_of!(pTxRx).read();
    if txrx.is_null() {
        return Err(TxQueueStateAdoptionError::TxRxUnavailable);
    }
    let instances = ptr::addr_of!(our_instances_ptr).read();
    if instances.is_null() {
        return Err(TxQueueStateAdoptionError::InstancesUnavailable);
    }

    let state = &mut *STRICT_TX_QUEUE_STATE.0.get();
    let mut logical = 0_u8;
    while usize::from(logical) < LOGICAL_QUEUE_COUNT {
        let entry = txrx.add(usize::from(logical) * TXRX_QUEUE_SIZE);
        let head_slot = entry.add(TXRX_QUEUE_HEAD_OFFSET).cast::<*mut u8>();
        if !head_slot.read().is_null() {
            return Err(TxQueueStateAdoptionError::QueueNotEmpty(logical));
        }
        let tail_link = entry
            .add(TXRX_QUEUE_TAIL_LINK_OFFSET)
            .cast::<*mut *mut u8>()
            .read();
        if tail_link != head_slot {
            return Err(TxQueueStateAdoptionError::InvalidEmptyTailLink(logical));
        }
        if entry.add(TXRX_QUEUE_BUSY_OFFSET).read() != 0 {
            return Err(TxQueueStateAdoptionError::QueueBusy(logical));
        }
        state.ppdu_length_flags[usize::from(logical)] =
            entry.add(TXRX_PPDU_LENGTH_FLAGS_OFFSET).read();
        state.ppdu_data_flags[usize::from(logical)] = entry.add(TXRX_PPDU_DATA_FLAGS_OFFSET).read();
        logical += 1;
    }

    let mut hardware = 0_usize;
    while hardware < HARDWARE_QUEUE_COUNT {
        let class = instances
            .add(hardware * TX_QUEUE_STATE_SIZE + TX_QUEUE_KIND_OFFSET)
            .read();
        if class != 3 {
            return Err(TxQueueStateAdoptionError::InvalidTxopClass {
                queue: hardware as u8,
                class,
            });
        }
        state.hardware_masks[hardware] = txrx
            .add(TXRX_HARDWARE_MASKS_OFFSET + hardware * 4)
            .cast::<u32>()
            .read() as u16;
        state.cursors[hardware] = txrx.add(TXRX_HARDWARE_CURSORS_OFFSET + hardware).read();
        hardware += 1;
    }
    state.queues = [LogicalQueue::empty(); LOGICAL_QUEUE_COUNT];
    (&mut *wifi_strict_txop_queue_status.0.get()).reset();
    STRICT_TX_QUEUE_STATE_ADOPTED.store(true, AtomicOrdering::Release);
    Ok(())
}

pub(crate) unsafe fn ppdu_format_flags(logical_queue: u8) -> Option<(u8, u8)> {
    let state = strict_tx_queue_state()?;
    let index = usize::from(logical_queue);
    Some((
        *state.ppdu_length_flags.get(index)?,
        *state.ppdu_data_flags.get(index)?,
    ))
}

#[inline(always)]
unsafe fn strict_tx_queue_state() -> Option<&'static mut StrictTxQueueState> {
    STRICT_TX_QUEUE_STATE_ADOPTED
        .load(AtomicOrdering::Acquire)
        .then(|| &mut *STRICT_TX_QUEUE_STATE.0.get())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TxopQueueError {
    UnsupportedHardwareQueue(u8),
    InstancesUnavailable,
    QueueAlreadyOwnsClass { queue: u8, class: u8 },
    InvalidOwnedClass { queue: u8, class: u8 },
}

/// Allocate the first free TXOP class and publish it into one hardware queue.
///
/// This is the exact finite state transform recovered from
/// `libpp.a[lmac.o]::lmacRequestTxopQueue`, with explicit ownership and bounds
/// checks added around the original two stores.
pub(crate) unsafe fn request_txop_queue(queue: u8) -> Result<bool, TxopQueueError> {
    if usize::from(queue) >= HARDWARE_QUEUE_COUNT {
        return Err(TxopQueueError::UnsupportedHardwareQueue(queue));
    }
    let instances = ptr::addr_of!(our_instances_ptr).read();
    if instances.is_null() {
        return Err(TxopQueueError::InstancesUnavailable);
    }
    let queue_kind = instances
        .add(usize::from(queue) * TX_QUEUE_STATE_SIZE + TX_QUEUE_KIND_OFFSET);
    let current = queue_kind.read();
    if current != 3 {
        return Err(TxopQueueError::QueueAlreadyOwnsClass {
            queue,
            class: current,
        });
    }

    let Some(class) = (&mut *wifi_strict_txop_queue_status.0.get()).request() else {
        return Ok(false);
    };
    queue_kind.write(class);
    Ok(true)
}

/// Return a hardware queue's TXOP class to the Rust-owned three-slot pool.
pub(crate) unsafe fn release_txop_queue(queue: u8) -> Result<(), TxopQueueError> {
    if usize::from(queue) >= HARDWARE_QUEUE_COUNT {
        return Err(TxopQueueError::UnsupportedHardwareQueue(queue));
    }
    let instances = ptr::addr_of!(our_instances_ptr).read();
    if instances.is_null() {
        return Err(TxopQueueError::InstancesUnavailable);
    }
    let queue_kind = instances
        .add(usize::from(queue) * TX_QUEUE_STATE_SIZE + TX_QUEUE_KIND_OFFSET);
    let class = queue_kind.read();
    if !(&mut *wifi_strict_txop_queue_status.0.get()).release(class) {
        return Err(TxopQueueError::InvalidOwnedClass { queue, class });
    }
    queue_kind.write(3);
    Ok(())
}

#[cfg(target_arch = "riscv32")]
#[cold]
#[inline(never)]
unsafe fn txop_abi_invariant_failure() -> ! {
    core::arch::asm!("ebreak", options(noreturn))
}

#[cfg(target_arch = "riscv32")]
#[no_mangle]
#[link_section = ".critical.text.wifi_strict.lmac_request_txop_queue"]
pub unsafe extern "C" fn wifi_strict_lmac_request_txop_queue(queue: u8) -> u32 {
    match request_txop_queue(queue) {
        Ok(allocated) => u32::from(allocated),
        Err(_) => txop_abi_invariant_failure(),
    }
}

#[cfg(target_arch = "riscv32")]
#[no_mangle]
#[link_section = ".critical.text.wifi_strict.lmac_release_txop_queue"]
pub unsafe extern "C" fn wifi_strict_lmac_release_txop_queue(queue: u8) {
    if release_txop_queue(queue).is_err() {
        txop_abi_invariant_failure();
    }
}

/// Read the exact finite `lmacIsIdle` state without entering its vendor leaf.
///
/// Queue four is a PP software-only class and is outside the strict hardware
/// submission profile.
pub(crate) unsafe fn hardware_queue_idle(queue: u8) -> bool {
    if queue > 3 {
        return false;
    }
    let instances = ptr::addr_of!(our_instances_ptr).read();
    !instances.is_null()
        && instances
            .add(usize::from(queue) * TX_QUEUE_STATE_SIZE + TX_QUEUE_STATUS_OFFSET)
            .read()
            == 0
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TxQueueProcessError {
    UnsupportedEventQueue(u8),
    InstancesUnavailable,
    TxRxUnavailable,
    UnsupportedQueueKind(u8),
    InvalidFrame,
    Submit {
        hardware_queue: u8,
        logical_queue: u8,
        error: crate::lmac::LmacAsyncError,
    },
}

/// HIL evidence captured immediately after one vendor TX-queue action returns.
///
/// `submitted` is the exact return-zero path that reached `lmacTxFrame`.
/// `input_logical_masks[event]` maps PP events 0..=4 to the descriptor queue
/// numbers actually installed in the matching hardware-queue SRAM state.
#[cfg(feature = "hil-vendor-tx")]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct HilTxQueueProcessSnapshot {
    pub calls: [u32; 5],
    pub submitted: [u32; 5],
    pub idle_or_disallowed: [u32; 5],
    pub no_frame: [u32; 5],
    pub unexpected_result: u32,
    pub input_logical_masks: [u32; 5],
    pub frame_null_after_submit: u32,
    pub descriptor_null: u32,
    pub hardware_queue_mask: u32,
    pub queue_status_mask: u32,
    pub queue_kind_mask: u32,
    pub logical_queue_mask: u32,
    pub selected_rate_low_mask: u32,
    pub selected_rate_high_mask: u32,
    pub descriptor_flags_or: u32,
    pub descriptor_queue_word_or: u32,
    pub layout_flags_or: u32,
    pub next_nonnull: u32,
    pub peer_null: u32,
}

#[cfg(feature = "hil-vendor-tx")]
struct HilTxQueueProcessCounters {
    calls: [AtomicU32; 5],
    submitted: [AtomicU32; 5],
    idle_or_disallowed: [AtomicU32; 5],
    no_frame: [AtomicU32; 5],
    unexpected_result: AtomicU32,
    input_logical_masks: [AtomicU32; 5],
    frame_null_after_submit: AtomicU32,
    descriptor_null: AtomicU32,
    hardware_queue_mask: AtomicU32,
    queue_status_mask: AtomicU32,
    queue_kind_mask: AtomicU32,
    logical_queue_mask: AtomicU32,
    selected_rate_low_mask: AtomicU32,
    selected_rate_high_mask: AtomicU32,
    descriptor_flags_or: AtomicU32,
    descriptor_queue_word_or: AtomicU32,
    layout_flags_or: AtomicU32,
    next_nonnull: AtomicU32,
    peer_null: AtomicU32,
}

#[cfg(feature = "hil-vendor-tx")]
impl HilTxQueueProcessCounters {
    const fn new() -> Self {
        Self {
            calls: [const { AtomicU32::new(0) }; 5],
            submitted: [const { AtomicU32::new(0) }; 5],
            idle_or_disallowed: [const { AtomicU32::new(0) }; 5],
            no_frame: [const { AtomicU32::new(0) }; 5],
            unexpected_result: AtomicU32::new(0),
            input_logical_masks: [const { AtomicU32::new(0) }; 5],
            frame_null_after_submit: AtomicU32::new(0),
            descriptor_null: AtomicU32::new(0),
            hardware_queue_mask: AtomicU32::new(0),
            queue_status_mask: AtomicU32::new(0),
            queue_kind_mask: AtomicU32::new(0),
            logical_queue_mask: AtomicU32::new(0),
            selected_rate_low_mask: AtomicU32::new(0),
            selected_rate_high_mask: AtomicU32::new(0),
            descriptor_flags_or: AtomicU32::new(0),
            descriptor_queue_word_or: AtomicU32::new(0),
            layout_flags_or: AtomicU32::new(0),
            next_nonnull: AtomicU32::new(0),
            peer_null: AtomicU32::new(0),
        }
    }
}

#[cfg(feature = "hil-vendor-tx")]
#[link_section = ".critical.bss.wifi_strict.tx_queue_process_hil"]
static HIL_COUNTERS: HilTxQueueProcessCounters = HilTxQueueProcessCounters::new();

#[cfg(feature = "hil-vendor-tx")]
pub fn hil_tx_queue_process_snapshot() -> HilTxQueueProcessSnapshot {
    let counters = &HIL_COUNTERS;
    HilTxQueueProcessSnapshot {
        calls: load_array(&counters.calls),
        submitted: load_array(&counters.submitted),
        idle_or_disallowed: load_array(&counters.idle_or_disallowed),
        no_frame: load_array(&counters.no_frame),
        unexpected_result: counters.unexpected_result.load(Ordering::Acquire),
        input_logical_masks: load_array(&counters.input_logical_masks),
        frame_null_after_submit: counters.frame_null_after_submit.load(Ordering::Acquire),
        descriptor_null: counters.descriptor_null.load(Ordering::Acquire),
        hardware_queue_mask: counters.hardware_queue_mask.load(Ordering::Acquire),
        queue_status_mask: counters.queue_status_mask.load(Ordering::Acquire),
        queue_kind_mask: counters.queue_kind_mask.load(Ordering::Acquire),
        logical_queue_mask: counters.logical_queue_mask.load(Ordering::Acquire),
        selected_rate_low_mask: counters.selected_rate_low_mask.load(Ordering::Acquire),
        selected_rate_high_mask: counters.selected_rate_high_mask.load(Ordering::Acquire),
        descriptor_flags_or: counters.descriptor_flags_or.load(Ordering::Acquire),
        descriptor_queue_word_or: counters.descriptor_queue_word_or.load(Ordering::Acquire),
        layout_flags_or: counters.layout_flags_or.load(Ordering::Acquire),
        next_nonnull: counters.next_nonnull.load(Ordering::Acquire),
        peer_null: counters.peer_null.load(Ordering::Acquire),
    }
}

#[cfg(feature = "hil-vendor-tx")]
fn load_array(counters: &[AtomicU32; 5]) -> [u32; 5] {
    core::array::from_fn(|index| counters[index].load(Ordering::Acquire))
}

/// Run one strict basic-MPDU TX-queue action.
///
/// Hardware qualification observed the admitted logical queues, queue kind
/// three, and one basic non-HE MPDU. The pinned binary supplies the matching
/// hardware queue as the event number. This leaf removes at most
/// one bounded-priority head and submits it through the already qualified finite
/// Rust LMAC path. Busy and empty states complete without retrying; their
/// completion/enqueue edges will post a later executor event.
///
/// # Safety
///
/// Must run under the same single radio owner as the original PP dispatcher.
#[cfg(feature = "hil-vendor-tx")]
#[no_mangle]
#[link_section = ".rwtext.wifi_strict.tx_queue_process_hil"]
pub(crate) unsafe fn process_tx_queue(queue: u8) -> Result<(), TxQueueProcessError> {
    let input = usize::from(queue);
    if input < HIL_COUNTERS.calls.len() {
        HIL_COUNTERS.calls[input].fetch_add(1, Ordering::Relaxed);
    }
    if queue > 3 {
        HIL_COUNTERS
            .unexpected_result
            .fetch_add(1, Ordering::Relaxed);
        return Err(TxQueueProcessError::UnsupportedEventQueue(queue));
    }

    let instances = ptr::addr_of!(our_instances_ptr).read();
    if instances.is_null() {
        return Err(TxQueueProcessError::InstancesUnavailable);
    }
    let queue_state = instances.add(usize::from(queue) * TX_QUEUE_STATE_SIZE);
    if queue_state.add(TX_QUEUE_STATUS_OFFSET).read() != 0 {
        HIL_COUNTERS.idle_or_disallowed[input].fetch_add(1, Ordering::Relaxed);
        return Ok(());
    }
    let queue_kind = queue_state.add(TX_QUEUE_KIND_OFFSET).read();
    if queue_kind != 3 {
        return Err(TxQueueProcessError::UnsupportedQueueKind(queue_kind));
    }

    let Some(expected_logical_queue) = select_logical_queue(queue)? else {
        HIL_COUNTERS.no_frame[input].fetch_add(1, Ordering::Relaxed);
        return Ok(());
    };
    let frame = dequeue_one(expected_logical_queue)?;
    if !frame
        .add(TX_FRAME_NEXT_OFFSET)
        .cast::<*mut u8>()
        .read()
        .is_null()
        || frame.add(0x2c).cast::<*mut u8>().read().is_null()
    {
        requeue_front(expected_logical_queue, frame)?;
        return Err(TxQueueProcessError::InvalidFrame);
    }
    let descriptor = frame
        .add(TX_FRAME_DESCRIPTOR_OFFSET)
        .cast::<*mut u8>()
        .read();
    if descriptor.is_null() {
        requeue_front(expected_logical_queue, frame)?;
        return Err(TxQueueProcessError::InvalidFrame);
    }
    let logical_queue = (descriptor
        .add(TX_DESCRIPTOR_QUEUE_WORD_OFFSET)
        .cast::<u32>()
        .read()
        >> 20)
        & 0x0f;
    if logical_queue != u32::from(expected_logical_queue) {
        requeue_front(expected_logical_queue, frame)?;
        return Err(TxQueueProcessError::InvalidFrame);
    }

    if let Err(error) = stamp_ap_beacon(frame) {
        requeue_front(expected_logical_queue, frame)?;
        return Err(error);
    }

    if let Err(error) = crate::lmac::submit_basic_non_he_frame(queue_state, frame) {
        requeue_front(expected_logical_queue, frame)?;
        return Err(TxQueueProcessError::Submit {
            hardware_queue: queue,
            logical_queue: expected_logical_queue,
            error,
        });
    }
    HIL_COUNTERS.submitted[input].fetch_add(1, Ordering::Relaxed);
    record_submitted(input, queue_state, frame);
    Ok(())
}

/// Publish a monotonic TSF and DTIM phase directly in a queued AP beacon.
///
/// The current S31 ROM `hal_get_tsf_time` export remains zero after both the
/// reset and set-time leaves.  Scan clients accept the otherwise valid frame,
/// but associated clients reject the zero-TSF stream as missed beacons.  The
/// vendor beacon builder also derives DTIM from that zero TSF, permanently
/// producing `count = period - 1`. The executor clock is already the sole
/// strict-runtime time source, so the submit boundary replaces both fields
/// before hardware ownership. This finite leaf performs no wait or allocation.
unsafe fn stamp_ap_beacon(frame: *mut u8) -> Result<(), TxQueueProcessError> {
    let first_buffer = frame.add(4).cast::<*mut u8>().read();
    if first_buffer.is_null() {
        return Err(TxQueueProcessError::InvalidFrame);
    }
    let metadata = first_buffer.add(4).cast::<*mut u8>().read();
    if metadata.is_null() {
        return Err(TxQueueProcessError::InvalidFrame);
    }
    let layout = frame
        .add(TX_FRAME_LAYOUT_FLAGS_OFFSET)
        .cast::<u16>()
        .read_unaligned();
    let prefix = if layout & 0x2000 != 0 { 8 } else { 0 };
    let header = metadata.add(prefix);
    let frame_control = header.cast::<u16>().read_unaligned();
    if frame_control != 0x0080 {
        return Ok(());
    }
    let length = usize::from(frame.add(0x14).cast::<u16>().read_unaligned())
        + usize::from(frame.add(0x16).cast::<u16>().read_unaligned());
    let Some(body_length) = length.checked_sub(prefix) else {
        return Err(TxQueueProcessError::InvalidFrame);
    };
    if body_length > 1600 {
        return Err(TxQueueProcessError::InvalidFrame);
    }
    let Some(timestamp) = crate::adapter::runtime_now_us() else {
        return Err(TxQueueProcessError::InvalidFrame);
    };
    let bytes = core::slice::from_raw_parts_mut(header, body_length);
    crate::beacon::stamp(
        bytes,
        timestamp,
        crate::net80211_tx::deferred_group_pending(),
    )
    .map(|_| ())
    .ok_or(TxQueueProcessError::InvalidFrame)
}

#[cfg(not(feature = "hil-vendor-tx"))]
#[no_mangle]
#[link_section = ".rwtext.wifi_strict.tx_queue_process"]
pub(crate) unsafe fn process_tx_queue(queue: u8) -> Result<(), TxQueueProcessError> {
    if queue > 3 {
        return Err(TxQueueProcessError::UnsupportedEventQueue(queue));
    }
    let instances = ptr::addr_of!(our_instances_ptr).read();
    if instances.is_null() {
        return Err(TxQueueProcessError::InstancesUnavailable);
    }
    let queue_state = instances.add(usize::from(queue) * TX_QUEUE_STATE_SIZE);
    if queue_state.add(TX_QUEUE_STATUS_OFFSET).read() != 0 {
        return Ok(());
    }
    let queue_kind = queue_state.add(TX_QUEUE_KIND_OFFSET).read();
    if queue_kind != 3 {
        return Err(TxQueueProcessError::UnsupportedQueueKind(queue_kind));
    }
    let Some(expected_logical_queue) = select_logical_queue(queue)? else {
        return Ok(());
    };
    let frame = dequeue_one(expected_logical_queue)?;
    let peer = frame.add(0x2c).cast::<*mut u8>().read();
    let descriptor = frame
        .add(TX_FRAME_DESCRIPTOR_OFFSET)
        .cast::<*mut u8>()
        .read();
    if peer.is_null()
        || descriptor.is_null()
        || !frame
            .add(TX_FRAME_NEXT_OFFSET)
            .cast::<*mut u8>()
            .read()
            .is_null()
        || (descriptor
            .add(TX_DESCRIPTOR_QUEUE_WORD_OFFSET)
            .cast::<u32>()
            .read()
            >> 20)
            & 0x0f
            != u32::from(expected_logical_queue)
    {
        requeue_front(expected_logical_queue, frame)?;
        return Err(TxQueueProcessError::InvalidFrame);
    }
    if let Err(error) = stamp_ap_beacon(frame) {
        requeue_front(expected_logical_queue, frame)?;
        return Err(error);
    }
    if let Err(error) = crate::lmac::submit_basic_non_he_frame(queue_state, frame) {
        requeue_front(expected_logical_queue, frame)?;
        return Err(TxQueueProcessError::Submit {
            hardware_queue: queue,
            logical_queue: expected_logical_queue,
            error,
        });
    }
    Ok(())
}

unsafe fn select_logical_queue(hardware_queue: u8) -> Result<Option<u8>, TxQueueProcessError> {
    if hardware_queue > 3 {
        return Err(TxQueueProcessError::UnsupportedEventQueue(hardware_queue));
    }
    let state = strict_tx_queue_state().ok_or(TxQueueProcessError::TxRxUnavailable)?;
    let mut ready_mask = 0_u16;
    let mut logical_queue = 0_u8;
    while logical_queue < 16 {
        if !state.queues[usize::from(logical_queue)].head.is_null() {
            ready_mask |= 1_u16 << logical_queue;
        }
        logical_queue += 1;
    }
    let hardware = usize::from(hardware_queue);
    let cursor = state.cursors[hardware];
    let advance = (cursor < 16)
        .then_some(usize::from(cursor))
        .is_some_and(|current| {
            let selected = state.queues[current].selected;
            if selected {
                state.queues[current].selected = false;
            }
            selected
        });
    let allowed_mask = state.hardware_masks[hardware];
    let selected =
        select_ready_logical_queue(hardware_queue, allowed_mask, cursor, ready_mask, advance);
    if let Some(logical_queue) = selected {
        state.cursors[hardware] = logical_queue;
        state.queues[usize::from(logical_queue)].selected = true;
    }
    Ok(selected)
}

pub(crate) unsafe fn dequeue_logical_queue(
    logical_queue: u8,
) -> Result<*mut u8, TxQueueProcessError> {
    dequeue_one(logical_queue)
}

unsafe fn dequeue_one(logical_queue: u8) -> Result<*mut u8, TxQueueProcessError> {
    let state = strict_tx_queue_state().ok_or(TxQueueProcessError::TxRxUnavailable)?;
    let Some(queue) = state.queues.get_mut(usize::from(logical_queue)) else {
        return Err(TxQueueProcessError::InvalidFrame);
    };
    Ok(queue.pop_front())
}

unsafe fn requeue_front(logical_queue: u8, frame: *mut u8) -> Result<(), TxQueueProcessError> {
    let state = strict_tx_queue_state().ok_or(TxQueueProcessError::TxRxUnavailable)?;
    let Some(queue) = state.queues.get_mut(usize::from(logical_queue)) else {
        return Err(TxQueueProcessError::InvalidFrame);
    };
    if !queue.push_front(frame) {
        return Err(TxQueueProcessError::InvalidFrame);
    }
    Ok(())
}

/// Append one exclusively owned frame to a Rust logical queue.
///
/// # Safety
///
/// `frame` must be live, unlinked, and remain owned by the strict radio hart
/// until it is dequeued for hardware submission or completion.
pub(crate) unsafe fn append_logical_queue(
    logical_queue: u8,
    frame: *mut u8,
) -> Result<(), TxQueueProcessError> {
    let state = strict_tx_queue_state().ok_or(TxQueueProcessError::TxRxUnavailable)?;
    let Some(queue) = state.queues.get_mut(usize::from(logical_queue)) else {
        return Err(TxQueueProcessError::InvalidFrame);
    };
    if !queue.append(frame) {
        return Err(TxQueueProcessError::InvalidFrame);
    }
    Ok(())
}

/// Prepend one already linked chain after a timeout discard split.
///
/// # Safety
///
/// `head..=tail` must be a finite live chain owned by the strict radio hart.
pub(crate) unsafe fn requeue_logical_chain_front(
    logical_queue: u8,
    head: *mut u8,
    tail: *mut u8,
) -> Result<(), TxQueueProcessError> {
    let state = strict_tx_queue_state().ok_or(TxQueueProcessError::TxRxUnavailable)?;
    let Some(queue) = state.queues.get_mut(usize::from(logical_queue)) else {
        return Err(TxQueueProcessError::InvalidFrame);
    };
    if !queue.prepend_chain(head, tail) {
        return Err(TxQueueProcessError::InvalidFrame);
    }
    Ok(())
}

#[cfg(feature = "hil-vendor-tx")]
unsafe fn record_submitted(input: usize, queue_state: *mut u8, frame: *mut u8) {
    record_small_mask(
        &HIL_COUNTERS.hardware_queue_mask,
        queue_state.add(TX_QUEUE_HARDWARE_INDEX_OFFSET).read(),
    );
    record_small_mask(
        &HIL_COUNTERS.queue_status_mask,
        queue_state.add(TX_QUEUE_STATUS_OFFSET).read(),
    );
    record_small_mask(
        &HIL_COUNTERS.queue_kind_mask,
        queue_state.add(TX_QUEUE_KIND_OFFSET).read(),
    );

    HIL_COUNTERS.layout_flags_or.fetch_or(
        frame.add(TX_FRAME_LAYOUT_FLAGS_OFFSET).cast::<u32>().read(),
        Ordering::Relaxed,
    );
    if !frame
        .add(TX_FRAME_NEXT_OFFSET)
        .cast::<*mut u8>()
        .read()
        .is_null()
    {
        HIL_COUNTERS.next_nonnull.fetch_add(1, Ordering::Relaxed);
    }
    if frame.add(0x2c).cast::<*mut u8>().read().is_null() {
        HIL_COUNTERS.peer_null.fetch_add(1, Ordering::Relaxed);
    }

    let descriptor = frame
        .add(TX_FRAME_DESCRIPTOR_OFFSET)
        .cast::<*mut u8>()
        .read();
    if descriptor.is_null() {
        HIL_COUNTERS.descriptor_null.fetch_add(1, Ordering::Relaxed);
        return;
    }
    HIL_COUNTERS
        .descriptor_flags_or
        .fetch_or(descriptor.cast::<u32>().read(), Ordering::Relaxed);
    let queue_word = descriptor
        .add(TX_DESCRIPTOR_QUEUE_WORD_OFFSET)
        .cast::<u32>()
        .read();
    HIL_COUNTERS
        .descriptor_queue_word_or
        .fetch_or(queue_word, Ordering::Relaxed);

    let logical_queue = ((queue_word >> 20) & 0x0f) as u8;
    let logical_bit = 1_u32 << logical_queue;
    HIL_COUNTERS
        .logical_queue_mask
        .fetch_or(logical_bit, Ordering::Relaxed);
    HIL_COUNTERS.input_logical_masks[input].fetch_or(logical_bit, Ordering::Relaxed);

    let rate = descriptor.add(TX_DESCRIPTOR_SELECTED_RATE_OFFSET).read();
    if rate < 32 {
        HIL_COUNTERS
            .selected_rate_low_mask
            .fetch_or(1_u32 << rate, Ordering::Relaxed);
    } else if rate < 64 {
        HIL_COUNTERS
            .selected_rate_high_mask
            .fetch_or(1_u32 << (rate - 32), Ordering::Relaxed);
    }
}

#[cfg(feature = "hil-vendor-tx")]
fn record_small_mask(counter: &AtomicU32, value: u8) {
    if value < 32 {
        counter.fetch_or(1_u32 << value, Ordering::Relaxed);
    }
}
