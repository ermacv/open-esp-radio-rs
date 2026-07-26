//! Allocation-free passive scanning on the strict ESP32-S31 radio owner.

#[cfg(all(target_arch = "riscv32", feature = "strict-no-wait"))]
use core::{
    cell::UnsafeCell,
    sync::atomic::{AtomicU32, AtomicU8, Ordering},
};

#[cfg(all(target_arch = "riscv32", feature = "strict-no-wait"))]
use crate::interrupt::InterruptSignal;

#[cfg(target_arch = "riscv32")]
unsafe extern "C" {
    #[link_name = "cnx_check_bssid_in_blacklist"]
    fn vendor_cnx_check_bssid_in_blacklist(bssid: *const u8) -> i32;
    #[link_name = "cnx_add_to_blacklist"]
    fn vendor_cnx_add_to_blacklist(bssid: *const u8);
    #[link_name = "cnx_remove_from_blacklist"]
    fn vendor_cnx_remove_from_blacklist(bssid: *const u8);
    #[link_name = "cnx_clear_blacklist"]
    fn vendor_cnx_clear_blacklist();
}

#[cfg(all(target_arch = "riscv32", feature = "strict-no-wait"))]
pub(crate) const SCAN_CHANNEL_EVENT: u32 = u32::MAX - 11;
pub const STRICT_SCAN_RECORD_CAPACITY: usize = 32;
pub const STRICT_SCAN_RSN_IE_CAPACITY: usize = 64;
pub const STRICT_SCAN_RSNXE_CAPACITY: usize = 16;
pub const STRICT_SCAN_EXTENDED_RATES_CAPACITY: usize = 16;
pub const STRICT_SCAN_HT_CAPABILITY_IE_LEN: usize = 28;
pub const STRICT_SCAN_HT_OPERATION_IE_LEN: usize = 24;
pub const STRICT_SCAN_HE_CAPABILITY_IE_CAPACITY: usize = 64;
pub const STRICT_SCAN_HE_OPERATION_IE_CAPACITY: usize = 32;
pub const STRICT_SCAN_WMM_IE_CAPACITY: usize = 26;

/// Verify that the strict final link cannot re-enter the vendor connection
/// manager's allocation-backed BSSID blacklist.
#[cfg(target_arch = "riscv32")]
pub(crate) fn connection_blacklist_link_wrappers_active() -> bool {
    core::ptr::eq(
        vendor_cnx_check_bssid_in_blacklist as *const (),
        __wrap_cnx_check_bssid_in_blacklist as *const (),
    ) && core::ptr::eq(
        vendor_cnx_add_to_blacklist as *const (),
        __wrap_cnx_add_to_blacklist as *const (),
    ) && core::ptr::eq(
        vendor_cnx_remove_from_blacklist as *const (),
        __wrap_cnx_remove_from_blacklist as *const (),
    ) && core::ptr::eq(
        vendor_cnx_clear_blacklist as *const (),
        __wrap_cnx_clear_blacklist as *const (),
    )
}

/// The strict Rust scanner/association state machine owns candidate
/// suppression. A vendor blacklist lookup is therefore always false.
///
/// The stock function follows an allocation-backed linked list and was
/// observed dereferencing a stale pre-handoff node during WPA2 association.
#[cfg(target_arch = "riscv32")]
#[no_mangle]
#[link_section = ".rwtext.wifi_strict.connection_blacklist"]
pub unsafe extern "C" fn __wrap_cnx_check_bssid_in_blacklist(_bssid: *const u8) -> i32 {
    0
}

/// Discard vendor reconnect bookkeeping; Rust owns reconnect policy and its
/// fixed-capacity scan records.
#[cfg(target_arch = "riscv32")]
#[no_mangle]
#[link_section = ".rwtext.wifi_strict.connection_blacklist"]
pub unsafe extern "C" fn __wrap_cnx_add_to_blacklist(_bssid: *const u8) {}

#[cfg(target_arch = "riscv32")]
#[no_mangle]
#[link_section = ".rwtext.wifi_strict.connection_blacklist"]
pub unsafe extern "C" fn __wrap_cnx_remove_from_blacklist(_bssid: *const u8) {}

#[cfg(target_arch = "riscv32")]
#[no_mangle]
#[link_section = ".rwtext.wifi_strict.connection_blacklist"]
pub unsafe extern "C" fn __wrap_cnx_clear_blacklist() {}

