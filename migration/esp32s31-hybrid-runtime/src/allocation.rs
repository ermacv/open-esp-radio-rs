#![cfg_attr(not(target_arch = "riscv32"), allow(dead_code))]

use core::sync::atomic::{AtomicUsize, Ordering};

use crate::context::in_radio_context;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(usize)]
pub enum AllocationSource {
    None = 0,
    DirectMalloc = 1,
    DirectCalloc = 2,
    DirectRealloc = 3,
    OsiMalloc = 4,
    OsiMallocInternal = 5,
    OsiReallocInternal = 6,
    OsiCallocInternal = 7,
    OsiZallocInternal = 8,
    OsiWifiMalloc = 9,
    OsiWifiRealloc = 10,
    OsiWifiCalloc = 11,
    OsiWifiZalloc = 12,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AllocationSnapshot {
    pub allocations: usize,
    pub reallocations: usize,
    pub frees: usize,
    pub requested_bytes: usize,
    pub largest_request: usize,
    pub failures: usize,
    pub radio_context_calls: usize,
    pub last_failure_source: AllocationSource,
    pub last_failure_size: usize,
    pub last_failure_caller: usize,
    pub last_free_pointer: usize,
    pub last_free_caller: usize,
}

#[cfg(feature = "hil-cold-allocation-trace")]
pub const COLD_ALLOCATION_TRACE_CAPACITY: usize = 128;

#[cfg(feature = "hil-cold-allocation-trace")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ColdAllocationTraceEntry {
    pub source: AllocationSource,
    pub caller: usize,
    pub size: usize,
    pub pointer: usize,
    pub realloc: bool,
    pub failed: bool,
}

#[cfg(feature = "hil-cold-allocation-trace")]
struct ColdAllocationTraceSlot {
    source: AtomicUsize,
    caller: AtomicUsize,
    size: AtomicUsize,
    pointer: AtomicUsize,
    flags: AtomicUsize,
}

#[cfg(feature = "hil-cold-allocation-trace")]
impl ColdAllocationTraceSlot {
    const fn new() -> Self {
        Self {
            source: AtomicUsize::new(0),
            caller: AtomicUsize::new(0),
            size: AtomicUsize::new(0),
            pointer: AtomicUsize::new(0),
            flags: AtomicUsize::new(0),
        }
    }
}

#[cfg(feature = "hil-cold-allocation-trace")]
static COLD_ALLOCATION_TRACE_LENGTH: AtomicUsize = AtomicUsize::new(0);
#[cfg(feature = "hil-cold-allocation-trace")]
// This is a laboratory-only cold-start journal. It is written before the
// working radio path is handed to the async executor and is never touched by
// an interrupt handler, so it must not consume the IRQ-critical SRAM arena.
#[link_section = ".psram.bss.wifi_strict.cold_allocation_trace"]
static COLD_ALLOCATION_TRACE: [ColdAllocationTraceSlot; COLD_ALLOCATION_TRACE_CAPACITY] =
    [const { ColdAllocationTraceSlot::new() }; COLD_ALLOCATION_TRACE_CAPACITY];

#[cfg(feature = "hil-cold-allocation-trace")]
fn record_cold_allocation(
    source: AllocationSource,
    caller: usize,
    size: usize,
    pointer: usize,
    realloc: bool,
    failed: bool,
) {
    let index = COLD_ALLOCATION_TRACE_LENGTH.fetch_add(1, Ordering::AcqRel);
    let Some(slot) = COLD_ALLOCATION_TRACE.get(index) else {
        return;
    };
    slot.source.store(source as usize, Ordering::Relaxed);
    slot.caller.store(caller, Ordering::Relaxed);
    slot.size.store(size, Ordering::Relaxed);
    slot.pointer.store(pointer, Ordering::Relaxed);
    slot.flags.store(
        4 | usize::from(realloc) | (usize::from(failed) << 1),
        Ordering::Release,
    );
}

#[cfg(feature = "hil-cold-allocation-trace")]
pub fn cold_allocation_trace_len() -> usize {
    COLD_ALLOCATION_TRACE_LENGTH
        .load(Ordering::Acquire)
        .min(COLD_ALLOCATION_TRACE_CAPACITY)
}

#[cfg(feature = "hil-cold-allocation-trace")]
pub fn cold_allocation_trace_overflow() -> usize {
    COLD_ALLOCATION_TRACE_LENGTH
        .load(Ordering::Acquire)
        .saturating_sub(COLD_ALLOCATION_TRACE_CAPACITY)
}

#[cfg(feature = "hil-cold-allocation-trace")]
pub fn cold_allocation_trace_entry(index: usize) -> Option<ColdAllocationTraceEntry> {
    let slot = COLD_ALLOCATION_TRACE.get(index)?;
    if index >= cold_allocation_trace_len() {
        return None;
    }
    let flags = slot.flags.load(Ordering::Acquire);
    if flags & 4 == 0 {
        return None;
    }
    Some(ColdAllocationTraceEntry {
        source: AllocationSource::from_raw(slot.source.load(Ordering::Relaxed)),
        caller: slot.caller.load(Ordering::Relaxed),
        size: slot.size.load(Ordering::Relaxed),
        pointer: slot.pointer.load(Ordering::Relaxed),
        realloc: flags & 1 != 0,
        failed: flags & 2 != 0,
    })
}

/// Counters shared by OSI allocator callbacks and final-link `__wrap_*`
/// guards for direct C allocator references in vendor archives.
pub struct AllocationProbe {
    allocations: AtomicUsize,
    reallocations: AtomicUsize,
    frees: AtomicUsize,
    requested_bytes: AtomicUsize,
    largest_request: AtomicUsize,
    failures: AtomicUsize,
    radio_context_calls: AtomicUsize,
    last_failure_source: AtomicUsize,
    last_failure_size: AtomicUsize,
    last_failure_caller: AtomicUsize,
    last_free_pointer: AtomicUsize,
    last_free_caller: AtomicUsize,
}

impl AllocationProbe {
    pub const fn new() -> Self {
        Self {
            allocations: AtomicUsize::new(0),
            reallocations: AtomicUsize::new(0),
            frees: AtomicUsize::new(0),
            requested_bytes: AtomicUsize::new(0),
            largest_request: AtomicUsize::new(0),
            failures: AtomicUsize::new(0),
            radio_context_calls: AtomicUsize::new(0),
            last_failure_source: AtomicUsize::new(AllocationSource::None as usize),
            last_failure_size: AtomicUsize::new(0),
            last_failure_caller: AtomicUsize::new(0),
            last_free_pointer: AtomicUsize::new(0),
            last_free_caller: AtomicUsize::new(0),
        }
    }

    fn record_request(&self, size: usize, failed: bool, realloc: bool) {
        self.record_request_at(size, failed, realloc, AllocationSource::None, 0, 0);
    }

    fn record_request_at(
        &self,
        size: usize,
        failed: bool,
        realloc: bool,
        source: AllocationSource,
        caller: usize,
        pointer: usize,
    ) {
        if realloc {
            self.reallocations.fetch_add(1, Ordering::Relaxed);
        } else {
            self.allocations.fetch_add(1, Ordering::Relaxed);
        }
        self.requested_bytes.fetch_add(size, Ordering::Relaxed);
        self.largest_request.fetch_max(size, Ordering::Relaxed);
        if failed {
            self.failures.fetch_add(1, Ordering::Relaxed);
            self.last_failure_source
                .store(source as usize, Ordering::Relaxed);
            self.last_failure_size.store(size, Ordering::Relaxed);
            self.last_failure_caller.store(caller, Ordering::Relaxed);
        }
        if in_radio_context() {
            self.radio_context_calls.fetch_add(1, Ordering::Relaxed);
        }
        #[cfg(feature = "hil-cold-allocation-trace")]
        record_cold_allocation(source, caller, size, pointer, realloc, failed);
    }

    fn record_free(&self) {
        self.record_free_at(0, 0);
    }

    fn record_free_at(&self, pointer: usize, caller: usize) {
        self.frees.fetch_add(1, Ordering::Relaxed);
        self.last_free_pointer.store(pointer, Ordering::Relaxed);
        self.last_free_caller.store(caller, Ordering::Relaxed);
        if in_radio_context() {
            self.radio_context_calls.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn snapshot(&self) -> AllocationSnapshot {
        AllocationSnapshot {
            allocations: self.allocations.load(Ordering::Acquire),
            reallocations: self.reallocations.load(Ordering::Acquire),
            frees: self.frees.load(Ordering::Acquire),
            requested_bytes: self.requested_bytes.load(Ordering::Acquire),
            largest_request: self.largest_request.load(Ordering::Acquire),
            failures: self.failures.load(Ordering::Acquire),
            radio_context_calls: self.radio_context_calls.load(Ordering::Acquire),
            last_failure_source: AllocationSource::from_raw(
                self.last_failure_source.load(Ordering::Acquire),
            ),
            last_failure_size: self.last_failure_size.load(Ordering::Acquire),
            last_failure_caller: self.last_failure_caller.load(Ordering::Acquire),
            last_free_pointer: self.last_free_pointer.load(Ordering::Acquire),
            last_free_caller: self.last_free_caller.load(Ordering::Acquire),
        }
    }
}

impl AllocationSource {
    const fn from_raw(raw: usize) -> Self {
        match raw {
            1 => Self::DirectMalloc,
            2 => Self::DirectCalloc,
            3 => Self::DirectRealloc,
            4 => Self::OsiMalloc,
            5 => Self::OsiMallocInternal,
            6 => Self::OsiReallocInternal,
            7 => Self::OsiCallocInternal,
            8 => Self::OsiZallocInternal,
            9 => Self::OsiWifiMalloc,
            10 => Self::OsiWifiRealloc,
            11 => Self::OsiWifiCalloc,
            12 => Self::OsiWifiZalloc,
            _ => Self::None,
        }
    }
}

impl Default for AllocationProbe {
    fn default() -> Self {
        Self::new()
    }
}

static PROBE: AllocationProbe = AllocationProbe::new();

pub fn allocation_probe() -> &'static AllocationProbe {
    &PROBE
}

#[cfg(feature = "rust-static-wifi-nvs-storage")]
const WIFI_NVS_CFG_ITEMS_SIZE: usize = 89 * 52;
#[cfg(feature = "rust-static-wifi-nvs-storage")]
const WIFI_NVS_LOAD_SCRATCH_SIZE: usize = 1024;
// Return address after the pinned `_wifi_zalloc(4628)` in
// `wifi_nvs_cfg_init`.
#[cfg(feature = "rust-static-wifi-nvs-storage")]
const WIFI_NVS_CFG_ITEMS_RETURN_OFFSET: usize = 0x46;
// `wifi_nvs_load` is local to the blob object, so anchor its return PC to the
// exported `wifi_nvs_cfg_init` symbol from the same object. The allocation
// returns at wifi_nvs_cfg_init + 0x13b6 in the pinned S31 archive.
#[cfg(feature = "rust-static-wifi-nvs-storage")]
const WIFI_NVS_LOAD_SCRATCH_RETURN_OFFSET: usize = 0x13b6;

#[cfg(feature = "rust-static-wifi-nvs-storage")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WifiNvsStaticAllocation {
    ConfigItems,
    LoadScratch,
}

#[cfg(feature = "rust-static-wifi-nvs-storage")]
fn classify_wifi_nvs_static_allocation(
    source: AllocationSource,
    size: usize,
    caller: usize,
    wifi_nvs_cfg_init_address: usize,
) -> Option<WifiNvsStaticAllocation> {
    match (source, size, caller.wrapping_sub(wifi_nvs_cfg_init_address)) {
        (
            AllocationSource::OsiWifiZalloc,
            WIFI_NVS_CFG_ITEMS_SIZE,
            WIFI_NVS_CFG_ITEMS_RETURN_OFFSET,
        ) => Some(WifiNvsStaticAllocation::ConfigItems),
        (
            AllocationSource::OsiMallocInternal,
            WIFI_NVS_LOAD_SCRATCH_SIZE,
            WIFI_NVS_LOAD_SCRATCH_RETURN_OFFSET,
        ) => Some(WifiNvsStaticAllocation::LoadScratch),
        _ => None,
    }
}

#[cfg(feature = "rust-static-function-table-storage")]
const WDEV_FUNCTION_TABLE_SIZE: usize = 1560;
#[cfg(feature = "rust-static-function-table-storage")]
const NET80211_FUNCTION_TABLE_SIZE: usize = 332;
#[cfg(feature = "rust-static-function-table-storage")]
const WDEV_FUNCTION_TABLE_RETURN_OFFSET: usize =
    if cfg!(feature = "rust-static-bindings-interpose") {
        // Replacing the preceding `wdev_data_init` call changes relaxation in
        // the pinned `wdev_funcs_init` body. The OSI calloc returns at +0x36
        // in the Rust-published profile and at +0x34 in the vendor-reference
        // profile.
        0x36
    } else {
        0x34
    };
#[cfg(feature = "rust-static-function-table-storage")]
const NET80211_FUNCTION_TABLE_RETURN_OFFSET: usize =
    if cfg!(feature = "rust-static-bindings-interpose") {
        // The adjacent `net80211_data_ptr_init` replacement similarly moves
        // the return from the fixed OSI calloc by two bytes.
        0x32
    } else {
        0x30
    };

#[cfg(feature = "rust-static-function-table-storage")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StaticFunctionTable {
    Wdev,
    Net80211,
}

#[cfg(feature = "rust-static-function-table-storage")]
fn classify_static_function_table(
    source: AllocationSource,
    size: usize,
    caller: usize,
    wdev_funcs_init_address: usize,
    net80211_funcs_init_address: usize,
) -> Option<StaticFunctionTable> {
    match (source, size) {
        (AllocationSource::OsiCallocInternal, WDEV_FUNCTION_TABLE_SIZE)
            if caller.wrapping_sub(wdev_funcs_init_address)
                == WDEV_FUNCTION_TABLE_RETURN_OFFSET =>
        {
            Some(StaticFunctionTable::Wdev)
        }
        (AllocationSource::OsiCallocInternal, NET80211_FUNCTION_TABLE_SIZE)
            if caller.wrapping_sub(net80211_funcs_init_address)
                == NET80211_FUNCTION_TABLE_RETURN_OFFSET =>
        {
            Some(StaticFunctionTable::Net80211)
        }
        _ => None,
    }
}

