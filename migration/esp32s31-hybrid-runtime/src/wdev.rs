use core::{
    cell::UnsafeCell,
    ffi::c_void,
    ptr,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
};

static FTM_ATTEMPTED: AtomicBool = AtomicBool::new(false);
#[cfg(target_arch = "riscv32")]
#[link_section = ".critical.bss.wifi_strict.rx_action_policy"]
static STRICT_ACTION_SIDE_PATHS_DISABLED: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WdevActionRxAdoptionError {
    NanInterfaceEnabled,
    FtmRxEnabled,
}

#[cfg(target_arch = "riscv32")]
use crate::{
    rx_descriptor::{
        decode_rx_metadata_layout, descriptor_buffer_length, descriptor_received_length,
        multi_rx_copy_plan, recycled_descriptor_word, rx_csi_length, rx_indicate_aggregate_flag,
        rx_sta_action_copy_mode, rx_sta_data_copy_mode, rx_sta_management_copy_mode,
        rx_sta_probe_request_is_discarded, rx_vendor_fallback_reason, single_rx_copy_plan,
        RxVendorFallbackFacts, RxVendorFallbackReason, SingleRxCopyPlan, RX_METADATA_PREFIX_BYTES,
    },
    timer::RawOsiTimer,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WdevRxContinuationError {
    WrongHart,
    ResetStateUnavailable,
    MissingLastDescriptor,
    MissingRxMetadata,
    DescriptorCountOverflow,
    DescriptorChainTooLong,
    ContinuationQueueFull,
}

#[cfg(target_arch = "riscv32")]
const MAX_RX_RECYCLE_DESCRIPTORS_PER_CHAIN: usize = 64;
#[cfg(target_arch = "riscv32")]
const RX_RELOAD_SETTLE_US: u32 = 5;
#[cfg(target_arch = "riscv32")]
const RX_DESCRIPTOR_NEXT_OFFSET: usize = 8;
#[cfg(target_arch = "riscv32")]
const RX_DESCRIPTOR_BUFFER_OFFSET: usize = 4;
#[cfg(target_arch = "riscv32")]
const RX_DESCRIPTOR_SENTINEL: u32 = 0xdead_beef;
#[cfg(target_arch = "riscv32")]
const WIFI_MAC_RX_CONTROL_REGISTER: *const u32 = 0x2010_4080 as *const u32;
#[cfg(target_arch = "riscv32")]
const WIFI_MAC_RX_BASE_REGISTER: *const u32 = 0x2010_4084 as *const u32;
#[cfg(target_arch = "riscv32")]
const WIFI_MAC_RX_METADATA_CONTROL_REGISTER: *const u32 = 0x2010_4098 as *const u32;
#[cfg(target_arch = "riscv32")]
const WIFI_MAC_RX_EXTENDED_METADATA_BIT: u32 = 1 << 23;

#[cfg(target_arch = "riscv32")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RxRecycleError {
    WrongHart,
    MissingHead,
    MissingTail,
    MissingBuffer,
    TailMismatch,
    ChainTooLong,
    TimerUnavailable,
    TimerCancelFailed,
    ReloadStillActive,
    MissingHardwareTail,
    UnexpectedCallback,
}

#[cfg(target_arch = "riscv32")]
struct RxRecycleState {
    reload_active: bool,
    terminal_restart_active: bool,
    failed: bool,
    reload_tail: *mut u8,
    pending_head: *mut u8,
    pending_tail: *mut u8,
    drained_head: *mut u8,
    drained_last: *mut u8,
}

#[cfg(target_arch = "riscv32")]
impl RxRecycleState {
    const fn new() -> Self {
        Self {
            reload_active: false,
            terminal_restart_active: false,
            failed: false,
            reload_tail: ptr::null_mut(),
            pending_head: ptr::null_mut(),
            pending_tail: ptr::null_mut(),
            drained_head: ptr::null_mut(),
            drained_last: ptr::null_mut(),
        }
    }
}

#[cfg(target_arch = "riscv32")]
struct RxRecycleStateCell(UnsafeCell<RxRecycleState>);

#[cfg(target_arch = "riscv32")]
unsafe impl Sync for RxRecycleStateCell {}

#[cfg(target_arch = "riscv32")]
struct RxRecycleTimerCell(UnsafeCell<RawOsiTimer>);

#[cfg(target_arch = "riscv32")]
unsafe impl Sync for RxRecycleTimerCell {}

#[cfg(target_arch = "riscv32")]
struct RxRecycleProbe {
    calls: AtomicUsize,
    immediate: AtomicUsize,
    deferred: AtomicUsize,
    timers_armed: AtomicUsize,
    completions: AtomicUsize,
    terminal_restarts: AtomicUsize,
    reload_active: AtomicUsize,
    pending_chains: AtomicUsize,
}

#[cfg(target_arch = "riscv32")]
impl RxRecycleProbe {
    const fn new() -> Self {
        Self {
            calls: AtomicUsize::new(0),
            immediate: AtomicUsize::new(0),
            deferred: AtomicUsize::new(0),
            timers_armed: AtomicUsize::new(0),
            completions: AtomicUsize::new(0),
            terminal_restarts: AtomicUsize::new(0),
            reload_active: AtomicUsize::new(0),
            pending_chains: AtomicUsize::new(0),
        }
    }
}

#[cfg(target_arch = "riscv32")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WdevRxRecycleSnapshot {
    pub calls: usize,
    pub immediate: usize,
    pub deferred: usize,
    pub timers_armed: usize,
    pub completions: usize,
    pub terminal_restarts: usize,
    pub reload_active: bool,
    pub pending_chains: usize,
    pub software_head: usize,
    pub software_head_word: u32,
    pub software_head_next: usize,
    pub software_tail: usize,
    pub software_tail_word: u32,
    pub software_tail_next: usize,
    pub hardware_control: u32,
    pub hardware_base: usize,
    pub hardware_next: usize,
    pub hardware_last_raw: usize,
    pub hardware_last: usize,
    pub hardware_end_state: u32,
}

#[cfg(target_arch = "riscv32")]
struct IndicateFrameProbe {
    calls: AtomicUsize,
    validated: AtomicUsize,
    max_descriptors: AtomicUsize,
}

#[cfg(target_arch = "riscv32")]
struct RxMetadataProbe {
    calls: AtomicUsize,
    decoded: AtomicUsize,
    rejected_layout: AtomicUsize,
    status_success: AtomicUsize,
    status_f5: AtomicUsize,
    status_c6: AtomicUsize,
    status_other: AtomicUsize,
    base_only: AtomicUsize,
    sublength_only: AtomicUsize,
    extra_only: AtomicUsize,
    sublength_and_extra: AtomicUsize,
    max_payload_offset: AtomicUsize,
    route_sta: AtomicUsize,
    route_ap: AtomicUsize,
    route_nan: AtomicUsize,
    route_other: AtomicUsize,
    frame_class_bitmap: AtomicUsize,
    management_subtype_bitmap: AtomicUsize,
    aggregate_flag_bitmap: AtomicUsize,
    rust_data_routes: AtomicUsize,
    rust_management_routes: AtomicUsize,
    rust_action_routes: AtomicUsize,
    rust_probe_request_discards: AtomicUsize,
    rust_indicate_routes: AtomicUsize,
    rust_multi_indicate_routes: AtomicUsize,
    rust_multi_copy_mode_discards: AtomicUsize,
    rust_indicate_allocation_rejects: AtomicUsize,
    rust_indicate_population_rejects: AtomicUsize,
    vendor_indicate_fallbacks: AtomicUsize,
    vendor_fallbacks: AtomicUsize,
    vendor_fallback_reasons: [AtomicUsize; RxVendorFallbackReason::COUNT],
}

#[cfg(target_arch = "riscv32")]
impl RxMetadataProbe {
    const fn new() -> Self {
        Self {
            calls: AtomicUsize::new(0),
            decoded: AtomicUsize::new(0),
            rejected_layout: AtomicUsize::new(0),
            status_success: AtomicUsize::new(0),
            status_f5: AtomicUsize::new(0),
            status_c6: AtomicUsize::new(0),
            status_other: AtomicUsize::new(0),
            base_only: AtomicUsize::new(0),
            sublength_only: AtomicUsize::new(0),
            extra_only: AtomicUsize::new(0),
            sublength_and_extra: AtomicUsize::new(0),
            max_payload_offset: AtomicUsize::new(0),
            route_sta: AtomicUsize::new(0),
            route_ap: AtomicUsize::new(0),
            route_nan: AtomicUsize::new(0),
            route_other: AtomicUsize::new(0),
            frame_class_bitmap: AtomicUsize::new(0),
            management_subtype_bitmap: AtomicUsize::new(0),
            aggregate_flag_bitmap: AtomicUsize::new(0),
            rust_data_routes: AtomicUsize::new(0),
            rust_management_routes: AtomicUsize::new(0),
            rust_action_routes: AtomicUsize::new(0),
            rust_probe_request_discards: AtomicUsize::new(0),
            rust_indicate_routes: AtomicUsize::new(0),
            rust_multi_indicate_routes: AtomicUsize::new(0),
            rust_multi_copy_mode_discards: AtomicUsize::new(0),
            rust_indicate_allocation_rejects: AtomicUsize::new(0),
            rust_indicate_population_rejects: AtomicUsize::new(0),
            vendor_indicate_fallbacks: AtomicUsize::new(0),
            vendor_fallbacks: AtomicUsize::new(0),
            vendor_fallback_reasons: [const { AtomicUsize::new(0) }; RxVendorFallbackReason::COUNT],
        }
    }

    #[inline(always)]
    fn record_vendor_fallback(&self, reason: RxVendorFallbackReason) {
        self.vendor_fallbacks.fetch_add(1, Ordering::Relaxed);
        self.vendor_fallback_reasons[reason.index()].fetch_add(1, Ordering::Relaxed);
    }
}

#[cfg(target_arch = "riscv32")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WdevRxVendorFallbackSnapshot {
    pub missing_head: usize,
    pub invalid_descriptor: usize,
    pub invalid_metadata_layout: usize,
    pub invalid_status_offset: usize,
    pub non_success_status: usize,
    pub invalid_chain: usize,
    pub extended_metadata: usize,
    pub csi_metadata: usize,
    pub copy_plan_rejected: usize,
    pub ap_route: usize,
    pub nan_route: usize,
    pub other_route: usize,
    pub optional_control_30: usize,
    pub optional_control_46: usize,
    pub non_ordinary_profile: usize,
    pub missing_station_interface: usize,
    pub unclassified_frame: usize,
}