#[cfg(all(target_arch = "riscv32", feature = "strict-no-wait"))]
const SESSION_IDLE: u8 = 0;
#[cfg(all(target_arch = "riscv32", feature = "strict-no-wait"))]
const SESSION_ARMING: u8 = 1;
#[cfg(all(target_arch = "riscv32", feature = "strict-no-wait"))]
const SESSION_ACTIVE: u8 = 2;
#[cfg(all(target_arch = "riscv32", feature = "strict-no-wait"))]
const OP_IDLE: u8 = 0;
#[cfg(all(target_arch = "riscv32", feature = "strict-no-wait"))]
const OP_QUEUED: u8 = 1;
#[cfg(all(target_arch = "riscv32", feature = "strict-no-wait"))]
const OP_RUNNING: u8 = 2;

/// One bounded, owned observation from a beacon or probe response.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StrictScanRecord {
    pub ssid: [u8; 32],
    pub ssid_len: u8,
    pub bssid: [u8; 6],
    pub channel: u8,
    pub rssi: i8,
    pub privacy: bool,
    pub rsn: bool,
    pub legacy_wpa: bool,
    pub information_elements_truncated: bool,
    pub capability_info: u16,
    pub beacon_interval_tu: u16,
    pub supported_rates: [u8; 8],
    pub supported_rates_len: u8,
    pub extended_supported_rates: [u8; STRICT_SCAN_EXTENDED_RATES_CAPACITY],
    pub extended_supported_rates_len: u8,
    pub ht_capability_ie: [u8; STRICT_SCAN_HT_CAPABILITY_IE_LEN],
    pub ht_capability_ie_present: bool,
    pub ht_operation_ie: [u8; STRICT_SCAN_HT_OPERATION_IE_LEN],
    pub ht_operation_ie_present: bool,
    pub he_capability_ie: [u8; STRICT_SCAN_HE_CAPABILITY_IE_CAPACITY],
    pub he_capability_ie_len: u8,
    pub he_operation_ie: [u8; STRICT_SCAN_HE_OPERATION_IE_CAPACITY],
    pub he_operation_ie_len: u8,
    pub wmm_ie: [u8; STRICT_SCAN_WMM_IE_CAPACITY],
    pub wmm_ie_len: u8,
    pub rsn_ie: [u8; STRICT_SCAN_RSN_IE_CAPACITY],
    pub rsn_ie_len: u8,
    pub rsnxe: [u8; STRICT_SCAN_RSNXE_CAPACITY],
    pub rsnxe_len: u8,
}

impl StrictScanRecord {
    pub const EMPTY: Self = Self {
        ssid: [0; 32],
        ssid_len: 0,
        bssid: [0; 6],
        channel: 0,
        rssi: i8::MIN,
        privacy: false,
        rsn: false,
        legacy_wpa: false,
        information_elements_truncated: false,
        capability_info: 0,
        beacon_interval_tu: 0,
        supported_rates: [0; 8],
        supported_rates_len: 0,
        extended_supported_rates: [0; STRICT_SCAN_EXTENDED_RATES_CAPACITY],
        extended_supported_rates_len: 0,
        ht_capability_ie: [0; STRICT_SCAN_HT_CAPABILITY_IE_LEN],
        ht_capability_ie_present: false,
        ht_operation_ie: [0; STRICT_SCAN_HT_OPERATION_IE_LEN],
        ht_operation_ie_present: false,
        he_capability_ie: [0; STRICT_SCAN_HE_CAPABILITY_IE_CAPACITY],
        he_capability_ie_len: 0,
        he_operation_ie: [0; STRICT_SCAN_HE_OPERATION_IE_CAPACITY],
        he_operation_ie_len: 0,
        wmm_ie: [0; STRICT_SCAN_WMM_IE_CAPACITY],
        wmm_ie_len: 0,
        rsn_ie: [0; STRICT_SCAN_RSN_IE_CAPACITY],
        rsn_ie_len: 0,
        rsnxe: [0; STRICT_SCAN_RSNXE_CAPACITY],
        rsnxe_len: 0,
    };

    pub fn ssid_bytes(&self) -> &[u8] {
        &self.ssid[..usize::from(self.ssid_len)]
    }

    pub fn supported_rates_bytes(&self) -> &[u8] {
        &self.supported_rates[..usize::from(self.supported_rates_len)]
    }