#[cfg(feature = "rust-static-interface-storage")]
const WIFI_INTERFACE_STATE_SIZE: usize = 612;
#[cfg(feature = "rust-static-interface-storage")]
const WIFI_INTERFACE_PHY_SIZE: usize = 1296;
#[cfg(feature = "rust-static-interface-storage")]
const WIFI_CREATE_STA_STATE_RETURN_OFFSET: usize = 0x30;
#[cfg(feature = "rust-static-interface-storage")]
const WIFI_CREATE_SOFTAP_STATE_RETURN_OFFSET: usize = 0x32;
#[cfg(feature = "rust-static-interface-storage")]
const WIFI_CREATE_PHY_RETURN_OFFSET: usize = 0x6e;

#[cfg(feature = "rust-static-interface-storage")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StaticInterfaceAllocation {
    State,
    Phy,
}

#[cfg(feature = "rust-static-interface-storage")]
fn classify_static_interface_allocation(
    source: AllocationSource,
    size: usize,
    caller: usize,
    wifi_create_sta_address: usize,
    wifi_create_softap_address: usize,
) -> Option<StaticInterfaceAllocation> {
    if source != AllocationSource::OsiWifiZalloc {
        return None;
    }
    let sta_offset = caller.wrapping_sub(wifi_create_sta_address);
    let softap_offset = caller.wrapping_sub(wifi_create_softap_address);
    match size {
        WIFI_INTERFACE_STATE_SIZE
            if sta_offset == WIFI_CREATE_STA_STATE_RETURN_OFFSET
                || softap_offset == WIFI_CREATE_SOFTAP_STATE_RETURN_OFFSET =>
        {
            Some(StaticInterfaceAllocation::State)
        }
        WIFI_INTERFACE_PHY_SIZE
            if sta_offset == WIFI_CREATE_PHY_RETURN_OFFSET
                || softap_offset == WIFI_CREATE_PHY_RETURN_OFFSET =>
        {
            Some(StaticInterfaceAllocation::Phy)
        }
        _ => None,
    }
}

#[cfg(feature = "rust-static-supplicant-callback-storage")]
const SUPPLICANT_CALLBACK_TABLE_SIZE: usize = 27 * core::mem::size_of::<u32>();
#[cfg(feature = "rust-static-supplicant-callback-storage")]
const SUPPLICANT_CALLBACK_TABLE_RETURN_OFFSET: usize = 0x26;

#[cfg(feature = "rust-static-supplicant-callback-storage")]
fn is_static_supplicant_callback_allocation(
    source: AllocationSource,
    size: usize,
    caller: usize,
    esp_supplicant_init_address: usize,
) -> bool {
    source == AllocationSource::DirectCalloc
        && size == SUPPLICANT_CALLBACK_TABLE_SIZE
        && caller.wrapping_sub(esp_supplicant_init_address)
            == SUPPLICANT_CALLBACK_TABLE_RETURN_OFFSET
}

#[cfg(feature = "rust-static-pp-bar-storage")]
const PP_BAR_SIZE: usize = 40;
#[cfg(feature = "rust-static-pp-bar-storage")]
const PP_BAR_CAPACITY: usize = 4;
// Return address immediately after the pinned S31 OSI malloc callback in
// `pp_attach`.
#[cfg(feature = "rust-static-pp-bar-storage")]
const PP_BAR_RETURN_OFFSET: usize = 0x4a;

#[cfg(feature = "rust-static-pp-bar-storage")]
fn is_static_pp_bar_allocation(
    source: AllocationSource,
    size: usize,
    caller: usize,
    pp_attach_address: usize,
) -> bool {
    source == AllocationSource::OsiMallocInternal
        && size == PP_BAR_SIZE
        && caller.wrapping_sub(pp_attach_address) == PP_BAR_RETURN_OFFSET
}

#[cfg(feature = "rust-static-cold-api-envelope-storage")]
const COLD_API_ENVELOPE_SIZE: usize = 24;
#[cfg(feature = "rust-static-cold-api-envelope-storage")]
const COLD_API_ENVELOPE_CAPACITY: usize = 2;
#[cfg(feature = "rust-static-cold-api-envelope-storage")]
const WIFI_INIT_ENVELOPE_RETURN_OFFSET: usize = 0xd6;
#[cfg(feature = "rust-static-cold-api-envelope-storage")]
const WIFI_START_ENVELOPE_RETURN_OFFSET: usize = 0x1a;

#[cfg(feature = "rust-static-cold-api-envelope-storage")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ColdApiEnvelopeKind {
    Init,
    Start,
}

#[cfg(feature = "rust-static-cold-api-envelope-storage")]
impl ColdApiEnvelopeKind {
    const fn index(self) -> usize {
        match self {
            Self::Init => 0,
            Self::Start => 1,
        }
    }
}

#[cfg(feature = "rust-static-cold-api-envelope-storage")]
fn classify_static_cold_api_envelope(
    source: AllocationSource,
    size: usize,
    caller: usize,
    esp_wifi_init_internal_address: usize,
    esp_wifi_start_address: usize,
) -> Option<ColdApiEnvelopeKind> {
    if source != AllocationSource::OsiWifiZalloc || size != COLD_API_ENVELOPE_SIZE {
        return None;
    }
    if caller.wrapping_sub(esp_wifi_init_internal_address) == WIFI_INIT_ENVELOPE_RETURN_OFFSET {
        Some(ColdApiEnvelopeKind::Init)
    } else if caller.wrapping_sub(esp_wifi_start_address) == WIFI_START_ENVELOPE_RETURN_OFFSET {
        Some(ColdApiEnvelopeKind::Start)
    } else {
        None
    }
}

#[cfg(target_arch = "riscv32")]
mod target {
    use core::{
        cell::UnsafeCell,
        ffi::c_void,
        mem,
        sync::atomic::{AtomicUsize, Ordering},
    };

    use esp_wifi_sys_esp32s31::include::wifi_osi_funcs_t;

    use crate::rate_control::{RateControlRecord, RATE_CONTROL_RECORD_SIZE};

    #[cfg(feature = "rust-static-function-table-storage")]
    use super::{
        classify_static_function_table, StaticFunctionTable, NET80211_FUNCTION_TABLE_SIZE,
        WDEV_FUNCTION_TABLE_SIZE,
    };
    #[cfg(feature = "rust-static-interface-storage")]
    use super::{
        classify_static_interface_allocation, StaticInterfaceAllocation, WIFI_INTERFACE_PHY_SIZE,
        WIFI_INTERFACE_STATE_SIZE,
    };
    #[cfg(feature = "rust-static-pp-bar-storage")]
    use super::{
        is_static_pp_bar_allocation, PP_BAR_CAPACITY, PP_BAR_SIZE,
    };
    #[cfg(feature = "rust-static-cold-api-envelope-storage")]
    use super::{
        classify_static_cold_api_envelope, ColdApiEnvelopeKind, COLD_API_ENVELOPE_CAPACITY,
        COLD_API_ENVELOPE_SIZE,
    };
    #[cfg(feature = "rust-static-wifi-nvs-storage")]
    use super::{
        classify_wifi_nvs_static_allocation, WifiNvsStaticAllocation, WIFI_NVS_CFG_ITEMS_SIZE,
        WIFI_NVS_LOAD_SCRATCH_SIZE,
    };
    #[cfg(feature = "rust-static-supplicant-callback-storage")]
    use super::{is_static_supplicant_callback_allocation, SUPPLICANT_CALLBACK_TABLE_SIZE};
    use super::{AllocationSource, PROBE};

    type Malloc = unsafe extern "C" fn(usize) -> *mut c_void;
    type Free = unsafe extern "C" fn(*mut c_void);
    type Realloc = unsafe extern "C" fn(*mut c_void, usize) -> *mut c_void;
    type Calloc = unsafe extern "C" fn(usize, usize) -> *mut c_void;

    static MALLOC: AtomicUsize = AtomicUsize::new(0);
    static FREE: AtomicUsize = AtomicUsize::new(0);
    static MALLOC_INTERNAL: AtomicUsize = AtomicUsize::new(0);
    static REALLOC_INTERNAL: AtomicUsize = AtomicUsize::new(0);
    static CALLOC_INTERNAL: AtomicUsize = AtomicUsize::new(0);
    static ZALLOC_INTERNAL: AtomicUsize = AtomicUsize::new(0);
    static WIFI_MALLOC: AtomicUsize = AtomicUsize::new(0);
    static WIFI_REALLOC: AtomicUsize = AtomicUsize::new(0);
    static WIFI_CALLOC: AtomicUsize = AtomicUsize::new(0);
    static WIFI_ZALLOC: AtomicUsize = AtomicUsize::new(0);
    static CALLBACKS_PATCHED: AtomicUsize = AtomicUsize::new(0);
    static RUNTIME_HEAP_FORBIDDEN: AtomicUsize = AtomicUsize::new(0);

    const BLACKLIST_NODE_SIZE: usize = 12;
    const BLACKLIST_NODE_CAPACITY: usize = 16;
    const BLACKLIST_NODE_MASK: usize = (1 << BLACKLIST_NODE_CAPACITY) - 1;
    // Return address immediately after the pinned S31 `_wifi_malloc(12)`
    // call in `cnx_add_to_blacklist`.
    const BLACKLIST_ALLOCATION_RETURN_OFFSET: usize = 0x5c;
    const IPC_ENVELOPE_SIZE: usize = 24;
    const IPC_ENVELOPE_CAPACITY: usize = 8;
    const IPC_ENVELOPE_MASK: usize = (1 << IPC_ENVELOPE_CAPACITY) - 1;
    // Return addresses after the two pinned S31 `_wifi_zalloc(24)` calls in
    // `esp_wifi_ipc_internal`.
    const IPC_ENVELOPE_RETURN_OFFSETS: [usize; 2] = [0x34, 0xce];
    const SET_APPIE_ENVELOPE_RETURN_OFFSET: usize = 0x2a;
    const WPA_IE_CAPACITY: usize = 256;
    const WPA_IE_SLOT_CAPACITY: usize = 8;
    const WPA_IE_SLOT_MASK: usize = (1 << WPA_IE_SLOT_CAPACITY) - 1;
    const OS_MEMDUP_MALLOC_RETURN_OFFSET: usize = 0x10;
    const RATE_CONTEXT_SIZE: usize = RATE_CONTROL_RECORD_SIZE;
    const RATE_CONTEXT_CAPACITY: usize = 16;
    const RATE_CONTEXT_MASK: usize = (1 << RATE_CONTEXT_CAPACITY) - 1;
    // Return address after the pinned S31 `_wifi_zalloc(152)` call in
    // `rc_enable_trc`. AP peer rate contexts are bounded by the vendor table
    // indices 1..=16 and are returned through `rc_disable_trc`.
    const RATE_CONTEXT_ALLOCATION_RETURN_OFFSET: usize = 0x3e;
    const RATE_TABLE_SCRATCH_SIZE: usize = 212;
    // Return address after the pinned S31 `_wifi_zalloc(212)` call in
    // `ieee80211_setup_ratetable`. The function uses this as serialized
    // scratch and frees it before returning.
    const RATE_TABLE_SCRATCH_ALLOCATION_RETURN_OFFSET: usize = 0x26;
    #[cfg(feature = "rust-static-rx-buffer-init")]
    const WDEV_RX_DESCRIPTOR_SIZE: usize = 12;
    #[cfg(feature = "rust-static-rx-buffer-init")]
    const WDEV_RX_DESCRIPTOR_CAPACITY: usize = 32;
    #[cfg(feature = "rust-static-rx-buffer-init")]
    const WDEV_RX_DESCRIPTOR_ARENA_SIZE: usize =
        WDEV_RX_DESCRIPTOR_SIZE * WDEV_RX_DESCRIPTOR_CAPACITY;
    #[cfg(feature = "rust-static-rx-buffer-init")]
    const WDEV_RX_PAYLOAD_SIZE: usize = 1704;
    #[cfg(feature = "rust-static-rx-buffer-init")]
    const WDEV_RX_PAYLOAD_CAPACITY: usize = 32;
    // Return addresses immediately after the pinned S31 allocator callbacks
    // in `wDev_Rxbuf_Init`.
    #[cfg(feature = "rust-static-rx-buffer-init")]
    const WDEV_RX_DESCRIPTOR_ALLOCATION_RETURN_OFFSET: usize = 0x36;
    #[cfg(feature = "rust-static-rx-buffer-init")]
    const WDEV_RX_PAYLOAD_ALLOCATION_RETURN_OFFSET: usize = 0x108;
    #[cfg(feature = "rust-static-esf-buffer-init")]
    const ESP32S31_ECO0_ESF_DYNAMIC_ALLOCATION_RETURN_ADDRESS: usize = 0x2f83_2460;
    #[cfg(feature = "rust-static-esf-buffer-init")]
    const ESF_WIFI_648_SIZE: usize = 648;
    #[cfg(feature = "rust-static-esf-buffer-init")]
    const ESF_WIFI_648_CAPACITY: usize = 8;
    #[cfg(feature = "rust-static-esf-buffer-init")]
    const ESF_INTERNAL_1748_SIZE: usize = 1748;
    #[cfg(feature = "rust-static-esf-buffer-init")]
    const ESF_INTERNAL_1748_CAPACITY: usize = 32;
    #[cfg(feature = "rust-static-esf-buffer-init")]
    const ESF_INTERNAL_788_SIZE: usize = 788;
    #[cfg(feature = "rust-static-esf-buffer-init")]
    const ESF_INTERNAL_788_CAPACITY: usize = 2;