#[cfg(target_arch = "riscv32")]
impl WdevRxVendorFallbackSnapshot {
    pub const fn total(self) -> usize {
        self.missing_head
            + self.invalid_descriptor
            + self.invalid_metadata_layout
            + self.invalid_status_offset
            + self.non_success_status
            + self.invalid_chain
            + self.extended_metadata
            + self.csi_metadata
            + self.copy_plan_rejected
            + self.ap_route
            + self.nan_route
            + self.other_route
            + self.optional_control_30
            + self.optional_control_46
            + self.non_ordinary_profile
            + self.missing_station_interface
            + self.unclassified_frame
    }
}

#[cfg(target_arch = "riscv32")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WdevRxMetadataSnapshot {
    pub calls: usize,
    pub decoded: usize,
    pub rejected_layout: usize,
    pub status_success: usize,
    pub status_f5: usize,
    pub status_c6: usize,
    pub status_other: usize,
    pub base_only: usize,
    pub sublength_only: usize,
    pub extra_only: usize,
    pub sublength_and_extra: usize,
    pub max_payload_offset: usize,
    pub route_sta: usize,
    pub route_ap: usize,
    pub route_nan: usize,
    pub route_other: usize,
    pub frame_class_bitmap: usize,
    pub management_subtype_bitmap: usize,
    pub aggregate_flag_bitmap: usize,
    pub rust_data_routes: usize,
    pub rust_management_routes: usize,
    pub rust_action_routes: usize,
    pub rust_probe_request_discards: usize,
    pub rust_indicate_routes: usize,
    pub rust_multi_indicate_routes: usize,
    pub rust_multi_copy_mode_discards: usize,
    pub rust_indicate_allocation_rejects: usize,
    pub rust_indicate_population_rejects: usize,
    pub vendor_indicate_fallbacks: usize,
    pub vendor_fallbacks: usize,
    pub vendor_fallback_reasons: WdevRxVendorFallbackSnapshot,
}

#[cfg(target_arch = "riscv32")]
impl IndicateFrameProbe {
    const fn new() -> Self {
        Self {
            calls: AtomicUsize::new(0),
            validated: AtomicUsize::new(0),
            max_descriptors: AtomicUsize::new(0),
        }
    }
}

#[cfg(target_arch = "riscv32")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WdevIndicateFrameSnapshot {
    pub calls: usize,
    pub validated: usize,
    pub max_descriptors: usize,
}

#[cfg(target_arch = "riscv32")]
#[link_section = ".critical.bss.wifi_strict.rx_recycle_state"]
static RX_RECYCLE_STATE: RxRecycleStateCell =
    RxRecycleStateCell(UnsafeCell::new(RxRecycleState::new()));

#[cfg(target_arch = "riscv32")]
#[link_section = ".critical.bss.wifi_strict.rx_recycle_timer"]
static RX_RECYCLE_TIMER: RxRecycleTimerCell = RxRecycleTimerCell(UnsafeCell::new(RawOsiTimer {
    next: ptr::null_mut(),
    expire: 0,
    period: 0,
    callback: None,
    argument: ptr::null_mut(),
}));

#[cfg(target_arch = "riscv32")]
#[link_section = ".critical.bss.wifi_strict.rx_recycle_probe"]
static RX_RECYCLE_PROBE: RxRecycleProbe = RxRecycleProbe::new();

#[cfg(target_arch = "riscv32")]
#[link_section = ".critical.bss.wifi_strict.indicate_frame_probe"]
static INDICATE_FRAME_PROBE: IndicateFrameProbe = IndicateFrameProbe::new();

#[cfg(target_arch = "riscv32")]
#[link_section = ".critical.bss.wifi_strict.rx_metadata_probe"]
static RX_METADATA_PROBE: RxMetadataProbe = RxMetadataProbe::new();

unsafe extern "C" {
    #[link_name = "wDev_record_ftm_data"]
    fn vendor_record_ftm_data(rx_control: *mut c_void, frame: *mut c_void);
    #[link_name = "pm_on_beacon_rx"]
    fn vendor_pm_on_beacon_rx(
        interface: *mut c_void,
        frame: *mut u8,
        frame_end: *mut u8,
        from_task: u32,
    );
    #[link_name = "pm_on_data_rx"]
    fn vendor_pm_on_data_rx(
        receiver: *mut u8,
        packet_class: u32,
        transmitter: *mut u8,
        interface: u32,
    );
    #[link_name = "pm_on_data_tx"]
    fn vendor_pm_on_data_tx();
    #[link_name = "pm_on_coex_schm_status_config"]
    fn vendor_pm_on_coex_schm_status_config(status: u32);
    #[link_name = "pm_set_beacon_duration"]
    fn vendor_pm_set_beacon_duration(duration: u32);
    #[link_name = "wDev_ftm_set_t1t4"]
    fn vendor_ftm_set_t1t4(frame: *mut c_void);
    #[link_name = "wDev_isNANPktInValidSlot"]
    fn vendor_is_nan_packet_in_valid_slot(frame: *mut u8) -> i32;
    #[link_name = "wDev_SnifferRxData"]
    fn vendor_sniffer_rx_data();
    #[link_name = "wdev_csi_rx_process"]
    fn vendor_csi_rx_process();
    #[link_name = "wDev_IndicateCtrlFrame"]
    fn vendor_indicate_ctrl_frame(frame: *mut u8, count: u32, kind: u32) -> i32;
    fn __real_wDev_isNANPktInValidSlot(frame: *mut u8) -> i32;
    fn __real_wDev_IndicateCtrlFrame(frame: *mut u8, count: u32, kind: u32) -> i32;
}

#[cfg(target_arch = "riscv32")]
const MAX_RX_SUCCESS_DESCRIPTORS_PER_EVENT: usize = 64;

#[cfg(target_arch = "riscv32")]
unsafe extern "C" {
    static mut wDevCtrl: u8;
    static g_wifi_menuconfig: u8;
    static mut g_wdev_last_desc_reset_ptr: *mut u8;
    static mut g_wdev_csi_rx: usize;
    #[link_name = "wDev_AppendRxBlocks"]
    fn vendor_append_rx_blocks(head: *mut u8, tail: *mut u8, count: u32);
    fn __real_wDev_AppendRxBlocks(head: *mut u8, tail: *mut u8, count: u32);
    #[link_name = "wDev_DiscardFrame"]
    fn vendor_discard_frame(tail: *mut u8, count: u32);
    fn __real_wDev_DiscardFrame(tail: *mut u8, count: u32);
    fn hal_mac_rx_get_last_dscr() -> *mut u8;
    fn hal_mac_rx_get_end_state() -> u32;
    fn hal_mac_rx_is_dscr_reload() -> u32;
    fn hal_mac_rx_read_rxdscrlast() -> *mut u8;
    fn hal_mac_rx_read_rxdscrnext() -> *mut u8;
    fn hal_mac_rx_disable();
    fn hal_mac_rx_enable();
    fn hal_mac_rx_set_base(descriptor: *mut u8);
    fn hal_mac_rx_set_dscr_reload();
    fn pp_post(kind: u32, argument: *mut c_void) -> i32;
    #[link_name = "wDev_ProcessRxSucData"]
    fn vendor_process_rx_success_data(descriptor: *mut u8, subframe_count: u32);
    fn __real_wDev_ProcessRxSucData(descriptor: *mut u8, subframe_count: u32);
    fn wDev_IndicateFrame(
        copy_mode: u32,
        aggregate_flag: u32,
        tail: *mut u8,
        count: u32,
        timestamp: u32,
    );
}

#[cfg(target_arch = "riscv32")]
#[link_section = ".rwtext.wifi_strict.rx_recycle"]
unsafe fn prepare_rx_recycle_chain(
    head: *mut u8,
    expected_tail: *mut u8,
) -> Result<(), RxRecycleError> {
    if head.is_null() {
        return Err(RxRecycleError::MissingHead);
    }
    if expected_tail.is_null() {
        return Err(RxRecycleError::MissingTail);
    }

    let mut descriptor = head;
    let mut last = ptr::null_mut();
    let mut seen = 0;
    while !descriptor.is_null() {
        if seen == MAX_RX_RECYCLE_DESCRIPTORS_PER_CHAIN {
            return Err(RxRecycleError::ChainTooLong);
        }
        seen += 1;

        let word_ptr = descriptor.cast::<u32>();
        let word = word_ptr.read_unaligned();
        let buffer = descriptor
            .add(RX_DESCRIPTOR_BUFFER_OFFSET)
            .cast::<*mut u8>()
            .read_unaligned();
        if buffer.is_null() {
            return Err(RxRecycleError::MissingBuffer);
        }
        let next = descriptor
            .add(RX_DESCRIPTOR_NEXT_OFFSET)
            .cast::<*mut u8>()
            .read_unaligned();

        word_ptr.write_unaligned(recycled_descriptor_word(word));
        buffer.cast::<u32>().write_unaligned(RX_DESCRIPTOR_SENTINEL);
        buffer
            .add(descriptor_buffer_length(word))
            .cast::<u32>()
            .write_unaligned(RX_DESCRIPTOR_SENTINEL);

        last = descriptor;
        descriptor = next;
    }
    if last != expected_tail {
        return Err(RxRecycleError::TailMismatch);
    }
    Ok(())
}

#[cfg(target_arch = "riscv32")]
#[link_section = ".rwtext.wifi_strict.rx_recycle"]
unsafe fn append_pending_rx_recycle_chain(
    state: &mut RxRecycleState,
    head: *mut u8,
    tail: *mut u8,
) -> Result<(), RxRecycleError> {
    if state.pending_head.is_null() {
        if !state.pending_tail.is_null() {
            return Err(RxRecycleError::MissingHead);
        }
        state.pending_head = head;
        state.pending_tail = tail;
        return Ok(());
    }
    if state.pending_tail.is_null() {
        return Err(RxRecycleError::MissingTail);
    }
    state
        .pending_tail
        .add(RX_DESCRIPTOR_NEXT_OFFSET)
        .cast::<*mut u8>()
        .write_unaligned(head);
    state.pending_tail = tail;
    Ok(())
}