    pub fn extended_supported_rates_bytes(&self) -> &[u8] {
        &self.extended_supported_rates[..usize::from(self.extended_supported_rates_len)]
    }

    /// Exact 802.11 HT Capabilities element, including id and length.
    pub fn ht_capability_ie_bytes(&self) -> Option<&[u8; STRICT_SCAN_HT_CAPABILITY_IE_LEN]> {
        self.ht_capability_ie_present
            .then_some(&self.ht_capability_ie)
    }

    /// Exact 802.11 HT Operation element, including id and length.
    pub fn ht_operation_ie_bytes(&self) -> Option<&[u8; STRICT_SCAN_HT_OPERATION_IE_LEN]> {
        self.ht_operation_ie_present
            .then_some(&self.ht_operation_ie)
    }

    /// Exact HE Capabilities extension element, including id and length.
    pub fn he_capability_ie_bytes(&self) -> &[u8] {
        &self.he_capability_ie[..usize::from(self.he_capability_ie_len)]
    }

    /// Exact HE Operation extension element, including id and length.
    pub fn he_operation_ie_bytes(&self) -> &[u8] {
        &self.he_operation_ie[..usize::from(self.he_operation_ie_len)]
    }

    /// Exact WMM information/parameter element, including id and length.
    pub fn wmm_ie_bytes(&self) -> &[u8] {
        &self.wmm_ie[..usize::from(self.wmm_ie_len)]
    }

    /// Exact RSN element, including its element id and length byte.
    pub fn rsn_ie_bytes(&self) -> &[u8] {
        &self.rsn_ie[..usize::from(self.rsn_ie_len)]
    }

    /// Exact RSN extension element, including its element id and length byte.
    pub fn rsnxe_bytes(&self) -> &[u8] {
        &self.rsnxe[..usize::from(self.rsnxe_len)]
    }
}