    #[repr(C, align(4))]
    struct BlacklistNode(UnsafeCell<[u8; BLACKLIST_NODE_SIZE]>);

    impl BlacklistNode {
        const fn new() -> Self {
            Self(UnsafeCell::new([0; BLACKLIST_NODE_SIZE]))
        }
    }

    unsafe impl Sync for BlacklistNode {}

    #[repr(C, align(4))]
    struct IpcEnvelope(UnsafeCell<[u8; IPC_ENVELOPE_SIZE]>);

    impl IpcEnvelope {
        const fn new() -> Self {
            Self(UnsafeCell::new([0; IPC_ENVELOPE_SIZE]))
        }
    }

    unsafe impl Sync for IpcEnvelope {}

    #[repr(C, align(4))]
    struct WpaIeSlot(UnsafeCell<[u8; WPA_IE_CAPACITY]>);

    impl WpaIeSlot {
        const fn new() -> Self {
            Self(UnsafeCell::new([0; WPA_IE_CAPACITY]))
        }
    }

    unsafe impl Sync for WpaIeSlot {}

    #[repr(C, align(4))]
    struct RateContext(UnsafeCell<RateControlRecord>);

    impl RateContext {
        const fn new() -> Self {
            Self(UnsafeCell::new(RateControlRecord::zeroed()))
        }
    }

    unsafe impl Sync for RateContext {}

    #[repr(C, align(4))]
    struct RateTableScratch(UnsafeCell<[u8; RATE_TABLE_SCRATCH_SIZE]>);

    impl RateTableScratch {
        const fn new() -> Self {
            Self(UnsafeCell::new([0; RATE_TABLE_SCRATCH_SIZE]))
        }
    }

    unsafe impl Sync for RateTableScratch {}

    #[cfg(feature = "rust-static-rx-buffer-init")]
    #[repr(C, align(16))]
    struct WdevRxDescriptorArena(UnsafeCell<[u8; WDEV_RX_DESCRIPTOR_ARENA_SIZE]>);

    #[cfg(feature = "rust-static-rx-buffer-init")]
    impl WdevRxDescriptorArena {
        const fn new() -> Self {
            Self(UnsafeCell::new([0; WDEV_RX_DESCRIPTOR_ARENA_SIZE]))
        }
    }

    #[cfg(feature = "rust-static-rx-buffer-init")]
    unsafe impl Sync for WdevRxDescriptorArena {}

    #[cfg(feature = "rust-static-rx-buffer-init")]
    #[repr(C, align(16))]
    struct WdevRxPayload(UnsafeCell<[u8; WDEV_RX_PAYLOAD_SIZE]>);

    #[cfg(feature = "rust-static-rx-buffer-init")]
    impl WdevRxPayload {
        const fn new() -> Self {
            Self(UnsafeCell::new([0; WDEV_RX_PAYLOAD_SIZE]))
        }
    }

    #[cfg(feature = "rust-static-rx-buffer-init")]
    unsafe impl Sync for WdevRxPayload {}

    #[cfg(feature = "rust-static-esf-buffer-init")]
    #[repr(C, align(16))]
    struct ColdEsfBuffer<const SIZE: usize>(UnsafeCell<[u8; SIZE]>);

    #[cfg(feature = "rust-static-esf-buffer-init")]
    impl<const SIZE: usize> ColdEsfBuffer<SIZE> {
        const fn new() -> Self {
            Self(UnsafeCell::new([0; SIZE]))
        }
    }

    #[cfg(feature = "rust-static-esf-buffer-init")]
    unsafe impl<const SIZE: usize> Sync for ColdEsfBuffer<SIZE> {}

    #[cfg(feature = "rust-static-wifi-nvs-storage")]
    #[repr(C, align(16))]
    struct WifiNvsStaticBuffer<const SIZE: usize>(UnsafeCell<[u8; SIZE]>);

    #[cfg(feature = "rust-static-wifi-nvs-storage")]
    impl<const SIZE: usize> WifiNvsStaticBuffer<SIZE> {
        const fn new() -> Self {
            Self(UnsafeCell::new([0; SIZE]))
        }
    }

    #[cfg(feature = "rust-static-wifi-nvs-storage")]
    unsafe impl<const SIZE: usize> Sync for WifiNvsStaticBuffer<SIZE> {}

    #[cfg(feature = "rust-static-function-table-storage")]
    #[repr(C, align(16))]
    struct StaticFunctionTableBuffer<const SIZE: usize>(UnsafeCell<[u8; SIZE]>);

    #[cfg(feature = "rust-static-function-table-storage")]
    impl<const SIZE: usize> StaticFunctionTableBuffer<SIZE> {
        const fn new() -> Self {
            Self(UnsafeCell::new([0; SIZE]))
        }
    }

    #[cfg(feature = "rust-static-function-table-storage")]
    unsafe impl<const SIZE: usize> Sync for StaticFunctionTableBuffer<SIZE> {}

    #[cfg(feature = "rust-static-interface-storage")]
    #[repr(C, align(16))]
    struct StaticInterfaceBuffer<const SIZE: usize>(UnsafeCell<[u8; SIZE]>);

    #[cfg(feature = "rust-static-interface-storage")]
    impl<const SIZE: usize> StaticInterfaceBuffer<SIZE> {
        const fn new() -> Self {
            Self(UnsafeCell::new([0; SIZE]))
        }
    }

    #[cfg(feature = "rust-static-interface-storage")]
    unsafe impl<const SIZE: usize> Sync for StaticInterfaceBuffer<SIZE> {}

    #[cfg(feature = "rust-static-supplicant-callback-storage")]
    #[repr(C, align(4))]
    struct SupplicantCallbackTable(UnsafeCell<[u8; SUPPLICANT_CALLBACK_TABLE_SIZE]>);

    #[cfg(feature = "rust-static-supplicant-callback-storage")]
    impl SupplicantCallbackTable {
        const fn new() -> Self {
            Self(UnsafeCell::new([0; SUPPLICANT_CALLBACK_TABLE_SIZE]))
        }
    }

    #[cfg(feature = "rust-static-supplicant-callback-storage")]
    unsafe impl Sync for SupplicantCallbackTable {}

    #[cfg(feature = "rust-static-pp-bar-storage")]
    #[repr(C, align(4))]
    struct PpBar(UnsafeCell<[u8; PP_BAR_SIZE]>);

    #[cfg(feature = "rust-static-pp-bar-storage")]
    impl PpBar {
        const fn new() -> Self {
            Self(UnsafeCell::new([0; PP_BAR_SIZE]))
        }
    }

    #[cfg(feature = "rust-static-pp-bar-storage")]
    unsafe impl Sync for PpBar {}

    #[cfg(feature = "rust-static-cold-api-envelope-storage")]
    #[repr(C, align(4))]
    struct ColdApiEnvelope(UnsafeCell<[u8; COLD_API_ENVELOPE_SIZE]>);

    #[cfg(feature = "rust-static-cold-api-envelope-storage")]
    impl ColdApiEnvelope {
        const fn new() -> Self {
            Self(UnsafeCell::new([0; COLD_API_ENVELOPE_SIZE]))
        }
    }

    #[cfg(feature = "rust-static-cold-api-envelope-storage")]
    unsafe impl Sync for ColdApiEnvelope {}

    static BLACKLIST_NODES: [BlacklistNode; BLACKLIST_NODE_CAPACITY] =
        [const { BlacklistNode::new() }; BLACKLIST_NODE_CAPACITY];
    static CLAIMED_BLACKLIST_NODES: AtomicUsize = AtomicUsize::new(0);
    static IPC_ENVELOPES: [IpcEnvelope; IPC_ENVELOPE_CAPACITY] =
        [const { IpcEnvelope::new() }; IPC_ENVELOPE_CAPACITY];
    static CLAIMED_IPC_ENVELOPES: AtomicUsize = AtomicUsize::new(0);
    static WPA_IE_SLOTS: [WpaIeSlot; WPA_IE_SLOT_CAPACITY] =
        [const { WpaIeSlot::new() }; WPA_IE_SLOT_CAPACITY];
    static CLAIMED_WPA_IE_SLOTS: AtomicUsize = AtomicUsize::new(0);
    #[link_section = ".critical.bss.wifi_strict.rate_contexts"]
    static RATE_CONTEXTS: [RateContext; RATE_CONTEXT_CAPACITY] =
        [const { RateContext::new() }; RATE_CONTEXT_CAPACITY];
    static CLAIMED_RATE_CONTEXTS: AtomicUsize = AtomicUsize::new(0);
    #[link_section = ".critical.bss.wifi_strict.rate_table_scratch"]
    static RATE_TABLE_SCRATCH: RateTableScratch = RateTableScratch::new();
    static RATE_TABLE_SCRATCH_CLAIMED: AtomicUsize = AtomicUsize::new(0);
    #[cfg(feature = "rust-static-rx-buffer-init")]
    #[link_section = ".critical.bss.wifi_strict.wdev_rx_descriptor_arena"]
    static WDEV_RX_DESCRIPTOR_ARENA: WdevRxDescriptorArena = WdevRxDescriptorArena::new();
    #[cfg(feature = "rust-static-rx-buffer-init")]
    static WDEV_RX_DESCRIPTOR_ARENA_CLAIMED: AtomicUsize = AtomicUsize::new(0);
    #[cfg(feature = "rust-static-rx-buffer-init")]
    #[link_section = ".critical.bss.wifi_strict.wdev_rx_payloads"]
    static WDEV_RX_PAYLOADS: [WdevRxPayload; WDEV_RX_PAYLOAD_CAPACITY] =
        [const { WdevRxPayload::new() }; WDEV_RX_PAYLOAD_CAPACITY];
    #[cfg(feature = "rust-static-rx-buffer-init")]
    static CLAIMED_WDEV_RX_PAYLOADS: [AtomicUsize; WDEV_RX_PAYLOAD_CAPACITY] =
        [const { AtomicUsize::new(0) }; WDEV_RX_PAYLOAD_CAPACITY];
    #[cfg(feature = "rust-static-esf-buffer-init")]
    #[link_section = ".critical.bss.wifi_strict.esf_cold_wifi_648"]
    static ESF_WIFI_648: [ColdEsfBuffer<ESF_WIFI_648_SIZE>; ESF_WIFI_648_CAPACITY] =
        [const { ColdEsfBuffer::new() }; ESF_WIFI_648_CAPACITY];
    #[cfg(feature = "rust-static-esf-buffer-init")]
    static CLAIMED_ESF_WIFI_648: [AtomicUsize; ESF_WIFI_648_CAPACITY] =
        [const { AtomicUsize::new(0) }; ESF_WIFI_648_CAPACITY];
    #[cfg(feature = "rust-static-esf-buffer-init")]
    #[link_section = ".critical.bss.wifi_strict.esf_cold_internal_1748"]
    static ESF_INTERNAL_1748:
        [ColdEsfBuffer<ESF_INTERNAL_1748_SIZE>; ESF_INTERNAL_1748_CAPACITY] =
        [const { ColdEsfBuffer::new() }; ESF_INTERNAL_1748_CAPACITY];
    #[cfg(feature = "rust-static-esf-buffer-init")]
    static CLAIMED_ESF_INTERNAL_1748: [AtomicUsize; ESF_INTERNAL_1748_CAPACITY] =
        [const { AtomicUsize::new(0) }; ESF_INTERNAL_1748_CAPACITY];
    #[cfg(feature = "rust-static-esf-buffer-init")]
    #[link_section = ".critical.bss.wifi_strict.esf_cold_internal_788"]
    static ESF_INTERNAL_788:
        [ColdEsfBuffer<ESF_INTERNAL_788_SIZE>; ESF_INTERNAL_788_CAPACITY] =
        [const { ColdEsfBuffer::new() }; ESF_INTERNAL_788_CAPACITY];
    #[cfg(feature = "rust-static-esf-buffer-init")]
    static CLAIMED_ESF_INTERNAL_788: [AtomicUsize; ESF_INTERNAL_788_CAPACITY] =
        [const { AtomicUsize::new(0) }; ESF_INTERNAL_788_CAPACITY];
    #[cfg(feature = "rust-static-wifi-nvs-storage")]
    #[link_section = ".critical.bss.wifi_strict.wifi_nvs_cfg_items"]
    static WIFI_NVS_CFG_ITEMS: WifiNvsStaticBuffer<WIFI_NVS_CFG_ITEMS_SIZE> =
        WifiNvsStaticBuffer::new();
    #[cfg(feature = "rust-static-wifi-nvs-storage")]
    static WIFI_NVS_CFG_ITEMS_CLAIMED: AtomicUsize = AtomicUsize::new(0);
    #[cfg(feature = "rust-static-wifi-nvs-storage")]
    #[link_section = ".critical.bss.wifi_strict.wifi_nvs_load_scratch"]
    static WIFI_NVS_LOAD_SCRATCH: WifiNvsStaticBuffer<WIFI_NVS_LOAD_SCRATCH_SIZE> =
        WifiNvsStaticBuffer::new();
    #[cfg(feature = "rust-static-wifi-nvs-storage")]
    static WIFI_NVS_LOAD_SCRATCH_CLAIMED: AtomicUsize = AtomicUsize::new(0);
    #[cfg(feature = "rust-static-function-table-storage")]
    #[link_section = ".critical.bss.wifi_strict.wdev_function_table"]
    static WDEV_FUNCTION_TABLE: StaticFunctionTableBuffer<WDEV_FUNCTION_TABLE_SIZE> =
        StaticFunctionTableBuffer::new();
    #[cfg(feature = "rust-static-function-table-storage")]
    static WDEV_FUNCTION_TABLE_CLAIMED: AtomicUsize = AtomicUsize::new(0);
    #[cfg(feature = "rust-static-function-table-storage")]
    #[link_section = ".critical.bss.wifi_strict.net80211_function_table"]
    static NET80211_FUNCTION_TABLE: StaticFunctionTableBuffer<NET80211_FUNCTION_TABLE_SIZE> =
        StaticFunctionTableBuffer::new();
    #[cfg(feature = "rust-static-function-table-storage")]
    static NET80211_FUNCTION_TABLE_CLAIMED: AtomicUsize = AtomicUsize::new(0);
    #[cfg(feature = "rust-static-interface-storage")]
    #[link_section = ".critical.bss.wifi_strict.wifi_interface_state"]
    static WIFI_INTERFACE_STATE: StaticInterfaceBuffer<WIFI_INTERFACE_STATE_SIZE> =
        StaticInterfaceBuffer::new();
    #[cfg(feature = "rust-static-interface-storage")]
    static WIFI_INTERFACE_STATE_CLAIMED: AtomicUsize = AtomicUsize::new(0);
    #[cfg(feature = "rust-static-interface-storage")]
    #[link_section = ".critical.bss.wifi_strict.wifi_interface_phy"]
    static WIFI_INTERFACE_PHY: StaticInterfaceBuffer<WIFI_INTERFACE_PHY_SIZE> =
        StaticInterfaceBuffer::new();
    #[cfg(feature = "rust-static-interface-storage")]
    static WIFI_INTERFACE_PHY_CLAIMED: AtomicUsize = AtomicUsize::new(0);
    #[cfg(feature = "rust-static-supplicant-callback-storage")]
    #[link_section = ".critical.bss.wifi_strict.supplicant_callbacks"]
    static SUPPLICANT_CALLBACK_TABLE: SupplicantCallbackTable = SupplicantCallbackTable::new();
    #[cfg(feature = "rust-static-supplicant-callback-storage")]
    static SUPPLICANT_CALLBACK_TABLE_CLAIMED: AtomicUsize = AtomicUsize::new(0);
    #[cfg(feature = "rust-static-pp-bar-storage")]
    #[link_section = ".critical.bss.wifi_strict.pp_bars"]
    static PP_BARS: [PpBar; PP_BAR_CAPACITY] =
        [const { PpBar::new() }; PP_BAR_CAPACITY];
    #[cfg(feature = "rust-static-pp-bar-storage")]
    static PP_BAR_CLAIMS: [AtomicUsize; PP_BAR_CAPACITY] =
        [const { AtomicUsize::new(0) }; PP_BAR_CAPACITY];
    #[cfg(feature = "rust-static-cold-api-envelope-storage")]
    #[link_section = ".critical.bss.wifi_strict.cold_api_envelopes"]
    static COLD_API_ENVELOPES: [ColdApiEnvelope; COLD_API_ENVELOPE_CAPACITY] =
        [const { ColdApiEnvelope::new() }; COLD_API_ENVELOPE_CAPACITY];
    #[cfg(feature = "rust-static-cold-api-envelope-storage")]
    static COLD_API_ENVELOPE_CLAIMS: [AtomicUsize; COLD_API_ENVELOPE_CAPACITY] =
        [const { AtomicUsize::new(0) }; COLD_API_ENVELOPE_CAPACITY];
    #[cfg(feature = "rust-static-cold-api-envelope-storage")]
    static COLD_API_ENVELOPE_USES: [AtomicUsize; COLD_API_ENVELOPE_CAPACITY] =
        [const { AtomicUsize::new(0) }; COLD_API_ENVELOPE_CAPACITY];
    #[cfg(feature = "rust-static-cold-api-envelope-storage")]
    static COLD_API_ENVELOPE_RELEASES: [AtomicUsize; COLD_API_ENVELOPE_CAPACITY] =
        [const { AtomicUsize::new(0) }; COLD_API_ENVELOPE_CAPACITY];