#[cfg(target_arch = "riscv32")]
#[link_section = ".rwtext.wifi_strict.rx_recycle"]
unsafe fn arm_rx_reload_settle_timer() -> Result<(), RxRecycleError> {
    if crate::adapter::schedule_internal_timer(
        RX_RECYCLE_TIMER.0.get().cast(),
        rx_reload_settled,
        ptr::null_mut(),
        RX_RELOAD_SETTLE_US,
    ) {
        RX_RECYCLE_PROBE
            .timers_armed
            .fetch_add(1, Ordering::Relaxed);
        Ok(())
    } else {
        Err(RxRecycleError::TimerUnavailable)
    }
}

/// Attach one prepared descriptor chain without waiting for the MAC reload bit.
///
/// Returns `true` when a later timer continuation is required. The caller owns
/// `state` and execution is serialized on the strict Wi-Fi hart.
#[cfg(target_arch = "riscv32")]
#[link_section = ".rwtext.wifi_strict.rx_recycle"]
unsafe fn publish_rx_recycle_chain(
    state: &mut RxRecycleState,
    head: *mut u8,
    tail: *mut u8,
) -> Result<bool, RxRecycleError> {
    let interrupt_state = crate::critical::strict_wifi_int_disable();
    let control = ptr::addr_of_mut!(wDevCtrl);
    let published_head = control.cast::<*mut u8>().read_unaligned();
    if published_head.is_null() {
        control.cast::<*mut u8>().write_unaligned(head);
        control.add(4).cast::<*mut u8>().write_unaligned(tail);
        // A runtime-empty list is stronger than the vendor cold-init case:
        // the RX descriptor walker has already reached its terminal state.
        // Hardware telemetry proves that neither a base write nor a reload
        // edge makes that state fetch a valid hardware-owned chain.  The
        // vendor MAC sleep path establishes the bounded state transition:
        // clear the RX-enable gate, publish the new base, then reopen it after
        // an asynchronous settle edge. All MMIO operations are finite leaves
        // and execute only while the software list is empty under the local
        // Wi-Fi interrupt lock.
        begin_terminal_rx_restart(state, head);
        crate::critical::strict_wifi_int_restore(interrupt_state);
        return Ok(true);
    }

    let published_tail = control.add(4).cast::<*mut u8>().read_unaligned();
    if published_tail.is_null() {
        crate::critical::strict_wifi_int_restore(interrupt_state);
        return Err(RxRecycleError::MissingTail);
    }
    published_tail
        .add(RX_DESCRIPTOR_NEXT_OFFSET)
        .cast::<*mut u8>()
        .write_unaligned(head);
    // The pinned vendor leaf keeps the previously published tail visible
    // until MAC reload completes. Publishing the future tail here lets RX
    // interrupt code observe a chain endpoint which hardware has not accepted
    // yet and eventually corrupts the descriptor list under sustained load.
    state.reload_active = true;
    state.reload_tail = tail;
    RX_RECYCLE_PROBE.reload_active.store(1, Ordering::Release);
    hal_mac_rx_set_dscr_reload();
    crate::critical::strict_wifi_int_restore(interrupt_state);
    Ok(true)
}

#[cfg(target_arch = "riscv32")]
#[link_section = ".rwtext.wifi_strict.rx_recycle"]
unsafe fn publish_or_defer_rx_recycle_chain(
    state: &mut RxRecycleState,
    head: *mut u8,
    tail: *mut u8,
) -> Result<(), RxRecycleError> {
    if state.reload_active {
        RX_RECYCLE_PROBE.deferred.fetch_add(1, Ordering::Relaxed);
        let interrupt_state = crate::critical::strict_wifi_int_disable();
        let result = append_pending_rx_recycle_chain(state, head, tail);
        crate::critical::strict_wifi_int_restore(interrupt_state);
        if result.is_ok() {
            RX_RECYCLE_PROBE
                .pending_chains
                .fetch_add(1, Ordering::Relaxed);
        }
        return result;
    }
    if publish_rx_recycle_chain(state, head, tail)? {
        arm_rx_reload_settle_timer()?;
    } else {
        RX_RECYCLE_PROBE.immediate.fetch_add(1, Ordering::Relaxed);
    }
    Ok(())
}

#[cfg(target_arch = "riscv32")]
#[link_section = ".rwtext.wifi_strict.rx_recycle"]
unsafe extern "C" fn rx_reload_settled(_argument: *mut c_void) {
    let state = &mut *RX_RECYCLE_STATE.0.get();
    if state.failed || !state.reload_active {
        fail_rx_recycle(state, RxRecycleError::UnexpectedCallback);
    }
    if !crate::critical::on_strict_wifi_hart() {
        fail_rx_recycle(state, RxRecycleError::WrongHart);
    }
    if state.terminal_restart_active {
        finish_terminal_rx_restart(state);
        return;
    }
    // Exactly one status observation per async continuation. A MAC which has
    // not completed within the declared settle interval is a hard invariant
    // failure; it is never converted back into polling or a retry timer.
    if hal_mac_rx_is_dscr_reload() != 0 {
        fail_rx_recycle(state, RxRecycleError::ReloadStillActive);
    }
    complete_rx_reload(state);
}

#[cfg(target_arch = "riscv32")]
#[link_section = ".rwtext.wifi_strict.rx_recycle"]
unsafe fn complete_rx_reload(state: &mut RxRecycleState) {
    RX_RECYCLE_PROBE.completions.fetch_add(1, Ordering::Relaxed);

    let reload_tail = state.reload_tail;
    if hal_mac_rx_read_rxdscrnext().is_null() {
        let hardware_tail = hal_mac_rx_get_last_dscr();
        if hardware_tail != reload_tail {
            if hardware_tail.is_null() {
                fail_rx_recycle(state, RxRecycleError::MissingHardwareTail);
            }
            let next = hardware_tail
                .add(RX_DESCRIPTOR_NEXT_OFFSET)
                .cast::<*mut u8>()
                .read_unaligned();
            if !next.is_null() {
                hal_mac_rx_set_base(next);
            }
        }
    }
    // Match the terminal store in `wDev_AppendRxBlocks`: the new tail becomes
    // globally visible only after the reload bit cleared and any base repair
    // completed.
    let interrupt_state = crate::critical::strict_wifi_int_disable();
    ptr::addr_of_mut!(wDevCtrl)
        .add(4)
        .cast::<*mut u8>()
        .write_unaligned(reload_tail);
    crate::critical::strict_wifi_int_restore(interrupt_state);
    state.reload_active = false;
    RX_RECYCLE_PROBE.reload_active.store(0, Ordering::Release);
    state.reload_tail = ptr::null_mut();
    let pending_head = state.pending_head;
    let pending_tail = state.pending_tail;
    state.pending_head = ptr::null_mut();
    state.pending_tail = ptr::null_mut();
    RX_RECYCLE_PROBE.pending_chains.store(0, Ordering::Release);
    // A reload which completed while the walker exhausted its old chain may
    // have left a proven terminal frontier. Start its separate async enable
    // edge only after the accepted software tail is visible and the ordinary
    // reload state no longer owns the shared settle timer.
    try_restart_drained_rx_chain(state, false);
    if !pending_head.is_null() {
        if let Err(error) = publish_or_defer_rx_recycle_chain(state, pending_head, pending_tail) {
            fail_rx_recycle(state, error);
        }
    }
}

/// Record the only state from which an exhausted RX engine may be restarted.
///
/// The caller has decoded `processed_last`, advanced the vendor software head,
/// and observed that the hardware last descriptor did not move during that
/// decode. A concurrently active descriptor reload still owns the MAC base;
/// the saved pair is therefore consumed only after reload completion.
#[cfg(target_arch = "riscv32")]
#[link_section = ".rwtext.wifi_strict.rx_success_dispatch"]
unsafe fn mark_drained_rx_chain(processed_last: *mut u8) {
    let state = &mut *RX_RECYCLE_STATE.0.get();
    let interrupt_state = crate::critical::strict_wifi_int_disable();
    let head = ptr::addr_of!(wDevCtrl).cast::<*mut u8>().read_unaligned();
    if !head.is_null()
        && head != processed_last
        && hal_mac_rx_read_rxdscrnext().is_null()
        && hal_mac_rx_get_last_dscr() == processed_last
    {
        state.drained_head = head;
        state.drained_last = processed_last;
    } else {
        state.drained_head = ptr::null_mut();
        state.drained_last = ptr::null_mut();
    }
    crate::critical::strict_wifi_int_restore(interrupt_state);
    try_restart_drained_rx_chain(state, false);
}

/// Restart a proven-drained chain only when no descriptor reload owns the MAC.
///
/// All evidence is revalidated in one bounded local critical section. A
/// changed software head, hardware tail, or hardware-next pointer invalidates
/// the saved proof instead of guessing from descriptor owner bits.
#[cfg(target_arch = "riscv32")]
#[link_section = ".rwtext.wifi_strict.rx_success_dispatch"]
unsafe fn try_restart_drained_rx_chain(state: &mut RxRecycleState, reload_settled: bool) {
    if (state.reload_active && !reload_settled) || state.drained_head.is_null() {
        return;
    }

    let interrupt_state = crate::critical::strict_wifi_int_disable();
    let saved_head = state.drained_head;
    let saved_last = state.drained_last;
    let current_head = ptr::addr_of!(wDevCtrl).cast::<*mut u8>().read_unaligned();
    let hardware_next = hal_mac_rx_read_rxdscrnext();
    let hardware_last = hal_mac_rx_get_last_dscr();
    if hardware_next.is_null()
        && hardware_last == saved_last
        && current_head == saved_head
        && saved_head != saved_last
    {
        begin_terminal_rx_restart(state, saved_head);
        RX_RECYCLE_PROBE
            .terminal_restarts
            .fetch_add(1, Ordering::Relaxed);
    }
    // A proof describes one exact publication state and is never reused.
    state.drained_head = ptr::null_mut();
    state.drained_last = ptr::null_mut();
    crate::critical::strict_wifi_int_restore(interrupt_state);
    if state.terminal_restart_active {
        if let Err(error) = arm_rx_reload_settle_timer() {
            fail_rx_recycle(state, error);
        }
    }
}

