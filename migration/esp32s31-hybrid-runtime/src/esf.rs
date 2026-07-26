//! Allocation-free strict replacement for the pinned S31 ESF buffer ABI.

use core::{
    cell::UnsafeCell,
    ffi::c_void,
    mem, ptr,
    sync::atomic::{AtomicUsize, Ordering},
};

use crate::rx_ownership::{RxBufferOwner, RxBufferOwnershipWord};

const DESCRIPTOR_COUNT: usize = 11;
const DESCRIPTOR_SIZE: usize = 0x14;
const ESF_HEADER_SIZE: usize = 0x90;
const MANAGEMENT_PAYLOAD_CAPACITY: usize = 1600;
const MANAGEMENT_SLOT_SIZE: usize = ESF_HEADER_SIZE + MANAGEMENT_PAYLOAD_CAPACITY;
// Active 2.4-GHz scanning can retain one management frame for each of the
// thirteen configured channels until TX completion catches up. Keep those
// frames plus bounded authentication/association headroom entirely in BSS.
const MANAGEMENT_SLOT_CAPACITY: usize = 16;
const MANAGEMENT_SLOT_MASK: usize = (1 << MANAGEMENT_SLOT_CAPACITY) - 1;

// `wDev_IndicateFrame` and `wDev_IndicateBeaconMemoryFrame` request ESF kind
// 7 for received frames larger than the vendor's 500-byte small-frame pool.
// The original allocator serves kind 7 from the heap. Match the configured
// static RX bound with fixed Rust-owned storage instead.
const LARGE_RX_PAYLOAD_CAPACITY: usize = 1700;
// Kind 8 is backed by the pinned 648-byte cold pool: a 0x90-byte ESF header
// plus the vendor's 500-byte small-RX payload and alignment.
const SMALL_RX_PAYLOAD_CAPACITY: usize = 500;
const LARGE_RX_SLOT_SIZE: usize = ESF_HEADER_SIZE + LARGE_RX_PAYLOAD_CAPACITY;
// The default profile uses one native atomic word. PSRAM-backed application
// profiles can spend more internal SRAM here and use two independent native
// words. This deliberately avoids emulated 64-bit atomics and critical
// sections in the interrupt-facing allocator.
#[cfg(feature = "large-rx-pool-48")]
const LARGE_RX_SLOT_CAPACITY: usize = 48;
#[cfg(not(feature = "large-rx-pool-48"))]
const LARGE_RX_SLOT_CAPACITY: usize = 32;
const LARGE_RX_CLAIM_WORD_BITS: usize = usize::BITS as usize;
const LARGE_RX_CLAIM_WORDS: usize =
    (LARGE_RX_SLOT_CAPACITY + LARGE_RX_CLAIM_WORD_BITS - 1) / LARGE_RX_CLAIM_WORD_BITS;

// A hardware MPDU may span several 1700-byte RX descriptors even though the
// common path fits one. Preserve the 14-bit indication ABI with two rare-path
// objects: only the intrusive ESF header and ownership state stay in
// interrupt-visible SRAM; their payloads live in PSRAM and are accessed after
// the descriptor prefix has been detached onto the radio executor.
pub(crate) const AGGREGATE_RX_SLOT_CAPACITY: usize = 2;
const AGGREGATE_RX_PAYLOAD_CAPACITY: usize = 0x4000;

const ESF_BUFFER_DESCRIPTOR_OFFSET: usize = 0x3c;
const ESF_TX_DESCRIPTOR_OFFSET: usize = 0x48;
const ESF_BUFFER_DESCRIPTOR_POINTER_OFFSET: usize = 0x04;
const ESF_BUFFER_POINTER_OFFSET: usize = 0x10;
const ESF_BUFFER_END_OFFSET: usize = 0x40;
const ESF_TYPE_OFFSET: usize = 0x1a;
const ESF_LENGTH_OFFSET: usize = 0x16;
const ESF_FREE_NEXT_OFFSET: usize = 0x30;
const ESF_TX_DESCRIPTOR_POINTER_OFFSET: usize = 0x34;

#[repr(C, align(4))]
struct ManagementSlot(UnsafeCell<[u8; MANAGEMENT_SLOT_SIZE]>);

impl ManagementSlot {
    const fn new() -> Self {
        Self(UnsafeCell::new([0; MANAGEMENT_SLOT_SIZE]))
    }
}

unsafe impl Sync for ManagementSlot {}

#[repr(C, align(4))]
struct LargeRxSlot(UnsafeCell<[u8; LARGE_RX_SLOT_SIZE]>);

impl LargeRxSlot {
    const fn new() -> Self {
        Self(UnsafeCell::new([0; LARGE_RX_SLOT_SIZE]))
    }
}

unsafe impl Sync for LargeRxSlot {}

#[repr(C, align(4))]
struct AggregateRxHeader(UnsafeCell<[u8; ESF_HEADER_SIZE]>);

impl AggregateRxHeader {
    const fn new() -> Self {
        Self(UnsafeCell::new([0; ESF_HEADER_SIZE]))
    }
}

unsafe impl Sync for AggregateRxHeader {}

#[repr(C, align(4))]
struct AggregateRxPayload(UnsafeCell<[u8; AGGREGATE_RX_PAYLOAD_CAPACITY]>);

impl AggregateRxPayload {
    const fn new() -> Self {
        Self(UnsafeCell::new([0; AGGREGATE_RX_PAYLOAD_CAPACITY]))
    }
}

unsafe impl Sync for AggregateRxPayload {}

#[cfg_attr(
    target_arch = "riscv32",
    link_section = ".critical.bss.wifi_strict.esf_management_slots"
)]
static MANAGEMENT_SLOTS: [ManagementSlot; MANAGEMENT_SLOT_CAPACITY] =
    [const { ManagementSlot::new() }; MANAGEMENT_SLOT_CAPACITY];
#[cfg_attr(
    target_arch = "riscv32",
    link_section = ".critical.bss.wifi_strict.esf_management_claims"
)]
static CLAIMED_MANAGEMENT_SLOTS: AtomicUsize = AtomicUsize::new(0);
#[cfg_attr(
    target_arch = "riscv32",
    link_section = ".critical.bss.wifi_strict.esf_large_rx_slots"
)]
static LARGE_RX_SLOTS: [LargeRxSlot; LARGE_RX_SLOT_CAPACITY] =
    [const { LargeRxSlot::new() }; LARGE_RX_SLOT_CAPACITY];
#[cfg_attr(
    target_arch = "riscv32",
    link_section = ".critical.bss.wifi_strict.esf_large_rx_claims"
)]
static LARGE_RX_OWNERS: [RxBufferOwnershipWord; LARGE_RX_CLAIM_WORDS] =
    [const { RxBufferOwnershipWord::new() }; LARGE_RX_CLAIM_WORDS];
#[cfg_attr(
    target_arch = "riscv32",
    link_section = ".critical.bss.wifi_strict.esf_aggregate_rx_headers"
)]
static AGGREGATE_RX_HEADERS: [AggregateRxHeader; AGGREGATE_RX_SLOT_CAPACITY] =
    [const { AggregateRxHeader::new() }; AGGREGATE_RX_SLOT_CAPACITY];
// The ISR-facing queue only reads and writes the ESF header above. These
// payloads are filled by the radio executor after PSRAM initialization and
// remain behind that owned header until the async network consumer drops it.
#[cfg_attr(
    target_arch = "riscv32",
    link_section = ".psram.bss.wifi_strict.esf_aggregate_rx_payloads"
)]
static AGGREGATE_RX_PAYLOADS: [AggregateRxPayload; AGGREGATE_RX_SLOT_CAPACITY] =
    [const { AggregateRxPayload::new() }; AGGREGATE_RX_SLOT_CAPACITY];