/// Select the strongest complete observation with an exact SSID match.
///
/// A zero or out-of-range channel is never returned. The caller can further
/// restrict security suites after inspecting the owned RSN bytes.
pub fn best_matching_ssid<'a>(
    records: &'a [StrictScanRecord],
    ssid: &[u8],
) -> Option<&'a StrictScanRecord> {
    records
        .iter()
        .filter(|record| record.ssid_bytes() == ssid && (1..=13).contains(&record.channel))
        .max_by_key(|record| record.rssi)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StrictScanSummary {
    pub records: usize,
    pub observed_frames: u32,
    pub dropped_unique_bss: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StrictScanError {
    Busy,
    InvalidDwell,
    QueueFull,
    ChannelStart(i32),
    ChannelCompletion(u32),
}

#[cfg(all(target_arch = "riscv32", feature = "strict-no-wait"))]
struct ScanTable(UnsafeCell<[StrictScanRecord; STRICT_SCAN_RECORD_CAPACITY]>);

// The table is written only by the radio-owner RX path while SESSION_ACTIVE.
// The initiating future copies it only after the last radio-owner completion.
#[cfg(all(target_arch = "riscv32", feature = "strict-no-wait"))]
unsafe impl Sync for ScanTable {}

#[cfg(all(target_arch = "riscv32", feature = "strict-no-wait"))]
static TABLE: ScanTable = ScanTable(UnsafeCell::new(
    [StrictScanRecord::EMPTY; STRICT_SCAN_RECORD_CAPACITY],
));
#[cfg(all(target_arch = "riscv32", feature = "strict-no-wait"))]
static TABLE_LEN: AtomicU8 = AtomicU8::new(0);
#[cfg(all(target_arch = "riscv32", feature = "strict-no-wait"))]
static SESSION: AtomicU8 = AtomicU8::new(SESSION_IDLE);
#[cfg(all(target_arch = "riscv32", feature = "strict-no-wait"))]
static CURRENT_CHANNEL: AtomicU8 = AtomicU8::new(0);
#[cfg(all(target_arch = "riscv32", feature = "strict-no-wait"))]
static OBSERVED_FRAMES: AtomicU32 = AtomicU32::new(0);
#[cfg(all(target_arch = "riscv32", feature = "strict-no-wait"))]
static DROPPED_UNIQUE_BSS: AtomicU32 = AtomicU32::new(0);

#[cfg(all(target_arch = "riscv32", feature = "strict-no-wait"))]
static OP_STATE: AtomicU8 = AtomicU8::new(OP_IDLE);
#[cfg(all(target_arch = "riscv32", feature = "strict-no-wait"))]
static OP_CHANNEL: AtomicU8 = AtomicU8::new(0);
#[cfg(all(target_arch = "riscv32", feature = "strict-no-wait"))]
static OP_DWELL_MS: AtomicU32 = AtomicU32::new(0);
#[cfg(all(target_arch = "riscv32", feature = "strict-no-wait"))]
static OP_RESULT: AtomicU32 = AtomicU32::new(0);
#[cfg(all(target_arch = "riscv32", feature = "strict-no-wait"))]
static OP_SIGNAL: InterruptSignal = InterruptSignal::new();

#[cfg(all(target_arch = "riscv32", feature = "strict-no-wait"))]
unsafe extern "C" {
    static mut g_ic: u8;
    fn ic_set_mac(index: u32, address: *const u8);
    fn ic_set_rx_policy(index: u32, mode: u32, control: u32, management: u32);
    fn ic_set_rx_policy_ubssid_check(index: u32, enabled: u32);
    fn chm_start_op(
        channel: *const u8,
        first_dwell_ms: u32,
        final_dwell_ms: u32,
        start: Option<unsafe extern "C" fn(*mut core::ffi::c_void, u32)>,
        end: Option<unsafe extern "C" fn(*mut core::ffi::c_void, u32)>,
        context: *mut core::ffi::c_void,
    ) -> i32;
}

#[cfg(all(target_arch = "riscv32", feature = "strict-no-wait"))]
struct SessionGuard;

#[cfg(all(target_arch = "riscv32", feature = "strict-no-wait"))]
impl SessionGuard {
    fn begin() -> Result<Self, StrictScanError> {
        if OP_STATE.load(Ordering::Acquire) != OP_IDLE {
            return Err(StrictScanError::Busy);
        }
        SESSION
            .compare_exchange(
                SESSION_IDLE,
                SESSION_ARMING,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .map_err(|_| StrictScanError::Busy)?;
        unsafe { (*TABLE.0.get()).fill(StrictScanRecord::EMPTY) };
        TABLE_LEN.store(0, Ordering::Relaxed);
        CURRENT_CHANNEL.store(0, Ordering::Relaxed);
        OBSERVED_FRAMES.store(0, Ordering::Relaxed);
        DROPPED_UNIQUE_BSS.store(0, Ordering::Relaxed);
        SESSION.store(SESSION_ACTIVE, Ordering::Release);
        Ok(Self)
    }
}

#[cfg(all(target_arch = "riscv32", feature = "strict-no-wait"))]
impl Drop for SessionGuard {
    fn drop(&mut self) {
        SESSION.store(SESSION_IDLE, Ordering::Release);
    }
}

/// Run a complete passive 2.4-GHz scan through the Rust-owned radio future.
///
/// The future has no polling timer of its own: every channel completion is
/// driven by the strict runtime's one-shot alarm and wakes this future once.
/// Results are copied into `output`; excess unique BSS entries are counted and
/// discarded immediately.
#[cfg(all(target_arch = "riscv32", feature = "strict-no-wait"))]
pub async fn passive_scan_2_4ghz(
    output: &mut [StrictScanRecord],
    dwell_ms: u32,
) -> Result<StrictScanSummary, StrictScanError> {
    if dwell_ms == 0 {
        return Err(StrictScanError::InvalidDwell);
    }
    let _guard = SessionGuard::begin()?;

    for channel in 1..=13 {
        run_channel(channel, dwell_ms).await?;
    }

    // Stop ingress before copying. The completion callback and RX observer run
    // on the same radio-owner stack, so no producer can still hold the table.
    SESSION.store(SESSION_ARMING, Ordering::Release);
    let available = usize::from(TABLE_LEN.load(Ordering::Acquire));
    let copied = available.min(output.len());
    let records = unsafe { &*TABLE.0.get() };
    output[..copied].copy_from_slice(&records[..copied]);
    Ok(StrictScanSummary {
        records: copied,
        observed_frames: OBSERVED_FRAMES.load(Ordering::Acquire),
        dropped_unique_bss: DROPPED_UNIQUE_BSS
            .load(Ordering::Acquire)
            .saturating_add((available - copied) as u32),
    })
}

/// Switch to one 2.4-GHz channel and make it the fixed home channel.
///
/// Completion is raised directly by the physical channel-switch callback.
/// There is no dwell timer, retry loop, register polling, or RTOS wait.
#[cfg(all(target_arch = "riscv32", feature = "strict-no-wait"))]
pub async fn tune_home_channel(channel: u8) -> Result<(), StrictScanError> {
    if !(1..=13).contains(&channel) {
        return Err(StrictScanError::ChannelStart(-1));
    }
    if SESSION.load(Ordering::Acquire) != SESSION_IDLE {
        return Err(StrictScanError::Busy);
    }
    run_channel(channel, 0).await
}

#[cfg(all(target_arch = "riscv32", feature = "strict-no-wait"))]
async fn run_channel(channel: u8, dwell_ms: u32) -> Result<(), StrictScanError> {
    OP_STATE
        .compare_exchange(OP_IDLE, OP_QUEUED, Ordering::AcqRel, Ordering::Acquire)
        .map_err(|_| StrictScanError::Busy)?;
    OP_CHANNEL.store(channel, Ordering::Relaxed);
    OP_DWELL_MS.store(dwell_ms, Ordering::Relaxed);
    OP_RESULT.store(0, Ordering::Relaxed);
    CURRENT_CHANNEL.store(channel, Ordering::Release);
    let observed = OP_SIGNAL.generation();
    if !crate::adapter::enqueue_internal_event(crate::event::PpEvent {
        kind: SCAN_CHANNEL_EVENT,
        argument: core::ptr::null_mut(),
    }) {
        OP_STATE.store(OP_IDLE, Ordering::Release);
        return Err(StrictScanError::QueueFull);
    }
    OP_SIGNAL.wait_after(observed).await;
    match OP_RESULT.load(Ordering::Acquire) {
        0 => Ok(()),
        value if value & 0x8000_0000 != 0 => {
            Err(StrictScanError::ChannelStart((value & 0x7fff_ffff) as i32))
        }
        value => Err(StrictScanError::ChannelCompletion(value)),
    }
}

#[cfg(all(target_arch = "riscv32", feature = "strict-no-wait"))]
pub(crate) unsafe fn dispatch_channel() {
    if OP_STATE
        .compare_exchange(OP_QUEUED, OP_RUNNING, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        OP_RESULT.store(1, Ordering::Release);
        OP_STATE.store(OP_IDLE, Ordering::Release);
        OP_SIGNAL.notify_from_isr();
        return;
    }
    let channel = [OP_CHANNEL.load(Ordering::Acquire), 0];
    let dwell = OP_DWELL_MS.load(Ordering::Acquire);
    if dwell != 0 && channel[0] == 1 {
        enable_scan_rx_policy();
    }
    let (start, end) = if dwell == 0 {
        (Some(channel_complete as unsafe extern "C" fn(_, _)), None)
    } else {
        (None, Some(channel_complete as unsafe extern "C" fn(_, _)))
    };
    let result = chm_start_op(
        channel.as_ptr(),
        dwell,
        dwell,
        start,
        end,
        core::ptr::null_mut(),
    );
    if result != 0 {
        restore_default_rx_policy();
        OP_RESULT.store(0x8000_0000 | result as u32, Ordering::Release);
        OP_STATE.store(OP_IDLE, Ordering::Release);
        OP_SIGNAL.notify_from_isr();
    }
}

#[cfg(all(target_arch = "riscv32", feature = "strict-no-wait"))]
pub(crate) unsafe extern "C" fn channel_complete(_context: *mut core::ffi::c_void, result: u32) {
    let tune = OP_DWELL_MS.load(Ordering::Acquire) == 0;
    let result = if result == 0 && tune {
        crate::channel_switch::make_current_channel_home()
            .err()
            .map_or(0, |error| error as u32)
    } else {
        result
    };
    if tune
        || result != 0
        || OP_CHANNEL.load(Ordering::Acquire) == 13
        || SESSION.load(Ordering::Acquire) != SESSION_ACTIVE
    {
        restore_default_rx_policy();
    }
    OP_RESULT.store(result, Ordering::Release);
    OP_STATE.store(OP_IDLE, Ordering::Release);
    OP_SIGNAL.notify_from_isr();
}

#[cfg(all(target_arch = "riscv32", feature = "strict-no-wait"))]
pub(crate) fn channel_switch_failed(error: u32) {
    if OP_STATE
        .compare_exchange(OP_RUNNING, OP_IDLE, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
    {
        unsafe { restore_default_rx_policy() };
        OP_RESULT.store(error, Ordering::Release);
        OP_SIGNAL.notify_from_isr();
    }
}

#[cfg(not(all(target_arch = "riscv32", feature = "strict-no-wait")))]
pub(crate) fn channel_switch_failed(_error: u32) {}

#[cfg(all(target_arch = "riscv32", feature = "strict-no-wait"))]
unsafe fn enable_scan_rx_policy() {
    // Exact policy-3 branch of the pinned `wifi_set_rx_policy` jump table.
    // Calling the three finite leaves directly removes the unproven indirect
    // dispatch while preserving management/control reception off-channel.
    ic_set_rx_policy(0, 2, 1, 1);
    ic_set_rx_policy_ubssid_check(0, 0);
    core::ptr::addr_of_mut!(g_ic).add(716).write(3);
}

#[cfg(all(target_arch = "riscv32", feature = "strict-no-wait"))]
pub(crate) unsafe fn enable_sta_link_rx_policy() {
    // Exact policy-5 branch of the pinned `wifi_set_rx_policy` jump table.
    // `cnx_connect_to_bss` installs it before sending Authentication. Keep the
    // finite leaves here so the vendor jump table and its unrelated modes can
    // never enter the strict runtime.
    ic_set_rx_policy(0, 0, 1, 1);
    ic_set_rx_policy_ubssid_check(0, 0);
    core::ptr::addr_of_mut!(g_ic).add(716).write(5);
}

#[cfg(all(target_arch = "riscv32", feature = "strict-no-wait"))]
unsafe fn restore_default_rx_policy() {
    // Exact policy-0 branch. Both addresses belong to the pinned `g_ic`
    // object and the called leaves audit without heap, waits, or cycles.
    let ic = core::ptr::addr_of_mut!(g_ic);
    ic_set_mac(0, ic.add(0x21a));
    ic_set_mac(1, ic.add(0x214));
    ic_set_rx_policy(0, 0, 0, 0);
    ic_set_rx_policy(1, 0, 0, 0);
    ic_set_rx_policy_ubssid_check(0, 0);
    ic.add(716).write(0);
}

#[cfg(all(target_arch = "riscv32", feature = "strict-no-wait"))]
pub(crate) fn observe_management(frame: &[u8], rssi: i8) {
    if SESSION.load(Ordering::Acquire) != SESSION_ACTIVE {
        return;
    }
    let fallback_channel = CURRENT_CHANNEL.load(Ordering::Acquire);
    let Some(record) = parse_management(frame, fallback_channel, rssi) else {
        return;
    };
    OBSERVED_FRAMES.fetch_add(1, Ordering::Relaxed);

    let length = usize::from(TABLE_LEN.load(Ordering::Acquire));
    let records = unsafe { &mut *TABLE.0.get() };
    for existing in &mut records[..length] {
        if existing.bssid == record.bssid {
            if record.rssi > existing.rssi || existing.ssid_len == 0 {
                *existing = record;
            }
            return;
        }
    }
    if length == STRICT_SCAN_RECORD_CAPACITY {
        DROPPED_UNIQUE_BSS.fetch_add(1, Ordering::Relaxed);
        return;
    }
    records[length] = record;
    TABLE_LEN.store((length + 1) as u8, Ordering::Release);
}

#[cfg(any(test, all(target_arch = "riscv32", feature = "strict-no-wait")))]
fn parse_management(frame: &[u8], fallback_channel: u8, rssi: i8) -> Option<StrictScanRecord> {
    if frame.len() < 36 {
        return None;
    }
    let frame_control = u16::from_le_bytes([frame[0], frame[1]]);
    let subtype = (frame_control >> 4) & 0x0f;
    if frame_control & 0x000c != 0 || !matches!(subtype, 5 | 8) {
        return None;
    }

    let mut record = StrictScanRecord::EMPTY;
    record.bssid.copy_from_slice(&frame[16..22]);
    record.channel = fallback_channel;
    record.rssi = rssi;
    record.beacon_interval_tu = u16::from_le_bytes([frame[32], frame[33]]);
    record.capability_info = u16::from_le_bytes([frame[34], frame[35]]);
    record.privacy = record.capability_info & 0x0010 != 0;

    let mut offset = 36;
    while offset + 2 <= frame.len() {
        let id = frame[offset];
        let length = usize::from(frame[offset + 1]);
        offset += 2;
        let Some(end) = offset.checked_add(length) else {
            record.information_elements_truncated = true;
            break;
        };
        if end > frame.len() {
            // Keep the parser valid for any deliberately bounded capture.
            // BSSID, capabilities, and all preceding complete IEs remain
            // trustworthy even when a later element is truncated.
            record.information_elements_truncated = true;
            break;
        }
        let value = &frame[offset..end];
        match id {
            0 if length <= record.ssid.len() => {
                record.ssid[..length].copy_from_slice(value);
                record.ssid_len = length as u8;
            }
            3 if length == 1 => record.channel = value[0],
            1 if length <= record.supported_rates.len() => {
                record.supported_rates[..length].copy_from_slice(value);
                record.supported_rates_len = length as u8;
            }
            48 => {
                record.rsn = true;
                let total = length + 2;
                if total <= record.rsn_ie.len() {
                    record.rsn_ie[..total].copy_from_slice(&frame[offset - 2..end]);
                    record.rsn_ie_len = total as u8;
                } else {
                    record.information_elements_truncated = true;
                }
            }
            50 => {
                let copied = length.min(record.extended_supported_rates.len());
                record.extended_supported_rates[..copied].copy_from_slice(&value[..copied]);
                record.extended_supported_rates_len = copied as u8;
                record.information_elements_truncated |= copied != length;
            }
            45 if length + 2 == STRICT_SCAN_HT_CAPABILITY_IE_LEN => {
                record
                    .ht_capability_ie
                    .copy_from_slice(&frame[offset - 2..end]);
                record.ht_capability_ie_present = true;
            }
            61 if length + 2 == STRICT_SCAN_HT_OPERATION_IE_LEN => {
                record
                    .ht_operation_ie
                    .copy_from_slice(&frame[offset - 2..end]);
                record.ht_operation_ie_present = true;
            }
            255 if value.first().copied() == Some(crate::he::HE_CAPABILITIES_EXTENSION_ID) => {
                let total = length + 2;
                if total <= record.he_capability_ie.len() {
                    record.he_capability_ie[..total].copy_from_slice(&frame[offset - 2..end]);
                    record.he_capability_ie_len = total as u8;
                } else {
                    record.information_elements_truncated = true;
                }
            }
            255 if value.first().copied() == Some(crate::he::HE_OPERATION_EXTENSION_ID) => {
                let total = length + 2;
                if total <= record.he_operation_ie.len() {
                    record.he_operation_ie[..total].copy_from_slice(&frame[offset - 2..end]);
                    record.he_operation_ie_len = total as u8;
                } else {
                    record.information_elements_truncated = true;
                }
            }
            244 => {
                let total = length + 2;
                if total <= record.rsnxe.len() {
                    record.rsnxe[..total].copy_from_slice(&frame[offset - 2..end]);
                    record.rsnxe_len = total as u8;
                } else {
                    record.information_elements_truncated = true;
                }
            }
            221 if length >= 4 && value[..4] == [0x00, 0x50, 0xf2, 0x01] => {
                record.legacy_wpa = true;
            }
            221 if length >= 6 && value[..4] == [0x00, 0x50, 0xf2, 0x02] => {
                let total = length + 2;
                if total <= record.wmm_ie.len() {
                    record.wmm_ie[..total].copy_from_slice(&frame[offset - 2..end]);
                    record.wmm_ie_len = total as u8;
                } else {
                    record.information_elements_truncated = true;
                }
            }
            _ => {}
        }
        offset = end;
    }
    Some(record)
}

#[cfg(test)]
mod tests {
    use super::{best_matching_ssid, parse_management, StrictScanRecord};
    use crate::he::{parse_he20_capabilities, parse_he20_operation};

    #[test]
    fn parses_beacon_into_owned_bounded_record() {
        let mut frame = [0_u8; 64];
        frame[0] = 0x80;
        frame[16..22].copy_from_slice(&[1, 2, 3, 4, 5, 6]);
        frame[34] = 0x10;
        frame[36..42].copy_from_slice(&[0, 4, b't', b'e', b's', b't']);
        frame[42..45].copy_from_slice(&[3, 1, 11]);
        frame[45..47].copy_from_slice(&[48, 0]);
        let record = parse_management(&frame[..47], 3, -42).unwrap();
        assert_eq!(record.ssid_bytes(), b"test");
        assert_eq!(record.bssid, [1, 2, 3, 4, 5, 6]);
        assert_eq!(record.channel, 11);
        assert_eq!(record.rssi, -42);
        assert!(record.privacy);
        assert!(record.rsn);
        assert_eq!(record.rsn_ie_bytes(), &[48, 0]);
        assert_eq!(record.capability_info, 0x10);
    }

    #[test]
    fn rejects_data_and_keeps_bounded_prefix_of_truncated_information_elements() {
        let mut frame = [0_u8; 40];
        frame[0] = 0x08;
        assert!(parse_management(&frame, 1, -1).is_none());
        frame[0] = 0x50;
        frame[36] = 0;
        frame[37] = 8;
        let record = parse_management(&frame, 1, -1).unwrap();
        assert!(record.information_elements_truncated);
    }

    #[test]
    fn owns_complete_ht_capability_and_operation_elements() {
        let mut frame = [0_u8; 88];
        frame[0] = 0x80;
        frame[16..22].copy_from_slice(&[1, 2, 3, 4, 5, 6]);
        frame[36] = 45;
        frame[37] = 26;
        frame[38..64].fill(0xa5);
        frame[64] = 61;
        frame[65] = 22;
        frame[66..88].fill(0x5a);

        let record = parse_management(&frame, 6, -20).unwrap();
        assert_eq!(record.ht_capability_ie_bytes().unwrap()[..2], [45, 26]);
        assert_eq!(record.ht_capability_ie_bytes().unwrap()[2..], [0xa5; 26]);
        assert_eq!(record.ht_operation_ie_bytes().unwrap()[..2], [61, 22]);
        assert_eq!(record.ht_operation_ie_bytes().unwrap()[2..], [0x5a; 22]);
    }

    #[test]
    fn owns_wmm_information_element_separately_from_legacy_wpa() {
        let mut frame = [0_u8; 45];
        frame[0] = 0x80;
        frame[36..45].copy_from_slice(&[221, 7, 0x00, 0x50, 0xf2, 0x02, 0, 1, 0]);

        let record = parse_management(&frame, 6, -20).unwrap();
        assert_eq!(
            record.wmm_ie_bytes(),
            &[221, 7, 0x00, 0x50, 0xf2, 0x02, 0, 1, 0]
        );
        assert!(!record.legacy_wpa);
    }

    #[test]
    fn owns_and_parses_bounded_he20_extension_elements() {
        let mut frame = [0_u8; 69];
        frame[0] = 0x80;
        frame[36..60].fill(0);
        frame[36..39].copy_from_slice(&[255, 22, 35]);
        frame[56..58].copy_from_slice(&0xfffd_u16.to_le_bytes());
        frame[58..60].copy_from_slice(&0xfffd_u16.to_le_bytes());
        frame[60..69].copy_from_slice(&[255, 7, 36, 0, 0, 0, 0xc5, 0xfd, 0xff]);

        let record = parse_management(&frame, 6, -20).unwrap();
        let capability = parse_he20_capabilities(record.he_capability_ie_bytes()).unwrap();
        let operation = parse_he20_operation(record.he_operation_ie_bytes()).unwrap();
        assert!(capability.supports_bidirectional_mcs9());
        assert_eq!(operation.bss_color, 5);
        assert_eq!(operation.basic_mcs_nss_map, 0xfffd);
    }

    #[test]
    fn strongest_exact_ssid_is_selected_without_hidden_or_invalid_entries() {
        let mut records = [StrictScanRecord::EMPTY; 4];
        for (record, rssi, channel) in [(-70, 1), (-25, 6), (-10, 0)]
            .into_iter()
            .zip(&mut records)
            .map(|((rssi, channel), record)| (record, rssi, channel))
        {
            record.ssid[..4].copy_from_slice(b"test");
            record.ssid_len = 4;
            record.rssi = rssi;
            record.channel = channel;
        }
        records[3].ssid[..5].copy_from_slice(b"other");
        records[3].ssid_len = 5;
        records[3].rssi = -1;
        records[3].channel = 11;
        assert_eq!(best_matching_ssid(&records, b"test").unwrap().channel, 6);
        assert!(best_matching_ssid(&records, b"missing").is_none());
    }
}