/// Start reopening the RX descriptor walker after a terminal-list transition.
///
/// The S31 MAC keeps bit 31 set even after its descriptor FSM reaches the end
/// of a chain. A base write and the ordinary append reload doorbell are both
/// acknowledged without leaving that terminal state. The vendor sleep/wakeup
/// and cold-init paths establish a falling/rising RX-enable edge with work
/// between the two writes. Strict mode models that work as one asynchronous
/// settle timer rather than a CPU delay. The caller holds the local Wi-Fi
/// interrupt lock and has proved that no hardware-current descriptor exists.
#[cfg(target_arch = "riscv32")]
#[link_section = ".rwtext.wifi_strict.rx_recycle"]
unsafe fn begin_terminal_rx_restart(state: &mut RxRecycleState, head: *mut u8) {
    hal_mac_rx_disable();
    hal_mac_rx_set_base(head);
    state.reload_active = true;
    state.terminal_restart_active = true;
    RX_RECYCLE_PROBE.reload_active.store(1, Ordering::Release);
}

#[cfg(target_arch = "riscv32")]
#[link_section = ".rwtext.wifi_strict.rx_recycle"]
unsafe fn finish_terminal_rx_restart(state: &mut RxRecycleState) {
    hal_mac_rx_enable();
    RX_RECYCLE_PROBE.completions.fetch_add(1, Ordering::Relaxed);
    state.terminal_restart_active = false;
    state.reload_active = false;
    RX_RECYCLE_PROBE.reload_active.store(0, Ordering::Release);

    let pending_head = state.pending_head;
    let pending_tail = state.pending_tail;
    state.pending_head = ptr::null_mut();
    state.pending_tail = ptr::null_mut();
    RX_RECYCLE_PROBE.pending_chains.store(0, Ordering::Release);
    if !pending_head.is_null() {
        if let Err(error) = publish_or_defer_rx_recycle_chain(state, pending_head, pending_tail) {
            fail_rx_recycle(state, error);
        }
    }
}

/// Finish a descriptor reload before decoding the RX event which proves that
/// the MAC has already advanced.
///
/// A hardware RX event can reach the executor before the conservative settle
/// timer.  Decoding that event while `wDevCtrl.tail` still names the previous
/// chain lets `wDev_DiscardFrame` recycle descriptors against stale software
/// list metadata.  Observe the reload bit exactly once here; when it is clear,
/// cancel the fallback timer and publish the accepted tail before entering the
/// vendor per-frame decoder.  A still-active reload remains owned by the
/// already armed timer and this function returns immediately.
#[cfg(target_arch = "riscv32")]
#[link_section = ".rwtext.wifi_strict.rx_recycle"]
pub(crate) unsafe fn settle_rx_reload_before_success() {
    let state = &mut *RX_RECYCLE_STATE.0.get();
    if state.failed || !state.reload_active || hal_mac_rx_is_dscr_reload() != 0 {
        return;
    }
    if !crate::adapter::cancel_internal_timer(RX_RECYCLE_TIMER.0.get().cast()) {
        fail_rx_recycle(state, RxRecycleError::TimerCancelFailed);
    }
    complete_rx_reload(state);
}

#[cfg(target_arch = "riscv32")]
#[cold]
#[inline(never)]
#[link_section = ".rwtext.wifi_strict.rx_recycle"]
unsafe fn fail_rx_recycle(state: &mut RxRecycleState, _error: RxRecycleError) -> ! {
    state.failed = true;
    core::arch::asm!("ebreak", options(noreturn))
}

/// Allocation-free replacement for the vendor RX descriptor recycle leaf.
///
/// The stock implementation spins up to 100,001 times on the MAC reload bit.
/// Strict mode instead formats a finite descriptor chain, publishes it under a
/// local interrupt critical section and returns. Completion is checked once by
/// an executor-driven Rust timer; additional chains coalesce in SRAM.
#[cfg(target_arch = "riscv32")]
#[no_mangle]
#[link_section = ".rwtext.wifi_strict.rx_recycle"]
pub unsafe extern "C" fn __wrap_wDev_AppendRxBlocks(head: *mut u8, tail: *mut u8, _count: u32) {
    if !crate::critical::strict_wifi_hart_armed() {
        __real_wDev_AppendRxBlocks(head, tail, _count);
        return;
    }
    let state = &mut *RX_RECYCLE_STATE.0.get();
    if state.failed || !crate::critical::on_strict_wifi_hart() {
        fail_rx_recycle(state, RxRecycleError::WrongHart);
    }
    RX_RECYCLE_PROBE.calls.fetch_add(1, Ordering::Relaxed);
    if let Err(error) = prepare_rx_recycle_chain(head, tail)
        .and_then(|()| publish_or_defer_rx_recycle_chain(state, head, tail))
    {
        fail_rx_recycle(state, error);
    }
}

/// A detached RX descriptor prefix with one remaining recycle authority.
///
/// Construction removes the prefix from `wDevCtrl.head`; consuming this token
/// is the only path into the fixed Rust descriptor recycler.
#[cfg(target_arch = "riscv32")]
struct DetachedRxPrefix {
    head: *mut u8,
    tail: *mut u8,
    count: u32,
}

/// One complete lower-MAC RX unit selected by the outer descriptor walk.
///
/// The pinned `wdevProcessRxSucDataAll+0x102` call passes the descriptor
/// carrying the bit-30 completion marker, not the first descriptor in the
/// unit. `wDev_ProcessRxSucData` obtains that first descriptor independently
/// from `wDevCtrl.head` and retains this tail identity for discard/recycle.
/// Keeping the pair in a non-`Copy` token prevents the Rust dispatcher from
/// accidentally publishing the same completed unit twice.
#[cfg(target_arch = "riscv32")]
struct CompletedRxUnit {
    tail: *mut u8,
    count: u32,
}

#[cfg(target_arch = "riscv32")]
impl CompletedRxUnit {
    #[link_section = ".rwtext.wifi_strict.rx_success_dispatch"]
    unsafe fn dispatch(self) {
        vendor_process_rx_success_data(self.tail, self.count);
    }
}

/// Own the finite single-descriptor body of the pinned
/// `wDev_IndicateFrame`.
///
/// The admitted layout has zero CSI/extended metadata. Its optional rounded
/// sublength is removed by the same two finite copies as the pinned ROM leaf.
/// Allocation is a claim from kind 7's Rust SRAM pool or kind 8's initialized
/// static free list; exhaustion discards this RX unit immediately.
///
/// Returning `false` leaves both input owners untouched and permits the
/// explicit ROM fallback. Returning `true` consumes the completed descriptor
/// prefix and either publishes or recycles the allocated ESF owner.
#[cfg(target_arch = "riscv32")]
#[link_section = ".rwtext.wifi_strict.rx_success_dispatch"]
unsafe fn indicate_single_received_frame(
    head: *mut u8,
    tail: *mut u8,
    count: u32,
    metadata: *mut u8,
    descriptor_length: usize,
    copy_mode: u32,
    copy_plan: SingleRxCopyPlan,
    aggregate: bool,
    timestamp: u32,
) -> bool {
    if head.is_null()
        || head != tail
        || count != 1
        || metadata.is_null()
        || copy_mode > 1
        || descriptor_length < 0x38
    {
        return false;
    }
    let Some(large_length) = crate::rx::strict_rx_descriptor_buffer_size() else {
        return false;
    };

    let Some((frame, allocated_length)) =
        crate::esf::allocate_strict_received_frame(copy_mode, descriptor_length, large_length)
    else {
        RX_METADATA_PROBE
            .rust_indicate_allocation_rejects
            .fetch_add(1, Ordering::Relaxed);
        vendor_discard_frame(tail, count);
        return true;
    };
    let control = ptr::addr_of!(wDevCtrl);
    if !crate::esf::populate_single_received_frame(
        frame,
        metadata,
        descriptor_length,
        allocated_length,
        copy_plan,
        timestamp,
        control.add(0x2c).read(),
        control.add(0x2d).read(),
        aggregate,
    ) {
        RX_METADATA_PROBE
            .rust_indicate_population_rejects
            .fetch_add(1, Ordering::Relaxed);
        crate::esf::recycle_received_packet(frame);
        vendor_discard_frame(tail, count);
        return true;
    }

    // Preserve the ROM ownership order: recycle the hardware descriptor
    // prefix before publishing the newly allocated ESF object.
    vendor_discard_frame(tail, count);
    RX_METADATA_PROBE
        .rust_indicate_routes
        .fetch_add(1, Ordering::Relaxed);
    crate::rx::wifi_strict_lmac_rx_done(frame);
    true
}

/// Own the base-layout multi-descriptor body of the pinned indication leaf.
///
/// Copy mode one is the vendor's explicit immediate-discard branch for a
/// split MPDU. Copy mode zero joins at most 64 hardware segments into one
/// fixed split SRAM-header/PSRAM-payload ESF object. Every rejection either
/// leaves ownership untouched for the explicit fallback or consumes both
/// owners through the bounded discard path.
#[cfg(target_arch = "riscv32")]
#[link_section = ".rwtext.wifi_strict.rx_success_dispatch"]
unsafe fn indicate_multi_received_frame(
    head: *mut u8,
    tail: *mut u8,
    count: u32,
    copy_mode: u32,
    aggregate: bool,
    timestamp: u32,
) -> bool {
    if head.is_null()
        || tail.is_null()
        || head == tail
        || !(2..=MAX_RX_SUCCESS_DESCRIPTORS_PER_EVENT as u32).contains(&count)
        || copy_mode > 1
    {
        return false;
    }
    if copy_mode != 0 {
        RX_METADATA_PROBE
            .rust_multi_copy_mode_discards
            .fetch_add(1, Ordering::Relaxed);
        vendor_discard_frame(tail, count);
        return true;
    }
    let head_word = head.cast::<u32>().read_unaligned();
    let tail_word = tail.cast::<u32>().read_unaligned();
    let Some(copy_plan) = multi_rx_copy_plan(
        count as usize,
        descriptor_buffer_length(head_word),
        descriptor_received_length(tail_word),
    ) else {
        return false;
    };
    let Some(frame) =
        crate::esf::allocate_strict_aggregate_received_frame(copy_plan.indicated_length)
    else {
        RX_METADATA_PROBE
            .rust_indicate_allocation_rejects
            .fetch_add(1, Ordering::Relaxed);
        vendor_discard_frame(tail, count);
        return true;
    };
    let control = ptr::addr_of!(wDevCtrl);
    if !crate::esf::populate_multi_received_frame(
        frame,
        head,
        tail,
        copy_plan,
        timestamp,
        control.add(0x2c).read(),
        control.add(0x2d).read(),
        aggregate,
    ) {
        RX_METADATA_PROBE
            .rust_indicate_population_rejects
            .fetch_add(1, Ordering::Relaxed);
        crate::esf::recycle_received_packet(frame);
        vendor_discard_frame(tail, count);
        return true;
    }

    vendor_discard_frame(tail, count);
    RX_METADATA_PROBE
        .rust_indicate_routes
        .fetch_add(1, Ordering::Relaxed);
    RX_METADATA_PROBE
        .rust_multi_indicate_routes
        .fetch_add(1, Ordering::Relaxed);
    crate::rx::wifi_strict_lmac_rx_done(frame);
    true
}