#[cfg_attr(
    target_arch = "riscv32",
    link_section = ".critical.bss.wifi_strict.esf_aggregate_rx_owners"
)]
static AGGREGATE_RX_OWNERS: RxBufferOwnershipWord = RxBufferOwnershipWord::new();
#[cfg_attr(
    target_arch = "riscv32",
    link_section = ".critical.bss.wifi_strict.esf_rejections"
)]
static REJECTED_ESF_OPERATIONS: AtomicUsize = AtomicUsize::new(0);
#[cfg_attr(
    target_arch = "riscv32",
    link_section = ".critical.bss.wifi_strict.esf_rejections"
)]
static LAST_REJECTED_ESF_KIND: AtomicUsize = AtomicUsize::new(0);
#[cfg_attr(
    target_arch = "riscv32",
    link_section = ".critical.bss.wifi_strict.esf_rejections"
)]
static LAST_REJECTED_ESF_ARGUMENT: AtomicUsize = AtomicUsize::new(0);
const NO_PREARM_HART: usize = usize::MAX;
#[cfg_attr(
    target_arch = "riscv32",
    link_section = ".critical.data.wifi_strict.esf_prearm_hart"
)]
static PREARM_MANAGEMENT_HART: AtomicUsize = AtomicUsize::new(NO_PREARM_HART);

pub(crate) const fn maximum_strict_large_rx_length() -> usize {
    LARGE_RX_PAYLOAD_CAPACITY
}

unsafe extern "C" {
    static mut g_eb_list_desc: u8;
    fn esf_buf_alloc(source: *const u8, kind: u32, length: u32) -> *mut u8;
    fn __real_esf_buf_alloc(source: *const u8, kind: u32, length: u32) -> *mut u8;
    fn esf_buf_recycle(frame: *mut c_void);
    fn __real_esf_buf_recycle(frame: *mut c_void);
    fn ppRecycleRxPkt(frame: *mut u8);
    fn __real_ppRecycleRxPkt(frame: *mut u8);
    #[link_name = "esp_wifi_internal_free_rx_buffer"]
    fn linked_esp_wifi_internal_free_rx_buffer(frame: *mut c_void);
}

pub(crate) fn link_wrappers_active() -> bool {
    ptr::eq(
        esf_buf_alloc as *const (),
        __wrap_esf_buf_alloc as *const (),
    ) && ptr::eq(
        esf_buf_recycle as *const (),
        __wrap_esf_buf_recycle as *const (),
    )
}

pub(crate) fn rx_packet_recycle_link_wrapper_active() -> bool {
    ptr::eq(
        ppRecycleRxPkt as *const (),
        wifi_strict_pp_recycle_rx_pkt as *const (),
    ) && ptr::eq(
        linked_esp_wifi_internal_free_rx_buffer as *const (),
        wifi_strict_esp_wifi_internal_free_rx_buffer as *const (),
    )
}

/// Route connection-time management frames into the fixed Rust pool before
/// the general runtime heap/core-stall gates are armed.
pub(crate) fn enable_prearm_management_pool(expected_hart: usize) {
    PREARM_MANAGEMENT_HART.store(expected_hart, Ordering::Release);
}

/// Route management allocations through the fixed SRAM pool before the
/// strict runtime is armed.
///
/// WPA2 AP startup constructs a larger beacon than the open-AP path. The
/// vendor pre-start ESF pool can reject that frame, so the composition root
/// must enable the same bounded pool used during strict association before it
/// applies the AP configuration.
///
/// Returns `false` when the required final-link ESF wrappers are absent.
pub fn enable_prestart_management_pool() -> bool {
    if !link_wrappers_active() {
        return false;
    }
    enable_prearm_management_pool(crate::critical::current_hart());
    true
}

fn prearm_management_pool_enabled() -> bool {
    PREARM_MANAGEMENT_HART.load(Ordering::Acquire) != NO_PREARM_HART
}

fn on_prearm_management_hart() -> bool {
    crate::critical::current_hart() == PREARM_MANAGEMENT_HART.load(Ordering::Acquire)
}

#[cfg_attr(target_arch = "riscv32", link_section = ".rwtext.wifi_strict.esf")]
#[inline(always)]
fn reject(kind: u32, argument: usize) {
    LAST_REJECTED_ESF_KIND.store(kind as usize, Ordering::Relaxed);
    LAST_REJECTED_ESF_ARGUMENT.store(argument, Ordering::Relaxed);
    REJECTED_ESF_OPERATIONS.fetch_add(1, Ordering::Relaxed);
    // The strict allocator is reached directly from the RX interrupt path.
    // Do not extend a bounded pool-exhaustion return into the general
    // diagnostic call graph: that graph is ordinary PSRAM/PSRAM code.
}

const fn is_vendor_static_kind(kind: u32) -> bool {
    matches!(kind, 1 | 5 | 6 | 8 | 9 | 10)
}

const fn is_management_kind(kind: u32) -> bool {
    matches!(kind, 2..=4)
}

const fn management_frame_allocation(header: u32, body: u32) -> Option<(u32, u32)> {
    let total = match header.checked_add(body) {
        Some(total) => total,
        None => return None,
    };
    let rounded = match total.checked_add(3) {
        Some(total) => total & !3,
        None => return None,
    };
    let kind = if rounded <= 64 {
        3
    } else if rounded <= 256 {
        2
    } else {
        4
    };
    Some((kind, rounded))
}

#[cfg_attr(target_arch = "riscv32", link_section = ".rwtext.wifi_strict.esf")]
#[inline(always)]
unsafe fn descriptor(kind: u32) -> *mut u8 {
    ptr::addr_of_mut!(g_eb_list_desc).add(kind as usize * DESCRIPTOR_SIZE)
}

#[cfg_attr(target_arch = "riscv32", link_section = ".rwtext.wifi_strict.esf")]
unsafe fn initialize_frame(
    frame: *mut u8,
    kind: u32,
    source: *const u8,
    length: usize,
    payload_capacity: usize,
) -> Option<*mut u8> {
    initialize_frame_with_payload(
        frame,
        frame.add(ESF_HEADER_SIZE),
        kind,
        source,
        length,
        payload_capacity,
    )
}

/// Initialize the pinned ESF header while keeping its payload in separately
/// owned storage.
///
/// This is used only by the rare multi-descriptor RX pool. The radio ISR sees
/// the SRAM header and its intrusive links; the executor and network owner
/// follow the explicit payload pointer into PSRAM.
#[cfg_attr(target_arch = "riscv32", link_section = ".rwtext.wifi_strict.esf")]
unsafe fn initialize_frame_with_payload(
    frame: *mut u8,
    payload: *mut u8,
    kind: u32,
    source: *const u8,
    length: usize,
    payload_capacity: usize,
) -> Option<*mut u8> {
    if frame.is_null()
        || payload.is_null()
        || kind as usize >= DESCRIPTOR_COUNT
        || length > u16::MAX as usize
    {
        return None;
    }
    let list = descriptor(kind);
    let prefix = list.add(0x0c).read() as usize;
    if prefix.checked_add(length)? > payload_capacity {
        return None;
    }

    frame.write_bytes(0, ESF_HEADER_SIZE);
    let buffer_descriptor = frame.add(ESF_BUFFER_DESCRIPTOR_OFFSET);
    let tx_descriptor = frame.add(ESF_TX_DESCRIPTOR_OFFSET);
    let data = payload.add(prefix);

    frame.add(0x04).cast::<*mut u8>().write(buffer_descriptor);
    frame.add(0x08).cast::<*mut u8>().write(buffer_descriptor);
    frame.add(0x0c).cast::<u16>().write(1);
    frame
        .add(ESF_BUFFER_POINTER_OFFSET)
        .cast::<*mut u8>()
        .write(payload);
    frame
        .add(ESF_LENGTH_OFFSET)
        .cast::<u16>()
        .write(length as u16);
    frame.add(ESF_TYPE_OFFSET).write(kind as u8);
    frame
        .add(ESF_TX_DESCRIPTOR_POINTER_OFFSET)
        .cast::<*mut u8>()
        .write(tx_descriptor);
    frame
        .add(ESF_BUFFER_END_OFFSET)
        .cast::<*mut u8>()
        .write(data);

    payload.write_bytes(0, prefix);
    if !source.is_null() {
        ptr::copy_nonoverlapping(source, data, length);
    }

    let packet_length = (prefix + length) as u32;
    let buffer_flags = buffer_descriptor.cast::<u32>().read() & 0xffff_c000;
    buffer_descriptor
        .cast::<u32>()
        .write(buffer_flags | packet_length);
    buffer_descriptor.add(4).cast::<*mut u8>().write(data);

    let descriptor_flags = list.add(4).cast::<u32>().read();
    tx_descriptor
        .cast::<u32>()
        .write(tx_descriptor.cast::<u32>().read() | descriptor_flags);
    tx_descriptor
        .add(4)
        .cast::<u32>()
        .write(tx_descriptor.add(4).cast::<u32>().read() | 0x0f);
    Some(frame)
}