    unsafe extern "C" {
        static mut g_osi_funcs_p: *const wifi_osi_funcs_t;
        fn cnx_add_to_blacklist(bssid: *const u8);
        fn esp_wifi_ipc_internal(request: *const c_void, copy_request: bool) -> i32;
        fn esp_wifi_set_appie_internal(
            interface: u32,
            appie: *const u8,
            length: usize,
            appie_type: u32,
        ) -> i32;
        fn os_memdup(source: *const c_void, length: usize) -> *mut c_void;
        fn rc_enable_trc(interface: u32, peer: *const u8, index: u32, mode: u32) -> *mut c_void;
        fn ieee80211_setup_ratetable(interface: *mut c_void, mode: u32, phy_mode: u32) -> i32;
        #[cfg(feature = "rust-static-rx-buffer-init")]
        fn wDev_Rxbuf_Init(count: u32) -> i32;
        #[cfg(feature = "rust-static-wifi-nvs-storage")]
        fn wifi_nvs_cfg_init() -> i32;
        #[cfg(feature = "rust-static-function-table-storage")]
        fn wdev_funcs_init(config: *mut c_void) -> i32;
        #[cfg(feature = "rust-static-function-table-storage")]
        fn net80211_funcs_init() -> i32;
        #[cfg(feature = "rust-static-interface-storage")]
        fn wifi_create_sta() -> i32;
        #[cfg(feature = "rust-static-interface-storage")]
        fn wifi_create_softap() -> i32;
        #[cfg(feature = "rust-static-supplicant-callback-storage")]
        fn esp_supplicant_init() -> i32;
        #[cfg(feature = "rust-static-supplicant-callback-storage")]
        static mut wpa_cb: *mut c_void;
        #[cfg(feature = "rust-static-pp-bar-storage")]
        fn pp_attach(config: *mut c_void) -> i32;
        #[cfg(feature = "rust-static-pp-bar-storage")]
        static mut s_bars: [*mut c_void; PP_BAR_CAPACITY];
        #[cfg(feature = "rust-static-cold-api-envelope-storage")]
        fn esp_wifi_init_internal(config: *mut c_void) -> i32;
        #[cfg(feature = "rust-static-cold-api-envelope-storage")]
        fn esp_wifi_start() -> i32;
    }

    /// Wrap every OSI allocator callback while preserving the original
    /// allocator for initialization. `prepare_strict_runtime` later switches
    /// these wrappers to a heap-denying runtime phase.
    ///
    /// # Safety
    /// Installation must be serialized with Wi-Fi init and performed only
    /// once. The captured original callbacks must remain valid permanently.
    pub unsafe fn patch_allocator_probes(table: &mut wifi_osi_funcs_t) {
        wrap(&mut table._malloc, &MALLOC, malloc);
        if let Some(original) = table._free {
            FREE.store(original as usize, Ordering::Release);
            table._free = Some(free);
        }
        wrap(
            &mut table._malloc_internal,
            &MALLOC_INTERNAL,
            malloc_internal,
        );
        wrap_realloc(
            &mut table._realloc_internal,
            &REALLOC_INTERNAL,
            realloc_internal,
        );
        wrap_calloc(
            &mut table._calloc_internal,
            &CALLOC_INTERNAL,
            calloc_internal,
        );
        wrap(
            &mut table._zalloc_internal,
            &ZALLOC_INTERNAL,
            zalloc_internal,
        );
        wrap(&mut table._wifi_malloc, &WIFI_MALLOC, wifi_malloc);
        wrap_realloc(&mut table._wifi_realloc, &WIFI_REALLOC, wifi_realloc);
        wrap_calloc(&mut table._wifi_calloc, &WIFI_CALLOC, wifi_calloc);
        wrap(&mut table._wifi_zalloc, &WIFI_ZALLOC, wifi_zalloc);
        CALLBACKS_PATCHED.store(1, Ordering::Release);
    }

    pub(crate) fn forbid_runtime_heap() -> bool {
        if !allocator_callbacks_patched() {
            return false;
        }
        RUNTIME_HEAP_FORBIDDEN.store(1, Ordering::Release);
        true
    }

    pub(crate) fn allocator_callbacks_patched() -> bool {
        if CALLBACKS_PATCHED.load(Ordering::Acquire) == 0 {
            return false;
        }
        let table = unsafe { core::ptr::addr_of!(g_osi_funcs_p).read().as_ref() };
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
        callback_is!(_malloc, malloc)
            && callback_is!(_free, free)
            && callback_is!(_malloc_internal, malloc_internal)
            && callback_is!(_realloc_internal, realloc_internal)
            && callback_is!(_calloc_internal, calloc_internal)
            && callback_is!(_zalloc_internal, zalloc_internal)
            && callback_is!(_wifi_malloc, wifi_malloc)
            && callback_is!(_wifi_realloc, wifi_realloc)
            && callback_is!(_wifi_calloc, wifi_calloc)
            && callback_is!(_wifi_zalloc, wifi_zalloc)
    }

    /// Re-enable the captured allocators after the strict executor and all
    /// references to its state have stopped.
    ///
    /// # Safety
    /// No strict runtime callback may execute concurrently or afterwards.
    pub unsafe fn allow_heap_for_wifi_teardown() {
        RUNTIME_HEAP_FORBIDDEN.store(0, Ordering::Release);
    }

    fn heap_forbidden() -> bool {
        RUNTIME_HEAP_FORBIDDEN.load(Ordering::Acquire) != 0
    }

    fn claim_blacklist_node(size: usize, caller: usize) -> Option<*mut c_void> {
        let expected_caller =
            cnx_add_to_blacklist as *const () as usize + BLACKLIST_ALLOCATION_RETURN_OFFSET;
        if size != BLACKLIST_NODE_SIZE || caller != expected_caller {
            return None;
        }
        let claimed = CLAIMED_BLACKLIST_NODES.load(Ordering::Acquire);
        let free = !claimed & BLACKLIST_NODE_MASK;
        if free == 0 {
            return None;
        }
        let index = free.trailing_zeros() as usize;
        let bit = 1_usize << index;
        CLAIMED_BLACKLIST_NODES
            .compare_exchange(claimed, claimed | bit, Ordering::AcqRel, Ordering::Acquire)
            .ok()?;
        let node = BLACKLIST_NODES[index].0.get();
        unsafe { node.write([0; BLACKLIST_NODE_SIZE]) };
        Some(node.cast())
    }

    fn blacklist_node_index(node: *mut c_void) -> Option<usize> {
        let base = core::ptr::addr_of!(BLACKLIST_NODES) as usize;
        let address = node as usize;
        let stride = mem::size_of::<BlacklistNode>();
        let offset = address.checked_sub(base)?;
        if offset % stride != 0 {
            return None;
        }
        let index = offset / stride;
        (index < BLACKLIST_NODE_CAPACITY).then_some(index)
    }

    fn release_blacklist_node(node: *mut c_void) -> bool {
        let Some(index) = blacklist_node_index(node) else {
            return false;
        };
        let bit = 1_usize << index;
        CLAIMED_BLACKLIST_NODES.fetch_and(!bit, Ordering::AcqRel) & bit != 0
    }

    fn claim_ipc_envelope(size: usize, caller: usize) -> Option<*mut c_void> {
        let function = esp_wifi_ipc_internal as *const () as usize;
        let ipc_caller = IPC_ENVELOPE_RETURN_OFFSETS
            .iter()
            .any(|offset| caller == function + offset);
        let set_appie_caller = caller
            == esp_wifi_set_appie_internal as *const () as usize + SET_APPIE_ENVELOPE_RETURN_OFFSET;
        if size != IPC_ENVELOPE_SIZE || (!ipc_caller && !set_appie_caller) {
            return None;
        }
        let claimed = CLAIMED_IPC_ENVELOPES.load(Ordering::Acquire);
        let free = !claimed & IPC_ENVELOPE_MASK;
        if free == 0 {
            return None;
        }
        let index = free.trailing_zeros() as usize;
        let bit = 1_usize << index;
        CLAIMED_IPC_ENVELOPES
            .compare_exchange(claimed, claimed | bit, Ordering::AcqRel, Ordering::Acquire)
            .ok()?;
        let envelope = IPC_ENVELOPES[index].0.get();
        unsafe { envelope.write([0; IPC_ENVELOPE_SIZE]) };
        Some(envelope.cast())
    }

    fn ipc_envelope_index(envelope: *mut c_void) -> Option<usize> {
        let base = core::ptr::addr_of!(IPC_ENVELOPES) as usize;
        let address = envelope as usize;
        let stride = mem::size_of::<IpcEnvelope>();
        let offset = address.checked_sub(base)?;
        if offset % stride != 0 {
            return None;
        }
        let index = offset / stride;
        (index < IPC_ENVELOPE_CAPACITY).then_some(index)
    }

    fn release_ipc_envelope(envelope: *mut c_void) -> bool {
        let Some(index) = ipc_envelope_index(envelope) else {
            return false;
        };
        let bit = 1_usize << index;
        CLAIMED_IPC_ENVELOPES.fetch_and(!bit, Ordering::AcqRel) & bit != 0
    }

    fn claim_wpa_ie_slot(size: usize, caller: usize) -> Option<*mut c_void> {
        let expected_caller = os_memdup as *const () as usize + OS_MEMDUP_MALLOC_RETURN_OFFSET;
        if size == 0 || size > WPA_IE_CAPACITY || caller != expected_caller {
            return None;
        }
        let claimed = CLAIMED_WPA_IE_SLOTS.load(Ordering::Acquire);
        let free = !claimed & WPA_IE_SLOT_MASK;
        if free == 0 {
            return None;
        }
        let index = free.trailing_zeros() as usize;
        let bit = 1_usize << index;
        CLAIMED_WPA_IE_SLOTS
            .compare_exchange(claimed, claimed | bit, Ordering::AcqRel, Ordering::Acquire)
            .ok()?;
        let slot = WPA_IE_SLOTS[index].0.get();
        unsafe { slot.write([0; WPA_IE_CAPACITY]) };
        Some(slot.cast())
    }

    fn wpa_ie_slot_index(slot: *mut c_void) -> Option<usize> {
        let base = core::ptr::addr_of!(WPA_IE_SLOTS) as usize;
        let address = slot as usize;
        let stride = mem::size_of::<WpaIeSlot>();
        let offset = address.checked_sub(base)?;
        if offset % stride != 0 {
            return None;
        }
        let index = offset / stride;
        (index < WPA_IE_SLOT_CAPACITY).then_some(index)
    }