/// Decode the exact metadata layout at the remaining vendor aggregate
/// boundary and own qualified common STA data/management routes in Rust.
///
/// The qualified route is deliberately narrow: successful base-layout STA
/// data plus association-response, beacon, authentication and qualified
/// Action management frames under the strict ordinary AP/STA mode. It
/// reproduces the pinned
/// `wDevCtrl` publications and, for the common single-descriptor layout, owns
/// the finite `wDev_IndicateFrame` body too. In the STA-only profile it also
/// reproduces the Probe Request
/// STA-to-AP rewrite outcome as a direct Rust-owned discard, after strict
/// preparation disabled the optional observation callback. Action is admitted
/// only after one-shot adoption proved both NAN and FTM side paths disabled.
/// Control frames, optional metadata, error, promiscuous and currently
/// unclassified routes retain an explicit ROM fallback until their state
/// transitions are ported.
#[cfg(target_arch = "riscv32")]
#[no_mangle]
#[link_section = ".rwtext.wifi_strict.rx_success_dispatch"]
pub unsafe extern "C" fn wifi_strict_wdev_process_rx_success_data(tail: *mut u8, count: u32) {
    if !crate::critical::strict_wifi_hart_armed() {
        __real_wDev_ProcessRxSucData(tail, count);
        return;
    }

    RX_METADATA_PROBE.calls.fetch_add(1, Ordering::Relaxed);
    let head = ptr::addr_of!(wDevCtrl).cast::<*mut u8>().read_unaligned();
    if head.is_null() {
        RX_METADATA_PROBE
            .rejected_layout
            .fetch_add(1, Ordering::Relaxed);
        RX_METADATA_PROBE.record_vendor_fallback(RxVendorFallbackReason::MissingHead);
        __real_wDev_ProcessRxSucData(tail, count);
        return;
    }
    let descriptor_word = head.cast::<u32>().read_unaligned();
    let descriptor_capacity = descriptor_buffer_length(descriptor_word);
    let descriptor_length = descriptor_received_length(descriptor_word);
    let metadata = head
        .add(RX_DESCRIPTOR_BUFFER_OFFSET)
        .cast::<*mut u8>()
        .read_unaligned();
    if metadata.is_null()
        || descriptor_length < RX_METADATA_PREFIX_BYTES
        || descriptor_length > descriptor_capacity
    {
        RX_METADATA_PROBE
            .rejected_layout
            .fetch_add(1, Ordering::Relaxed);
        RX_METADATA_PROBE.record_vendor_fallback(RxVendorFallbackReason::InvalidDescriptor);
        __real_wDev_ProcessRxSucData(tail, count);
        return;
    }

    let mut prefix = [0_u8; RX_METADATA_PREFIX_BYTES];
    ptr::copy_nonoverlapping(metadata, prefix.as_mut_ptr(), prefix.len());
    let extended_metadata_enabled = WIFI_MAC_RX_METADATA_CONTROL_REGISTER.read_volatile()
        & WIFI_MAC_RX_EXTENDED_METADATA_BIT
        != 0;
    let Some(layout) = decode_rx_metadata_layout(&prefix, extended_metadata_enabled) else {
        RX_METADATA_PROBE
            .rejected_layout
            .fetch_add(1, Ordering::Relaxed);
        RX_METADATA_PROBE.record_vendor_fallback(RxVendorFallbackReason::InvalidMetadataLayout);
        __real_wDev_ProcessRxSucData(tail, count);
        return;
    };
    let Some(status_offset) = layout.payload_offset.checked_add(4) else {
        RX_METADATA_PROBE
            .rejected_layout
            .fetch_add(1, Ordering::Relaxed);
        RX_METADATA_PROBE.record_vendor_fallback(RxVendorFallbackReason::InvalidStatusOffset);
        __real_wDev_ProcessRxSucData(tail, count);
        return;
    };
    if status_offset >= descriptor_length {
        RX_METADATA_PROBE
            .rejected_layout
            .fetch_add(1, Ordering::Relaxed);
        RX_METADATA_PROBE.record_vendor_fallback(RxVendorFallbackReason::InvalidStatusOffset);
        __real_wDev_ProcessRxSucData(tail, count);
        return;
    }

    RX_METADATA_PROBE.decoded.fetch_add(1, Ordering::Relaxed);
    RX_METADATA_PROBE
        .max_payload_offset
        .fetch_max(layout.payload_offset, Ordering::Relaxed);
    match (layout.has_sublength, layout.has_extra_field) {
        (false, false) => &RX_METADATA_PROBE.base_only,
        (true, false) => &RX_METADATA_PROBE.sublength_only,
        (false, true) => &RX_METADATA_PROBE.extra_only,
        (true, true) => &RX_METADATA_PROBE.sublength_and_extra,
    }
    .fetch_add(1, Ordering::Relaxed);
    match metadata.add(status_offset).read() {
        0 => &RX_METADATA_PROBE.status_success,
        0xf5 => &RX_METADATA_PROBE.status_f5,
        0xc6 => &RX_METADATA_PROBE.status_c6,
        _ => &RX_METADATA_PROBE.status_other,
    }
    .fetch_add(1, Ordering::Relaxed);
    match prefix[3] & 0x70 {
        0x10 => &RX_METADATA_PROBE.route_sta,
        0x20 => &RX_METADATA_PROBE.route_ap,
        0x40 => &RX_METADATA_PROBE.route_nan,
        _ => &RX_METADATA_PROBE.route_other,
    }
    .fetch_add(1, Ordering::Relaxed);
    if layout.payload_offset + 10 <= descriptor_length {
        let frame_control = metadata
            .add(layout.payload_offset + 8)
            .cast::<u16>()
            .read_unaligned();
        RX_METADATA_PROBE.frame_class_bitmap.fetch_or(
            1_usize << usize::from(frame_control & 0x0f),
            Ordering::Relaxed,
        );
        if frame_control & 0x0f == 0 {
            RX_METADATA_PROBE.management_subtype_bitmap.fetch_or(
                1_usize << usize::from((frame_control >> 4) & 0x0f),
                Ordering::Relaxed,
            );
        }
    }
    let aggregate_flag = rx_indicate_aggregate_flag(&prefix).unwrap_or(0) as usize;
    RX_METADATA_PROBE
        .aggregate_flag_bitmap
        .fetch_or(1 << aggregate_flag, Ordering::Relaxed);

    let status = metadata.add(status_offset).read();
    let frame_offset = layout.payload_offset + 8;
    let frame_control = if frame_offset + 2 <= descriptor_length {
        Some(metadata.add(frame_offset).cast::<u16>().read_unaligned())
    } else {
        None
    };
    let data_copy_mode = frame_control.and_then(rx_sta_data_copy_mode);
    let management_copy_mode = frame_control.and_then(rx_sta_management_copy_mode);
    let action_copy_mode = STRICT_ACTION_SIDE_PATHS_DISABLED
        .load(Ordering::Acquire)
        .then(|| frame_control.and_then(rx_sta_action_copy_mode))
        .flatten();
    let probe_request_discard = frame_control.is_some_and(rx_sta_probe_request_is_discarded);
    let copy_mode = data_copy_mode.or(management_copy_mode).or(action_copy_mode);
    let control = ptr::addr_of_mut!(wDevCtrl);
    let csi_length = rx_csi_length(&prefix);
    let copy_plan = csi_length
        .and_then(|csi_length| single_rx_copy_plan(descriptor_length, layout, csi_length));
    let chain_valid =
        !tail.is_null() && count != 0 && count <= MAX_RX_SUCCESS_DESCRIPTORS_PER_EVENT as u32;
    let route = prefix[3] & 0x70;
    let optional_control_30 = control.add(0x30).read() != 0;
    let optional_control_46 = control.add(0x46).read() != 0;
    let ordinary_profile = crate::net80211_state::ordinary_sta_ap_profile();
    let station_interface_present = crate::net80211_state::station_interface().is_some();
    let base_facts = RxVendorFallbackFacts {
        status,
        chain_valid,
        has_extra_field: layout.has_extra_field,
        csi_length,
        copy_plan_valid: copy_plan.is_some(),
        route,
        optional_control_30,
        optional_control_46,
        ordinary_profile,
        station_interface_present,
        frame_classified: true,
    };
    // In pinned `wDev_IndicateFrame+0xf4..0x10a`, `s5` is overwritten with
    // the ten-bit CSI length from metadata bytes 0x26..0x27. The call at
    // +0x298 is gated by `beqz s5`, so `g_wdev_csi_rx` is unobservable for
    // this route: `rx_vendor_fallback_reason` has already required
    // `csi_length == Some(0)`. Do not turn a later callback registration into
    // a false fallback for ordinary non-CSI STA traffic.
    let strict_sta_base_route = rx_vendor_fallback_reason(base_facts).is_none();
    let strict_sta_probe_request_discard = strict_sta_base_route
        && probe_request_discard
        && crate::net80211_state::access_point_interface().is_none();
    if strict_sta_probe_request_discard {
        RX_METADATA_PROBE
            .rust_probe_request_discards
            .fetch_add(1, Ordering::Relaxed);
        vendor_discard_frame(tail, count);
        return;
    }

    let strict_sta_common_route = strict_sta_base_route && copy_mode.is_some();
    if strict_sta_common_route {
        let Some(copy_plan) = copy_plan else {
            RX_METADATA_PROBE.record_vendor_fallback(RxVendorFallbackReason::CopyPlanRejected);
            __real_wDev_ProcessRxSucData(tail, count);
            return;
        };
        // Pinned prelude stores the current RX rate/channel fields into the
        // metadata envelope before publishing the frame pointer.
        metadata.add(0x1c).write(control.add(0x2c).read());
        let metadata_flags = metadata.add(0x1d).read();
        metadata
            .add(0x1d)
            .write((metadata_flags & 0xf0) | (control.add(0x2d).read() & 0x0f));
        control.add(0x44).write(status);
        control
            .add(0x45)
            .write(metadata.add(layout.payload_offset + 5).read());
        let frame = metadata.add(frame_offset);
        control.add(0x40).cast::<*mut u8>().write_unaligned(frame);

        // The safe classifier reproduces the strict real-chip branch and its
        // fragment-clearing join before this unsafe ownership boundary.
        let copy_mode = copy_mode.unwrap_or(0);
        let timestamp =
            u32::from_le_bytes([prefix[0x0c], prefix[0x0d], prefix[0x0e], prefix[0x0f]]);
        if action_copy_mode.is_some() {
            RX_METADATA_PROBE
                .rust_action_routes
                .fetch_add(1, Ordering::Relaxed);
            RX_METADATA_PROBE
                .rust_management_routes
                .fetch_add(1, Ordering::Relaxed);
        } else if management_copy_mode.is_some() {
            RX_METADATA_PROBE
                .rust_management_routes
                .fetch_add(1, Ordering::Relaxed);
        } else {
            RX_METADATA_PROBE
                .rust_data_routes
                .fetch_add(1, Ordering::Relaxed);
        }
        if count > 1
            && !layout.has_sublength
            && rx_csi_length(&prefix) == Some(0)
            && indicate_multi_received_frame(
                head,
                tail,
                count,
                copy_mode,
                aggregate_flag != 0,
                timestamp,
            )
        {
            return;
        }
        if indicate_single_received_frame(
            head,
            tail,
            count,
            metadata,
            descriptor_length,
            copy_mode,
            copy_plan,
            aggregate_flag != 0,
            timestamp,
        ) {
            return;
        }
        RX_METADATA_PROBE
            .vendor_indicate_fallbacks
            .fetch_add(1, Ordering::Relaxed);
        wDev_IndicateFrame(copy_mode, aggregate_flag as u32, tail, count, timestamp);
        return;
    }

    let fallback_reason = rx_vendor_fallback_reason(RxVendorFallbackFacts {
        frame_classified: copy_mode.is_some(),
        ..base_facts
    })
    .unwrap_or(RxVendorFallbackReason::UnclassifiedFrame);
    RX_METADATA_PROBE.record_vendor_fallback(fallback_reason);
    __real_wDev_ProcessRxSucData(tail, count);
}