#[cfg_attr(target_arch = "riscv32", link_section = ".rwtext.wifi_strict.esf")]
fn claim_management_slot() -> Option<usize> {
    let claimed = CLAIMED_MANAGEMENT_SLOTS.load(Ordering::Acquire);
    let free = !claimed & MANAGEMENT_SLOT_MASK;
    if free == 0 {
        return None;
    }
    let index = free.trailing_zeros() as usize;
    let bit = 1_usize << index;
    (CLAIMED_MANAGEMENT_SLOTS.fetch_or(bit, Ordering::AcqRel) & bit == 0).then_some(index)
}

#[cfg_attr(target_arch = "riscv32", link_section = ".rwtext.wifi_strict.esf")]
fn claim_large_rx_slot() -> Option<usize> {
    // This is a bounded scan of one or two independent native words, never a
    // retry loop. A racing owner may cause one immediate failed claim, but
    // allocation never waits or enters a critical section.
    let mut word_index = 0;
    while word_index < LARGE_RX_CLAIM_WORDS {
        let ownership = &LARGE_RX_OWNERS[word_index];
        let claimed = ownership.claimed_bits();
        let first_slot = word_index * LARGE_RX_CLAIM_WORD_BITS;
        let remaining = LARGE_RX_SLOT_CAPACITY - first_slot;
        let valid_mask = if remaining >= LARGE_RX_CLAIM_WORD_BITS {
            usize::MAX
        } else {
            (1_usize << remaining) - 1
        };
        let free = !claimed & valid_mask;
        if free != 0 {
            let word_slot = free.trailing_zeros() as usize;
            if ownership.try_claim_radio(word_slot) {
                let index = first_slot + word_slot;
                return Some(index);
            }
        }
        word_index += 1;
    }
    None
}

#[inline(always)]
fn large_rx_slot_claimed(index: usize) -> bool {
    let word_index = index / LARGE_RX_CLAIM_WORD_BITS;
    LARGE_RX_OWNERS[word_index].claimed_bits() & (1_usize << (index % LARGE_RX_CLAIM_WORD_BITS))
        != 0
}

#[inline(always)]
fn release_large_rx_slot(index: usize, owner: RxBufferOwner) -> bool {
    let word_index = index / LARGE_RX_CLAIM_WORD_BITS;
    LARGE_RX_OWNERS[word_index].try_release(index % LARGE_RX_CLAIM_WORD_BITS, owner)
}

#[inline(always)]
fn large_rx_owner(index: usize) -> Option<RxBufferOwner> {
    let word_index = index / LARGE_RX_CLAIM_WORD_BITS;
    LARGE_RX_OWNERS[word_index].owner(index % LARGE_RX_CLAIM_WORD_BITS)
}

#[cfg_attr(target_arch = "riscv32", link_section = ".rwtext.wifi_strict.esf")]
fn claim_aggregate_rx_slot() -> Option<usize> {
    // Exactly one finite pass over two bits. A racing transition may reject
    // one admission, but this path never retries or waits.
    let mut index = 0;
    while index < AGGREGATE_RX_SLOT_CAPACITY {
        if AGGREGATE_RX_OWNERS.try_claim_radio(index) {
            return Some(index);
        }
        index += 1;
    }
    None
}

#[inline(always)]
fn aggregate_rx_slot_claimed(index: usize) -> bool {
    AGGREGATE_RX_OWNERS.claimed_bits() & (1_usize << index) != 0
}

#[inline(always)]
fn aggregate_rx_owner(index: usize) -> Option<RxBufferOwner> {
    AGGREGATE_RX_OWNERS.owner(index)
}

#[inline(always)]
fn release_aggregate_rx_slot(index: usize, owner: RxBufferOwner) -> bool {
    AGGREGATE_RX_OWNERS.try_release(index, owner)
}

#[cfg_attr(target_arch = "riscv32", link_section = ".rwtext.wifi_strict.esf")]
fn management_slot_index(frame: *mut u8) -> Option<usize> {
    let base = ptr::addr_of!(MANAGEMENT_SLOTS) as usize;
    let address = frame as usize;
    let stride = mem::size_of::<ManagementSlot>();
    let offset = address.checked_sub(base)?;
    if offset % stride != 0 {
        return None;
    }
    let index = offset / stride;
    (index < MANAGEMENT_SLOT_CAPACITY).then_some(index)
}

#[cfg_attr(target_arch = "riscv32", link_section = ".rwtext.wifi_strict.esf")]
fn large_rx_slot_index(frame: *mut u8) -> Option<usize> {
    let base = ptr::addr_of!(LARGE_RX_SLOTS) as usize;
    let address = frame as usize;
    let stride = mem::size_of::<LargeRxSlot>();
    let offset = address.checked_sub(base)?;
    if offset % stride != 0 {
        return None;
    }
    let index = offset / stride;
    (index < LARGE_RX_SLOT_CAPACITY).then_some(index)
}

#[cfg_attr(target_arch = "riscv32", link_section = ".rwtext.wifi_strict.esf")]
fn aggregate_rx_slot_index(frame: *mut u8) -> Option<usize> {
    let base = ptr::addr_of!(AGGREGATE_RX_HEADERS) as usize;
    let address = frame as usize;
    let stride = mem::size_of::<AggregateRxHeader>();
    let offset = address.checked_sub(base)?;
    if offset % stride != 0 {
        return None;
    }
    let index = offset / stride;
    (index < AGGREGATE_RX_SLOT_CAPACITY).then_some(index)
}

/// Map a live kind-7 ESF object to the fixed slot ID used by the safe reorder
/// state. The pointer is validated against the exact internal-SRAM pool.
#[cfg_attr(target_arch = "riscv32", link_section = ".rwtext.wifi_strict.esf")]
pub(crate) fn large_rx_slot_id(frame: *mut u8) -> Option<u8> {
    if let Some(index) = large_rx_slot_index(frame) {
        return (large_rx_slot_claimed(index)
            && large_rx_owner(index) == Some(RxBufferOwner::Radio))
        .then_some(index as u8);
    }
    let index = aggregate_rx_slot_index(frame)?;
    (aggregate_rx_slot_claimed(index) && aggregate_rx_owner(index) == Some(RxBufferOwner::Radio))
        .then_some((LARGE_RX_SLOT_CAPACITY + index) as u8)
}