    fn release_wpa_ie_slot(slot: *mut c_void) -> bool {
        let Some(index) = wpa_ie_slot_index(slot) else {
            return false;
        };
        let bit = 1_usize << index;
        CLAIMED_WPA_IE_SLOTS.fetch_and(!bit, Ordering::AcqRel) & bit != 0
    }

    fn claim_rate_context(size: usize, caller: usize) -> Option<*mut c_void> {
        let expected_caller =
            rc_enable_trc as *const () as usize + RATE_CONTEXT_ALLOCATION_RETURN_OFFSET;
        if size != RATE_CONTEXT_SIZE || caller != expected_caller {
            return None;
        }
        let claimed = CLAIMED_RATE_CONTEXTS.load(Ordering::Acquire);
        let free = !claimed & RATE_CONTEXT_MASK;
        if free == 0 {
            return None;
        }
        let index = free.trailing_zeros() as usize;
        let bit = 1_usize << index;
        CLAIMED_RATE_CONTEXTS
            .compare_exchange(claimed, claimed | bit, Ordering::AcqRel, Ordering::Acquire)
            .ok()?;
        let context = RATE_CONTEXTS[index].0.get();
        unsafe { context.write(RateControlRecord::zeroed()) };
        Some(context.cast())
    }

    fn rate_context_index(context: *mut c_void) -> Option<usize> {
        let base = core::ptr::addr_of!(RATE_CONTEXTS) as usize;
        let address = context as usize;
        let stride = mem::size_of::<RateContext>();
        let offset = address.checked_sub(base)?;
        if offset % stride != 0 {
            return None;
        }
        let index = offset / stride;
        (index < RATE_CONTEXT_CAPACITY).then_some(index)
    }

    fn release_rate_context(context: *mut c_void) -> bool {
        let Some(index) = rate_context_index(context) else {
            return false;
        };
        let bit = 1_usize << index;
        if CLAIMED_RATE_CONTEXTS.fetch_and(!bit, Ordering::AcqRel) & bit == 0 {
            return false;
        }
        unsafe {
            RATE_CONTEXTS[index]
                .0
                .get()
                .write(RateControlRecord::zeroed())
        };
        true
    }

    /// Verify that a temporary C ABI pointer names a currently claimed
    /// Rust-owned peer rate-control record.
    pub(crate) fn owns_rate_control_record(context: *mut u8) -> bool {
        let Some(index) = rate_context_index(context.cast()) else {
            return false;
        };
        CLAIMED_RATE_CONTEXTS.load(Ordering::Acquire) & (1_usize << index) != 0
    }

    fn claim_rate_table_scratch(size: usize, caller: usize) -> Option<*mut c_void> {
        let expected_caller = ieee80211_setup_ratetable as *const () as usize
            + RATE_TABLE_SCRATCH_ALLOCATION_RETURN_OFFSET;
        if size != RATE_TABLE_SCRATCH_SIZE || caller != expected_caller {
            return None;
        }
        RATE_TABLE_SCRATCH_CLAIMED
            .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
            .ok()?;
        let scratch = RATE_TABLE_SCRATCH.0.get();
        unsafe { scratch.write([0; RATE_TABLE_SCRATCH_SIZE]) };
        Some(scratch.cast())
    }

    fn release_rate_table_scratch(scratch: *mut c_void) -> bool {
        if scratch != RATE_TABLE_SCRATCH.0.get().cast() {
            return false;
        }
        if RATE_TABLE_SCRATCH_CLAIMED.swap(0, Ordering::AcqRel) == 0 {
            return false;
        }
        unsafe {
            RATE_TABLE_SCRATCH
                .0
                .get()
                .write([0; RATE_TABLE_SCRATCH_SIZE])
        };
        true
    }

    #[cfg(feature = "rust-static-rx-buffer-init")]
    fn claim_wdev_rx_descriptor_arena(size: usize, caller: usize) -> Option<*mut c_void> {
        let expected_caller =
            wDev_Rxbuf_Init as *const () as usize + WDEV_RX_DESCRIPTOR_ALLOCATION_RETURN_OFFSET;
        if caller != expected_caller
            || size == 0
            || size > WDEV_RX_DESCRIPTOR_ARENA_SIZE
            || size % WDEV_RX_DESCRIPTOR_SIZE != 0
        {
            return None;
        }
        WDEV_RX_DESCRIPTOR_ARENA_CLAIMED
            .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
            .ok()?;
        let arena = WDEV_RX_DESCRIPTOR_ARENA.0.get();
        unsafe { arena.write([0; WDEV_RX_DESCRIPTOR_ARENA_SIZE]) };
        Some(arena.cast())
    }

    #[cfg(feature = "rust-static-rx-buffer-init")]
    fn release_wdev_rx_descriptor_arena(arena: *mut c_void) -> bool {
        if arena != WDEV_RX_DESCRIPTOR_ARENA.0.get().cast() {
            return false;
        }
        if WDEV_RX_DESCRIPTOR_ARENA_CLAIMED.swap(0, Ordering::AcqRel) == 0 {
            return false;
        }
        unsafe {
            WDEV_RX_DESCRIPTOR_ARENA
                .0
                .get()
                .write([0; WDEV_RX_DESCRIPTOR_ARENA_SIZE])
        };
        true
    }