#[cfg(target_arch = "riscv32")]
impl DetachedRxPrefix {
    #[link_section = ".rwtext.wifi_strict.rx_recycle"]
    unsafe fn recycle(self) {
        __wrap_wDev_AppendRxBlocks(self.head, self.tail, self.count);
    }
}

/// Detach the completed RX prefix from the software descriptor frontier.
///
/// This is the exact state transform recovered from the 0x20-byte pinned
/// `wDev_DiscardFrame` leaf: retain the old `wDevCtrl.head`, publish
/// `tail.next` as the new head, clear `tail.next`, and transfer the detached
/// `(head, tail)` prefix to one Rust owner. A local interrupt mask makes that
/// publication one finite critical section; it never waits for the MAC.
#[cfg(target_arch = "riscv32")]
#[link_section = ".rwtext.wifi_strict.rx_recycle"]
unsafe fn detach_completed_rx_prefix(
    tail: *mut u8,
    count: u32,
) -> Result<DetachedRxPrefix, RxRecycleError> {
    if tail.is_null() {
        return Err(RxRecycleError::MissingTail);
    }
    let interrupt_state = crate::critical::strict_wifi_int_disable();
    let control = ptr::addr_of_mut!(wDevCtrl);
    let head = control.cast::<*mut u8>().read_unaligned();
    if head.is_null() {
        crate::critical::strict_wifi_int_restore(interrupt_state);
        return Err(RxRecycleError::MissingHead);
    }
    let next = tail
        .add(RX_DESCRIPTOR_NEXT_OFFSET)
        .cast::<*mut u8>()
        .read_unaligned();
    tail.add(RX_DESCRIPTOR_NEXT_OFFSET)
        .cast::<*mut u8>()
        .write_unaligned(ptr::null_mut());
    control.cast::<*mut u8>().write_unaligned(next);
    crate::critical::strict_wifi_int_restore(interrupt_state);
    Ok(DetachedRxPrefix { head, tail, count })
}

/// Allocation-free Rust replacement for the RX discard ownership leaf.
///
/// The vendor body contains no protocol work: it only detaches one completed
/// intrusive prefix and tail-calls `wDev_AppendRxBlocks`. Strict mode performs
/// the same transform through a non-`Copy` Rust token and then enters the
/// already qualified asynchronous descriptor recycler. There is no allocator,
/// OSI primitive, polling loop, delay, or task handoff.
#[cfg(target_arch = "riscv32")]
#[no_mangle]
#[link_section = ".rwtext.wifi_strict.rx_recycle"]
pub unsafe extern "C" fn wifi_strict_wdev_discard_frame(tail: *mut u8, count: u32) {
    if !crate::critical::strict_wifi_hart_armed() {
        __real_wDev_DiscardFrame(tail, count);
        return;
    }
    let state = &mut *RX_RECYCLE_STATE.0.get();
    if state.failed || !crate::critical::on_strict_wifi_hart() {
        fail_rx_recycle(state, RxRecycleError::WrongHart);
    }
    match detach_completed_rx_prefix(tail, count) {
        Ok(prefix) => prefix.recycle(),
        Err(error) => fail_rx_recycle(state, error),
    }
}

#[cfg(target_arch = "riscv32")]
pub fn rx_recycle_snapshot() -> WdevRxRecycleSnapshot {
    let control = ptr::addr_of!(wDevCtrl);
    let software_head = unsafe { control.cast::<*mut u8>().read_unaligned() };
    let software_tail = unsafe { control.add(4).cast::<*mut u8>().read_unaligned() };
    WdevRxRecycleSnapshot {
        calls: RX_RECYCLE_PROBE.calls.load(Ordering::Acquire),
        immediate: RX_RECYCLE_PROBE.immediate.load(Ordering::Acquire),
        deferred: RX_RECYCLE_PROBE.deferred.load(Ordering::Acquire),
        timers_armed: RX_RECYCLE_PROBE.timers_armed.load(Ordering::Acquire),
        completions: RX_RECYCLE_PROBE.completions.load(Ordering::Acquire),
        terminal_restarts: RX_RECYCLE_PROBE.terminal_restarts.load(Ordering::Acquire),
        reload_active: RX_RECYCLE_PROBE.reload_active.load(Ordering::Acquire) != 0,
        pending_chains: RX_RECYCLE_PROBE.pending_chains.load(Ordering::Acquire),
        software_head: software_head as usize,
        software_head_word: if software_head.is_null() {
            0
        } else {
            unsafe { software_head.cast::<u32>().read_volatile() }
        },
        software_head_next: if software_head.is_null() {
            0
        } else {
            unsafe {
                software_head
                    .add(RX_DESCRIPTOR_NEXT_OFFSET)
                    .cast::<*mut u8>()
                    .read_volatile() as usize
            }
        },
        software_tail: software_tail as usize,
        software_tail_word: if software_tail.is_null() {
            0
        } else {
            unsafe { software_tail.cast::<u32>().read_volatile() }
        },
        software_tail_next: if software_tail.is_null() {
            0
        } else {
            unsafe {
                software_tail
                    .add(RX_DESCRIPTOR_NEXT_OFFSET)
                    .cast::<*mut u8>()
                    .read_volatile() as usize
            }
        },
        hardware_control: unsafe { WIFI_MAC_RX_CONTROL_REGISTER.read_volatile() },
        hardware_base: unsafe { WIFI_MAC_RX_BASE_REGISTER.read_volatile() as usize },
        hardware_next: unsafe { hal_mac_rx_read_rxdscrnext() as usize },
        hardware_last_raw: unsafe { hal_mac_rx_read_rxdscrlast() as usize },
        hardware_last: unsafe { hal_mac_rx_get_last_dscr() as usize },
        hardware_end_state: unsafe { hal_mac_rx_get_end_state() },
    }
}

#[cfg(target_arch = "riscv32")]
fn runtime_rx_recycle_link_wrapper_active() -> bool {
    core::ptr::eq(
        vendor_append_rx_blocks as *const (),
        __wrap_wDev_AppendRxBlocks as *const (),
    ) && core::ptr::eq(
        vendor_discard_frame as *const (),
        wifi_strict_wdev_discard_frame as *const (),
    ) && crate::esf::rx_packet_recycle_link_wrapper_active()
}

#[cfg(not(target_arch = "riscv32"))]
fn runtime_rx_recycle_link_wrapper_active() -> bool {
    true
}