/// Resolve a reorder slot ID back to its still-owned ESF object.
///
/// The returned raw pointer remains owned by the fixed pool. The caller may
/// pass it through the RX protocol path exactly once or recycle it.
#[cfg_attr(target_arch = "riscv32", link_section = ".rwtext.wifi_strict.esf")]
pub(crate) fn large_rx_frame(slot: u8) -> Option<*mut u8> {
    let index = usize::from(slot);
    if index < LARGE_RX_SLOT_CAPACITY {
        if !large_rx_slot_claimed(index) || large_rx_owner(index) != Some(RxBufferOwner::Radio) {
            return None;
        }
        return Some(LARGE_RX_SLOTS[index].0.get().cast::<u8>());
    }
    let aggregate_index = index.checked_sub(LARGE_RX_SLOT_CAPACITY)?;
    if aggregate_index >= AGGREGATE_RX_SLOT_CAPACITY
        || !aggregate_rx_slot_claimed(aggregate_index)
        || aggregate_rx_owner(aggregate_index) != Some(RxBufferOwner::Radio)
    {
        return None;
    }
    Some(AGGREGATE_RX_HEADERS[aggregate_index].0.get().cast::<u8>())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LargeRxNetworkAdoptionError {
    NotRustPool,
    NotRadioOwned,
    InvalidView,
}

/// Unique, safe network-stack ownership of one kind-7 ESF frame.
///
/// The ABI address is reconstructed from `slot`; it is never exposed to the
/// network consumer. Dropping this value is the only Network -> Free
/// transition and returns the backing storage directly to the Rust pool.
pub(crate) struct OwnedLargeRxNetworkFrame {
    backing: LargeRxNetworkBacking,
    buffer_offset: u16,
    length: u16,
}

#[derive(Clone, Copy)]
enum LargeRxNetworkBacking {
    Internal { slot: u8 },
    Aggregate { slot: u8 },
}

impl OwnedLargeRxNetworkFrame {
    fn payload(&self) -> *mut u8 {
        match self.backing {
            LargeRxNetworkBacking::Internal { slot } => unsafe {
                LARGE_RX_SLOTS[usize::from(slot)]
                    .0
                    .get()
                    .cast::<u8>()
                    .add(ESF_HEADER_SIZE)
            },
            LargeRxNetworkBacking::Aggregate { slot } => AGGREGATE_RX_PAYLOADS[usize::from(slot)]
                .0
                .get()
                .cast::<u8>(),
        }
    }

    pub(crate) fn as_bytes(&self) -> &[u8] {
        let buffer = unsafe { self.payload().add(usize::from(self.buffer_offset)) };
        unsafe { core::slice::from_raw_parts(buffer, usize::from(self.length)) }
    }

    pub(crate) fn as_bytes_mut(&mut self) -> &mut [u8] {
        let buffer = unsafe { self.payload().add(usize::from(self.buffer_offset)) };
        unsafe { core::slice::from_raw_parts_mut(buffer, usize::from(self.length)) }
    }
}

impl Drop for OwnedLargeRxNetworkFrame {
    fn drop(&mut self) {
        let frame = match self.backing {
            LargeRxNetworkBacking::Internal { slot } => {
                LARGE_RX_SLOTS[usize::from(slot)].0.get().cast::<u8>()
            }
            LargeRxNetworkBacking::Aggregate { slot } => {
                AGGREGATE_RX_HEADERS[usize::from(slot)].0.get().cast::<u8>()
            }
        };
        unsafe {
            let buffer_descriptor = frame.add(0x04).cast::<*mut u8>().read();
            let payload = frame
                .add(ESF_BUFFER_POINTER_OFFSET)
                .cast::<*mut u8>()
                .read();
            if !buffer_descriptor.is_null() {
                buffer_descriptor.add(4).cast::<*mut u8>().write(payload);
            }
        }
        let released = match self.backing {
            LargeRxNetworkBacking::Internal { slot } => {
                release_large_rx_slot(usize::from(slot), RxBufferOwner::Network)
            }
            LargeRxNetworkBacking::Aggregate { slot } => {
                release_aggregate_rx_slot(usize::from(slot), RxBufferOwner::Network)
            }
        };
        if !released {
            reject(u32::MAX, frame as usize);
        }
    }
}

/// Transfer a live kind-7 ESF object from the radio owner to the safe network
/// channel.
///
/// This is the sole Radio -> Network ownership edge. A second callback for the
/// same object is rejected rather than creating another safe token.
///
/// # Safety
///
/// `frame` and `buffer` come from the pinned RX callback ABI. `buffer` must be
/// readable for `length` bytes while the radio still owns `frame`.
#[cfg_attr(target_arch = "riscv32", link_section = ".rwtext.wifi_strict.esf")]
pub(crate) unsafe fn adopt_large_rx_for_network(
    frame: *mut u8,
    buffer: *mut u8,
    length: usize,
) -> Result<OwnedLargeRxNetworkFrame, LargeRxNetworkAdoptionError> {
    if buffer.is_null() || length == 0 {
        return Err(LargeRxNetworkAdoptionError::InvalidView);
    }
    let (backing, start, capacity) = if let Some(index) = large_rx_slot_index(frame) {
        if !large_rx_slot_claimed(index) || large_rx_owner(index) != Some(RxBufferOwner::Radio) {
            return Err(LargeRxNetworkAdoptionError::NotRadioOwned);
        }
        (
            LargeRxNetworkBacking::Internal { slot: index as u8 },
            frame.add(ESF_HEADER_SIZE) as usize,
            LARGE_RX_PAYLOAD_CAPACITY,
        )
    } else if let Some(index) = aggregate_rx_slot_index(frame) {
        if !aggregate_rx_slot_claimed(index)
            || aggregate_rx_owner(index) != Some(RxBufferOwner::Radio)
        {
            return Err(LargeRxNetworkAdoptionError::NotRadioOwned);
        }
        (
            LargeRxNetworkBacking::Aggregate { slot: index as u8 },
            AGGREGATE_RX_PAYLOADS[index].0.get().cast::<u8>() as usize,
            AGGREGATE_RX_PAYLOAD_CAPACITY,
        )
    } else {
        return Err(LargeRxNetworkAdoptionError::NotRustPool);
    };
    let Some(end) = (buffer as usize).checked_add(length) else {
        return Err(LargeRxNetworkAdoptionError::InvalidView);
    };
    if (buffer as usize) < start || end > start + capacity {
        return Err(LargeRxNetworkAdoptionError::InvalidView);
    }
    if length > u16::MAX as usize {
        return Err(LargeRxNetworkAdoptionError::InvalidView);
    }
    let offset = buffer as usize - start;
    if offset > u16::MAX as usize {
        return Err(LargeRxNetworkAdoptionError::InvalidView);
    }
    let transferred = match backing {
        LargeRxNetworkBacking::Internal { slot } => {
            let index = usize::from(slot);
            let word_index = index / LARGE_RX_CLAIM_WORD_BITS;
            LARGE_RX_OWNERS[word_index].try_transfer_to_network(index % LARGE_RX_CLAIM_WORD_BITS)
        }
        LargeRxNetworkBacking::Aggregate { slot } => {
            AGGREGATE_RX_OWNERS.try_transfer_to_network(usize::from(slot))
        }
    };
    if !transferred {
        return Err(LargeRxNetworkAdoptionError::NotRadioOwned);
    };
    Ok(OwnedLargeRxNetworkFrame {
        backing,
        buffer_offset: offset as u16,
        length: length as u16,
    })
}

/// Return whether `frame` belongs to one of the fixed pools handled by the
/// strict recycler. The caller must hold a live ESF object.
#[cfg_attr(target_arch = "riscv32", link_section = ".rwtext.wifi_strict.esf")]
pub(crate) unsafe fn is_strict_recyclable_frame(frame: *mut u8) -> bool {
    management_slot_index(frame).is_some()
        || large_rx_slot_index(frame).is_some()
        || aggregate_rx_slot_index(frame).is_some()
        || is_vendor_static_kind(frame.add(ESF_TYPE_OFFSET).read() as u32)
}

#[cfg_attr(target_arch = "riscv32", link_section = ".rwtext.wifi_strict.esf")]
unsafe fn allocate_management(source: *const u8, kind: u32, length: usize) -> Option<*mut u8> {
    let index = claim_management_slot()?;
    let frame = MANAGEMENT_SLOTS[index].0.get().cast::<u8>();
    if initialize_frame(frame, kind, source, length, MANAGEMENT_PAYLOAD_CAPACITY).is_none() {
        CLAIMED_MANAGEMENT_SLOTS.fetch_and(!(1_usize << index), Ordering::AcqRel);
        return None;
    }
    Some(frame)
}

#[cfg_attr(target_arch = "riscv32", link_section = ".rwtext.wifi_strict.esf")]
unsafe fn allocate_large_rx(source: *const u8, length: usize) -> Option<*mut u8> {
    let index = claim_large_rx_slot()?;
    let frame = LARGE_RX_SLOTS[index].0.get().cast::<u8>();
    if initialize_frame(frame, 7, source, length, LARGE_RX_PAYLOAD_CAPACITY).is_none() {
        release_large_rx_slot(index, RxBufferOwner::Radio);
        return None;
    }
    Some(frame)
}

#[cfg_attr(target_arch = "riscv32", link_section = ".rwtext.wifi_strict.esf")]
unsafe fn allocate_aggregate_rx(length: usize) -> Option<*mut u8> {
    let index = claim_aggregate_rx_slot()?;
    let frame = AGGREGATE_RX_HEADERS[index].0.get().cast::<u8>();
    let payload = AGGREGATE_RX_PAYLOADS[index].0.get().cast::<u8>();
    if initialize_frame_with_payload(
        frame,
        payload,
        7,
        ptr::null(),
        length,
        AGGREGATE_RX_PAYLOAD_CAPACITY,
    )
    .is_none()
    {
        release_aggregate_rx_slot(index, RxBufferOwner::Radio);
        return None;
    }
    Some(frame)
}

#[cfg_attr(target_arch = "riscv32", link_section = ".rwtext.wifi_strict.esf")]
unsafe fn allocate_vendor_static(source: *const u8, kind: u32, length: usize) -> Option<*mut u8> {
    if length > u16::MAX as usize {
        return None;
    }
    let list = descriptor(kind);
    let interrupt_state = crate::critical::strict_wifi_int_disable();
    let frame = list.cast::<*mut u8>().read();
    if !frame.is_null() {
        list.cast::<*mut u8>()
            .write(frame.add(ESF_FREE_NEXT_OFFSET).cast::<*mut u8>().read());
        frame
            .add(ESF_FREE_NEXT_OFFSET)
            .cast::<*mut u8>()
            .write(ptr::null_mut());
    }
    crate::critical::strict_wifi_int_restore(interrupt_state);
    if frame.is_null() {
        return None;
    }

    // Strict buffers never carry a netstack ownership token. This keeps the
    // optional callback in `ieee80211_recycle_cache_eb` unreachable.
    frame.cast::<*mut c_void>().write(ptr::null_mut());
    let prefix = list.add(0x0c).read() as usize;
    frame
        .add(ESF_LENGTH_OFFSET)
        .cast::<u16>()
        .write(length as u16);
    let buffer_descriptor = frame.add(0x04).cast::<*mut u8>().read();
    if !buffer_descriptor.is_null() {
        let payload = frame
            .add(ESF_BUFFER_POINTER_OFFSET)
            .cast::<*mut u8>()
            .read();
        let data = payload.add(prefix);
        buffer_descriptor.add(4).cast::<*mut u8>().write(data);
        let packet_length = prefix.checked_add(length)? as u32;
        let flags = buffer_descriptor.cast::<u32>().read() & 0xffff_c000;
        buffer_descriptor
            .cast::<u32>()
            .write(flags | (packet_length & 0x3fff));
        if !source.is_null() {
            ptr::copy_nonoverlapping(source, data, length);
        }
    } else if !source.is_null() {
        recycle_vendor_static(frame, kind);
        return None;
    }

    let tx_descriptor = frame
        .add(ESF_TX_DESCRIPTOR_POINTER_OFFSET)
        .cast::<*mut u8>()
        .read();
    if tx_descriptor.is_null() {
        recycle_vendor_static(frame, kind);
        return None;
    }
    tx_descriptor
        .cast::<u32>()
        .write(tx_descriptor.cast::<u32>().read() | list.add(4).cast::<u32>().read());
    tx_descriptor
        .add(4)
        .cast::<u32>()
        .write(tx_descriptor.add(4).cast::<u32>().read() | 0x0f);
    Some(frame)
}

/// Claim one fixed ESF object for the Rust-owned single-descriptor RX path.
///
/// This reproduces the pinned selection order without entering the public
/// allocator wrapper: copy mode zero uses kind 7; copy mode one first uses the
/// finite kind-8 pool for inputs up to 500 bytes and immediately falls back to
/// kind 7 when that pool is empty. No failed preferred-pool claim is reported
/// as an allocation failure when the fallback succeeds.
#[cfg_attr(target_arch = "riscv32", link_section = ".rwtext.wifi_strict.esf")]
pub(crate) unsafe fn allocate_strict_received_frame(
    copy_mode: u32,
    descriptor_length: usize,
    large_length: usize,
) -> Option<(*mut u8, usize)> {
    if !crate::critical::strict_wifi_hart_armed() || !crate::critical::on_strict_wifi_hart() {
        reject(u32::MAX, descriptor_length);
        return None;
    }
    if copy_mode > 1 || descriptor_length > large_length {
        reject(u32::MAX, descriptor_length);
        return None;
    }
    if copy_mode != 0 && descriptor_length <= SMALL_RX_PAYLOAD_CAPACITY {
        if let Some(frame) = allocate_vendor_static(ptr::null(), 8, descriptor_length) {
            return Some((frame, descriptor_length));
        }
    }
    let frame = allocate_large_rx(ptr::null(), large_length);
    if frame.is_none() {
        reject(7, large_length);
    }
    frame.map(|frame| (frame, large_length))
}

/// Claim one split SRAM-header/PSRAM-payload ESF object for a bounded
/// multi-descriptor MPDU.
#[cfg_attr(target_arch = "riscv32", link_section = ".rwtext.wifi_strict.esf")]
pub(crate) unsafe fn allocate_strict_aggregate_received_frame(
    indicated_length: usize,
) -> Option<*mut u8> {
    if !crate::critical::strict_wifi_hart_armed()
        || !crate::critical::on_strict_wifi_hart()
        || indicated_length == 0
        || indicated_length > 0x3fff
    {
        reject(7, indicated_length);
        return None;
    }
    let frame = allocate_aggregate_rx(indicated_length);
    if frame.is_none() {
        reject(7, indicated_length);
    }
    frame
}

/// Populate the pinned S31 ESF layout for one singleton RX descriptor.
///
/// `wDev_IndicateFrame` copies the fixed 0x38-byte RX-control prefix, skips
/// the optional rounded sublength, then copies the MPDU bytes. The safe copy
/// plan owns every variable offset; this leaf contains only pointer chasing,
/// two finite copies and recovered ABI stores. The caller owns both objects
/// and must recycle `frame` if this function rejects malformed input.
///
/// # Safety
///
/// `frame` must be an outstanding kind-7 or kind-8 object returned by
/// [`allocate_strict_received_frame`]. `source` must be readable for
/// `descriptor_length` bytes, and the source and ESF payload must not overlap.
#[cfg_attr(target_arch = "riscv32", link_section = ".rwtext.wifi_strict.esf")]
pub(crate) unsafe fn populate_single_received_frame(
    frame: *mut u8,
    source: *const u8,
    descriptor_length: usize,
    allocated_length: usize,
    copy_plan: crate::rx_descriptor::SingleRxCopyPlan,
    timestamp: u32,
    rx_rate: u8,
    rx_channel: u8,
    aggregate: bool,
) -> bool {
    if frame.is_null()
        || source.is_null()
        || descriptor_length < 0x38
        || descriptor_length > allocated_length
        || copy_plan.source_payload_offset > descriptor_length
        || copy_plan.payload_length != descriptor_length - copy_plan.source_payload_offset
        || copy_plan.indicated_length > allocated_length
    {
        return false;
    }
    let kind = frame.add(ESF_TYPE_OFFSET).read();
    if kind != 7 && kind != 8 {
        return false;
    }
    let buffer_descriptor = frame
        .add(ESF_BUFFER_DESCRIPTOR_POINTER_OFFSET)
        .cast::<*mut u8>()
        .read();
    let rx_descriptor = frame
        .add(ESF_TX_DESCRIPTOR_POINTER_OFFSET)
        .cast::<*mut u8>()
        .read();
    if buffer_descriptor.is_null() || rx_descriptor.is_null() {
        return false;
    }
    let destination = buffer_descriptor.add(4).cast::<*mut u8>().read();
    if destination.is_null() {
        return false;
    }
    let Some(descriptor_word) = crate::rx_descriptor::indicated_rx_descriptor_word(
        buffer_descriptor.cast::<u32>().read(),
        copy_plan.indicated_length,
    ) else {
        return false;
    };

    ptr::copy_nonoverlapping(source, destination, 0x38);
    ptr::copy_nonoverlapping(
        source.add(copy_plan.source_payload_offset),
        destination.add(0x38),
        copy_plan.payload_length,
    );
    buffer_descriptor.cast::<u32>().write(descriptor_word);
    rx_descriptor.add(4).cast::<u32>().write(timestamp);
    rx_descriptor.add(8).write(rx_rate);
    rx_descriptor.add(9).write(rx_channel);
    let flags = crate::rx_descriptor::indicated_rx_flags_word(
        rx_descriptor.cast::<u32>().read(),
        1,
        aggregate,
    );
    rx_descriptor.cast::<u32>().write(flags);
    true
}

/// Join one hardware descriptor chain into a contiguous kind-7 ESF payload.
///
/// All sizes and iteration counts come from a prevalidated safe copy plan.
/// Each descriptor contributes exactly one finite copy; the raw chain is
/// required to terminate at `tail` after `descriptor_count` nodes.
///
/// # Safety
///
/// `frame` must be a radio-owned object returned by
/// [`allocate_strict_aggregate_received_frame`]. `head..=tail` must be the
/// detached hardware-owned chain described by `copy_plan`, and every buffer
/// pointer must remain readable for its selected segment length.
#[cfg_attr(target_arch = "riscv32", link_section = ".rwtext.wifi_strict.esf")]
pub(crate) unsafe fn populate_multi_received_frame(
    frame: *mut u8,
    head: *mut u8,
    tail: *mut u8,
    copy_plan: crate::rx_descriptor::MultiRxCopyPlan,
    timestamp: u32,
    rx_rate: u8,
    rx_channel: u8,
    aggregate: bool,
) -> bool {
    if frame.is_null()
        || head.is_null()
        || tail.is_null()
        || head == tail
        || copy_plan.descriptor_count < 2
        || copy_plan.descriptor_count > 64
        || copy_plan.segment_capacity < 0x38
        || copy_plan.first_payload_length != copy_plan.segment_capacity - 0x38
        || copy_plan.middle_descriptor_count != copy_plan.descriptor_count - 2
        || copy_plan.indicated_length > 0x3fff
    {
        return false;
    }
    let Some(index) = aggregate_rx_slot_index(frame) else {
        return false;
    };
    if !aggregate_rx_slot_claimed(index) || aggregate_rx_owner(index) != Some(RxBufferOwner::Radio)
    {
        return false;
    }
    let buffer_descriptor = frame
        .add(ESF_BUFFER_DESCRIPTOR_POINTER_OFFSET)
        .cast::<*mut u8>()
        .read();
    let rx_descriptor = frame
        .add(ESF_TX_DESCRIPTOR_POINTER_OFFSET)
        .cast::<*mut u8>()
        .read();
    if buffer_descriptor.is_null() || rx_descriptor.is_null() {
        return false;
    }
    let destination = buffer_descriptor.add(4).cast::<*mut u8>().read();
    if destination.is_null() {
        return false;
    }
    let Some(descriptor_word) = crate::rx_descriptor::indicated_rx_descriptor_word(
        buffer_descriptor.cast::<u32>().read(),
        copy_plan.indicated_length,
    ) else {
        return false;
    };

    let mut descriptor_node = head;
    let first_source = descriptor_node.add(4).cast::<*mut u8>().read_unaligned();
    if first_source.is_null()
        || crate::rx_descriptor::descriptor_buffer_length(
            descriptor_node.cast::<u32>().read_unaligned(),
        ) != copy_plan.segment_capacity
    {
        return false;
    }
    ptr::copy_nonoverlapping(first_source, destination, 0x38);
    ptr::copy_nonoverlapping(
        first_source.add(0x38),
        destination.add(0x38),
        copy_plan.first_payload_length,
    );
    let mut destination_offset = copy_plan.segment_capacity;
    let mut descriptor_index = 1;
    while descriptor_index < copy_plan.descriptor_count {
        descriptor_node = descriptor_node.add(8).cast::<*mut u8>().read_unaligned();
        if descriptor_node.is_null() {
            return false;
        }
        let word = descriptor_node.cast::<u32>().read_unaligned();
        if crate::rx_descriptor::descriptor_buffer_length(word) != copy_plan.segment_capacity {
            return false;
        }
        let source = descriptor_node.add(4).cast::<*mut u8>().read_unaligned();
        if source.is_null() {
            return false;
        }
        let chunk_length = if descriptor_index + 1 == copy_plan.descriptor_count {
            if descriptor_node != tail
                || crate::rx_descriptor::descriptor_received_length(word)
                    != copy_plan.tail_payload_length
            {
                return false;
            }
            copy_plan.tail_payload_length
        } else {
            copy_plan.segment_capacity
        };
        ptr::copy_nonoverlapping(source, destination.add(destination_offset), chunk_length);
        destination_offset += chunk_length;
        descriptor_index += 1;
    }
    if descriptor_node != tail || destination_offset != copy_plan.indicated_length {
        return false;
    }

    buffer_descriptor.cast::<u32>().write(descriptor_word);
    rx_descriptor.add(4).cast::<u32>().write(timestamp);
    rx_descriptor.add(8).write(rx_rate);
    rx_descriptor.add(9).write(rx_channel);
    let flags = crate::rx_descriptor::indicated_rx_flags_word(
        rx_descriptor.cast::<u32>().read(),
        copy_plan.descriptor_count as u32,
        aggregate,
    );
    rx_descriptor.cast::<u32>().write(flags);
    true
}

#[cfg_attr(target_arch = "riscv32", link_section = ".rwtext.wifi_strict.esf")]
unsafe fn recycle_vendor_static(frame: *mut u8, kind: u32) {
    let tx_descriptor = frame
        .add(ESF_TX_DESCRIPTOR_POINTER_OFFSET)
        .cast::<*mut u8>()
        .read();
    if !tx_descriptor.is_null() {
        tx_descriptor.write_bytes(0, 0x48);
    }
    frame.add(0x24).cast::<u32>().write(0);
    frame.add(0x14).cast::<u16>().write(0);
    frame.add(0x22).cast::<u16>().write(0);
    frame.add(0x28).write(0);
    frame.add(0x38).cast::<u16>().write(0);
    frame.add(0x3a).write(0);

    let list = descriptor(kind);
    let interrupt_state = crate::critical::strict_wifi_int_disable();
    frame
        .add(ESF_FREE_NEXT_OFFSET)
        .cast::<*mut u8>()
        .write(list.cast::<*mut u8>().read());
    list.cast::<*mut u8>().write(frame);
    crate::critical::strict_wifi_int_restore(interrupt_state);
}

/// Final-link replacement for the vendor ESF allocator.
///
/// Before strict mode it delegates to the original initialization path. The
/// strict path uses only initialized vendor free lists or the Rust management
/// pool and returns null immediately on exhaustion.
///
/// # Safety
///
/// `source`, when non-null, must be valid for `length` readable bytes and must
/// not overlap the selected ESF payload. `kind` must follow the vendor ABI.
#[no_mangle]
#[cfg_attr(target_arch = "riscv32", link_section = ".rwtext.wifi_strict.esf")]
pub unsafe extern "C" fn __wrap_esf_buf_alloc(
    source: *const u8,
    kind: u32,
    length: u32,
) -> *mut u8 {
    if !crate::critical::strict_wifi_hart_armed() {
        if prearm_management_pool_enabled() && is_management_kind(kind) {
            if !on_prearm_management_hart() {
                reject(kind, length as usize);
                return ptr::null_mut();
            }
            let frame = allocate_management(source, kind, length as usize);
            if frame.is_none() {
                reject(kind, length as usize);
            }
            return frame.unwrap_or(ptr::null_mut());
        }
        return __real_esf_buf_alloc(source, kind, length);
    }
    if !crate::critical::on_strict_wifi_hart() {
        reject(kind, length as usize);
        return ptr::null_mut();
    }
    let frame = if is_management_kind(kind) {
        allocate_management(source, kind, length as usize)
    } else if kind == 7 {
        allocate_large_rx(source, length as usize)
    } else if is_vendor_static_kind(kind) {
        allocate_vendor_static(source, kind, length as usize)
    } else {
        None
    };
    if frame.is_none() {
        reject(kind, length as usize);
    }
    frame.unwrap_or(ptr::null_mut())
}

/// Allocate and expose a bounded management-frame body from the Rust ESF pool.
///
/// Reference: the complete pinned
/// `libnet80211.a[ieee80211_ets.o]::ieee80211_getmgtframe` body, size `0x5c`.
/// It rounds `header + body` to four bytes, selects ESF kind 3 up to 64 bytes,
/// kind 2 up to 256 bytes and kind 4 above that, then exposes
/// `descriptor.data + header` while storing the unrounded body length at ESF
/// offset `0x16`.
///
/// This replacement calls the already interposed allocation-free ESF
/// boundary. In strict/prearmed operation the returned object has one
/// Rust-owned management-slot token; before prearm the ESF boundary may still
/// delegate to the cold vendor allocator. There is no wait, delay, retry loop,
/// heap call, or second ownership ledger here.
///
/// # Safety
///
/// `body_out` must be valid for one pointer write. A successful returned ESF
/// object must be recycled exactly once through the matching ESF boundary.
#[cfg(target_arch = "riscv32")]
#[no_mangle]
pub unsafe extern "C" fn wifi_strict_ieee80211_getmgtframe(
    body_out: *mut *mut u8,
    header_length: u32,
    body_length: u32,
) -> *mut u8 {
    if body_out.is_null() || body_length > u16::MAX as u32 {
        return ptr::null_mut();
    }
    let Some((kind, allocation_length)) = management_frame_allocation(header_length, body_length)
    else {
        return ptr::null_mut();
    };
    let frame = __wrap_esf_buf_alloc(ptr::null(), kind, allocation_length);
    if frame.is_null() {
        return ptr::null_mut();
    }

    let buffer_descriptor = frame.add(0x04).cast::<*mut u8>().read();
    if buffer_descriptor.is_null() {
        __wrap_esf_buf_recycle(frame.cast());
        return ptr::null_mut();
    }
    let data = buffer_descriptor.add(0x04).cast::<*mut u8>().read();
    if data.is_null() {
        __wrap_esf_buf_recycle(frame.cast());
        return ptr::null_mut();
    }

    body_out.write(data.add(header_length as usize));
    frame
        .add(ESF_LENGTH_OFFSET)
        .cast::<u16>()
        .write(body_length as u16);
    frame
}

/// Final-link replacement for vendor ESF recycling.
///
/// # Safety
///
/// `frame` must be null or an outstanding ESF object returned by the matching
/// allocator. Recycling transfers the object back to its fixed pool.
#[no_mangle]
#[cfg_attr(target_arch = "riscv32", link_section = ".rwtext.wifi_strict.esf")]
pub unsafe extern "C" fn __wrap_esf_buf_recycle(frame: *mut c_void) {
    if !crate::critical::strict_wifi_hart_armed() {
        if !frame.is_null() {
            let strict_frame = frame.cast::<u8>();
            if let Some(index) = management_slot_index(strict_frame) {
                if !on_prearm_management_hart() {
                    reject(u32::MAX, strict_frame as usize);
                    return;
                }
                let bit = 1_usize << index;
                if CLAIMED_MANAGEMENT_SLOTS.fetch_and(!bit, Ordering::AcqRel) & bit == 0 {
                    reject(u32::MAX, strict_frame as usize);
                }
                return;
            }
        }
        __real_esf_buf_recycle(frame);
        return;
    }
    if frame.is_null() {
        return;
    }
    if !crate::critical::on_strict_wifi_hart() {
        reject(u32::MAX, frame as usize);
        return;
    }
    let frame = frame.cast::<u8>();
    if let Some(index) = management_slot_index(frame) {
        let bit = 1_usize << index;
        if CLAIMED_MANAGEMENT_SLOTS.fetch_and(!bit, Ordering::AcqRel) & bit == 0 {
            reject(u32::MAX, frame as usize);
        }
        return;
    }
    if let Some(index) = large_rx_slot_index(frame) {
        if !release_large_rx_slot(index, RxBufferOwner::Radio) {
            reject(u32::MAX, frame as usize);
        }
        return;
    }
    if let Some(index) = aggregate_rx_slot_index(frame) {
        if !release_aggregate_rx_slot(index, RxBufferOwner::Radio) {
            reject(u32::MAX, frame as usize);
        }
        return;
    }
    let kind = frame.add(ESF_TYPE_OFFSET).read() as u32;
    if !is_vendor_static_kind(kind) {
        reject(kind, frame as usize);
        return;
    }
    recycle_vendor_static(frame, kind);
}

/// Allocation-free replacement for the pinned PP RX recycle leaf.
///
/// The packet must be owned by one of the admitted fixed ESF pools. Rust
/// restores the original RX buffer view and transfers the object directly to
/// the matching pool; no vendor PP state, allocator or OSI primitive remains.
/// The linker interception below retains the cold pre-handoff delegation.
///
/// # Safety
///
/// `frame` must be an outstanding radio-owned RX ESF object. Calling this
/// function transfers that ownership back to the fixed pool exactly once.
#[cfg_attr(target_arch = "riscv32", link_section = ".rwtext.wifi_strict.esf")]
pub(crate) unsafe fn recycle_received_packet(frame: *mut u8) {
    if frame.is_null()
        || !crate::critical::strict_wifi_hart_armed()
        || !crate::critical::on_strict_wifi_hart()
        || !is_strict_recyclable_frame(frame)
    {
        reject(u32::MAX, frame as usize);
        return;
    }
    if !crate::rx_descriptor::restore_received_packet_buffer_view(frame) {
        // The pool still owns the ESF object even when its protocol view was
        // malformed. Record the invariant violation, then release that owner
        // rather than leaking a finite RX credit.
        reject(u32::MAX, frame as usize);
    }
    __wrap_esf_buf_recycle(frame.cast());
}

/// Linker interception for remaining calls through the pinned public symbol.
///
/// Rust RX code calls [`recycle_received_packet`] directly. This ABI boundary
/// remains because ROM and archive code may still reference the public leaf.
/// A unique name is required: the S31 ROM linker fragment exports
/// `ppRecycleRxPkt` as an absolute symbol and would otherwise capture GNU
/// `--wrap`'s generated name before LTO can retain this function.
///
/// # Safety
///
/// `frame` follows the same ownership contract as
/// [`recycle_received_packet`].
#[no_mangle]
#[cfg_attr(target_arch = "riscv32", link_section = ".rwtext.wifi_strict.esf")]
pub unsafe extern "C" fn wifi_strict_pp_recycle_rx_pkt(frame: *mut u8) {
    if !crate::critical::strict_wifi_hart_armed() {
        __real_ppRecycleRxPkt(frame);
        return;
    }
    recycle_received_packet(frame);
}

/// Rust-owned implementation of the public RX-buffer release API.
///
/// The pinned `libpp.a[if_hwctrl.o]` body is exactly eight bytes and only
/// tail-calls `ppRecycleRxPkt` with the unchanged argument. Keep that ABI, but
/// route it through the already qualified fixed-pool Rust owner. This boundary
/// contains no mutex, wait, queue operation, allocation, or hidden state.
///
/// The public `esp_wifi_internal_free_rx_buffer` name is assigned to this
/// unique symbol by the late linker fragment. A unique implementation name
/// also lets the final-ELF audit distinguish this code from the unextracted
/// archive leaf.
///
/// # Safety
///
/// `frame` must be the outstanding ESF owner supplied as the third argument of
/// a registered Wi-Fi RX callback. The call consumes that owner exactly once.
#[no_mangle]
#[cfg_attr(target_arch = "riscv32", link_section = ".rwtext.wifi_strict.esf")]
pub unsafe extern "C" fn wifi_strict_esp_wifi_internal_free_rx_buffer(frame: *mut c_void) {
    wifi_strict_pp_recycle_rx_pkt(frame.cast());
}

pub fn rejected_esf_operations() -> usize {
    REJECTED_ESF_OPERATIONS.load(Ordering::Acquire)
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct FixedEsfPoolSnapshot {
    pub management_claimed: usize,
    pub management_capacity: usize,
    pub large_rx_claimed: usize,
    pub large_rx_radio_owned: usize,
    pub large_rx_network_owned: usize,
    pub large_rx_capacity: usize,
    pub aggregate_rx_claimed: usize,
    pub aggregate_rx_radio_owned: usize,
    pub aggregate_rx_network_owned: usize,
    pub aggregate_rx_capacity: usize,
    pub rejected_operations: usize,
    pub last_rejected_kind: u32,
    pub last_rejected_argument: usize,
}

pub fn fixed_esf_pool_snapshot() -> FixedEsfPoolSnapshot {
    FixedEsfPoolSnapshot {
        management_claimed: CLAIMED_MANAGEMENT_SLOTS
            .load(Ordering::Acquire)
            .count_ones() as usize,
        management_capacity: MANAGEMENT_SLOT_CAPACITY,
        large_rx_claimed: LARGE_RX_OWNERS
            .iter()
            .map(|ownership| ownership.claimed_bits().count_ones() as usize)
            .sum(),
        large_rx_radio_owned: (0..LARGE_RX_SLOT_CAPACITY)
            .filter(|index| large_rx_owner(*index) == Some(RxBufferOwner::Radio))
            .count(),
        large_rx_network_owned: (0..LARGE_RX_SLOT_CAPACITY)
            .filter(|index| large_rx_owner(*index) == Some(RxBufferOwner::Network))
            .count(),
        large_rx_capacity: LARGE_RX_SLOT_CAPACITY,
        aggregate_rx_claimed: AGGREGATE_RX_OWNERS.claimed_bits().count_ones() as usize,
        aggregate_rx_radio_owned: (0..AGGREGATE_RX_SLOT_CAPACITY)
            .filter(|index| aggregate_rx_owner(*index) == Some(RxBufferOwner::Radio))
            .count(),
        aggregate_rx_network_owned: (0..AGGREGATE_RX_SLOT_CAPACITY)
            .filter(|index| aggregate_rx_owner(*index) == Some(RxBufferOwner::Network))
            .count(),
        aggregate_rx_capacity: AGGREGATE_RX_SLOT_CAPACITY,
        rejected_operations: rejected_esf_operations(),
        last_rejected_kind: LAST_REJECTED_ESF_KIND.load(Ordering::Acquire) as u32,
        last_rejected_argument: LAST_REJECTED_ESF_ARGUMENT.load(Ordering::Acquire),
    }
}

const _: () = assert!(mem::size_of::<ManagementSlot>() == MANAGEMENT_SLOT_SIZE);
const _: () = assert!(MANAGEMENT_SLOT_CAPACITY < usize::BITS as usize);
const _: () = assert!(mem::size_of::<LargeRxSlot>() == LARGE_RX_SLOT_SIZE);
const _: () = assert!(LARGE_RX_SLOT_CAPACITY <= u8::MAX as usize);
const _: () = assert!(LARGE_RX_CLAIM_WORDS <= 2);
const _: () = assert!(LARGE_RX_SLOT_CAPACITY == crate::rx_ampdu::RX_ESF_SLOT_ID_CAPACITY);
const _: () = assert!(mem::size_of::<AggregateRxHeader>() == ESF_HEADER_SIZE);
const _: () = assert!(mem::size_of::<AggregateRxPayload>() == AGGREGATE_RX_PAYLOAD_CAPACITY);
const _: () = assert!(AGGREGATE_RX_SLOT_CAPACITY < usize::BITS as usize);
const _: () = assert!(
    LARGE_RX_SLOT_CAPACITY + AGGREGATE_RX_SLOT_CAPACITY
        == crate::rx_ampdu::RX_REORDER_SLOT_ID_CAPACITY
);

#[cfg(test)]
mod tests {
    use super::management_frame_allocation;

    #[test]
    fn management_frame_size_classes_match_the_pinned_allocator_wrapper() {
        assert_eq!(management_frame_allocation(0, 0), Some((3, 0)));
        assert_eq!(management_frame_allocation(24, 40), Some((3, 64)));
        assert_eq!(management_frame_allocation(24, 41), Some((2, 68)));
        assert_eq!(management_frame_allocation(24, 232), Some((2, 256)));
        assert_eq!(management_frame_allocation(24, 233), Some((4, 260)));
        assert_eq!(management_frame_allocation(1, 1), Some((3, 4)));
    }

    #[test]
    fn management_frame_rounding_rejects_u32_overflow() {
        assert_eq!(management_frame_allocation(u32::MAX, 1), None);
        assert_eq!(management_frame_allocation(u32::MAX - 2, 0), None);
        assert_eq!(
            management_frame_allocation(u32::MAX - 3, 0),
            Some((4, u32::MAX - 3))
        );
    }
}