    #[cfg(feature = "rust-static-rx-buffer-init")]
    fn claim_wdev_rx_payload(size: usize, caller: usize) -> Option<*mut c_void> {
        let expected_caller =
            wDev_Rxbuf_Init as *const () as usize + WDEV_RX_PAYLOAD_ALLOCATION_RETURN_OFFSET;
        if caller != expected_caller || size != WDEV_RX_PAYLOAD_SIZE {
            return None;
        }
        for (index, claimed) in CLAIMED_WDEV_RX_PAYLOADS.iter().enumerate() {
            if claimed
                .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                let payload = WDEV_RX_PAYLOADS[index].0.get();
                unsafe { payload.write([0; WDEV_RX_PAYLOAD_SIZE]) };
                return Some(payload.cast());
            }
        }
        None
    }

    #[cfg(feature = "rust-static-rx-buffer-init")]
    fn wdev_rx_payload_index(payload: *mut c_void) -> Option<usize> {
        let base = core::ptr::addr_of!(WDEV_RX_PAYLOADS) as usize;
        let address = payload as usize;
        let stride = mem::size_of::<WdevRxPayload>();
        let offset = address.checked_sub(base)?;
        if offset % stride != 0 {
            return None;
        }
        let index = offset / stride;
        (index < WDEV_RX_PAYLOAD_CAPACITY).then_some(index)
    }

    #[cfg(feature = "rust-static-rx-buffer-init")]
    fn release_wdev_rx_payload(payload: *mut c_void) -> bool {
        let Some(index) = wdev_rx_payload_index(payload) else {
            return false;
        };
        if CLAIMED_WDEV_RX_PAYLOADS[index].swap(0, Ordering::AcqRel) == 0 {
            return false;
        }
        unsafe {
            WDEV_RX_PAYLOADS[index]
                .0
                .get()
                .write([0; WDEV_RX_PAYLOAD_SIZE])
        };
        true
    }

    #[cfg(feature = "rust-static-esf-buffer-init")]
    fn claim_cold_esf_slot<const SIZE: usize, const CAPACITY: usize>(
        buffers: &'static [ColdEsfBuffer<SIZE>; CAPACITY],
        claims: &'static [AtomicUsize; CAPACITY],
    ) -> Option<*mut c_void> {
        for (index, claimed) in claims.iter().enumerate() {
            if claimed
                .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                let buffer = buffers[index].0.get();
                unsafe { buffer.write([0; SIZE]) };
                return Some(buffer.cast());
            }
        }
        None
    }

    #[cfg(feature = "rust-static-esf-buffer-init")]
    fn release_cold_esf_slot<const SIZE: usize, const CAPACITY: usize>(
        pointer: *mut c_void,
        buffers: &'static [ColdEsfBuffer<SIZE>; CAPACITY],
        claims: &'static [AtomicUsize; CAPACITY],
    ) -> bool {
        let base = buffers.as_ptr() as usize;
        let address = pointer as usize;
        let stride = mem::size_of::<ColdEsfBuffer<SIZE>>();
        let Some(offset) = address.checked_sub(base) else {
            return false;
        };
        if offset % stride != 0 {
            return false;
        }
        let index = offset / stride;
        if index >= CAPACITY || claims[index].swap(0, Ordering::AcqRel) == 0 {
            return false;
        }
        unsafe { buffers[index].0.get().write([0; SIZE]) };
        true
    }

    #[cfg(feature = "rust-static-esf-buffer-init")]
    fn claim_cold_esf_buffer(
        source: AllocationSource,
        size: usize,
        caller: usize,
    ) -> Option<*mut c_void> {
        if caller != ESP32S31_ECO0_ESF_DYNAMIC_ALLOCATION_RETURN_ADDRESS {
            return None;
        }
        match (source, size) {
            (AllocationSource::OsiWifiMalloc, ESF_WIFI_648_SIZE) => {
                claim_cold_esf_slot(&ESF_WIFI_648, &CLAIMED_ESF_WIFI_648)
            }
            (AllocationSource::OsiMallocInternal, ESF_INTERNAL_1748_SIZE) => {
                claim_cold_esf_slot(&ESF_INTERNAL_1748, &CLAIMED_ESF_INTERNAL_1748)
            }
            (AllocationSource::OsiMallocInternal, ESF_INTERNAL_788_SIZE) => {
                claim_cold_esf_slot(&ESF_INTERNAL_788, &CLAIMED_ESF_INTERNAL_788)
            }
            _ => None,
        }
    }

    #[cfg(feature = "rust-static-esf-buffer-init")]
    fn release_cold_esf_buffer(pointer: *mut c_void) -> bool {
        release_cold_esf_slot(pointer, &ESF_WIFI_648, &CLAIMED_ESF_WIFI_648)
            || release_cold_esf_slot(
                pointer,
                &ESF_INTERNAL_1748,
                &CLAIMED_ESF_INTERNAL_1748,
            )
            || release_cold_esf_slot(pointer, &ESF_INTERNAL_788, &CLAIMED_ESF_INTERNAL_788)
    }

    #[cfg(feature = "rust-static-wifi-nvs-storage")]
    fn claim_wifi_nvs_static_buffer<const SIZE: usize>(
        buffer: &'static WifiNvsStaticBuffer<SIZE>,
        claimed: &'static AtomicUsize,
    ) -> Option<*mut c_void> {
        claimed
            .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
            .ok()?;
        let pointer = buffer.0.get();
        unsafe { pointer.write([0; SIZE]) };
        Some(pointer.cast())
    }

    #[cfg(feature = "rust-static-wifi-nvs-storage")]
    fn release_wifi_nvs_static_buffer<const SIZE: usize>(
        pointer: *mut c_void,
        buffer: &'static WifiNvsStaticBuffer<SIZE>,
        claimed: &'static AtomicUsize,
    ) -> bool {
        if pointer != buffer.0.get().cast() || claimed.swap(0, Ordering::AcqRel) == 0 {
            return false;
        }
        unsafe { buffer.0.get().write([0; SIZE]) };
        true
    }

    #[cfg(feature = "rust-static-wifi-nvs-storage")]
    fn claim_wifi_nvs_storage(
        source: AllocationSource,
        size: usize,
        caller: usize,
    ) -> Option<*mut c_void> {
        let allocation = classify_wifi_nvs_static_allocation(
            source,
            size,
            caller,
            wifi_nvs_cfg_init as *const () as usize,
        )?;
        match allocation {
            WifiNvsStaticAllocation::ConfigItems => claim_wifi_nvs_static_buffer(
                &WIFI_NVS_CFG_ITEMS,
                &WIFI_NVS_CFG_ITEMS_CLAIMED,
            ),
            WifiNvsStaticAllocation::LoadScratch => claim_wifi_nvs_static_buffer(
                &WIFI_NVS_LOAD_SCRATCH,
                &WIFI_NVS_LOAD_SCRATCH_CLAIMED,
            ),
        }
    }

    #[cfg(feature = "rust-static-wifi-nvs-storage")]
    fn release_wifi_nvs_storage(pointer: *mut c_void) -> bool {
        release_wifi_nvs_static_buffer(
            pointer,
            &WIFI_NVS_CFG_ITEMS,
            &WIFI_NVS_CFG_ITEMS_CLAIMED,
        ) || release_wifi_nvs_static_buffer(
            pointer,
            &WIFI_NVS_LOAD_SCRATCH,
            &WIFI_NVS_LOAD_SCRATCH_CLAIMED,
        )
    }

    #[cfg(feature = "rust-static-function-table-storage")]
    fn claim_static_function_table_buffer<const SIZE: usize>(
        buffer: &'static StaticFunctionTableBuffer<SIZE>,
        claimed: &'static AtomicUsize,
    ) -> Option<*mut c_void> {
        claimed
            .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
            .ok()?;
        let pointer = buffer.0.get();
        unsafe { pointer.write([0; SIZE]) };
        Some(pointer.cast())
    }

    #[cfg(feature = "rust-static-function-table-storage")]
    fn release_static_function_table_buffer<const SIZE: usize>(
        pointer: *mut c_void,
        buffer: &'static StaticFunctionTableBuffer<SIZE>,
        claimed: &'static AtomicUsize,
    ) -> bool {
        if pointer != buffer.0.get().cast() || claimed.swap(0, Ordering::AcqRel) == 0 {
            return false;
        }
        unsafe { buffer.0.get().write([0; SIZE]) };
        true
    }

    #[cfg(feature = "rust-static-function-table-storage")]
    fn claim_static_function_table(
        source: AllocationSource,
        size: usize,
        caller: usize,
    ) -> Option<*mut c_void> {
        let table = classify_static_function_table(
            source,
            size,
            caller,
            wdev_funcs_init as *const () as usize,
            net80211_funcs_init as *const () as usize,
        )?;
        match table {
            StaticFunctionTable::Wdev => claim_static_function_table_buffer(
                &WDEV_FUNCTION_TABLE,
                &WDEV_FUNCTION_TABLE_CLAIMED,
            ),
            StaticFunctionTable::Net80211 => claim_static_function_table_buffer(
                &NET80211_FUNCTION_TABLE,
                &NET80211_FUNCTION_TABLE_CLAIMED,
            ),
        }
    }

    #[cfg(feature = "rust-static-function-table-storage")]
    fn release_static_function_table(pointer: *mut c_void) -> bool {
        release_static_function_table_buffer(
            pointer,
            &WDEV_FUNCTION_TABLE,
            &WDEV_FUNCTION_TABLE_CLAIMED,
        ) || release_static_function_table_buffer(
            pointer,
            &NET80211_FUNCTION_TABLE,
            &NET80211_FUNCTION_TABLE_CLAIMED,
        )
    }

    #[cfg(feature = "rust-static-interface-storage")]
    fn claim_static_interface_buffer<const SIZE: usize>(
        buffer: &'static StaticInterfaceBuffer<SIZE>,
        claimed: &'static AtomicUsize,
    ) -> Option<*mut c_void> {
        claimed
            .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
            .ok()?;
        let pointer = buffer.0.get();
        unsafe { pointer.write([0; SIZE]) };
        Some(pointer.cast())
    }

    #[cfg(feature = "rust-static-interface-storage")]
    fn release_static_interface_buffer<const SIZE: usize>(
        pointer: *mut c_void,
        buffer: &'static StaticInterfaceBuffer<SIZE>,
        claimed: &'static AtomicUsize,
    ) -> bool {
        if pointer != buffer.0.get().cast() || claimed.swap(0, Ordering::AcqRel) == 0 {
            return false;
        }
        unsafe { buffer.0.get().write([0; SIZE]) };
        true
    }

    #[cfg(feature = "rust-static-interface-storage")]
    fn claim_static_interface_storage(
        source: AllocationSource,
        size: usize,
        caller: usize,
    ) -> Option<*mut c_void> {
        let allocation = classify_static_interface_allocation(
            source,
            size,
            caller,
            wifi_create_sta as *const () as usize,
            wifi_create_softap as *const () as usize,
        )?;
        match allocation {
            StaticInterfaceAllocation::State => claim_static_interface_buffer(
                &WIFI_INTERFACE_STATE,
                &WIFI_INTERFACE_STATE_CLAIMED,
            ),
            StaticInterfaceAllocation::Phy => {
                claim_static_interface_buffer(&WIFI_INTERFACE_PHY, &WIFI_INTERFACE_PHY_CLAIMED)
            }
        }
    }

    #[cfg(feature = "rust-static-interface-storage")]
    fn release_static_interface_storage(pointer: *mut c_void) -> bool {
        release_static_interface_buffer(
            pointer,
            &WIFI_INTERFACE_STATE,
            &WIFI_INTERFACE_STATE_CLAIMED,
        ) || release_static_interface_buffer(
            pointer,
            &WIFI_INTERFACE_PHY,
            &WIFI_INTERFACE_PHY_CLAIMED,
        )
    }

    #[cfg(feature = "rust-static-supplicant-callback-storage")]
    fn claim_static_supplicant_callbacks(
        source: AllocationSource,
        size: usize,
        caller: usize,
    ) -> Option<*mut c_void> {
        if !is_static_supplicant_callback_allocation(
            source,
            size,
            caller,
            esp_supplicant_init as *const () as usize,
        ) {
            return None;
        }
        SUPPLICANT_CALLBACK_TABLE_CLAIMED
            .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
            .ok()?;
        let table = SUPPLICANT_CALLBACK_TABLE.0.get();
        unsafe { table.write([0; SUPPLICANT_CALLBACK_TABLE_SIZE]) };
        Some(table.cast())
    }

    #[cfg(feature = "rust-static-supplicant-callback-storage")]
    fn release_static_supplicant_callbacks(pointer: *mut c_void) -> bool {
        if pointer != SUPPLICANT_CALLBACK_TABLE.0.get().cast()
            || SUPPLICANT_CALLBACK_TABLE_CLAIMED.swap(0, Ordering::AcqRel) == 0
        {
            return false;
        }
        unsafe {
            SUPPLICANT_CALLBACK_TABLE
                .0
                .get()
                .write([0; SUPPLICANT_CALLBACK_TABLE_SIZE])
        };
        true
    }

    #[cfg(feature = "rust-static-supplicant-callback-storage")]
    pub unsafe fn static_supplicant_callback_table_bound() -> bool {
        SUPPLICANT_CALLBACK_TABLE_CLAIMED.load(Ordering::Acquire) == 1
            && core::ptr::addr_of!(wpa_cb).read() == SUPPLICANT_CALLBACK_TABLE.0.get().cast()
    }

    #[cfg(feature = "rust-static-pp-bar-storage")]
    fn claim_static_pp_bar() -> Option<*mut c_void> {
        for (index, claim) in PP_BAR_CLAIMS.iter().enumerate() {
            if claim
                .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                let pointer = PP_BARS[index].0.get();
                unsafe { pointer.write([0; PP_BAR_SIZE]) };
                return Some(pointer.cast());
            }
        }
        None
    }

    #[cfg(feature = "rust-static-pp-bar-storage")]
    fn release_static_pp_bar(pointer: *mut c_void) -> bool {
        let base = PP_BARS.as_ptr() as usize;
        let address = pointer as usize;
        let stride = mem::size_of::<PpBar>();
        let Some(offset) = address.checked_sub(base) else {
            return false;
        };
        if offset % stride != 0 {
            return false;
        }
        let index = offset / stride;
        if index >= PP_BAR_CAPACITY {
            return false;
        }
        if PP_BAR_CLAIMS[index].swap(0, Ordering::AcqRel) != 0 {
            unsafe { PP_BARS[index].0.get().write([0; PP_BAR_SIZE]) };
        }
        // An exact pool address never belongs to the captured heap, even if a
        // drifting vendor teardown presents it twice. Consume such a free
        // rather than forwarding a static SRAM address to the heap.
        true
    }

    #[cfg(feature = "rust-static-pp-bar-storage")]
    pub unsafe fn static_pp_bar_storage_bound() -> bool {
        (0..PP_BAR_CAPACITY).all(|index| {
            PP_BAR_CLAIMS[index].load(Ordering::Acquire) == 1
                && core::ptr::addr_of!(s_bars)
                    .cast::<*mut c_void>()
                    .add(index)
                    .read_volatile()
                    == PP_BARS[index].0.get().cast()
        })
    }

    #[cfg(feature = "rust-static-cold-api-envelope-storage")]
    fn claim_static_cold_api_envelope(kind: ColdApiEnvelopeKind) -> Option<*mut c_void> {
        let index = kind.index();
        COLD_API_ENVELOPE_CLAIMS[index]
            .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
            .ok()?;
        let pointer = COLD_API_ENVELOPES[index].0.get();
        unsafe { pointer.write([0; COLD_API_ENVELOPE_SIZE]) };
        COLD_API_ENVELOPE_USES[index].fetch_add(1, Ordering::Relaxed);
        Some(pointer.cast())
    }

    #[cfg(feature = "rust-static-cold-api-envelope-storage")]
    fn release_static_cold_api_envelope(pointer: *mut c_void) -> bool {
        let base = COLD_API_ENVELOPES.as_ptr() as usize;
        let address = pointer as usize;
        let stride = mem::size_of::<ColdApiEnvelope>();
        let Some(offset) = address.checked_sub(base) else {
            return false;
        };
        if offset % stride != 0 {
            return false;
        }
        let index = offset / stride;
        if index >= COLD_API_ENVELOPE_CAPACITY {
            return false;
        }
        if COLD_API_ENVELOPE_CLAIMS[index].swap(0, Ordering::AcqRel) != 0 {
            unsafe { COLD_API_ENVELOPES[index].0.get().write([0; COLD_API_ENVELOPE_SIZE]) };
            COLD_API_ENVELOPE_RELEASES[index].fetch_add(1, Ordering::Relaxed);
        }
        // An exact pool address never belongs to the captured heap. Consume
        // duplicate frees rather than forwarding static SRAM to it.
        true
    }

    #[cfg(feature = "rust-static-cold-api-envelope-storage")]
    pub fn static_cold_api_envelope_storage_quiescent() -> bool {
        (0..COLD_API_ENVELOPE_CAPACITY).all(|index| {
            let uses = COLD_API_ENVELOPE_USES[index].load(Ordering::Acquire);
            COLD_API_ENVELOPE_CLAIMS[index].load(Ordering::Acquire) == 0
                && uses != 0
                && COLD_API_ENVELOPE_RELEASES[index].load(Ordering::Acquire) == uses
        })
    }

    fn release_strict_allocation(ptr: *mut c_void) -> bool {
        #[cfg(feature = "rust-static-cold-api-envelope-storage")]
        if release_static_cold_api_envelope(ptr) {
            return true;
        }
        #[cfg(feature = "rust-static-pp-bar-storage")]
        if release_static_pp_bar(ptr) {
            return true;
        }
        #[cfg(feature = "rust-static-supplicant-callback-storage")]
        if release_static_supplicant_callbacks(ptr) {
            return true;
        }
        #[cfg(feature = "rust-static-interface-storage")]
        if release_static_interface_storage(ptr) {
            return true;
        }
        #[cfg(feature = "rust-static-function-table-storage")]
        if release_static_function_table(ptr) {
            return true;
        }
        #[cfg(feature = "rust-static-wifi-nvs-storage")]
        if release_wifi_nvs_storage(ptr) {
            return true;
        }
        #[cfg(feature = "rust-static-esf-buffer-init")]
        if release_cold_esf_buffer(ptr) {
            return true;
        }
        #[cfg(feature = "rust-static-rx-buffer-init")]
        if release_wdev_rx_descriptor_arena(ptr) || release_wdev_rx_payload(ptr) {
            return true;
        }
        release_blacklist_node(ptr)
            || release_ipc_envelope(ptr)
            || release_wpa_ie_slot(ptr)
            || release_rate_context(ptr)
            || release_rate_table_scratch(ptr)
            || unsafe { crate::wpa2_s31::release_static_vendor_key_object(ptr) }
            || unsafe { crate::wpa2_s31::release_static_ap_node(ptr) }
    }

    #[inline(always)]
    fn caller_address() -> usize {
        let caller: usize;
        unsafe {
            core::arch::asm!("mv {caller}, ra", caller = out(reg) caller, options(nomem, nostack))
        };
        caller
    }

    unsafe extern "C" {
        #[link_name = "malloc"]
        fn direct_malloc(size: usize) -> *mut c_void;
        #[link_name = "calloc"]
        fn direct_calloc(count: usize, size: usize) -> *mut c_void;
        #[link_name = "realloc"]
        fn direct_realloc(ptr: *mut c_void, size: usize) -> *mut c_void;
        #[link_name = "free"]
        fn direct_free(ptr: *mut c_void);
        fn __real_malloc(size: usize) -> *mut c_void;
        fn __real_calloc(count: usize, size: usize) -> *mut c_void;
        fn __real_realloc(ptr: *mut c_void, size: usize) -> *mut c_void;
        fn __real_free(ptr: *mut c_void);
    }

    pub(crate) fn direct_heap_link_wrappers_active() -> bool {
        core::ptr::eq(direct_malloc as *const (), __wrap_malloc as *const ())
            && core::ptr::eq(direct_calloc as *const (), __wrap_calloc as *const ())
            && core::ptr::eq(direct_realloc as *const (), __wrap_realloc as *const ())
            && core::ptr::eq(direct_free as *const (), __wrap_free as *const ())
    }

    /// Final-link guard for direct C `malloc` references in vendor archives.
    /// Requires `-Wl,--wrap=malloc` in the firmware link.
    #[no_mangle]
    pub unsafe extern "C" fn __wrap_malloc(size: usize) -> *mut c_void {
        let caller = caller_address();
        if heap_forbidden() {
            if let Some(slot) = claim_wpa_ie_slot(size, caller) {
                return slot;
            }
            PROBE.record_request_at(
                size,
                true,
                false,
                AllocationSource::DirectMalloc,
                caller,
                0,
            );
            return core::ptr::null_mut();
        }
        let result = __real_malloc(size);
        PROBE.record_request_at(
            size,
            result.is_null(),
            false,
            AllocationSource::DirectMalloc,
            caller,
            result as usize,
        );
        result
    }

    /// Final-link guard for direct C `calloc` references.
    #[no_mangle]
    pub unsafe extern "C" fn __wrap_calloc(count: usize, size: usize) -> *mut c_void {
        let requested = count.saturating_mul(size);
        let caller = caller_address();
        #[cfg(feature = "rust-static-supplicant-callback-storage")]
        if is_static_supplicant_callback_allocation(
            AllocationSource::DirectCalloc,
            requested,
            caller,
            esp_supplicant_init as *const () as usize,
        ) {
            let result = claim_static_supplicant_callbacks(
                AllocationSource::DirectCalloc,
                requested,
                caller,
            )
            .unwrap_or(core::ptr::null_mut());
            if result.is_null() {
                PROBE.record_request_at(
                    requested,
                    true,
                    false,
                    AllocationSource::DirectCalloc,
                    caller,
                    0,
                );
            }
            return result;
        }
        if heap_forbidden() {
            PROBE.record_request_at(
                requested,
                true,
                false,
                AllocationSource::DirectCalloc,
                caller,
                0,
            );
            return core::ptr::null_mut();
        }
        let result = __real_calloc(count, size);
        PROBE.record_request_at(
            requested,
            result.is_null(),
            false,
            AllocationSource::DirectCalloc,
            caller,
            result as usize,
        );
        result
    }

    /// Final-link guard for direct C `realloc` references.
    #[no_mangle]
    pub unsafe extern "C" fn __wrap_realloc(ptr: *mut c_void, size: usize) -> *mut c_void {
        let caller = caller_address();
        if heap_forbidden() {
            PROBE.record_request_at(
                size,
                true,
                true,
                AllocationSource::DirectRealloc,
                caller,
                0,
            );
            return core::ptr::null_mut();
        }
        let result = __real_realloc(ptr, size);
        PROBE.record_request_at(
            size,
            result.is_null() && size != 0,
            true,
            AllocationSource::DirectRealloc,
            caller,
            result as usize,
        );
        result
    }

    /// Final-link guard for direct C `free` references.
    #[no_mangle]
    pub unsafe extern "C" fn __wrap_free(ptr: *mut c_void) {
        // ISO C defines `free(NULL)` as a no-op.  The vendor peer teardown
        // unconditionally frees its optional per-peer rate-control context;
        // strict AP may deliberately leave that slot null.  Do not report the
        // absence of an allocation as a runtime heap operation.
        if ptr.is_null() {
            return;
        }
        let caller = caller_address();
        if release_strict_allocation(ptr) {
            return;
        }
        PROBE.record_free_at(ptr as usize, caller);
        if !heap_forbidden() {
            __real_free(ptr);
        }
    }

    unsafe fn wrap(slot: &mut Option<Malloc>, saved: &AtomicUsize, wrapper: Malloc) {
        if let Some(original) = *slot {
            saved.store(original as usize, Ordering::Release);
            *slot = Some(wrapper);
        }
    }

    unsafe fn wrap_realloc(slot: &mut Option<Realloc>, saved: &AtomicUsize, wrapper: Realloc) {
        if let Some(original) = *slot {
            saved.store(original as usize, Ordering::Release);
            *slot = Some(wrapper);
        }
    }

    unsafe fn wrap_calloc(slot: &mut Option<Calloc>, saved: &AtomicUsize, wrapper: Calloc) {
        if let Some(original) = *slot {
            saved.store(original as usize, Ordering::Release);
            *slot = Some(wrapper);
        }
    }

    unsafe fn call_malloc(
        saved: &AtomicUsize,
        size: usize,
        source: AllocationSource,
        caller: usize,
    ) -> *mut c_void {
        #[cfg(feature = "rust-static-cold-api-envelope-storage")]
        if let Some(kind) = classify_static_cold_api_envelope(
            source,
            size,
            caller,
            esp_wifi_init_internal as *const () as usize,
            esp_wifi_start as *const () as usize,
        ) {
            let result =
                claim_static_cold_api_envelope(kind).unwrap_or(core::ptr::null_mut());
            if result.is_null() {
                PROBE.record_request_at(size, true, false, source, caller, 0);
            }
            return result;
        }
        #[cfg(feature = "rust-static-pp-bar-storage")]
        if is_static_pp_bar_allocation(
            source,
            size,
            caller,
            pp_attach as *const () as usize,
        ) {
            let result = claim_static_pp_bar().unwrap_or(core::ptr::null_mut());
            if result.is_null() {
                PROBE.record_request_at(size, true, false, source, caller, 0);
            }
            return result;
        }
        #[cfg(feature = "rust-static-interface-storage")]
        if let Some(buffer) = claim_static_interface_storage(source, size, caller) {
            return buffer;
        }
        #[cfg(feature = "rust-static-wifi-nvs-storage")]
        if let Some(buffer) = claim_wifi_nvs_storage(source, size, caller) {
            return buffer;
        }
        #[cfg(feature = "rust-static-esf-buffer-init")]
        if let Some(buffer) = claim_cold_esf_buffer(source, size, caller) {
            return buffer;
        }
        #[cfg(feature = "rust-static-rx-buffer-init")]
        match source {
            AllocationSource::OsiZallocInternal => {
                if let Some(arena) = claim_wdev_rx_descriptor_arena(size, caller) {
                    return arena;
                }
            }
            AllocationSource::OsiMallocInternal => {
                if let Some(payload) = claim_wdev_rx_payload(size, caller) {
                    return payload;
                }
            }
            _ => {}
        }
        #[cfg(feature = "rust-static-rate-table-storage")]
        if source == AllocationSource::OsiWifiZalloc {
            if let Some(scratch) = claim_rate_table_scratch(size, caller) {
                return scratch;
            }
        }
        if heap_forbidden() {
            if source == AllocationSource::OsiWifiMalloc {
                if let Some(node) = claim_blacklist_node(size, caller) {
                    return node;
                }
            }
            if source == AllocationSource::OsiWifiZalloc {
                if let Some(envelope) = claim_ipc_envelope(size, caller) {
                    return envelope;
                }
                if let Some(context) = claim_rate_context(size, caller) {
                    return context;
                }
                if let Some(scratch) = claim_rate_table_scratch(size, caller) {
                    return scratch;
                }
            }
            PROBE.record_request_at(size, true, false, source, caller, 0);
            return core::ptr::null_mut();
        }
        let original = mem::transmute::<usize, Malloc>(saved.load(Ordering::Acquire));
        let result = original(size);
        PROBE.record_request_at(
            size,
            result.is_null(),
            false,
            source,
            caller,
            result as usize,
        );
        result
    }

    unsafe fn call_realloc(
        saved: &AtomicUsize,
        ptr: *mut c_void,
        size: usize,
        source: AllocationSource,
        caller: usize,
    ) -> *mut c_void {
        if heap_forbidden() {
            PROBE.record_request_at(size, true, true, source, caller, 0);
            return core::ptr::null_mut();
        }
        let original = mem::transmute::<usize, Realloc>(saved.load(Ordering::Acquire));
        let result = original(ptr, size);
        PROBE.record_request_at(
            size,
            result.is_null() && size != 0,
            true,
            source,
            caller,
            result as usize,
        );
        result
    }

    unsafe fn call_calloc(
        saved: &AtomicUsize,
        count: usize,
        size: usize,
        source: AllocationSource,
        caller: usize,
    ) -> *mut c_void {
        let total_size = count.saturating_mul(size);
        #[cfg(feature = "rust-static-function-table-storage")]
        if let Some(buffer) = claim_static_function_table(source, total_size, caller) {
            return buffer;
        }
        if heap_forbidden() {
            PROBE.record_request_at(total_size, true, false, source, caller, 0);
            return core::ptr::null_mut();
        }
        let original = mem::transmute::<usize, Calloc>(saved.load(Ordering::Acquire));
        let result = original(count, size);
        PROBE.record_request_at(
            total_size,
            result.is_null(),
            false,
            source,
            caller,
            result as usize,
        );
        result
    }

    unsafe extern "C" fn malloc(size: usize) -> *mut c_void {
        call_malloc(&MALLOC, size, AllocationSource::OsiMalloc, caller_address())
    }
    unsafe extern "C" fn free(ptr: *mut c_void) {
        if ptr.is_null() {
            return;
        }
        let caller = caller_address();
        if release_strict_allocation(ptr) {
            return;
        }
        PROBE.record_free_at(ptr as usize, caller);
        if heap_forbidden() {
            return;
        }
        let original = mem::transmute::<usize, Free>(FREE.load(Ordering::Acquire));
        original(ptr);
    }
    unsafe extern "C" fn malloc_internal(size: usize) -> *mut c_void {
        call_malloc(
            &MALLOC_INTERNAL,
            size,
            AllocationSource::OsiMallocInternal,
            caller_address(),
        )
    }
    unsafe extern "C" fn realloc_internal(ptr: *mut c_void, size: usize) -> *mut c_void {
        call_realloc(
            &REALLOC_INTERNAL,
            ptr,
            size,
            AllocationSource::OsiReallocInternal,
            caller_address(),
        )
    }
    unsafe extern "C" fn calloc_internal(count: usize, size: usize) -> *mut c_void {
        call_calloc(
            &CALLOC_INTERNAL,
            count,
            size,
            AllocationSource::OsiCallocInternal,
            caller_address(),
        )
    }
    unsafe extern "C" fn zalloc_internal(size: usize) -> *mut c_void {
        call_malloc(
            &ZALLOC_INTERNAL,
            size,
            AllocationSource::OsiZallocInternal,
            caller_address(),
        )
    }
    unsafe extern "C" fn wifi_malloc(size: usize) -> *mut c_void {
        call_malloc(
            &WIFI_MALLOC,
            size,
            AllocationSource::OsiWifiMalloc,
            caller_address(),
        )
    }
    unsafe extern "C" fn wifi_realloc(ptr: *mut c_void, size: usize) -> *mut c_void {
        call_realloc(
            &WIFI_REALLOC,
            ptr,
            size,
            AllocationSource::OsiWifiRealloc,
            caller_address(),
        )
    }
    unsafe extern "C" fn wifi_calloc(count: usize, size: usize) -> *mut c_void {
        call_calloc(
            &WIFI_CALLOC,
            count,
            size,
            AllocationSource::OsiWifiCalloc,
            caller_address(),
        )
    }
    unsafe extern "C" fn wifi_zalloc(size: usize) -> *mut c_void {
        call_malloc(
            &WIFI_ZALLOC,
            size,
            AllocationSource::OsiWifiZalloc,
            caller_address(),
        )
    }

    const _: () = assert!(mem::size_of::<BlacklistNode>() == BLACKLIST_NODE_SIZE);
    const _: () = assert!(BLACKLIST_NODE_CAPACITY < usize::BITS as usize);
    const _: () = assert!(mem::size_of::<IpcEnvelope>() == IPC_ENVELOPE_SIZE);
    const _: () = assert!(IPC_ENVELOPE_CAPACITY < usize::BITS as usize);
    const _: () = assert!(mem::size_of::<WpaIeSlot>() == WPA_IE_CAPACITY);
    const _: () = assert!(WPA_IE_SLOT_CAPACITY < usize::BITS as usize);
    const _: () = assert!(mem::size_of::<RateContext>() == RATE_CONTEXT_SIZE);
    const _: () = assert!(RATE_CONTEXT_CAPACITY < usize::BITS as usize);
    const _: () = assert!(mem::size_of::<RateTableScratch>() == RATE_TABLE_SCRATCH_SIZE);
    #[cfg(feature = "rust-static-rx-buffer-init")]
    const _: () = assert!(mem::align_of::<WdevRxDescriptorArena>() >= 16);
    #[cfg(feature = "rust-static-rx-buffer-init")]
    const _: () = assert!(mem::align_of::<WdevRxPayload>() >= 16);
    #[cfg(feature = "rust-static-esf-buffer-init")]
    const _: () = assert!(mem::align_of::<ColdEsfBuffer<ESF_WIFI_648_SIZE>>() >= 16);
    #[cfg(feature = "rust-static-esf-buffer-init")]
    const _: () = assert!(mem::align_of::<ColdEsfBuffer<ESF_INTERNAL_1748_SIZE>>() >= 16);
    #[cfg(feature = "rust-static-esf-buffer-init")]
    const _: () = assert!(mem::align_of::<ColdEsfBuffer<ESF_INTERNAL_788_SIZE>>() >= 16);
    #[cfg(feature = "rust-static-wifi-nvs-storage")]
    const _: () =
        assert!(mem::align_of::<WifiNvsStaticBuffer<WIFI_NVS_CFG_ITEMS_SIZE>>() >= 16);
    #[cfg(feature = "rust-static-wifi-nvs-storage")]
    const _: () =
        assert!(mem::align_of::<WifiNvsStaticBuffer<WIFI_NVS_LOAD_SCRATCH_SIZE>>() >= 16);
    #[cfg(feature = "rust-static-function-table-storage")]
    const _: () =
        assert!(mem::align_of::<StaticFunctionTableBuffer<WDEV_FUNCTION_TABLE_SIZE>>() >= 16);
    #[cfg(feature = "rust-static-function-table-storage")]
    const _: () = assert!(
        mem::align_of::<StaticFunctionTableBuffer<NET80211_FUNCTION_TABLE_SIZE>>() >= 16
    );
    #[cfg(feature = "rust-static-interface-storage")]
    const _: () =
        assert!(mem::align_of::<StaticInterfaceBuffer<WIFI_INTERFACE_STATE_SIZE>>() >= 16);
    #[cfg(feature = "rust-static-interface-storage")]
    const _: () =
        assert!(mem::align_of::<StaticInterfaceBuffer<WIFI_INTERFACE_PHY_SIZE>>() >= 16);
    #[cfg(feature = "rust-static-supplicant-callback-storage")]
    const _: () =
        assert!(mem::size_of::<SupplicantCallbackTable>() == SUPPLICANT_CALLBACK_TABLE_SIZE);
    #[cfg(feature = "rust-static-supplicant-callback-storage")]
    const _: () = assert!(mem::align_of::<SupplicantCallbackTable>() >= 4);
    #[cfg(feature = "rust-static-pp-bar-storage")]
    const _: () = assert!(mem::size_of::<PpBar>() == PP_BAR_SIZE);
    #[cfg(feature = "rust-static-pp-bar-storage")]
    const _: () = assert!(mem::align_of::<PpBar>() >= 4);
    #[cfg(feature = "rust-static-cold-api-envelope-storage")]
    const _: () = assert!(mem::size_of::<ColdApiEnvelope>() == COLD_API_ENVELOPE_SIZE);
    #[cfg(feature = "rust-static-cold-api-envelope-storage")]
    const _: () = assert!(mem::align_of::<ColdApiEnvelope>() >= 4);
}