/// Replace the vendor event-25 outer descriptor walk and both indirect OSI
/// critical-section calls.
///
/// The hardware publishes one finite linked prefix ending at the descriptor
/// returned by `hal_mac_rx_get_last_dscr`. Rust preserves the vendor rule that
/// bit 30 marks the final descriptor of a receive unit and passes the bounded
/// prefix count to the still-audited per-unit decoder. A malformed list can no
/// longer cycle forever: at most 64 descriptors are consumed per executor
/// event, and every error restores local interrupts before returning.
#[cfg(target_arch = "riscv32")]
#[link_section = ".rwtext.wifi_strict.rx_success_dispatch"]
#[inline(never)]
pub(crate) unsafe fn process_rx_success() -> Result<(), WdevRxContinuationError> {
    if !crate::critical::on_strict_wifi_hart() {
        return Err(WdevRxContinuationError::WrongHart);
    }
    let reset = ptr::addr_of!(g_wdev_last_desc_reset_ptr).read();
    if reset.is_null() {
        return Err(WdevRxContinuationError::ResetStateUnavailable);
    }

    let mut last = hal_mac_rx_get_last_dscr();
    if reset.read() != 0 {
        if !last.is_null() {
            reset.write(0);
        }
    } else if last.is_null() {
        return Err(WdevRxContinuationError::MissingLastDescriptor);
    }

    let interrupt_state = crate::critical::strict_wifi_int_disable();
    let mut descriptor = ptr::addr_of!(wDevCtrl).cast::<*mut u8>().read_unaligned();
    let mut subframe_count = 0_u32;
    let mut descriptors_seen = 0_usize;
    while !descriptor.is_null() {
        if descriptors_seen == MAX_RX_SUCCESS_DESCRIPTORS_PER_EVENT {
            crate::critical::strict_wifi_int_restore(interrupt_state);
            // Yield only between complete RX units. Splitting an aggregate
            // would lose its prefix count and is therefore still rejected as
            // a malformed/unsupported chain.
            if subframe_count != 0 {
                return Err(WdevRxContinuationError::DescriptorChainTooLong);
            }
            if pp_post(25, ptr::null_mut()) != 0 {
                return Err(WdevRxContinuationError::ContinuationQueueFull);
            }
            return Ok(());
        }
        descriptors_seen += 1;
        let next = descriptor.add(8).cast::<*mut u8>().read_unaligned();
        if descriptor
            .add(RX_DESCRIPTOR_BUFFER_OFFSET)
            .cast::<*mut u8>()
            .read_unaligned()
            .is_null()
        {
            crate::critical::strict_wifi_int_restore(interrupt_state);
            return Err(WdevRxContinuationError::MissingRxMetadata);
        }
        subframe_count = match subframe_count.checked_add(1) {
            Some(count) if count <= u32::from(u16::MAX) => count,
            _ => {
                crate::critical::strict_wifi_int_restore(interrupt_state);
                return Err(WdevRxContinuationError::DescriptorCountOverflow);
            }
        };

        if descriptor.cast::<u32>().read_unaligned() & (1 << 30) != 0 {
            let completed = CompletedRxUnit {
                tail: descriptor,
                count: subframe_count,
            };
            INDICATE_FRAME_PROBE.calls.fetch_add(1, Ordering::Relaxed);
            INDICATE_FRAME_PROBE
                .validated
                .fetch_add(1, Ordering::Relaxed);
            INDICATE_FRAME_PROBE
                .max_descriptors
                .fetch_max(subframe_count as usize, Ordering::Relaxed);
            crate::critical::strict_wifi_int_restore(interrupt_state);
            // Match the pinned vendor outer walk exactly: the decoder receives
            // the descriptor carrying this unit's bit-30 completion marker.
            // It obtains the unit head from `wDevCtrl.head`; this argument is
            // retained as the exact tail passed to discard/recycle.
            completed.dispatch();
            subframe_count = 0;
            if descriptor == last {
                let latest = hal_mac_rx_get_last_dscr();
                if latest == descriptor {
                    mark_drained_rx_chain(descriptor);
                    return Ok(());
                }
                if reset.read() == 0 && latest.is_null() {
                    return Err(WdevRxContinuationError::MissingLastDescriptor);
                }
                last = latest;
            } else {
                last = hal_mac_rx_get_last_dscr();
                if reset.read() == 0 && last.is_null() {
                    return Err(WdevRxContinuationError::MissingLastDescriptor);
                }
            }
            // The decoder may recycle the completed unit and mutate its
            // descriptor links. Continue from the bounded pre-callback
            // snapshot, as the pinned vendor walk does.
            descriptor = next;
            if descriptor.is_null() {
                return Ok(());
            }
            // Match the vendor outer walk: only the pointer publication is
            // protected; the per-unit decoder executes with interrupts on.
            let new_interrupt_state = crate::critical::strict_wifi_int_disable();
            // The strict local primitive can only return the current MIE bit.
            // It must match the state initially captured for this radio event.
            if new_interrupt_state & 8 != interrupt_state & 8 {
                crate::critical::strict_wifi_int_restore(new_interrupt_state);
                return Err(WdevRxContinuationError::WrongHart);
            }
            continue;
        }
        if descriptor == last {
            crate::critical::strict_wifi_int_restore(interrupt_state);
            return Ok(());
        }
        last = hal_mac_rx_get_last_dscr();
        if reset.read() == 0 && last.is_null() {
            crate::critical::strict_wifi_int_restore(interrupt_state);
            return Err(WdevRxContinuationError::MissingLastDescriptor);
        }
        descriptor = next;
    }
    crate::critical::strict_wifi_int_restore(interrupt_state);
    Ok(())
}

#[cfg(target_arch = "riscv32")]
pub(crate) unsafe fn strict_optional_rx_mode_state() -> (u8, u8, usize) {
    let control = ptr::addr_of!(wDevCtrl);
    (
        control.add(0x30).read(),
        control.add(0x46).read(),
        ptr::addr_of!(g_wdev_csi_rx).read(),
    )
}

/// Adopt the immutable optional-policy guards surrounding management Action
/// RX before the strict executor takes ownership.
///
/// The pinned `ic_interface_enabled(2)` is exactly bit two of
/// `wDevCtrl+0x31`. The adjacent FTM branch tests bit `0x04` in the aligned
/// word at `g_wifi_menuconfig+0x40`. Post-handoff APIs cannot enable NAN or
/// FTM, so publishing this one-shot Rust proof removes both hidden global
/// reads from the RX hot path.
///
/// # Safety
///
/// Vendor initialization must be quiescent and the caller must prevent NAN
/// and FTM configuration changes after a successful adoption.
#[cfg(target_arch = "riscv32")]
pub(crate) unsafe fn adopt_action_rx_policy() -> Result<(), WdevActionRxAdoptionError> {
    if STRICT_ACTION_SIDE_PATHS_DISABLED.load(Ordering::Acquire) {
        return Ok(());
    }
    if ptr::addr_of!(wDevCtrl).add(0x31).read_volatile() & (1 << 2) != 0 {
        return Err(WdevActionRxAdoptionError::NanInterfaceEnabled);
    }
    if ptr::addr_of!(g_wifi_menuconfig)
        .add(0x40)
        .cast::<u32>()
        .read_volatile()
        & 0x04
        != 0
    {
        return Err(WdevActionRxAdoptionError::FtmRxEnabled);
    }
    STRICT_ACTION_SIDE_PATHS_DISABLED.store(true, Ordering::Release);
    Ok(())
}

pub(crate) fn runtime_wdev_link_wrapper_active() -> bool {
    core::ptr::eq(
        vendor_record_ftm_data as *const (),
        __wrap_wDev_record_ftm_data as *const (),
    ) && core::ptr::eq(
        vendor_pm_on_beacon_rx as *const (),
        __wrap_pm_on_beacon_rx as *const (),
    ) && core::ptr::eq(
        vendor_pm_on_data_rx as *const (),
        __wrap_pm_on_data_rx as *const (),
    ) && core::ptr::eq(
        vendor_pm_on_data_tx as *const (),
        __wrap_pm_on_data_tx as *const (),
    ) && core::ptr::eq(
        vendor_pm_on_coex_schm_status_config as *const (),
        __wrap_pm_on_coex_schm_status_config as *const (),
    ) && core::ptr::eq(
        vendor_pm_set_beacon_duration as *const (),
        __wrap_pm_set_beacon_duration as *const (),
    ) && core::ptr::eq(
        vendor_ftm_set_t1t4 as *const (),
        __wrap_wDev_ftm_set_t1t4 as *const (),
    ) && core::ptr::eq(
        vendor_is_nan_packet_in_valid_slot as *const (),
        __wrap_wDev_isNANPktInValidSlot as *const (),
    ) && core::ptr::eq(
        vendor_sniffer_rx_data as *const (),
        __wrap_wDev_SnifferRxData as *const (),
    ) && core::ptr::eq(
        vendor_csi_rx_process as *const (),
        __wrap_wdev_csi_rx_process as *const (),
    ) && core::ptr::eq(
        vendor_indicate_ctrl_frame as *const (),
        __wrap_wDev_IndicateCtrlFrame as *const (),
    ) && runtime_rx_recycle_link_wrapper_active()
        && runtime_rx_success_decoder_link_wrapper_active()
}

#[cfg(target_arch = "riscv32")]
fn runtime_rx_success_decoder_link_wrapper_active() -> bool {
    core::ptr::eq(
        vendor_process_rx_success_data as *const (),
        wifi_strict_wdev_process_rx_success_data as *const (),
    )
}

#[cfg(not(target_arch = "riscv32"))]
fn runtime_rx_success_decoder_link_wrapper_active() -> bool {
    true
}

pub(crate) fn take_ftm_attempted() -> bool {
    FTM_ATTEMPTED.swap(false, Ordering::AcqRel)
}

/// Reject Fine Timing Measurement RX accounting in the strict profile.
///
/// The pinned vendor implementation starts with `ets_delay_us(50)`. The final
/// link must use `--wrap=wDev_record_ftm_data`; the enclosing event handler
/// observes this marker and fails after returning from its finite RX section.
#[no_mangle]
pub unsafe extern "C" fn __wrap_wDev_record_ftm_data(
    _rx_control: *mut c_void,
    _frame: *mut c_void,
) {
    FTM_ATTEMPTED.store(true, Ordering::Release);
}

/// Reject the optional TX FTM timestamp callback under the disabled-FTM
/// invariant.
#[no_mangle]
pub unsafe extern "C" fn __wrap_wDev_ftm_set_t1t4(_frame: *mut c_void) {
    FTM_ATTEMPTED.store(true, Ordering::Release);
}