#[cfg(all(
    target_arch = "riscv32",
    feature = "rust-static-cold-api-envelope-storage"
))]
pub use target::static_cold_api_envelope_storage_quiescent;
#[cfg(all(target_arch = "riscv32", feature = "rust-static-pp-bar-storage"))]
pub use target::static_pp_bar_storage_bound;
#[cfg(all(
    target_arch = "riscv32",
    feature = "rust-static-supplicant-callback-storage"
))]
pub use target::static_supplicant_callback_table_bound;
#[cfg(target_arch = "riscv32")]
pub(crate) use target::{
    allocator_callbacks_patched, direct_heap_link_wrappers_active, forbid_runtime_heap,
    owns_rate_control_record,
};
#[cfg(target_arch = "riscv32")]
pub use target::{allow_heap_for_wifi_teardown, patch_allocator_probes};

#[cfg(test)]
mod tests {
    use super::AllocationProbe;

    #[test]
    fn allocation_probe_tracks_requests() {
        #[cfg(feature = "hil-cold-allocation-trace")]
        let trace_before = super::cold_allocation_trace_len();
        let probe = AllocationProbe::new();
        probe.record_request(16, false, false);
        probe.record_request(48, true, true);
        probe.record_free();
        let snapshot = probe.snapshot();
        assert_eq!(snapshot.allocations, 1);
        assert_eq!(snapshot.reallocations, 1);
        assert_eq!(snapshot.frees, 1);
        assert_eq!(snapshot.requested_bytes, 64);
        assert_eq!(snapshot.largest_request, 48);
        assert_eq!(snapshot.failures, 1);
        #[cfg(feature = "hil-cold-allocation-trace")]
        {
            assert_eq!(super::cold_allocation_trace_len(), trace_before + 2);
            assert_eq!(
                super::cold_allocation_trace_entry(trace_before),
                Some(super::ColdAllocationTraceEntry {
                    source: super::AllocationSource::None,
                    caller: 0,
                    size: 16,
                    pointer: 0,
                    realloc: false,
                    failed: false,
                })
            );
            assert_eq!(
                super::cold_allocation_trace_entry(trace_before + 1),
                Some(super::ColdAllocationTraceEntry {
                    source: super::AllocationSource::None,
                    caller: 0,
                    size: 48,
                    pointer: 0,
                    realloc: true,
                    failed: true,
                })
            );
        }
    }

    #[cfg(feature = "rust-static-wifi-nvs-storage")]
    #[test]
    fn wifi_nvs_static_admission_is_exact() {
        use super::{
            classify_wifi_nvs_static_allocation, AllocationSource, WifiNvsStaticAllocation,
            WIFI_NVS_CFG_ITEMS_RETURN_OFFSET, WIFI_NVS_CFG_ITEMS_SIZE,
            WIFI_NVS_LOAD_SCRATCH_RETURN_OFFSET, WIFI_NVS_LOAD_SCRATCH_SIZE,
        };

        const BASE: usize = 0x4006_b980;
        assert_eq!(
            classify_wifi_nvs_static_allocation(
                AllocationSource::OsiWifiZalloc,
                WIFI_NVS_CFG_ITEMS_SIZE,
                BASE + WIFI_NVS_CFG_ITEMS_RETURN_OFFSET,
                BASE,
            ),
            Some(WifiNvsStaticAllocation::ConfigItems)
        );
        assert_eq!(
            classify_wifi_nvs_static_allocation(
                AllocationSource::OsiMallocInternal,
                WIFI_NVS_LOAD_SCRATCH_SIZE,
                BASE + WIFI_NVS_LOAD_SCRATCH_RETURN_OFFSET,
                BASE,
            ),
            Some(WifiNvsStaticAllocation::LoadScratch)
        );

        for (source, size, caller) in [
            (
                AllocationSource::OsiWifiMalloc,
                WIFI_NVS_CFG_ITEMS_SIZE,
                BASE + WIFI_NVS_CFG_ITEMS_RETURN_OFFSET,
            ),
            (
                AllocationSource::OsiWifiZalloc,
                WIFI_NVS_CFG_ITEMS_SIZE - 1,
                BASE + WIFI_NVS_CFG_ITEMS_RETURN_OFFSET,
            ),
            (
                AllocationSource::OsiWifiZalloc,
                WIFI_NVS_CFG_ITEMS_SIZE,
                BASE + WIFI_NVS_CFG_ITEMS_RETURN_OFFSET + 2,
            ),
            (
                AllocationSource::OsiWifiZalloc,
                WIFI_NVS_LOAD_SCRATCH_SIZE,
                BASE + WIFI_NVS_LOAD_SCRATCH_RETURN_OFFSET,
            ),
        ] {
            assert_eq!(
                classify_wifi_nvs_static_allocation(source, size, caller, BASE),
                None
            );
        }
    }

    #[cfg(feature = "rust-static-function-table-storage")]
    #[test]
    fn static_function_table_admission_is_exact() {
        use super::{
            classify_static_function_table, AllocationSource, StaticFunctionTable,
            NET80211_FUNCTION_TABLE_RETURN_OFFSET, NET80211_FUNCTION_TABLE_SIZE,
            WDEV_FUNCTION_TABLE_RETURN_OFFSET, WDEV_FUNCTION_TABLE_SIZE,
        };

        const WDEV_BASE: usize = 0x4005_1000;
        const NET80211_BASE: usize = 0x4005_2000;
        assert_eq!(
            classify_static_function_table(
                AllocationSource::OsiCallocInternal,
                WDEV_FUNCTION_TABLE_SIZE,
                WDEV_BASE + WDEV_FUNCTION_TABLE_RETURN_OFFSET,
                WDEV_BASE,
                NET80211_BASE,
            ),
            Some(StaticFunctionTable::Wdev)
        );
        assert_eq!(
            classify_static_function_table(
                AllocationSource::OsiCallocInternal,
                NET80211_FUNCTION_TABLE_SIZE,
                NET80211_BASE + NET80211_FUNCTION_TABLE_RETURN_OFFSET,
                WDEV_BASE,
                NET80211_BASE,
            ),
            Some(StaticFunctionTable::Net80211)
        );

        for (source, size, caller) in [
            (
                AllocationSource::OsiWifiCalloc,
                WDEV_FUNCTION_TABLE_SIZE,
                WDEV_BASE + WDEV_FUNCTION_TABLE_RETURN_OFFSET,
            ),
            (
                AllocationSource::OsiCallocInternal,
                WDEV_FUNCTION_TABLE_SIZE - 1,
                WDEV_BASE + WDEV_FUNCTION_TABLE_RETURN_OFFSET,
            ),
            (
                AllocationSource::OsiCallocInternal,
                WDEV_FUNCTION_TABLE_SIZE,
                WDEV_BASE + WDEV_FUNCTION_TABLE_RETURN_OFFSET + 2,
            ),
            (
                AllocationSource::OsiCallocInternal,
                NET80211_FUNCTION_TABLE_SIZE,
                WDEV_BASE + WDEV_FUNCTION_TABLE_RETURN_OFFSET,
            ),
        ] {
            assert_eq!(
                classify_static_function_table(
                    source,
                    size,
                    caller,
                    WDEV_BASE,
                    NET80211_BASE,
                ),
                None
            );
        }
    }

    #[cfg(feature = "rust-static-interface-storage")]
    #[test]
    fn static_interface_admission_is_exact() {
        use super::{
            classify_static_interface_allocation, AllocationSource, StaticInterfaceAllocation,
            WIFI_CREATE_PHY_RETURN_OFFSET, WIFI_CREATE_SOFTAP_STATE_RETURN_OFFSET,
            WIFI_CREATE_STA_STATE_RETURN_OFFSET, WIFI_INTERFACE_PHY_SIZE,
            WIFI_INTERFACE_STATE_SIZE,
        };

        const STA_BASE: usize = 0x4004_ce2a;
        const SOFTAP_BASE: usize = 0x4004_cd04;
        for (caller, expected) in [
            (
                STA_BASE + WIFI_CREATE_STA_STATE_RETURN_OFFSET,
                StaticInterfaceAllocation::State,
            ),
            (
                SOFTAP_BASE + WIFI_CREATE_SOFTAP_STATE_RETURN_OFFSET,
                StaticInterfaceAllocation::State,
            ),
            (
                STA_BASE + WIFI_CREATE_PHY_RETURN_OFFSET,
                StaticInterfaceAllocation::Phy,
            ),
            (
                SOFTAP_BASE + WIFI_CREATE_PHY_RETURN_OFFSET,
                StaticInterfaceAllocation::Phy,
            ),
        ] {
            let size = match expected {
                StaticInterfaceAllocation::State => WIFI_INTERFACE_STATE_SIZE,
                StaticInterfaceAllocation::Phy => WIFI_INTERFACE_PHY_SIZE,
            };
            assert_eq!(
                classify_static_interface_allocation(
                    AllocationSource::OsiWifiZalloc,
                    size,
                    caller,
                    STA_BASE,
                    SOFTAP_BASE,
                ),
                Some(expected)
            );
        }

        for (source, size, caller) in [
            (
                AllocationSource::OsiWifiMalloc,
                WIFI_INTERFACE_STATE_SIZE,
                STA_BASE + WIFI_CREATE_STA_STATE_RETURN_OFFSET,
            ),
            (
                AllocationSource::OsiWifiZalloc,
                WIFI_INTERFACE_STATE_SIZE - 1,
                STA_BASE + WIFI_CREATE_STA_STATE_RETURN_OFFSET,
            ),
            (
                AllocationSource::OsiWifiZalloc,
                WIFI_INTERFACE_STATE_SIZE,
                STA_BASE + WIFI_CREATE_STA_STATE_RETURN_OFFSET + 2,
            ),
            (
                AllocationSource::OsiWifiZalloc,
                WIFI_INTERFACE_PHY_SIZE,
                SOFTAP_BASE + WIFI_CREATE_SOFTAP_STATE_RETURN_OFFSET,
            ),
        ] {
            assert_eq!(
                classify_static_interface_allocation(
                    source,
                    size,
                    caller,
                    STA_BASE,
                    SOFTAP_BASE,
                ),
                None
            );
        }
    }

    #[cfg(feature = "rust-static-supplicant-callback-storage")]
    #[test]
    fn static_supplicant_callback_admission_is_exact() {
        use super::{
            is_static_supplicant_callback_allocation, AllocationSource,
            SUPPLICANT_CALLBACK_TABLE_RETURN_OFFSET, SUPPLICANT_CALLBACK_TABLE_SIZE,
        };

        const BASE: usize = 0x4008_2000;
        assert!(is_static_supplicant_callback_allocation(
            AllocationSource::DirectCalloc,
            SUPPLICANT_CALLBACK_TABLE_SIZE,
            BASE + SUPPLICANT_CALLBACK_TABLE_RETURN_OFFSET,
            BASE,
        ));
        for (source, size, caller) in [
            (
                AllocationSource::OsiCallocInternal,
                SUPPLICANT_CALLBACK_TABLE_SIZE,
                BASE + SUPPLICANT_CALLBACK_TABLE_RETURN_OFFSET,
            ),
            (
                AllocationSource::DirectCalloc,
                SUPPLICANT_CALLBACK_TABLE_SIZE - 4,
                BASE + SUPPLICANT_CALLBACK_TABLE_RETURN_OFFSET,
            ),
            (
                AllocationSource::DirectCalloc,
                SUPPLICANT_CALLBACK_TABLE_SIZE,
                BASE + SUPPLICANT_CALLBACK_TABLE_RETURN_OFFSET + 2,
            ),
        ] {
            assert!(!is_static_supplicant_callback_allocation(
                source, size, caller, BASE,
            ));
        }
    }

    #[cfg(feature = "rust-static-pp-bar-storage")]
    #[test]
    fn static_pp_bar_admission_is_exact() {
        use super::{
            is_static_pp_bar_allocation, AllocationSource, PP_BAR_RETURN_OFFSET, PP_BAR_SIZE,
        };

        const BASE: usize = 0x4008_3000;
        assert!(is_static_pp_bar_allocation(
            AllocationSource::OsiMallocInternal,
            PP_BAR_SIZE,
            BASE + PP_BAR_RETURN_OFFSET,
            BASE,
        ));
        for (source, size, caller) in [
            (
                AllocationSource::OsiWifiMalloc,
                PP_BAR_SIZE,
                BASE + PP_BAR_RETURN_OFFSET,
            ),
            (
                AllocationSource::OsiMallocInternal,
                PP_BAR_SIZE - 4,
                BASE + PP_BAR_RETURN_OFFSET,
            ),
            (
                AllocationSource::OsiMallocInternal,
                PP_BAR_SIZE,
                BASE + PP_BAR_RETURN_OFFSET + 2,
            ),
        ] {
            assert!(!is_static_pp_bar_allocation(
                source, size, caller, BASE,
            ));
        }
    }

    #[cfg(feature = "rust-static-cold-api-envelope-storage")]
    #[test]
    fn static_cold_api_envelope_admission_is_exact() {
        use super::{
            classify_static_cold_api_envelope, AllocationSource, ColdApiEnvelopeKind,
            COLD_API_ENVELOPE_SIZE, WIFI_INIT_ENVELOPE_RETURN_OFFSET,
            WIFI_START_ENVELOPE_RETURN_OFFSET,
        };

        const INIT: usize = 0x4007_1000;
        const START: usize = 0x4007_2000;
        assert_eq!(
            classify_static_cold_api_envelope(
                AllocationSource::OsiWifiZalloc,
                COLD_API_ENVELOPE_SIZE,
                INIT + WIFI_INIT_ENVELOPE_RETURN_OFFSET,
                INIT,
                START,
            ),
            Some(ColdApiEnvelopeKind::Init)
        );
        assert_eq!(
            classify_static_cold_api_envelope(
                AllocationSource::OsiWifiZalloc,
                COLD_API_ENVELOPE_SIZE,
                START + WIFI_START_ENVELOPE_RETURN_OFFSET,
                INIT,
                START,
            ),
            Some(ColdApiEnvelopeKind::Start)
        );
        for (source, size, caller) in [
            (
                AllocationSource::OsiWifiMalloc,
                COLD_API_ENVELOPE_SIZE,
                INIT + WIFI_INIT_ENVELOPE_RETURN_OFFSET,
            ),
            (
                AllocationSource::OsiWifiZalloc,
                COLD_API_ENVELOPE_SIZE - 4,
                INIT + WIFI_INIT_ENVELOPE_RETURN_OFFSET,
            ),
            (
                AllocationSource::OsiWifiZalloc,
                COLD_API_ENVELOPE_SIZE,
                START + WIFI_START_ENVELOPE_RETURN_OFFSET + 2,
            ),
        ] {
            assert_eq!(
                classify_static_cold_api_envelope(source, size, caller, INIT, START),
                None
            );
        }
    }
}