/// Preserve ordinary AP/STA TX while rejecting the callback-driven NAN path.
#[no_mangle]
pub unsafe extern "C" fn __wrap_wDev_isNANPktInValidSlot(frame: *mut u8) -> i32 {
    if !crate::critical::strict_wifi_hart_armed() {
        return __real_wDev_isNANPktInValidSlot(frame);
    }
    if frame.is_null() {
        return 0;
    }
    let descriptor = frame.add(0x34).cast::<*mut u8>().read();
    if descriptor.is_null() {
        return 0;
    }
    let packet_kind = descriptor.add(0x10).cast::<u32>().read() & 0x00c0_0000;
    i32::from(packet_kind != 0x0080_0000)
}

/// Remove promiscuous delivery from the strict basic AP/STA receive profile.
///
/// Preparation disables promiscuous mode through the public control API,
/// verifies its readback and the pinned `wDevCtrl` state, and unregisters the
/// callback before the RTOS handoff.
#[no_mangle]
pub unsafe extern "C" fn __wrap_wDev_SnifferRxData() {}

/// Remove CSI capture from the strict basic AP/STA receive profile.
///
/// The pinned vendor implementation allocates a 100-byte callback envelope.
/// Strict configuration rejects CSI and preparation verifies that the callback
/// pointer remains null before this boundary can be armed.
#[no_mangle]
pub unsafe extern "C" fn __wrap_wdev_csi_rx_process() {}

/// Elide the allocation-only CSI control-frame envelope in strict AP/STA mode.
///
/// The pinned function returns one on every path. Its only observable work is
/// an OSI Wi-Fi allocation, two finite copies, `wdev_csi_rx_process`, and the
/// matching OSI free. Preparation has disabled CSI and verified both the
/// callback and `wDevCtrl` state, so the constant return preserves the caller's
/// control-flow result without constructing an unused dynamic envelope.
#[no_mangle]
#[link_section = ".rwtext.wifi_strict.csi_control"]
pub unsafe extern "C" fn __wrap_wDev_IndicateCtrlFrame(
    frame: *mut u8,
    count: u32,
    kind: u32,
) -> i32 {
    if crate::critical::strict_wifi_hart_armed() {
        1
    } else {
        __real_wDev_IndicateCtrlFrame(frame, count, kind)
    }
}

#[cfg(target_arch = "riscv32")]
pub fn indicate_frame_snapshot() -> WdevIndicateFrameSnapshot {
    WdevIndicateFrameSnapshot {
        calls: INDICATE_FRAME_PROBE.calls.load(Ordering::Acquire),
        validated: INDICATE_FRAME_PROBE.validated.load(Ordering::Acquire),
        max_descriptors: INDICATE_FRAME_PROBE.max_descriptors.load(Ordering::Acquire),
    }
}

#[cfg(target_arch = "riscv32")]
pub fn rx_metadata_snapshot() -> WdevRxMetadataSnapshot {
    let fallback_count = |reason: RxVendorFallbackReason| {
        RX_METADATA_PROBE.vendor_fallback_reasons[reason.index()].load(Ordering::Acquire)
    };
    let vendor_fallback_reasons = WdevRxVendorFallbackSnapshot {
        missing_head: fallback_count(RxVendorFallbackReason::MissingHead),
        invalid_descriptor: fallback_count(RxVendorFallbackReason::InvalidDescriptor),
        invalid_metadata_layout: fallback_count(RxVendorFallbackReason::InvalidMetadataLayout),
        invalid_status_offset: fallback_count(RxVendorFallbackReason::InvalidStatusOffset),
        non_success_status: fallback_count(RxVendorFallbackReason::NonSuccessStatus),
        invalid_chain: fallback_count(RxVendorFallbackReason::InvalidChain),
        extended_metadata: fallback_count(RxVendorFallbackReason::ExtendedMetadata),
        csi_metadata: fallback_count(RxVendorFallbackReason::CsiMetadata),
        copy_plan_rejected: fallback_count(RxVendorFallbackReason::CopyPlanRejected),
        ap_route: fallback_count(RxVendorFallbackReason::ApRoute),
        nan_route: fallback_count(RxVendorFallbackReason::NanRoute),
        other_route: fallback_count(RxVendorFallbackReason::OtherRoute),
        optional_control_30: fallback_count(RxVendorFallbackReason::OptionalControl30),
        optional_control_46: fallback_count(RxVendorFallbackReason::OptionalControl46),
        non_ordinary_profile: fallback_count(RxVendorFallbackReason::NonOrdinaryProfile),
        missing_station_interface: fallback_count(RxVendorFallbackReason::MissingStationInterface),
        unclassified_frame: fallback_count(RxVendorFallbackReason::UnclassifiedFrame),
    };
    WdevRxMetadataSnapshot {
        calls: RX_METADATA_PROBE.calls.load(Ordering::Acquire),
        decoded: RX_METADATA_PROBE.decoded.load(Ordering::Acquire),
        rejected_layout: RX_METADATA_PROBE.rejected_layout.load(Ordering::Acquire),
        status_success: RX_METADATA_PROBE.status_success.load(Ordering::Acquire),
        status_f5: RX_METADATA_PROBE.status_f5.load(Ordering::Acquire),
        status_c6: RX_METADATA_PROBE.status_c6.load(Ordering::Acquire),
        status_other: RX_METADATA_PROBE.status_other.load(Ordering::Acquire),
        base_only: RX_METADATA_PROBE.base_only.load(Ordering::Acquire),
        sublength_only: RX_METADATA_PROBE.sublength_only.load(Ordering::Acquire),
        extra_only: RX_METADATA_PROBE.extra_only.load(Ordering::Acquire),
        sublength_and_extra: RX_METADATA_PROBE
            .sublength_and_extra
            .load(Ordering::Acquire),
        max_payload_offset: RX_METADATA_PROBE.max_payload_offset.load(Ordering::Acquire),
        route_sta: RX_METADATA_PROBE.route_sta.load(Ordering::Acquire),
        route_ap: RX_METADATA_PROBE.route_ap.load(Ordering::Acquire),
        route_nan: RX_METADATA_PROBE.route_nan.load(Ordering::Acquire),
        route_other: RX_METADATA_PROBE.route_other.load(Ordering::Acquire),
        frame_class_bitmap: RX_METADATA_PROBE.frame_class_bitmap.load(Ordering::Acquire),
        management_subtype_bitmap: RX_METADATA_PROBE
            .management_subtype_bitmap
            .load(Ordering::Acquire),
        aggregate_flag_bitmap: RX_METADATA_PROBE
            .aggregate_flag_bitmap
            .load(Ordering::Acquire),
        rust_data_routes: RX_METADATA_PROBE.rust_data_routes.load(Ordering::Acquire),
        rust_management_routes: RX_METADATA_PROBE
            .rust_management_routes
            .load(Ordering::Acquire),
        rust_action_routes: RX_METADATA_PROBE.rust_action_routes.load(Ordering::Acquire),
        rust_probe_request_discards: RX_METADATA_PROBE
            .rust_probe_request_discards
            .load(Ordering::Acquire),
        rust_indicate_routes: RX_METADATA_PROBE
            .rust_indicate_routes
            .load(Ordering::Acquire),
        rust_multi_indicate_routes: RX_METADATA_PROBE
            .rust_multi_indicate_routes
            .load(Ordering::Acquire),
        rust_multi_copy_mode_discards: RX_METADATA_PROBE
            .rust_multi_copy_mode_discards
            .load(Ordering::Acquire),
        rust_indicate_allocation_rejects: RX_METADATA_PROBE
            .rust_indicate_allocation_rejects
            .load(Ordering::Acquire),
        rust_indicate_population_rejects: RX_METADATA_PROBE
            .rust_indicate_population_rejects
            .load(Ordering::Acquire),
        vendor_indicate_fallbacks: RX_METADATA_PROBE
            .vendor_indicate_fallbacks
            .load(Ordering::Acquire),
        vendor_fallbacks: RX_METADATA_PROBE.vendor_fallbacks.load(Ordering::Acquire),
        vendor_fallback_reasons,
    }
}

/// Remove the vendor power-save/mesh beacon tail under `WIFI_PS_NONE`.
///
/// PP/net80211 has already parsed and delivered the beacon before this hook.
/// The stock function only updates power-save state and contains the path from
/// TIM processing to radio shutdown and `ets_delay_us`.
#[no_mangle]
pub unsafe extern "C" fn __wrap_pm_on_beacon_rx(
    _interface: *mut c_void,
    _frame: *mut u8,
    _frame_end: *mut u8,
    _from_task: u32,
) {
}

/// Remove RX power-management accounting under the verified `WIFI_PS_NONE`
/// invariant.
///
/// `ppRxProtoProc` has already classified the ordinary frame and retains its
/// independent receive-rate update. The stock ROM hook only advances modem
/// sleep state and may enter OSI timers and Wi-Fi API locks.
#[no_mangle]
pub unsafe extern "C" fn __wrap_pm_on_data_rx(
    _receiver: *mut u8,
    _packet_class: u32,
    _transmitter: *mut u8,
    _interface: u32,
) {
}

/// Remove TX power-management accounting under the verified `WIFI_PS_NONE`
/// invariant. The stock eight-byte trampoline enters the complete sleep/null
/// frame state machine even though that mode is disabled.
#[no_mangle]
pub unsafe extern "C" fn __wrap_pm_on_data_tx() {}

/// Remove the coexistence-to-power-management status bridge in the strict
/// Wi-Fi-only profile.
///
/// The pinned body queries connectionless power-save state when `status` is
/// zero, then enters OSI Wi-Fi locks and rearms a vendor PM timer. Bluetooth
/// and IEEE 802.15.4 coexistence are not initialized by this profile and
/// `WIFI_PS_NONE` is verified before handoff, so none of those state changes
/// has a consumer. Keeping the leaf would also let a delayed status edge
/// dereference connectionless-PM state which our taskless STA path never
/// initializes.
#[no_mangle]
#[link_section = ".rwtext.wifi_strict.pm_coex_status"]
pub unsafe extern "C" fn __wrap_pm_on_coex_schm_status_config(_status: u32) {}

/// Remove the sampled-beacon-duration update under `WIFI_PS_NONE`.
///
/// The stock function only maintains modem-sleep state. Its first-sample path
/// invokes two optional beacon-offset callbacks; neither is part of an always
/// awake STA/AP profile.
#[no_mangle]
pub unsafe extern "C" fn __wrap_pm_set_beacon_duration(_duration: u32) {}
