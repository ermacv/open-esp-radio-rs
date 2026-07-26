use core::{
    cell::UnsafeCell,
    ffi::c_void,
    ptr,
    sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering},
};

#[cfg(feature = "hil-vendor-tx")]
use core::sync::atomic::AtomicU8;

use esp_wifi_sys_esp32s31::include::wifi_osi_funcs_t;

use crate::{
    event::PpEvent,
    rate_control::{
        beamforming_report_rate, RateControlState, RateScheduleState, ScheduleSelection,
    },
    rate_schedule::{schedule_from_pointer, schedule_pointer, schedule_state},
};

#[cfg(all(target_arch = "riscv32", feature = "hil-vendor-tx"))]
unsafe extern "C" {
    fn ets_printf(format: *const u8, ...) -> i32;
}

pub(crate) const TX_DONE_CONTINUATION: u32 = u32::MAX - 2;
pub(crate) const LMAC_TX_DONE_CONTINUATION: u32 = u32::MAX - 3;

const TX_DONE_HEAD_OFFSET: usize = 0x38c;
const TX_DONE_TAIL_LINK_OFFSET: usize = 0x390;
const TX_CALLBACK_MODE0_MASK_OFFSET: usize = 0x39c;
const TX_CALLBACK_MODE1_MASK_OFFSET: usize = 0x410;
const TX_CALLBACK_TABLE_FIRST_OFFSET: usize = (0xe8 + 1) * 4;
const FRAME_NEXT_OFFSET: usize = 0x30;
const FRAME_DESCRIPTOR_OFFSET: usize = 0x34;
const FRAME_TYPE_OFFSET: usize = 0x1a;
const DESCRIPTOR_CALLBACK_MASK_OFFSET: usize = 0x14;
const DESCRIPTOR_PERSISTENT_BIT: u32 = 0x0080_0000;
const DESCRIPTOR_DIRECT_RECYCLE_BIT: u32 = 0x0400_0000;
const DESCRIPTOR_RATE_CONTROL_BIT: u32 = 0x0000_0008;
const DESCRIPTOR_RATE_CONTROL_SKIP_MASK: u32 = 0x4040_4000;
const DESCRIPTOR_RATE_CONTROL_SKIP_VALUE: u32 = 0x0040_0000;
// `libpp.a[wdev.o]::.data.wDevCtrl` byte 0x2e is 0x60. Archive-wide
// relocation inspection finds readers but no writer. It converts the encoded
// descriptor ACK-SNR byte into the signed value consumed by rate control.
const ACK_SNR_ENCODING_OFFSET: u8 = 0x60;
const RATE_RETRY_PRESSURE_OFFSET: usize = 0x07;
const RATE_MAXIMUM_SCHEDULE_INDEX_OFFSET: usize = 0x05;
const RATE_WEIGHTED_RETRIES_OFFSET: usize = 0x2c;
const RATE_TRANSMISSIONS_OFFSET: usize = 0x30;
const RATE_LAST_MAC_TIME_OFFSET: usize = 0x34;
const RATE_COMPLETED_OFFSET: usize = 0x40;
const RATE_REEVALUATE_AFTER_OFFSET: usize = 0x60;
const RATE_CURRENT_SCHEDULE_OFFSET: usize = 0x64;
const RATE_LEGACY_SCHEDULE_OFFSET: usize = 0x74;
const RATE_RETRY_STATE_1D_OFFSET: usize = 0x1d;
const RATE_RETRY_STATE_1E_OFFSET: usize = 0x1e;
const RATE_HE_FEATURE_8F_OFFSET: usize = 0x8f;
const RATE_HE_FEATURE_90_OFFSET: usize = 0x90;
const MAC_TIME_LOW_REGISTER: *const u32 = 0x2010_d800 as *const u32;
const PHY_NOISE_FLOOR_REGISTER: *const u32 = 0x2010_708c as *const u32;
const HE_BF_REPORT_RATE_REGISTER: *mut u32 = 0x2010_4464 as *mut u32;
const HE_ERSU_ACK_RATE_REGISTER: *mut u32 = 0x2010_4404 as *mut u32;

const CALLBACK_MGMT: u8 = 2;
const CALLBACK_STA_EAPOL: u8 = 3;
const CALLBACK_AP_BEACON: u8 = 4;
const CALLBACK_AP_DATA: u8 = 11;
const CALLBACK_AP_POWER_SAVE: u8 = 12;
const CALLBACK_ADDBA_RESPONSE: u8 = 13;
const BASIC_MODE0_CALLBACKS: u32 = (1 << CALLBACK_MGMT)
    | (1 << CALLBACK_AP_BEACON)
    | (1 << CALLBACK_AP_DATA)
    | (1 << CALLBACK_AP_POWER_SAVE);
const BASIC_MODE1_CALLBACKS: u32 = (1 << CALLBACK_STA_EAPOL) | (1 << CALLBACK_ADDBA_RESPONSE);
const SUPPORTED_CALLBACK_BITS: [u8; 6] = [
    CALLBACK_MGMT,
    CALLBACK_STA_EAPOL,
    CALLBACK_AP_BEACON,
    CALLBACK_AP_DATA,
    CALLBACK_AP_POWER_SAVE,
    CALLBACK_ADDBA_RESPONSE,
];

const PHASE_IDLE: u8 = 0;
const PHASE_LOAD: u8 = 1;
const PHASE_CALLBACK: u8 = 2;
const PHASE_RECYCLE: u8 = 3;
// Ordinary data has no mode-0 callbacks. Drain a finite prefix in one radio
// executor dispatch so ownership return does not require two wakeups per
// MPDU. Callback-bearing management/EAPOL frames still stop at the callback
// boundary and retain the one-callback-per-event contract.
const CALLBACK_FREE_RECYCLE_QUANTUM: usize = 4;

#[cfg(feature = "hil-vendor-tx")]
static HIL_EAPOL_TXDONE_COUNT: AtomicUsize = AtomicUsize::new(0);
#[cfg(feature = "hil-vendor-tx")]
static HIL_EAPOL_FRAME_CONTROL: AtomicUsize = AtomicUsize::new(0);
#[cfg(feature = "hil-vendor-tx")]
static HIL_EAPOL_QOS_CONTROL: AtomicUsize = AtomicUsize::new(0);
#[cfg(feature = "hil-vendor-tx")]
static HIL_EAPOL_HW_STATUS: AtomicUsize = AtomicUsize::new(0);
#[cfg(feature = "hil-vendor-tx")]
static HIL_EAPOL_DESCRIPTOR_STATUS: AtomicUsize = AtomicUsize::new(0);
#[cfg(feature = "hil-vendor-tx")]
static HIL_DATA_TXDONE_COUNT: AtomicUsize = AtomicUsize::new(0);
#[cfg(feature = "hil-vendor-tx")]
static HIL_DATA_FRAME_CONTROL: AtomicUsize = AtomicUsize::new(0);
#[cfg(feature = "hil-vendor-tx")]
static HIL_DATA_HW_STATUS: AtomicUsize = AtomicUsize::new(0);
#[cfg(feature = "hil-vendor-tx")]
static HIL_DATA_DESCRIPTOR_STATUS: AtomicUsize = AtomicUsize::new(0);
#[cfg(feature = "hil-vendor-tx")]
static HIL_DATA_TRANSMITTER: [AtomicU8; 6] = [const { AtomicU8::new(0) }; 6];
#[cfg(feature = "hil-vendor-tx")]
static HIL_DATA_CCMP_HEADER: [AtomicU8; 8] = [const { AtomicU8::new(0) }; 8];
#[cfg(feature = "hil-vendor-tx")]
static HIL_DATA_PAYLOAD_PREFIX: [AtomicU8; 8] = [const { AtomicU8::new(0) }; 8];

#[cfg(feature = "hil-vendor-tx")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HilEapolTxDoneSnapshot {
    pub count: usize,
    pub frame_control: u16,
    pub qos_control: u16,
    pub hardware_status: u8,
    pub descriptor_status: u32,
}

#[cfg(feature = "hil-vendor-tx")]
pub fn hil_eapol_tx_done_snapshot() -> HilEapolTxDoneSnapshot {
    HilEapolTxDoneSnapshot {
        count: HIL_EAPOL_TXDONE_COUNT.load(Ordering::Acquire),
        frame_control: HIL_EAPOL_FRAME_CONTROL.load(Ordering::Acquire) as u16,
        qos_control: HIL_EAPOL_QOS_CONTROL.load(Ordering::Acquire) as u16,
        hardware_status: HIL_EAPOL_HW_STATUS.load(Ordering::Acquire) as u8,
        descriptor_status: HIL_EAPOL_DESCRIPTOR_STATUS.load(Ordering::Acquire) as u32,
    }
}

#[cfg(feature = "hil-vendor-tx")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HilDataTxDoneSnapshot {
    pub count: usize,
    pub frame_control: u16,
    pub qos_control: u16,
    pub hardware_status: u8,
    pub descriptor_status: u32,
    pub receiver: [u8; 6],
    pub transmitter: [u8; 6],
    pub address3: [u8; 6],
    pub sequence_control: u16,
    pub header_len: u16,
    pub remaining_len: u16,
    pub layout: u16,
    pub buffer_flags: u32,
    pub descriptor_flags: u32,
    pub ccmp_header: [u8; 8],
    pub payload_prefix: [u8; 8],
}

#[cfg(feature = "hil-vendor-tx")]
pub fn hil_data_tx_done_snapshot() -> HilDataTxDoneSnapshot {
    let count = HIL_DATA_TXDONE_COUNT.load(Ordering::Acquire);
    HilDataTxDoneSnapshot {
        count,
        frame_control: HIL_DATA_FRAME_CONTROL.load(Ordering::Acquire) as u16,
        qos_control: HIL_DATA_QOS_CONTROL.load(Ordering::Acquire) as u16,
        hardware_status: HIL_DATA_HW_STATUS.load(Ordering::Acquire) as u8,
        descriptor_status: HIL_DATA_DESCRIPTOR_STATUS.load(Ordering::Acquire) as u32,
        receiver: load_hil_bytes(&HIL_DATA_RECEIVER),
        transmitter: load_hil_bytes(&HIL_DATA_TRANSMITTER),
        address3: load_hil_bytes(&HIL_DATA_ADDRESS3),
        sequence_control: HIL_DATA_SEQUENCE_CONTROL.load(Ordering::Acquire) as u16,
        header_len: HIL_DATA_HEADER_LEN.load(Ordering::Acquire) as u16,
        remaining_len: HIL_DATA_REMAINING_LEN.load(Ordering::Acquire) as u16,
        layout: HIL_DATA_LAYOUT.load(Ordering::Acquire) as u16,
        buffer_flags: HIL_DATA_BUFFER_FLAGS.load(Ordering::Acquire) as u32,
        descriptor_flags: HIL_DATA_DESCRIPTOR_FLAGS.load(Ordering::Acquire) as u32,
        ccmp_header: load_hil_bytes(&HIL_DATA_CCMP_HEADER),
        payload_prefix: load_hil_bytes(&HIL_DATA_PAYLOAD_PREFIX),
    }
}

#[cfg(feature = "hil-vendor-tx")]
static HIL_DATA_QOS_CONTROL: AtomicUsize = AtomicUsize::new(0);
#[cfg(feature = "hil-vendor-tx")]
static HIL_DATA_RECEIVER: [AtomicU8; 6] = [const { AtomicU8::new(0) }; 6];
#[cfg(feature = "hil-vendor-tx")]
static HIL_DATA_ADDRESS3: [AtomicU8; 6] = [const { AtomicU8::new(0) }; 6];
#[cfg(feature = "hil-vendor-tx")]
static HIL_DATA_SEQUENCE_CONTROL: AtomicUsize = AtomicUsize::new(0);
#[cfg(feature = "hil-vendor-tx")]
static HIL_DATA_HEADER_LEN: AtomicUsize = AtomicUsize::new(0);
#[cfg(feature = "hil-vendor-tx")]
static HIL_DATA_REMAINING_LEN: AtomicUsize = AtomicUsize::new(0);
#[cfg(feature = "hil-vendor-tx")]
static HIL_DATA_LAYOUT: AtomicUsize = AtomicUsize::new(0);
#[cfg(feature = "hil-vendor-tx")]
static HIL_DATA_BUFFER_FLAGS: AtomicUsize = AtomicUsize::new(0);
#[cfg(feature = "hil-vendor-tx")]
static HIL_DATA_DESCRIPTOR_FLAGS: AtomicUsize = AtomicUsize::new(0);

#[cfg(feature = "hil-vendor-tx")]
fn load_hil_bytes<const N: usize>(source: &[AtomicU8; N]) -> [u8; N] {
    let mut bytes = [0; N];
    let mut index = 0;
    while index < N {
        bytes[index] = source[index].load(Ordering::Acquire);
        index += 1;
    }
    bytes
}

type TxCallback = unsafe extern "C" fn(*mut c_void);

unsafe extern "C" {
    static mut pTxRx: *mut u8;
    static mut our_instances_ptr: *mut u8;
    static mut g_tx_done_cb_func: usize;
    static g_wifi_menuconfig: u8;
    static mut g_ic: u8;
    static mut g_osi_funcs_p: *const wifi_osi_funcs_t;
    static TmpSTAAPCloseAP: u8;
    #[link_name = "__esp_s31_beacon_send_start_flag"]
    static mut BEACON_SEND_START_FLAG: u8;
    #[link_name = "__esp_s31_beacon_timer"]
    static mut BEACON_TIMER: [u8; 0x14];
    #[link_name = "__esp_s31_beacon_next_tbtt"]
    static mut BEACON_NEXT_TBTT: u32;
    #[link_name = "__esp_s31_beacon_dtim_send_mc"]
    static BEACON_DTIM_SEND_MC: u8;
    static mut BcnSendTick: u32;
    static BcnInterval: u32;

    fn sta_eapol_txdone_cb(frame: *mut c_void);
    #[link_name = "ieee80211_tx_mgt_cb"]
    fn vendor_tx_mgt_cb(frame: *mut c_void);
    #[link_name = "ieee80211_hostapd_beacon_txcb"]
    fn vendor_hostapd_beacon_txcb(frame: *mut c_void);
    #[cfg(target_arch = "riscv32")]
    #[link_name = "__real_ieee80211_hostapd_beacon_txcb"]
    fn initialization_hostapd_beacon_txcb(frame: *mut c_void);
    fn ieee80211_hostapd_ps_txcb(frame: *mut c_void);
    #[link_name = "__esp_s31_addba_response_txcb"]
    fn addba_response_txcb(frame: *mut c_void);
    #[link_name = "ic_get_next_tbtt"]
    fn vendor_ic_get_next_tbtt() -> u32;
    #[cfg(not(feature = "strict-no-wait"))]
    fn pp_coex_tx_release(frame: *mut c_void);
    fn esf_buf_recycle(frame: *mut c_void);
    #[link_name = "rcUpdateTxDone"]
    fn vendor_rc_update_tx_done(rate_control: *mut c_void, descriptor: *mut c_void);
    fn pp_post(kind: u32, argument: *mut c_void) -> i32;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TxDoneError {
    PreviousFailure,
    TxRxUnavailable,
    InternalQueueFull,
    MissingDescriptor,
    MissingTxDoneTail,
    UnsupportedCallbackBits(u32),
    UnexpectedAmpduCallbacks(u32),
    UnexpectedBeaconCallbacks(u32),
    InvalidBeaconFrame,
    CallbackRegistryMismatch(u8),
    UserCallbackInstalled,
    UnsupportedDescriptorFlags(u32),
    UnsupportedPersistentLayout(u32, u32, u16, u16, u16, u16, u16),
    NonStaticFrameType(u8),
    LmacPipelineBusy,
    UnsupportedLmacDescriptorFlags(u32),
    UnsupportedLmacMode(u32),
    InstancesUnavailable,
    TxopQueue(crate::tx_queue::TxopQueueError),
    TxTimeRecordingEnabled,
    InvalidPhase,
    StrictCallbackFailed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TxDoneStateAdoptionError {
    TxRxUnavailable,
    QueueNotEmpty,
    InvalidEmptyTailLink,
}

struct StrictTxDoneRegistry {
    head: *mut u8,
    tail: *mut u8,
    mode0_mask: u32,
    mode1_mask: u32,
    callbacks: [usize; SUPPORTED_CALLBACK_BITS.len()],
}

impl StrictTxDoneRegistry {
    const fn empty() -> Self {
        Self {
            head: ptr::null_mut(),
            tail: ptr::null_mut(),
            mode0_mask: 0,
            mode1_mask: 0,
            callbacks: [0; SUPPORTED_CALLBACK_BITS.len()],
        }
    }

    unsafe fn append(&mut self, frame: *mut u8) -> bool {
        if frame.is_null() {
            return false;
        }
        frame
            .add(FRAME_NEXT_OFFSET)
            .cast::<*mut u8>()
            .write(ptr::null_mut());
        if self.tail.is_null() {
            if !self.head.is_null() {
                return false;
            }
            self.head = frame;
        } else {
            self.tail
                .add(FRAME_NEXT_OFFSET)
                .cast::<*mut u8>()
                .write(frame);
        }
        self.tail = frame;
        true
    }

    unsafe fn pop_front(&mut self) -> *mut u8 {
        let frame = self.head;
        if frame.is_null() {
            return frame;
        }
        self.head = frame.add(FRAME_NEXT_OFFSET).cast::<*mut u8>().read();
        if self.head.is_null() {
            self.tail = ptr::null_mut();
        }
        frame
            .add(FRAME_NEXT_OFFSET)
            .cast::<*mut u8>()
            .write(ptr::null_mut());
        frame
    }

    fn callback(&self, bit: u8) -> Option<usize> {
        supported_callback_index(bit).map(|index| self.callbacks[index])
    }
}

struct StrictTxDoneRegistryCell(UnsafeCell<StrictTxDoneRegistry>);

// Handoff publishes this state once; afterwards only the strict Wi-Fi hart
// mutates the intrusive queue or reads its adopted callback policy.
unsafe impl Sync for StrictTxDoneRegistryCell {}

#[cfg_attr(
    target_arch = "riscv32",
    link_section = ".critical.bss.wifi_strict.tx_done_registry"
)]
static STRICT_TX_DONE_REGISTRY: StrictTxDoneRegistryCell =
    StrictTxDoneRegistryCell(UnsafeCell::new(StrictTxDoneRegistry::empty()));
static STRICT_TX_DONE_REGISTRY_ADOPTED: AtomicBool = AtomicBool::new(false);

const fn supported_callback_index(bit: u8) -> Option<usize> {
    let mut index = 0;
    while index < SUPPORTED_CALLBACK_BITS.len() {
        if SUPPORTED_CALLBACK_BITS[index] == bit {
            return Some(index);
        }
        index += 1;
    }
    None
}

/// Adopt the initialized TX-done policy without retaining the mixed `pTxRx`
/// object as runtime storage.
///
/// The ownership edge is fail-closed: the vendor completion list must be
/// empty and its tail link must point at its own head slot. Only the two masks
/// and six callback addresses admitted by the strict profile are copied.
///
/// # Safety
///
/// Wi-Fi initialization must be quiescent. No TX completion producer may run
/// until this function returns and the strict radio owner is armed.
#[cfg(target_arch = "riscv32")]
pub(crate) unsafe fn adopt_vendor_tx_done_state() -> Result<(), TxDoneStateAdoptionError> {
    if STRICT_TX_DONE_REGISTRY_ADOPTED.load(Ordering::Acquire) {
        return Ok(());
    }
    let txrx = ptr::addr_of!(pTxRx).read();
    if txrx.is_null() {
        return Err(TxDoneStateAdoptionError::TxRxUnavailable);
    }
    let head_slot = txrx.add(TX_DONE_HEAD_OFFSET).cast::<*mut u8>();
    if !head_slot.read().is_null() {
        return Err(TxDoneStateAdoptionError::QueueNotEmpty);
    }
    let tail_link = txrx
        .add(TX_DONE_TAIL_LINK_OFFSET)
        .cast::<*mut *mut u8>()
        .read();
    if tail_link != head_slot {
        return Err(TxDoneStateAdoptionError::InvalidEmptyTailLink);
    }

    let state = &mut *STRICT_TX_DONE_REGISTRY.0.get();
    state.head = ptr::null_mut();
    state.tail = ptr::null_mut();
    state.mode0_mask = txrx.add(TX_CALLBACK_MODE0_MASK_OFFSET).cast::<u32>().read();
    state.mode1_mask = txrx.add(TX_CALLBACK_MODE1_MASK_OFFSET).cast::<u32>().read();
    let mut index = 0;
    while index < SUPPORTED_CALLBACK_BITS.len() {
        let bit = SUPPORTED_CALLBACK_BITS[index];
        state.callbacks[index] = txrx
            .add(TX_CALLBACK_TABLE_FIRST_OFFSET + usize::from(bit) * 4)
            .cast::<usize>()
            .read();
        index += 1;
    }
    STRICT_TX_DONE_REGISTRY_ADOPTED.store(true, Ordering::Release);
    Ok(())
}

#[inline(always)]
unsafe fn tx_done_registry() -> Result<&'static mut StrictTxDoneRegistry, TxDoneError> {
    if !STRICT_TX_DONE_REGISTRY_ADOPTED.load(Ordering::Acquire) {
        return Err(TxDoneError::TxRxUnavailable);
    }
    Ok(&mut *STRICT_TX_DONE_REGISTRY.0.get())
}

#[derive(Clone, Copy)]
struct TxDoneState {
    active: bool,
    failed: bool,
    phase: u8,
    frame: *mut u8,
    callbacks: u32,
    resume_timeout: bool,
    resume_queue: bool,
    resume_event: u8,
    resume_intercept: bool,
}

impl TxDoneState {
    const fn new() -> Self {
        Self {
            active: false,
            failed: false,
            phase: PHASE_IDLE,
            frame: ptr::null_mut(),
            callbacks: 0,
            resume_timeout: false,
            resume_queue: false,
            resume_event: 0,
            resume_intercept: false,
        }
    }
}

struct StateCell(UnsafeCell<TxDoneState>);

unsafe impl Sync for StateCell {}

#[cfg_attr(
    target_arch = "riscv32",
    link_section = ".critical.bss.wifi_strict.tx_done_state"
)]
static STATE: StateCell = StateCell(UnsafeCell::new(TxDoneState::new()));
#[cfg_attr(
    target_arch = "riscv32",
    link_section = ".critical.bss.wifi_strict.lmac_tx_done_state"
)]
static LMAC_STATE: StateCell = StateCell(UnsafeCell::new(TxDoneState::new()));
static STRICT_CALLBACK_FAILED: AtomicBool = AtomicBool::new(false);
static STRICT_MANAGEMENT_TX_DONE_ACCEPTED: AtomicUsize = AtomicUsize::new(0);
static STRICT_MANAGEMENT_TX_DONE_REJECTED: AtomicUsize = AtomicUsize::new(0);
static STRICT_MANAGEMENT_TX_DONE_SUBTYPES: [AtomicUsize; 16] = [const { AtomicUsize::new(0) }; 16];
static STRICT_MANAGEMENT_TX_DONE_LAST_FRAME_CONTROL: AtomicU32 = AtomicU32::new(0);
static STRICT_MANAGEMENT_TX_DONE_LAST_DESCRIPTOR_FLAGS: AtomicU32 = AtomicU32::new(0);
static STRICT_MANAGEMENT_TX_DONE_LAST_DESCRIPTOR_SECURITY: AtomicU32 = AtomicU32::new(0);

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct StrictManagementTxDoneSnapshot {
    pub accepted: usize,
    pub rejected: usize,
    pub subtypes: [usize; 16],
    pub last_frame_control: u16,
    pub last_descriptor_flags: u32,
    pub last_descriptor_security: u32,
}

pub fn strict_management_tx_done_snapshot() -> StrictManagementTxDoneSnapshot {
    StrictManagementTxDoneSnapshot {
        accepted: STRICT_MANAGEMENT_TX_DONE_ACCEPTED.load(Ordering::Acquire),
        rejected: STRICT_MANAGEMENT_TX_DONE_REJECTED.load(Ordering::Acquire),
        subtypes: core::array::from_fn(|index| {
            STRICT_MANAGEMENT_TX_DONE_SUBTYPES[index].load(Ordering::Acquire)
        }),
        last_frame_control: STRICT_MANAGEMENT_TX_DONE_LAST_FRAME_CONTROL.load(Ordering::Acquire)
            as u16,
        last_descriptor_flags: STRICT_MANAGEMENT_TX_DONE_LAST_DESCRIPTOR_FLAGS
            .load(Ordering::Acquire),
        last_descriptor_security: STRICT_MANAGEMENT_TX_DONE_LAST_DESCRIPTOR_SECURITY
            .load(Ordering::Acquire),
    }
}

const fn is_ap_deauthentication_completion(frame_control: u16, descriptor_security: u32) -> bool {
    frame_control & 0x00fc == 0x00c0 && descriptor_security & 0x0500_0000 != 0
}

const fn is_ap_addba_response_completion_layout(
    frame_control: u16,
    header_len: u16,
    remaining_len: u16,
    layout: u16,
    buffer_flags: u32,
    descriptor_flags: u32,
    descriptor_security: u32,
    descriptor_callbacks: u32,
    hardware_status: u8,
) -> bool {
    // Hardware may set the standard 802.11 Retry bit after an unsuccessful
    // first attempt. No other frame-control bit is mutable here.
    frame_control & !0x0800 == 0x00d0
        && header_len == 0x0020
        && remaining_len == 0x000d
        // The low twelve bits mirror Sequence Control >> 4 and therefore
        // advance independently of the structural post-security layout.
        && layout & 0xf000 == 0x2000
        && buffer_flags == 0xc00b_402c
        && descriptor_flags == 0
        && descriptor_callbacks == (1 << CALLBACK_MGMT) | (1 << CALLBACK_ADDBA_RESPONSE)
        && matches!(
            (descriptor_security, hardware_status),
            (0x0104_0000, 1) | (0x0114_0000, 1) | (0x0214_0000, 2)
        )
}

unsafe fn strict_ap_addba_response_txdone(frame: *mut u8) -> Result<(), TxDoneError> {
    if frame.is_null() || !crate::esf::is_strict_recyclable_frame(frame) {
        return Err(TxDoneError::NonStaticFrameType(if frame.is_null() {
            u8::MAX
        } else {
            frame.add(FRAME_TYPE_OFFSET).read()
        }));
    }
    let descriptor = descriptor(frame)?;
    let buffer = frame.add(4).cast::<*mut u8>().read();
    if buffer.is_null() {
        return Err(TxDoneError::MissingDescriptor);
    }
    let lengths = frame.add(0x14).cast::<u32>().read_unaligned();
    let layout = frame.add(0x24).cast::<u16>().read_unaligned();
    let frame_control = tx_trace_frame_control(frame);
    let buffer_flags = buffer.cast::<u32>().read_unaligned();
    let descriptor_flags = descriptor.cast::<u32>().read_unaligned();
    let descriptor_security = descriptor.add(0x10).cast::<u32>().read_unaligned();
    let descriptor_callbacks = descriptor
        .add(DESCRIPTOR_CALLBACK_MASK_OFFSET)
        .cast::<u32>()
        .read_unaligned();
    let hardware_status = descriptor.add(19).read();
    if !is_ap_addba_response_completion_layout(
        frame_control,
        lengths as u16,
        (lengths >> 16) as u16,
        layout,
        buffer_flags,
        descriptor_flags,
        descriptor_security,
        descriptor_callbacks,
        hardware_status,
    ) {
        #[cfg(all(target_arch = "riscv32", feature = "hil-vendor-tx"))]
        ets_printf(
            c"HIL ADDBA reject tuple: fc=%04x len=%04x:%04x layout=%04x buffer=%08x df=%08x ds=%08x cb=%08x hw=%02x\r\n"
                .as_ptr()
                .cast(),
            u32::from(frame_control),
            lengths as u16 as u32,
            (lengths >> 16) as u16 as u32,
            u32::from(layout),
            buffer_flags,
            descriptor_flags,
            descriptor_security,
            descriptor_callbacks,
            u32::from(hardware_status),
        );
        return Err(TxDoneError::StrictCallbackFailed);
    }
    let mut header = buffer.add(4).cast::<*mut u8>().read_unaligned();
    if header.is_null() {
        return Err(TxDoneError::MissingDescriptor);
    }
    if layout & 0x2000 != 0 {
        header = header.add(8);
    }
    if header.add(24).read() != 3 || header.add(25).read() != 1 {
        return Err(TxDoneError::StrictCallbackFailed);
    }
    let response_status = u16::from_le_bytes([header.add(27).read(), header.add(28).read()]);
    if response_status == 0 {
        #[cfg(not(feature = "hil-rx-ampdu"))]
        return Err(TxDoneError::StrictCallbackFailed);
        #[cfg(feature = "hil-rx-ampdu")]
        {
            let parameters = u16::from_le_bytes([header.add(29).read(), header.add(30).read()]);
            let timeout = u16::from_le_bytes([header.add(31).read(), header.add(32).read()]);
            let expected_parameters = 1_u16 << 1 | crate::rx_ampdu::RX_BLOCK_ACK_MAX_WINDOW << 6;
            if parameters != expected_parameters || timeout != 0 {
                return Err(TxDoneError::StrictCallbackFailed);
            }
            // A terminal no-ACK result must tear down only this agreement;
            // it is a recoverable link event and must not stop radio-owner.
            if descriptor.add(19).read() == 2 {
                let mut peer = [0_u8; 6];
                ptr::copy_nonoverlapping(header.add(4), peer.as_mut_ptr(), peer.len());
                crate::rx_ampdu_ap::rollback_failed_response(peer);
            }
        }
    } else if response_status != 1 {
        return Err(TxDoneError::StrictCallbackFailed);
    }
    // The pinned vendor callback returns immediately on acknowledged TX. A
    // failed successful response is handled above by the Rust agreement
    // owner; a declined response owns no BlockAck state. Neither terminal
    // outcome is fatal to the radio executor.
    Ok(())
}

#[cfg(target_arch = "riscv32")]
unsafe fn initial_ap_is_active() -> bool {
    let ic = ptr::addr_of!(g_ic).cast::<u8>();
    let interface = ic.add(0x14).cast::<*mut u8>().read();
    !interface.is_null() && interface.add(321).read() & 1 != 0
}

#[cfg(target_arch = "riscv32")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InitialApStartError {
    StrictRuntimeAlreadyArmed,
    DeferredTransitionMissing,
    CompletionDidNotStartAp,
}

/// Complete the taskless initialization transition that normally follows the
/// first beacon TX callback.
///
/// The pinned blob can publish `beacon_send_start_flag == 0b11` while the AP
/// interface is still disabled, so no physical TX completion can arrive. The
/// original completion leaf does not inspect its frame argument; invoking it
/// once consumes the deferred bit and calls the already-registered softAP
/// start leaf. This function does not poll, delay, allocate, or wait on an RTOS
/// primitive.
///
/// # Safety
/// Call exactly once from the serialized composition-root context after
/// `WifiController` start and before strict takeover. No radio callback may run
/// concurrently.
#[cfg(target_arch = "riscv32")]
pub unsafe fn complete_initial_ap_start() -> Result<(), InitialApStartError> {
    if unsafe { initial_ap_is_active() } {
        return Ok(());
    }
    if crate::critical::strict_wifi_hart_armed() {
        return Err(InitialApStartError::StrictRuntimeAlreadyArmed);
    }

    if unsafe { BEACON_SEND_START_FLAG } & 3 != 3 || unsafe { TmpSTAAPCloseAP } == 0 {
        return Err(InitialApStartError::DeferredTransitionMissing);
    }

    unsafe { initialization_hostapd_beacon_txcb(ptr::null_mut()) };
    if unsafe { initial_ap_is_active() } {
        Ok(())
    } else {
        Err(InitialApStartError::CompletionDidNotStartAp)
    }
}

pub(crate) fn runtime_callback_link_wrappers_active() -> bool {
    core::ptr::eq(
        vendor_hostapd_beacon_txcb as *const (),
        __wrap_ieee80211_hostapd_beacon_txcb as *const (),
    ) && core::ptr::eq(
        vendor_tx_mgt_cb as *const (),
        __wrap_ieee80211_tx_mgt_cb as *const (),
    ) && core::ptr::eq(
        vendor_ic_get_next_tbtt as *const (),
        __wrap_ic_get_next_tbtt as *const (),
    ) && core::ptr::eq(
        vendor_rc_update_tx_done as *const (),
        wifi_strict_rc_update_tx_done as *const (),
    )
}

/// Pure two-byte ACK-SNR filter recovered from `libpp.a[trc.o]`.
///
/// Byte zero is the previous signed sample and byte one is the smoothed
/// sample. `0x7f` is the vendor "not measured" sentinel. Keeping this
/// transform safe and value-based is intentional: the radio owner decides
/// which record is current, while the target adapter below only copies the
/// two bytes across the temporary C ABI boundary.
fn update_ack_snr_filter(mut state: [i8; 2], sample: i32) -> [i8; 2] {
    const NOT_MEASURED: i8 = 0x7f;

    if sample == i32::from(NOT_MEASURED) {
        return state;
    }

    let midpoint = if state[0] == NOT_MEASURED {
        0
    } else {
        (i32::from(state[0]) + sample) >> 1
    };
    let previous_smoothed = state[1];
    state[0] = sample as u8 as i8;
    state[1] = if previous_smoothed == NOT_MEASURED {
        midpoint as u8 as i8
    } else {
        ((i32::from(previous_smoothed) * 3 + midpoint) / 4) as u8 as i8
    };
    state
}

/// Temporary C/ROM ABI adapter for the now Rust-owned ACK-SNR transform.
///
/// The caller must provide unique writable access to the first two bytes of a
/// live rate-control record. No pointer escapes this function and the safe
/// filter has no access to MMIO, blob globals, or ROM state.
#[no_mangle]
pub unsafe extern "C" fn wifi_strict_rc_update_ack_snr(rate_control: *mut c_void, ack_snr: i32) {
    let rate_control = rate_control.cast::<i8>();
    if rate_control.is_null() {
        return;
    }

    let state = [rate_control.read(), rate_control.add(1).read()];
    let updated = update_ack_snr_filter(state, ack_snr);
    rate_control.write(updated[0]);
    rate_control.add(1).write(updated[1]);
}

unsafe fn owns_rate_control_record(rate_control: *mut u8) -> bool {
    crate::static_trc::owns_rate_control_record(rate_control)
        || crate::allocation::owns_rate_control_record(rate_control)
}

/// Exact S31 ROM `phy_read_hw_noisefloor` value transform.
///
/// The complete 0x1a-byte ROM body at `0x2f827d72` reads only
/// `0x2010_708c`, converts its low 12 bits to the signed hardware encoding,
/// and divides by four. Keeping the MMIO load here removes both the archive
/// trampoline and the absolute ROM dependency from TX completion.
unsafe fn read_noise_floor() -> i32 {
    let raw = PHY_NOISE_FLOOR_REGISTER.read_volatile() & 0x0fff;
    (raw as i32 - 4096) >> 2
}

/// Direct port of the complete `hal_he_set_bf_report_rate` register body.
pub(crate) unsafe fn set_bf_report_rate(mode: u8, rate: u16, dcm: bool, ersu: bool) {
    let mut encoded = rate;
    if mode != 0 {
        let mode_bits = (u16::from(mode) << 5) & 0x60;
        encoded = if rate.wrapping_sub(16) <= 9 {
            rate.wrapping_sub(16)
        } else {
            rate.wrapping_sub(26)
        };
        encoded |= mode_bits;
    }
    if dcm {
        encoded |= 0x80;
    }
    if ersu {
        encoded |= 0x100;
    }

    let register = HE_BF_REPORT_RATE_REGISTER;
    let old = register.read_volatile();
    register.write_volatile((old & 0xf803_ffff) | ((u32::from(encoded) << 18) & 0x07fc_0000));
    let old = register.read_volatile();
    register.write_volatile((old & 0xfffc_01ff) | ((u32::from(encoded) << 9) & 0x0003_fe00));
    let old = register.read_volatile();
    register.write_volatile((old & !0x1ff) | (u32::from(encoded) & 0x1ff));
}

/// Direct port of the complete `hal_he_set_ersu_ack_rate` register body.
pub(crate) unsafe fn set_ersu_ack_rate(enabled: bool) {
    let value = if enabled { 0xa0_u32 } else { 0x80_u32 };
    let register = HE_ERSU_ACK_RATE_REGISTER;

    let old = register.read_volatile();
    register.write_volatile((old & !0x0000_00ff) | value);
    let old = register.read_volatile();
    register.write_volatile((old & !0x0000_ff00) | (value << 8));
    let old = register.read_volatile();
    register.write_volatile((old & !0x00ff_0000) | (value << 16));
    let old = register.read_volatile();
    register.write_volatile((old & !0xff00_0000) | (value << 24));
}

/// Rust-owned TX PER transition with a narrow validated ABI projection.
///
/// All scalar mutation is performed by [`RateControlState`]. The adapter
/// accepts only the fixed Rust default records or a currently claimed
/// Rust-owned peer record, and it converts schedule pointers into typed
/// [`crate::rate_schedule::RateScheduleRef`] values before policy runs.
/// Unlike `rcClearCurSched`, it does not write the shared vendor
/// `schedule[11]`: the only remaining reader was the excluded stateful
/// `rcUpdateRate`, so that mutable bit is now eliminated from strict runtime.
#[no_mangle]
pub unsafe extern "C" fn wifi_strict_rc_update_tx_per(rate_control: *mut c_void, retries: u32) {
    let rate_control = rate_control.cast::<u8>();
    if rate_control.is_null() || !owns_rate_control_record(rate_control) {
        STRICT_CALLBACK_FAILED.store(true, Ordering::Release);
        return;
    }

    let current_pointer = rate_control
        .add(RATE_CURRENT_SCHEDULE_OFFSET)
        .cast::<*mut u8>()
        .read_unaligned();
    let Some(current_reference) = schedule_from_pointer(current_pointer) else {
        STRICT_CALLBACK_FAILED.store(true, Ordering::Release);
        return;
    };
    let current = schedule_state(current_reference);
    let legacy_pointer = rate_control
        .add(RATE_LEGACY_SCHEDULE_OFFSET)
        .cast::<*mut u8>()
        .read_unaligned();
    let Some(legacy_schedule) = schedule_from_pointer(legacy_pointer) else {
        STRICT_CALLBACK_FAILED.store(true, Ordering::Release);
        return;
    };

    let mut state = RateControlState {
        retry_pressure: rate_control.add(RATE_RETRY_PRESSURE_OFFSET).read(),
        weighted_retries: rate_control
            .add(RATE_WEIGHTED_RETRIES_OFFSET)
            .cast::<u32>()
            .read_unaligned(),
        transmissions: rate_control
            .add(RATE_TRANSMISSIONS_OFFSET)
            .cast::<u32>()
            .read_unaligned(),
        completed: rate_control
            .add(RATE_COMPLETED_OFFSET)
            .cast::<u32>()
            .read_unaligned(),
        reevaluate_after_us: rate_control
            .add(RATE_REEVALUATE_AFTER_OFFSET)
            .cast::<u32>()
            .read_unaligned(),
        retry_state_1d: rate_control.add(RATE_RETRY_STATE_1D_OFFSET).read(),
        retry_state_1e: rate_control.add(RATE_RETRY_STATE_1E_OFFSET).read(),
        maximum_schedule_index: rate_control.add(RATE_MAXIMUM_SCHEDULE_INDEX_OFFSET).read(),
        current_schedule: RateScheduleState {
            reference: current_reference,
            retry_limit: current.retry_limit,
            adaptive: current.adaptive,
        },
        legacy_schedule,
    };
    let update = state.update_tx_per(retries);

    rate_control
        .add(RATE_RETRY_PRESSURE_OFFSET)
        .write(state.retry_pressure);
    rate_control
        .add(RATE_WEIGHTED_RETRIES_OFFSET)
        .cast::<u32>()
        .write_unaligned(state.weighted_retries);
    rate_control
        .add(RATE_TRANSMISSIONS_OFFSET)
        .cast::<u32>()
        .write_unaligned(state.transmissions);
    rate_control
        .add(RATE_COMPLETED_OFFSET)
        .cast::<u32>()
        .write_unaligned(state.completed);

    if update.schedule == ScheduleSelection::Unchanged {
        return;
    }

    rate_control
        .add(RATE_REEVALUATE_AFTER_OFFSET)
        .cast::<u32>()
        .write_unaligned(state.reevaluate_after_us);
    rate_control
        .add(RATE_RETRY_STATE_1D_OFFSET)
        .write(state.retry_state_1d);
    rate_control
        .add(RATE_RETRY_STATE_1E_OFFSET)
        .write(state.retry_state_1e);

    let selected = match update.schedule {
        ScheduleSelection::Unchanged => return,
        ScheduleSelection::Selected(schedule) => schedule,
        ScheduleSelection::Invalid => {
            STRICT_CALLBACK_FAILED.store(true, Ordering::Release);
            return;
        }
    };
    rate_control
        .add(RATE_CURRENT_SCHEDULE_OFFSET)
        .cast::<*mut u8>()
        .write_unaligned(schedule_pointer(selected));

    rate_control
        .add(RATE_LAST_MAC_TIME_OFFSET)
        .cast::<u32>()
        .write_unaligned(MAC_TIME_LOW_REGISTER.read_volatile());

    let quarter_noise_floor = read_noise_floor().wrapping_add(2) >> 2;
    let report = beamforming_report_rate(
        rate_control.add(1).read(),
        quarter_noise_floor,
        rate_control.add(RATE_HE_FEATURE_8F_OFFSET).read() != 0,
        rate_control.add(RATE_HE_FEATURE_90_OFFSET).read() != 0,
    );
    set_bf_report_rate(report.mode, report.rate, report.dcm, report.ersu);
    set_ersu_ack_rate(report.ersu_ack);
}

/// Exact finite non-mesh port of the pinned `rcUpdateTxDone` boundary.
///
/// ACK-SNR filtering and TX PER/schedule lowering are safe Rust state
/// transitions. The hidden `wDevCtrl[0x2e]` read is replaced by its immutable
/// archive initializer. Mesh-specific retry clamping is intentionally absent
/// from the strict basic AP/STA profile.
#[no_mangle]
pub unsafe extern "C" fn wifi_strict_rc_update_tx_done(
    rate_control: *mut c_void,
    descriptor: *mut c_void,
) {
    let rate_control = rate_control.cast::<u8>();
    let descriptor = descriptor.cast::<u8>();
    if rate_control.is_null() || descriptor.is_null() {
        return;
    }

    let flags = rate_control.add(0x0c).cast::<u16>().read();
    if flags & 0x80 != 0 {
        return;
    }
    let schedule = rate_control.add(0x64).cast::<*const u8>().read();
    let descriptor_schedule = descriptor.add(0x1c).cast::<*const u8>().read();
    if descriptor_schedule != schedule || flags & 0x04 != 0 {
        return;
    }

    let completed = rate_control.add(0x40).cast::<u32>();
    completed.write(completed.read().wrapping_add(1));

    let retries = match descriptor.add(0x13).read() {
        1 => {
            if rate_control.add(0x1b).read() & 0x04 == 0 {
                let encoded = descriptor.add(0x0d).read();
                let ack_snr = encoded.wrapping_add(ACK_SNR_ENCODING_OFFSET) as i8;
                wifi_strict_rc_update_ack_snr(rate_control.cast(), i32::from(ack_snr));
            }
            u32::from(descriptor.add(0x05).read())
        }
        2 | 3 => {
            if schedule.is_null() {
                return;
            }
            u32::from(schedule.add(0x08).read())
        }
        _ => return,
    };
    wifi_strict_rc_update_tx_per(rate_control.cast(), retries);
}

/// Constant-time replacement for the vendor TBTT catch-up loop.
///
/// The final strict link must use `--wrap=ic_get_next_tbtt`. This function
/// reads the same two exported words and the same TSF source as the pinned
/// vendor body, but never advances one missed beacon at a time.
#[no_mangle]
pub unsafe extern "C" fn __wrap_ic_get_next_tbtt() -> u32 {
    let interval = ptr::addr_of!(BcnInterval).read_volatile();
    let send_tick = ptr::addr_of!(BcnSendTick).read_volatile();
    let Some(now) = crate::adapter::runtime_now_us() else {
        STRICT_CALLBACK_FAILED.store(true, Ordering::Release);
        return 0;
    };
    let Some((next_tick, delay)) = crate::tbtt::next_tbtt_delay(send_tick, interval, now as u32)
    else {
        STRICT_CALLBACK_FAILED.store(true, Ordering::Release);
        return 0;
    };
    ptr::addr_of_mut!(BcnSendTick).write_volatile(next_tick);
    delay
}

unsafe fn strict_management_txdone(frame: *mut u8) -> Result<(), ()> {
    if frame.is_null() {
        return Err(());
    }
    let buffer = frame.add(4).cast::<*mut u8>().read();
    if buffer.is_null() {
        return Err(());
    }
    let mut header = buffer.add(4).cast::<*const u8>().read();
    if header.is_null() {
        return Err(());
    }
    if frame.add(0x24).cast::<u16>().read() & 0x2000 != 0 {
        header = header.add(8);
    }
    let frame_control = u16::from_le_bytes([header.read(), header.add(1).read()]);
    let descriptor = frame.add(0x34).cast::<*mut u8>().read();
    if descriptor.is_null() {
        return Err(());
    }
    let descriptor_flags = descriptor.cast::<u32>().read();
    let descriptor_security = descriptor.add(0x10).cast::<u32>().read();
    STRICT_MANAGEMENT_TX_DONE_SUBTYPES[usize::from((frame_control >> 4) & 0x0f)]
        .fetch_add(1, Ordering::Relaxed);
    STRICT_MANAGEMENT_TX_DONE_LAST_FRAME_CONTROL.store(u32::from(frame_control), Ordering::Release);
    STRICT_MANAGEMENT_TX_DONE_LAST_DESCRIPTOR_FLAGS.store(descriptor_flags, Ordering::Release);
    STRICT_MANAGEMENT_TX_DONE_LAST_DESCRIPTOR_SECURITY
        .store(descriptor_security, Ordering::Release);
    if frame_control & 0x000c != 0 {
        return Err(());
    }
    let result = match frame_control & 0x00f0 {
        // Disconnect and off-channel action completions enter node/key/channel
        // state machines in the stock callback. They require explicit async
        // commands and are not allowed to run implicitly from TX completion.
        0xd0 if crate::sta_link::complete_owned_action_management(frame)
            || strict_ap_addba_response_txdone(frame).is_ok() =>
        {
            Ok(())
        }
        // AP deauthentication carries the direction bit recovered from the
        // pinned callback. The static AP node remains Rust-owned until an
        // explicit peer-removal command; TX completion itself has no required
        // hardware side effect and must not terminate the radio owner.
        0xc0 if is_ap_deauthentication_completion(frame_control, descriptor_security) => Ok(()),
        0xa0 | 0xc0 | 0xd0 => Err(()),
        _ => {
            crate::sta_link::management_tx_done(
                frame_control,
                descriptor.add(19).read(),
                descriptor_security,
            );
            Ok(())
        }
    };
    if result.is_ok() {
        STRICT_MANAGEMENT_TX_DONE_ACCEPTED.fetch_add(1, Ordering::Relaxed);
    } else {
        STRICT_MANAGEMENT_TX_DONE_REJECTED.fetch_add(1, Ordering::Relaxed);
    }
    result
}

/// Strict fixed-channel management completion. Authentication, association,
/// probe, and ordinary beacon frames need no stock completion side effect.
#[no_mangle]
pub unsafe extern "C" fn __wrap_ieee80211_tx_mgt_cb(frame: *mut c_void) {
    if strict_management_txdone(frame.cast()).is_err() {
        STRICT_CALLBACK_FAILED.store(true, Ordering::Release);
    }
}

unsafe fn strict_beacon_dtim(frame: *mut u8) -> Option<(u8, u8)> {
    if frame.is_null() {
        return None;
    }
    let storage = frame.add(4).cast::<*mut u8>().read();
    if storage.is_null() {
        return None;
    }
    let data = storage.add(4).cast::<*mut u8>().read();
    if data.is_null() {
        return None;
    }
    let length = usize::from(frame.add(0x14).cast::<u16>().read_unaligned())
        + usize::from(frame.add(0x16).cast::<u16>().read_unaligned());
    if length > 1600 {
        return None;
    }
    let bytes = core::slice::from_raw_parts(data, length);
    let (_, count, period) = crate::beacon::dtim(bytes)?;
    Some((count, period))
}

unsafe fn strict_ap_beacon_txdone(frame: *mut u8) -> Result<(), ()> {
    const STRICT_AP_BEACON_INTERVAL_US: u32 = 100 * 1_024;

    if TmpSTAAPCloseAP != 0 || !crate::net80211_state::ordinary_sta_ap_profile() {
        return Err(());
    }
    let dtim = strict_beacon_dtim(frame);
    let interface = crate::net80211_state::access_point_interface()
        .map(|interface| interface.as_ptr())
        .unwrap_or(ptr::null_mut());
    if !interface.is_null()
        && BEACON_DTIM_SEND_MC != 0
        && !interface.add(0xec).cast::<*mut u8>().read().is_null()
    {
        // Sleeping-client multicast queues require `pwrsave_flushq`, whose
        // send/PM path is deliberately outside the PS-none strict profile.
        return Err(());
    }

    BEACON_SEND_START_FLAG &= !1;
    let interval = ptr::addr_of!(BcnInterval).read_volatile();
    if interval != STRICT_AP_BEACON_INTERVAL_US {
        return Err(());
    }
    let send_tick = ptr::addr_of!(BcnSendTick).read_volatile();
    ptr::addr_of_mut!(BcnSendTick).write_volatile(send_tick.wrapping_add(interval));
    BEACON_NEXT_TBTT = interval;
    let Some(osi) = ptr::addr_of!(g_osi_funcs_p).read().as_ref() else {
        return Err(());
    };
    let Some(disarm) = osi._timer_disarm else {
        return Err(());
    };
    let Some(arm_us) = osi._timer_arm_us else {
        return Err(());
    };
    let timer = ptr::addr_of_mut!(BEACON_TIMER).cast::<c_void>();
    disarm(timer);
    arm_us(timer, interval, false);
    crate::ap_power_save::observe_beacon_dtim(dtim);
    Ok(())
}

/// Strict AP-beacon completion. The final link redirects the callback table
/// with `--wrap=ieee80211_hostapd_beacon_txcb`; no vendor power-save, mesh, or
/// indirect application callback is entered.
#[no_mangle]
pub unsafe extern "C" fn __wrap_ieee80211_hostapd_beacon_txcb(frame: *mut c_void) {
    #[cfg(target_arch = "riscv32")]
    if !crate::critical::strict_wifi_hart_armed() {
        // The first initialization beacon completion may carry the deferred
        // softAP-start transition. Preserve that finite leaf until strict
        // takeover; replacing it too early leaves beacon_send_start_flag at
        // 0b11 and the AP interface permanently disabled.
        initialization_hostapd_beacon_txcb(frame);
        return;
    }
    if strict_ap_beacon_txdone(frame.cast()).is_err() {
        STRICT_CALLBACK_FAILED.store(true, Ordering::Release);
    }
}

/// Complete the measured successful AP-beacon transmission without placing
/// its persistent double-buffer frame on the ordinary recyclable TX-done
/// list. The callback only advances TBTT state and arms the next async timer.
///
/// # Safety
///
/// `frame` must be the exact live beacon frame owned by logical queue one,
/// after the completion decoder has made that queue idle.
#[cfg(target_arch = "riscv32")]
#[link_section = ".rwtext.wifi_strict.ap_beacon_success"]
pub(crate) unsafe fn complete_ap_beacon_success(frame: *mut u8) -> Result<(), TxDoneError> {
    let descriptor = descriptor(frame)?;
    let descriptor_flags = descriptor.cast::<u32>().read();
    if descriptor_flags != 0x0080_0412 {
        return Err(TxDoneError::UnsupportedDescriptorFlags(descriptor_flags));
    }
    let registry = tx_done_registry()?;
    let registered_mask = registry.mode0_mask;
    let callbacks = descriptor
        .add(DESCRIPTOR_CALLBACK_MASK_OFFSET)
        .cast::<u32>()
        .read()
        & registered_mask;
    let expected = 1_u32 << CALLBACK_AP_BEACON;
    if callbacks != expected {
        return Err(TxDoneError::UnexpectedBeaconCallbacks(callbacks));
    }
    let registered = registry.callback(CALLBACK_AP_BEACON).unwrap_or(0);
    if registered != __wrap_ieee80211_hostapd_beacon_txcb as usize {
        return Err(TxDoneError::CallbackRegistryMismatch(CALLBACK_AP_BEACON));
    }
    let first_buffer = frame.add(4).cast::<*mut u8>().read();
    let tail_buffer = frame.add(8).cast::<*mut u8>().read();
    if first_buffer.is_null() || first_buffer != tail_buffer {
        #[cfg(feature = "hil-tx-deep-telemetry")]
        crate::tx_trace::record_descriptor_transition(
            crate::tx_trace::TxTraceEvent::PipelineRejected,
            frame,
            descriptor,
            0x0080,
            0,
            1,
            first_buffer.addr() as u32,
            tail_buffer.addr() as u32,
            0,
        );
        return Err(TxDoneError::InvalidBeaconFrame);
    }
    let metadata = first_buffer.add(4).cast::<*mut u8>().read();
    let lengths = frame.add(0x14).cast::<u32>().read_unaligned();
    let expected_metadata_len = (lengths as u16)
        .checked_add((lengths >> 16) as u16)
        .and_then(|length| length.checked_sub(8))
        .map(u32::from);
    let metadata_len = if metadata.is_null() {
        None
    } else {
        Some(metadata.cast::<u32>().read_unaligned())
    };
    if metadata_len != expected_metadata_len {
        #[cfg(feature = "hil-tx-deep-telemetry")]
        crate::tx_trace::record_descriptor_transition(
            crate::tx_trace::TxTraceEvent::PipelineRejected,
            frame,
            descriptor,
            0x0080,
            0,
            2,
            metadata.addr() as u32,
            metadata_len.unwrap_or(0),
            expected_metadata_len.unwrap_or(u32::MAX),
        );
        return Err(TxDoneError::InvalidBeaconFrame);
    }
    let layout_word = frame.add(0x24).cast::<u16>().read_unaligned();
    let buffer_flags = first_buffer.cast::<u32>().read_unaligned();
    let completion_input = crate::tx_security::TxSecurityLayoutInput {
        header_len: lengths as u16,
        remaining_len: (lengths >> 16) as u16,
        layout: layout_word,
        buffer_flags,
        descriptor_flags,
        descriptor_security: descriptor.add(0x10).cast::<u32>().read_unaligned(),
        frame_control: metadata.add(8).cast::<u16>().read_unaligned(),
    };
    let Some(layout) =
        crate::tx_security::strict_persistent_frame_completion_layout(completion_input)
    else {
        #[cfg(feature = "hil-tx-deep-telemetry")]
        crate::tx_trace::record_descriptor_transition(
            crate::tx_trace::TxTraceEvent::PipelineRejected,
            frame,
            descriptor,
            completion_input.frame_control,
            0,
            3,
            lengths,
            u32::from(layout_word),
            buffer_flags,
        );
        return Err(TxDoneError::InvalidBeaconFrame);
    };
    frame
        .add(0x14)
        .cast::<u32>()
        .write_unaligned(u32::from(layout.header_len) | (u32::from(layout.remaining_len) << 16));
    frame.add(0x24).cast::<u16>().write_unaligned(layout.layout);
    first_buffer
        .add(4)
        .cast::<*mut u8>()
        .write_unaligned(metadata.add(8));
    first_buffer
        .cast::<u32>()
        .write_unaligned(layout.buffer_flags);
    descriptor
        .add(0x10)
        .cast::<u32>()
        .write_unaligned(layout.descriptor_security);
    // `ieee80211_hostap_send_beacon_process` sets this ownership bit before
    // handing the persistent buffer to PP and refuses to reuse either beacon
    // buffer while it remains set. Hardware is complete at this boundary, so
    // publish the buffer as reusable before the callback arms the next TBTT.
    descriptor.cast::<u32>().write(layout.descriptor_flags);
    __wrap_ieee80211_hostapd_beacon_txcb(frame.cast());
    if STRICT_CALLBACK_FAILED.load(Ordering::Acquire) {
        return Err(TxDoneError::StrictCallbackFailed);
    }
    Ok(())
}

pub(crate) const fn is_continuation(kind: u32) -> bool {
    kind == TX_DONE_CONTINUATION
}

pub(crate) const fn is_lmac_continuation(kind: u32) -> bool {
    kind == LMAC_TX_DONE_CONTINUATION
}

/// Replace the finite `lmacTxDone(frame, 0)` prefix used by the strict
/// timeout/discard path. Mode-1 callbacks are split into separate executor
/// events before the frame is appended to the vendor TX-done list.
pub(crate) unsafe fn begin_from_lmac(frame: *mut u8) -> Result<(), TxDoneError> {
    begin_lmac(frame, true, false, 0, false)
}

/// Continue a Rust-owned successful LMAC completion. This is the recovered
/// mode-1 `lmacTxDone` ownership transfer: callback work and queue resumption
/// remain separate bounded executor events.
pub(crate) unsafe fn begin_from_tx_success(
    frame: *mut u8,
    hardware_event: u8,
) -> Result<(), TxDoneError> {
    crate::channel_switch::tx_done_edge();
    begin_lmac(frame, false, true, hardware_event, false)
}

/// Append one acknowledged strict STA data MPDU to the ordinary TX-done list
/// without posting an event for this individual frame.
///
/// A-MPDU interception admits only large QoS data. Requiring the raw callback
/// mask to be zero keeps management, EAPOL and AP completion policy on the
/// ordinary staged path. The caller publishes the finite batch with
/// `publish_callback_free_ampdu_batch` before yielding the radio executor.
pub(crate) unsafe fn commit_callback_free_ampdu_success(frame: *mut u8) -> Result<(), TxDoneError> {
    crate::channel_switch::tx_done_edge();
    let descriptor = descriptor(frame)?;
    let callbacks = descriptor
        .add(DESCRIPTOR_CALLBACK_MASK_OFFSET)
        .cast::<u32>()
        .read();
    if callbacks != 0 {
        return Err(TxDoneError::UnexpectedAmpduCallbacks(callbacks));
    }
    let flags = descriptor.cast::<u32>().read();
    if flags & DESCRIPTOR_DIRECT_RECYCLE_BIT != 0 {
        return Err(TxDoneError::UnsupportedLmacDescriptorFlags(flags));
    }
    if ptr::addr_of!(g_wifi_menuconfig)
        .cast::<u8>()
        .add(0x40)
        .read()
        & 0x08
        != 0
    {
        return Err(TxDoneError::TxTimeRecordingEnabled);
    }

    #[cfg(feature = "hil-tx-deep-telemetry")]
    crate::tx_trace::record_descriptor_transition(
        crate::tx_trace::TxTraceEvent::TxDoneBegin,
        frame,
        descriptor,
        tx_trace_frame_control(frame),
        u8::MAX,
        descriptor.add(0x0d).read(),
        4,
        0,
        u32::from(descriptor.add(19).read()),
    );
    append_tx_done(frame)?;
    if flags & DESCRIPTOR_RATE_CONTROL_BIT != 0
        && flags & DESCRIPTOR_RATE_CONTROL_SKIP_MASK != DESCRIPTOR_RATE_CONTROL_SKIP_VALUE
    {
        vendor_rc_update_tx_done(frame.add(0x2c).cast(), descriptor.cast());
    }
    #[cfg(feature = "hil-tx-deep-telemetry")]
    crate::tx_trace::record_descriptor_transition(
        crate::tx_trace::TxTraceEvent::TxDoneCommit,
        frame,
        descriptor,
        tx_trace_frame_control(frame),
        u8::MAX,
        descriptor.add(0x0d).read(),
        flags,
        16,
        u32::from(descriptor.add(19).read()),
    );
    Ok(())
}

/// Publish a previously appended callback-free A-MPDU completion prefix.
pub(crate) unsafe fn publish_callback_free_ampdu_batch() -> Result<(), TxDoneError> {
    if pp_post(16, ptr::null_mut()) == 0 {
        Ok(())
    } else {
        Err(TxDoneError::InternalQueueFull)
    }
}

/// Complete one Rust-owned directly submitted MPDU and resume its fixed
/// executor queue only after TX-done ownership has been transferred.
#[cfg(feature = "hil-ampdu-intercept")]
pub(crate) unsafe fn begin_from_intercept_success(frame: *mut u8) -> Result<(), TxDoneError> {
    crate::channel_switch::tx_done_edge();
    begin_lmac(frame, false, false, 0, true)
}

unsafe fn begin_lmac(
    frame: *mut u8,
    resume_timeout: bool,
    resume_queue: bool,
    resume_event: u8,
    resume_intercept: bool,
) -> Result<(), TxDoneError> {
    let state = &mut *LMAC_STATE.0.get();
    if state.failed {
        return Err(TxDoneError::PreviousFailure);
    }
    if state.active {
        return Err(TxDoneError::LmacPipelineBusy);
    }
    let descriptor = descriptor(frame)?;
    #[cfg(feature = "hil-tx-deep-telemetry")]
    crate::tx_trace::record_descriptor_transition(
        crate::tx_trace::TxTraceEvent::TxDoneBegin,
        frame,
        descriptor,
        tx_trace_frame_control(frame),
        if resume_queue { resume_event } else { u8::MAX },
        descriptor.add(0x0d).read(),
        u32::from(resume_timeout)
            | (u32::from(resume_queue) << 1)
            | (u32::from(resume_intercept) << 2),
        descriptor
            .add(DESCRIPTOR_CALLBACK_MASK_OFFSET)
            .cast::<u32>()
            .read(),
        u32::from(descriptor.add(19).read()),
    );
    let registered = tx_done_registry()?.mode1_mask;
    let callbacks = descriptor
        .add(DESCRIPTOR_CALLBACK_MASK_OFFSET)
        .cast::<u32>()
        .read()
        & registered;
    let unsupported = callbacks & !BASIC_MODE1_CALLBACKS;
    if unsupported != 0 {
        return Err(TxDoneError::UnsupportedCallbackBits(unsupported));
    }

    state.active = true;
    state.frame = frame;
    state.callbacks = callbacks;
    state.resume_timeout = resume_timeout;
    state.resume_queue = resume_queue;
    state.resume_event = resume_event;
    state.resume_intercept = resume_intercept;
    state.phase = if callbacks == 0 {
        PHASE_RECYCLE
    } else {
        PHASE_CALLBACK
    };
    run_lmac_step(state)
}

pub(crate) unsafe fn dispatch_lmac_continuation() -> Result<(), TxDoneError> {
    let state = &mut *LMAC_STATE.0.get();
    if state.failed {
        return Err(TxDoneError::PreviousFailure);
    }
    run_lmac_step(state)
}

unsafe fn run_lmac_step(state: &mut TxDoneState) -> Result<(), TxDoneError> {
    let result = match state.phase {
        PHASE_CALLBACK => dispatch_one_lmac_callback(state),
        PHASE_RECYCLE => commit_lmac_tx_done(state),
        _ => Err(TxDoneError::InvalidPhase),
    };
    if result.is_err() {
        state.failed = true;
        state.active = false;
        state.phase = PHASE_IDLE;
    }
    result
}

unsafe fn dispatch_one_lmac_callback(state: &mut TxDoneState) -> Result<(), TxDoneError> {
    let bit = state.callbacks.trailing_zeros() as u8;
    let callback =
        lmac_callback_for_bit(bit).ok_or(TxDoneError::UnsupportedCallbackBits(1 << bit))?;
    let registered = tx_done_registry()?.callback(bit).unwrap_or(0);
    if registered != callback as usize {
        return Err(TxDoneError::CallbackRegistryMismatch(bit));
    }

    #[cfg(feature = "hil-vendor-tx")]
    if bit == CALLBACK_STA_EAPOL {
        capture_hil_eapol_tx_done(state.frame)?;
    }

    if bit == CALLBACK_ADDBA_RESPONSE {
        strict_ap_addba_response_txdone(state.frame)?;
    } else if bit == CALLBACK_STA_EAPOL {
        if !crate::wpa2_txdone::ingest_completed_sta_frame(
            state.frame,
            descriptor(state.frame)?.add(19).read() != 1,
        ) {
            return Err(TxDoneError::StrictCallbackFailed);
        }
    } else {
        callback(state.frame.cast());
        if STRICT_CALLBACK_FAILED.load(Ordering::Acquire) {
            return Err(TxDoneError::StrictCallbackFailed);
        }
    }
    state.callbacks &= !(1 << bit);
    if state.callbacks == 0 {
        state.phase = PHASE_RECYCLE;
    }
    enqueue_lmac_step()
}

#[cfg(feature = "hil-vendor-tx")]
unsafe fn capture_hil_eapol_tx_done(frame: *mut u8) -> Result<(), TxDoneError> {
    let descriptor = descriptor(frame)?;
    let payload_owner = frame.add(4).cast::<*const u8>().read();
    if payload_owner.is_null() {
        return Err(TxDoneError::MissingDescriptor);
    }
    let mut payload = payload_owner.add(4).cast::<*const u8>().read();
    if payload.is_null() {
        return Err(TxDoneError::MissingDescriptor);
    }
    if frame.add(36).cast::<u16>().read() & 0x2000 != 0 {
        payload = payload.add(8);
    }
    let frame_control = u16::from_le_bytes([payload.read(), payload.add(1).read()]);
    let qos_offset = if frame_control & 0x0300 == 0x0300 {
        30
    } else {
        24
    };
    let qos_control = if frame_control & 0x0080 != 0 {
        payload.add(qos_offset).cast::<u16>().read_unaligned()
    } else {
        0
    };
    let descriptor_status = descriptor.add(0x10).cast::<u32>().read();
    HIL_EAPOL_FRAME_CONTROL.store(usize::from(frame_control), Ordering::Release);
    HIL_EAPOL_QOS_CONTROL.store(usize::from(qos_control), Ordering::Release);
    HIL_EAPOL_HW_STATUS.store(usize::from(descriptor.add(19).read()), Ordering::Release);
    HIL_EAPOL_DESCRIPTOR_STATUS.store(descriptor_status as usize, Ordering::Release);
    HIL_EAPOL_TXDONE_COUNT.fetch_add(1, Ordering::AcqRel);
    Ok(())
}

unsafe fn commit_lmac_tx_done(state: &mut TxDoneState) -> Result<(), TxDoneError> {
    let frame = state.frame;
    let descriptor = descriptor(frame)?;
    let flags = descriptor.cast::<u32>().read();
    if flags & DESCRIPTOR_DIRECT_RECYCLE_BIT != 0 {
        return Err(TxDoneError::UnsupportedLmacDescriptorFlags(flags));
    }
    if ptr::addr_of!(g_wifi_menuconfig)
        .cast::<u8>()
        .add(0x40)
        .read()
        & 0x08
        != 0
    {
        return Err(TxDoneError::TxTimeRecordingEnabled);
    }

    append_tx_done(frame)?;
    if flags & DESCRIPTOR_RATE_CONTROL_BIT != 0
        && flags & DESCRIPTOR_RATE_CONTROL_SKIP_MASK != DESCRIPTOR_RATE_CONTROL_SKIP_VALUE
    {
        vendor_rc_update_tx_done(frame.add(0x2c).cast(), descriptor.cast());
    }
    #[cfg(feature = "hil-tx-deep-telemetry")]
    crate::tx_trace::record_descriptor_transition(
        crate::tx_trace::TxTraceEvent::RateControlDone,
        frame,
        descriptor,
        tx_trace_frame_control(frame),
        if state.resume_queue {
            state.resume_event
        } else {
            u8::MAX
        },
        descriptor.add(0x0d).read(),
        flags,
        descriptor
            .add(DESCRIPTOR_CALLBACK_MASK_OFFSET)
            .cast::<u32>()
            .read(),
        u32::from(descriptor.add(19).read()),
    );
    if pp_post(16, ptr::null_mut()) != 0 {
        return Err(TxDoneError::InternalQueueFull);
    }
    #[cfg(feature = "hil-tx-deep-telemetry")]
    crate::tx_trace::record_descriptor_transition(
        crate::tx_trace::TxTraceEvent::TxDoneCommit,
        frame,
        descriptor,
        tx_trace_frame_control(frame),
        if state.resume_queue {
            state.resume_event
        } else {
            u8::MAX
        },
        descriptor.add(0x0d).read(),
        flags,
        16,
        u32::from(descriptor.add(19).read()),
    );

    let resume_timeout = state.resume_timeout;
    let resume_queue = state.resume_queue;
    let resume_event = state.resume_event;
    let resume_intercept = state.resume_intercept;
    state.active = false;
    state.phase = PHASE_IDLE;
    state.frame = ptr::null_mut();
    state.resume_timeout = false;
    state.resume_queue = false;
    state.resume_event = 0;
    state.resume_intercept = false;
    if resume_timeout {
        return crate::lmac::resume_after_tx_done().map_err(|_| TxDoneError::InternalQueueFull);
    }
    if resume_queue {
        let instances = ptr::addr_of!(our_instances_ptr).read();
        if instances.is_null() {
            return Err(TxDoneError::InstancesUnavailable);
        }
        if resume_event > 3 {
            return Err(TxDoneError::InvalidPhase);
        }
        if instances
            .add(usize::from(resume_event) * 0x38 + 0x1d)
            .read()
            <= 2
        {
            crate::tx_queue::release_txop_queue(resume_event).map_err(TxDoneError::TxopQueue)?;
        }
        if pp_post(u32::from(resume_event), ptr::null_mut()) != 0 {
            return Err(TxDoneError::InternalQueueFull);
        }
    }
    if resume_intercept {
        #[cfg(feature = "hil-ampdu-intercept")]
        {
            return crate::tx_intercept::on_direct_hardware_completion()
                .map_err(|_| TxDoneError::InternalQueueFull);
        }
        #[cfg(not(feature = "hil-ampdu-intercept"))]
        return Err(TxDoneError::InvalidPhase);
    }
    Ok(())
}

unsafe fn begin_from_wrapped_lmac(frame: *mut u8, mode: u32) -> Result<(), TxDoneError> {
    match mode {
        0 => begin_lmac(frame, false, false, 0, false),
        1 => {
            let hardware_event = hardware_event_for_frame(frame)?;
            begin_lmac(frame, false, true, hardware_event, false)
        }
        value => Err(TxDoneError::UnsupportedLmacMode(value)),
    }
}

unsafe fn hardware_event_for_frame(frame: *mut u8) -> Result<u8, TxDoneError> {
    let instances = ptr::addr_of!(our_instances_ptr).read();
    if instances.is_null() {
        return Err(TxDoneError::InstancesUnavailable);
    }
    let mut hardware_event = 0_u8;
    while hardware_event < 4 {
        let queue_state = instances.add(usize::from(hardware_event) * 0x38);
        if queue_state.cast::<*mut u8>().read() == frame {
            return Ok(hardware_event);
        }
        hardware_event += 1;
    }
    Err(TxDoneError::InvalidPhase)
}

/// Final-link replacement for the vendor TX-done convergence point. GNU ld
/// must receive `--wrap=lmacTxDone`; the archive itself is not modified.
/// Every mode-1 callback and the queue resume become executor continuations,
/// so the stock inline `ppProcTxDone`/power-management tail is never entered.
#[no_mangle]
#[cfg_attr(
    target_arch = "riscv32",
    link_section = ".rwtext.wifi_strict.lmac_tx_done"
)]
pub unsafe extern "C" fn __wrap_lmacTxDone(frame: *mut c_void, mode: u32) {
    crate::channel_switch::tx_done_edge();
    if begin_from_wrapped_lmac(frame.cast(), mode).is_err() {
        #[cfg(feature = "hil-tx-deep-telemetry")]
        {
            let raw_frame = frame.cast::<u8>();
            if !raw_frame.is_null() {
                if let Ok(raw_descriptor) = descriptor(raw_frame) {
                    crate::tx_trace::record_descriptor_transition(
                        crate::tx_trace::TxTraceEvent::PipelineRejected,
                        raw_frame,
                        raw_descriptor,
                        tx_trace_frame_control(raw_frame),
                        u8::MAX,
                        raw_descriptor.add(0x0d).read(),
                        mode,
                        raw_descriptor
                            .add(DESCRIPTOR_CALLBACK_MASK_OFFSET)
                            .cast::<u32>()
                            .read(),
                        u32::from(raw_descriptor.add(19).read()),
                    );
                }
            }
            crate::tx_trace::freeze_tx_trace();
        }
        let state = &mut *LMAC_STATE.0.get();
        state.failed = true;
        state.active = false;
        state.phase = PHASE_IDLE;
        state.frame = ptr::null_mut();
        state.resume_timeout = false;
        state.resume_queue = false;
        state.resume_event = 0;
        state.resume_intercept = false;
        // A wrapper cannot return a Rust error through the vendor C ABI. Post
        // a private event so the radio owner observes `PreviousFailure` and
        // terminates instead of continuing with partially completed TX state.
        let _ = enqueue_lmac_step();
    }
}

unsafe fn append_tx_done(frame: *mut u8) -> Result<(), TxDoneError> {
    if tx_done_registry()?.append(frame) {
        Ok(())
    } else {
        Err(TxDoneError::MissingTxDoneTail)
    }
}

fn enqueue_lmac_step() -> Result<(), TxDoneError> {
    if crate::adapter::enqueue_internal_event(PpEvent {
        kind: LMAC_TX_DONE_CONTINUATION,
        argument: ptr::null_mut(),
    }) {
        Ok(())
    } else {
        Err(TxDoneError::InternalQueueFull)
    }
}

/// Begin the strict replacement for PP event 16. Repeated vendor events can
/// coalesce because the linked TX-done list remains the source of truth.
pub(crate) unsafe fn begin() -> Result<(), TxDoneError> {
    let state = &mut *STATE.0.get();
    if state.failed {
        return Err(TxDoneError::PreviousFailure);
    }
    if state.active {
        return Ok(());
    }
    state.active = true;
    state.phase = PHASE_LOAD;
    run_step(state)
}

pub(crate) unsafe fn dispatch_continuation() -> Result<(), TxDoneError> {
    let state = &mut *STATE.0.get();
    if state.failed {
        return Err(TxDoneError::PreviousFailure);
    }
    run_step(state)
}

unsafe fn run_step(state: &mut TxDoneState) -> Result<(), TxDoneError> {
    let result = dispatch_quantum(state);
    if result.is_err() {
        state.failed = true;
        state.active = false;
        state.phase = PHASE_IDLE;
    }
    result
}

unsafe fn dispatch_quantum(state: &mut TxDoneState) -> Result<(), TxDoneError> {
    let mut recycled = 0_usize;
    loop {
        match state.phase {
            PHASE_LOAD => {
                load_one(state)?;
                if !state.active {
                    return Ok(());
                }
                if state.phase == PHASE_CALLBACK {
                    return enqueue_step();
                }
            }
            PHASE_CALLBACK => return dispatch_one_callback(state),
            PHASE_RECYCLE => {
                recycle_one(state)?;
                recycled += 1;
                if recycled == CALLBACK_FREE_RECYCLE_QUANTUM {
                    return enqueue_step();
                }
            }
            _ => return Err(TxDoneError::InvalidPhase),
        }
    }
}

unsafe fn load_one(state: &mut TxDoneState) -> Result<(), TxDoneError> {
    let registry = tx_done_registry()?;
    let frame = registry.pop_front();
    if frame.is_null() {
        state.active = false;
        state.phase = PHASE_IDLE;
        return Ok(());
    }

    let descriptor = descriptor(frame)?;
    let registered = registry.mode0_mask;
    let callbacks = descriptor
        .add(DESCRIPTOR_CALLBACK_MASK_OFFSET)
        .cast::<u32>()
        .read()
        & registered;
    let unsupported = callbacks & !BASIC_MODE0_CALLBACKS;
    if unsupported != 0 {
        return Err(TxDoneError::UnsupportedCallbackBits(unsupported));
    }

    state.frame = frame;
    state.callbacks = callbacks;
    state.phase = if callbacks == 0 {
        PHASE_RECYCLE
    } else {
        PHASE_CALLBACK
    };
    Ok(())
}

unsafe fn dispatch_one_callback(state: &mut TxDoneState) -> Result<(), TxDoneError> {
    let bit = state.callbacks.trailing_zeros() as u8;
    let callback = callback_for_bit(bit).ok_or(TxDoneError::UnsupportedCallbackBits(1 << bit))?;
    let registered = tx_done_registry()?.callback(bit).unwrap_or(0);
    if registered != callback as usize {
        return Err(TxDoneError::CallbackRegistryMismatch(bit));
    }

    if bit == CALLBACK_AP_POWER_SAVE {
        strict_ap_power_save_txdone(state.frame)?;
    } else {
        callback(state.frame.cast());
        if STRICT_CALLBACK_FAILED.load(Ordering::Acquire) {
            return Err(TxDoneError::StrictCallbackFailed);
        }
    }
    state.callbacks &= !(1 << bit);
    if state.callbacks == 0 {
        state.phase = PHASE_RECYCLE;
    }
    enqueue_step()
}

/// Consume the hostap power-save callback attached by the stock AP transmit
/// leaf without entering its connection-node/TIM state machine.
///
/// The pinned callback performs only software power-save bookkeeping:
/// `cnx_node_search`, counter updates and a possible `ieee80211_set_tim` call.
/// Strict AP deliberately has no sleeping-client queues, while WPA2 EAPOL
/// retransmission is owned by the Rust async state machine. Callback slot 12
/// is also attached to ordinary AP group data. In the strict PS-none profile
/// the measured protected DHCP/ARP layouts have no TIM or sleeping-client
/// side effect, so they complete as a validated no-op.
unsafe fn strict_ap_power_save_txdone(frame: *mut u8) -> Result<(), TxDoneError> {
    if frame.is_null() || !crate::esf::is_strict_recyclable_frame(frame) {
        return Err(TxDoneError::NonStaticFrameType(if frame.is_null() {
            u8::MAX
        } else {
            frame.add(FRAME_TYPE_OFFSET).read()
        }));
    }
    let descriptor = descriptor(frame)?;
    let descriptor_callbacks = descriptor
        .add(DESCRIPTOR_CALLBACK_MASK_OFFSET)
        .cast::<u32>()
        .read();
    if descriptor_callbacks & (1 << CALLBACK_AP_POWER_SAVE) == 0 {
        return Err(TxDoneError::UnsupportedCallbackBits(descriptor_callbacks));
    }

    let buffer = frame.add(4).cast::<*mut u8>().read();
    if buffer.is_null() {
        return Err(TxDoneError::MissingDescriptor);
    }
    let mut header = buffer.add(4).cast::<*const u8>().read();
    if header.is_null() {
        return Err(TxDoneError::MissingDescriptor);
    }
    let layout = frame.add(0x24).cast::<u16>().read_unaligned();
    if layout & 0x2000 != 0 {
        header = header.add(8);
    }
    let frame_control = header.cast::<u16>().read_unaligned();
    let lengths = frame.add(0x14).cast::<u32>().read_unaligned();
    let descriptor_flags = descriptor.cast::<u32>().read_unaligned();
    let descriptor_security = descriptor.add(0x10).cast::<u32>().read_unaligned();
    let buffer_flags = buffer.cast::<u32>().read_unaligned();
    let protected_data = crate::tx_security::strict_ap_group_power_save_completion(
        frame_control,
        lengths as u16,
        (lengths >> 16) as u16,
        layout,
        buffer_flags,
        descriptor_flags,
        descriptor_security,
    ) || crate::tx_security::strict_ap_pairwise_power_save_completion(
        frame_control,
        lengths as u16,
        (lengths >> 16) as u16,
        layout,
        buffer_flags,
        descriptor_flags,
        descriptor_security,
    );
    if protected_data {
        return Ok(());
    }
    let Some(llc_offset) = crate::tx_security::strict_ap_eapol_power_save_completion_llc_offset(
        frame_control,
        lengths as u16,
        (lengths >> 16) as u16,
        layout,
    ) else {
        #[cfg(all(target_arch = "riscv32", feature = "hil-vendor-tx"))]
        ets_printf(
            c"HIL PS reject tuple: fc=%04x len=%04x:%04x layout=%04x buffer=%08x df=%08x ds=%08x cb=%08x\r\n"
                .as_ptr()
                .cast(),
            u32::from(frame_control),
            lengths as u16 as u32,
            (lengths >> 16) as u16 as u32,
            u32::from(layout),
            buffer_flags,
            descriptor_flags,
            descriptor_security,
            descriptor_callbacks,
        );
        return Err(TxDoneError::StrictCallbackFailed);
    };
    const LLC_EAPOL: [u8; 8] = [0xaa, 0xaa, 0x03, 0, 0, 0, 0x88, 0x8e];
    if core::slice::from_raw_parts(header.add(llc_offset), LLC_EAPOL.len()) != LLC_EAPOL {
        #[cfg(all(target_arch = "riscv32", feature = "hil-vendor-tx"))]
        ets_printf(
            c"HIL PS reject LLC: fc=%04x len=%04x:%04x layout=%04x off=%02x bytes=%02x%02x%02x%02x%02x%02x%02x%02x\r\n"
                .as_ptr()
                .cast(),
            u32::from(frame_control),
            lengths as u16 as u32,
            (lengths >> 16) as u16 as u32,
            u32::from(layout),
            llc_offset as u32,
            u32::from(header.add(llc_offset).read()),
            u32::from(header.add(llc_offset + 1).read()),
            u32::from(header.add(llc_offset + 2).read()),
            u32::from(header.add(llc_offset + 3).read()),
            u32::from(header.add(llc_offset + 4).read()),
            u32::from(header.add(llc_offset + 5).read()),
            u32::from(header.add(llc_offset + 6).read()),
            u32::from(header.add(llc_offset + 7).read()),
        );
        return Err(TxDoneError::StrictCallbackFailed);
    }
    Ok(())
}

unsafe fn recycle_one(state: &mut TxDoneState) -> Result<(), TxDoneError> {
    let frame = state.frame;
    let descriptor = descriptor(frame)?;
    #[cfg(feature = "hil-tx-deep-telemetry")]
    if crate::data_tx::owns_hardware_wifi_data_tx(frame) {
        capture_hil_data_tx_done(frame, descriptor)?;
    }
    let flags = descriptor.cast::<u32>().read();
    // The stock bit-13 branch only feeds `trc_onPPTxDone` after inspecting
    // optional tracing metadata. Strict mode has no tracing consumer and
    // intentionally omits that side effect; the bit does not alter ownership
    // or recycling. Bit 23 marks a retained management object: restore its
    // original cached layout and deliberately leave it owned by net80211.
    if flags & DESCRIPTOR_PERSISTENT_BIT != 0 {
        restore_persistent_frame(frame, descriptor)?;
        state.frame = ptr::null_mut();
        state.phase = PHASE_LOAD;
        return Ok(());
    }
    if ptr::addr_of!(g_tx_done_cb_func).read() != 0 {
        return Err(TxDoneError::UserCallbackInstalled);
    }
    let frame_type = frame.add(FRAME_TYPE_OFFSET).read();
    if !crate::esf::is_strict_recyclable_frame(frame) {
        return Err(TxDoneError::NonStaticFrameType(frame_type));
    }

    // With the compile-time `coex` feature disabled, the registered adapter
    // target is an exact no-op returning zero. Avoid its frame classifier and
    // indirect OSI-table tail in the strict Wi-Fi-only profile.
    #[cfg(not(feature = "strict-no-wait"))]
    pp_coex_tx_release(frame.cast());
    // The strict ESF wrapper accepts only its fixed Rust management pool or
    // initialized vendor static free lists; dynamic/cache branches remain
    // unreachable.
    #[cfg(feature = "hil-tx-deep-telemetry")]
    crate::tx_trace::record_descriptor_transition(
        crate::tx_trace::TxTraceEvent::Recycle,
        frame,
        descriptor,
        tx_trace_frame_control(frame),
        u8::MAX,
        descriptor.add(0x0d).read(),
        u32::from(frame_type),
        flags,
        u32::from(descriptor.add(19).read()),
    );
    esf_buf_recycle(frame.cast());
    crate::data_tx::complete_hardware_wifi_data_tx(frame);

    state.frame = ptr::null_mut();
    state.phase = PHASE_LOAD;
    Ok(())
}

unsafe fn restore_persistent_frame(frame: *mut u8, descriptor: *mut u8) -> Result<(), TxDoneError> {
    let first_buffer = frame.add(4).cast::<*mut u8>().read_unaligned();
    let tail_buffer = frame.add(8).cast::<*mut u8>().read_unaligned();
    if first_buffer.is_null()
        || first_buffer != tail_buffer
        || !crate::esf::is_strict_recyclable_frame(frame)
    {
        return Err(TxDoneError::NonStaticFrameType(
            frame.add(FRAME_TYPE_OFFSET).read(),
        ));
    }
    let metadata = first_buffer.add(4).cast::<*mut u8>().read_unaligned();
    if metadata.is_null() {
        return Err(TxDoneError::MissingDescriptor);
    }
    let lengths = frame.add(0x14).cast::<u32>().read_unaligned();
    let layout = frame.add(0x24).cast::<u16>().read_unaligned();
    let descriptor_flags = descriptor.cast::<u32>().read_unaligned();
    let input = crate::tx_security::TxSecurityLayoutInput {
        header_len: lengths as u16,
        remaining_len: (lengths >> 16) as u16,
        layout,
        buffer_flags: first_buffer.cast::<u32>().read_unaligned(),
        descriptor_flags,
        descriptor_security: descriptor.add(0x10).cast::<u32>().read_unaligned(),
        frame_control: metadata.add(8).cast::<u16>().read_unaligned(),
    };
    let output = crate::tx_security::strict_persistent_frame_completion_layout(input).ok_or(
        TxDoneError::UnsupportedPersistentLayout(
            descriptor_flags,
            input.descriptor_security,
            input.frame_control,
            input.header_len,
            input.remaining_len,
            input.layout,
            ((input.buffer_flags & 0x0fff_c000) >> 14) as u16,
        ),
    )?;

    first_buffer
        .add(4)
        .cast::<*mut u8>()
        .write_unaligned(metadata.add(8));
    frame
        .add(0x14)
        .cast::<u16>()
        .write_unaligned(output.header_len);
    frame
        .add(0x16)
        .cast::<u16>()
        .write_unaligned(output.remaining_len);
    frame.add(0x24).cast::<u16>().write_unaligned(output.layout);
    first_buffer
        .cast::<u32>()
        .write_unaligned(output.buffer_flags);
    descriptor
        .cast::<u32>()
        .write_unaligned(output.descriptor_flags);
    descriptor
        .add(0x10)
        .cast::<u32>()
        .write_unaligned(output.descriptor_security);
    Ok(())
}

#[cfg(feature = "hil-tx-deep-telemetry")]
unsafe fn capture_hil_data_tx_done(frame: *mut u8, descriptor: *mut u8) -> Result<(), TxDoneError> {
    let payload_owner = frame.add(4).cast::<*const u8>().read();
    if payload_owner.is_null() {
        return Err(TxDoneError::MissingDescriptor);
    }
    let mut payload = payload_owner.add(4).cast::<*const u8>().read();
    if payload.is_null() {
        return Err(TxDoneError::MissingDescriptor);
    }
    if frame.add(36).cast::<u16>().read() & 0x2000 != 0 {
        payload = payload.add(8);
    }
    let frame_control = u16::from_le_bytes([payload.read(), payload.add(1).read()]);
    if frame_control & 0x000c != 0x0008 {
        return Ok(());
    }
    let header_len = ieee80211_data_header_len(frame_control);
    let protected = frame_control & 0x4000 != 0;
    let security_len = if protected { 8 } else { 0 };
    store_hil_bytes(&HIL_DATA_RECEIVER, payload.add(4));
    store_hil_bytes(&HIL_DATA_TRANSMITTER, payload.add(10));
    store_hil_bytes(&HIL_DATA_ADDRESS3, payload.add(16));
    if protected {
        store_hil_bytes(&HIL_DATA_CCMP_HEADER, payload.add(header_len));
    } else {
        clear_hil_bytes(&HIL_DATA_CCMP_HEADER);
    }
    // CCMP is applied while hardware consumes the DMA buffer. RAM keeps the
    // plaintext LLC/SNAP prefix after the inserted CCMP header.
    store_hil_bytes(
        &HIL_DATA_PAYLOAD_PREFIX,
        payload.add(header_len + security_len),
    );
    HIL_DATA_FRAME_CONTROL.store(usize::from(frame_control), Ordering::Release);
    HIL_DATA_QOS_CONTROL.store(
        if frame_control & 0x0080 != 0 {
            usize::from(payload.add(header_len - 2).cast::<u16>().read_unaligned())
        } else {
            0
        },
        Ordering::Release,
    );
    HIL_DATA_SEQUENCE_CONTROL.store(
        usize::from(payload.add(22).cast::<u16>().read_unaligned()),
        Ordering::Release,
    );
    let lengths = frame.add(0x14).cast::<u32>().read_unaligned();
    HIL_DATA_HEADER_LEN.store(usize::from(lengths as u16), Ordering::Release);
    HIL_DATA_REMAINING_LEN.store(usize::from((lengths >> 16) as u16), Ordering::Release);
    HIL_DATA_LAYOUT.store(
        usize::from(frame.add(0x24).cast::<u16>().read_unaligned()),
        Ordering::Release,
    );
    HIL_DATA_BUFFER_FLAGS.store(
        payload_owner.cast::<u32>().read_unaligned() as usize,
        Ordering::Release,
    );
    HIL_DATA_DESCRIPTOR_FLAGS.store(
        descriptor.cast::<u32>().read_unaligned() as usize,
        Ordering::Release,
    );
    HIL_DATA_HW_STATUS.store(usize::from(descriptor.add(19).read()), Ordering::Release);
    HIL_DATA_DESCRIPTOR_STATUS.store(
        descriptor.add(0x10).cast::<u32>().read() as usize,
        Ordering::Release,
    );
    HIL_DATA_TXDONE_COUNT.fetch_add(1, Ordering::AcqRel);
    Ok(())
}

#[cfg(feature = "hil-vendor-tx")]
const fn ieee80211_data_header_len(frame_control: u16) -> usize {
    let mut len = if frame_control & 0x0300 == 0x0300 {
        30
    } else {
        24
    };
    if frame_control & 0x0080 != 0 {
        len += 2;
        if frame_control & 0x8000 != 0 {
            len += 4;
        }
    }
    len
}

#[cfg(feature = "hil-tx-deep-telemetry")]
unsafe fn store_hil_bytes<const N: usize>(destination: &[AtomicU8; N], source: *const u8) {
    let mut index = 0;
    while index < N {
        destination[index].store(source.add(index).read(), Ordering::Release);
        index += 1;
    }
}

#[cfg(feature = "hil-tx-deep-telemetry")]
fn clear_hil_bytes<const N: usize>(destination: &[AtomicU8; N]) {
    let mut index = 0;
    while index < N {
        destination[index].store(0, Ordering::Release);
        index += 1;
    }
}

fn enqueue_step() -> Result<(), TxDoneError> {
    if crate::adapter::enqueue_internal_event(PpEvent {
        kind: TX_DONE_CONTINUATION,
        argument: ptr::null_mut(),
    }) {
        Ok(())
    } else {
        Err(TxDoneError::InternalQueueFull)
    }
}

unsafe fn descriptor(frame: *mut u8) -> Result<*mut u8, TxDoneError> {
    let descriptor = frame.add(FRAME_DESCRIPTOR_OFFSET).cast::<*mut u8>().read();
    if descriptor.is_null() {
        Err(TxDoneError::MissingDescriptor)
    } else {
        Ok(descriptor)
    }
}

#[cfg(feature = "hil-vendor-tx")]
#[cfg_attr(
    target_arch = "riscv32",
    link_section = ".rwtext.wifi_strict.tx_trace_frame_control"
)]
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
    if frame.add(0x24).cast::<u16>().read_unaligned() & 0x2000 != 0 {
        data = data.add(8);
    }
    data.cast::<u16>().read_unaligned()
}

unsafe fn descriptor_queue(descriptor: *mut u8) -> u8 {
    ((descriptor.add(0x10).cast::<u32>().read() >> 20) & 0x0f) as u8
}

/// The pinned callback reads `g_ic+0x14`, then returns immediately when
/// `g_ic+0x74` is zero. Strict handoff proves that ordinary non-mesh profile,
/// so its exact reachable behavior has no frame side effect.
unsafe extern "C" fn strict_ap_data_txdone(_frame: *mut c_void) {}

fn callback_for_bit(bit: u8) -> Option<TxCallback> {
    match bit {
        CALLBACK_MGMT => Some(__wrap_ieee80211_tx_mgt_cb),
        CALLBACK_AP_BEACON => Some(__wrap_ieee80211_hostapd_beacon_txcb),
        CALLBACK_AP_DATA => Some(strict_ap_data_txdone),
        CALLBACK_AP_POWER_SAVE => Some(ieee80211_hostapd_ps_txcb),
        _ => None,
    }
}

fn lmac_callback_for_bit(bit: u8) -> Option<TxCallback> {
    match bit {
        CALLBACK_STA_EAPOL => Some(sta_eapol_txdone_cb),
        CALLBACK_ADDBA_RESPONSE => Some(addba_response_txcb),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "hil-vendor-tx")]
    use super::ieee80211_data_header_len;
    use super::{
        beacon_dtim, is_ap_addba_response_completion_layout, is_ap_deauthentication_completion,
        supported_callback_index, update_ack_snr_filter, StrictTxDoneRegistry,
        CALLBACK_ADDBA_RESPONSE, CALLBACK_AP_POWER_SAVE, CALLBACK_MGMT, FRAME_NEXT_OFFSET,
    };

    #[test]
    fn ack_snr_filter_is_safe_and_matches_the_pinned_two_byte_transform() {
        assert_eq!(update_ack_snr_filter([12, -4], 0x7f), [12, -4]);
        assert_eq!(update_ack_snr_filter([0x7f, 0x7f], -20), [-20, 0]);
        assert_eq!(update_ack_snr_filter([-20, 0], -10), [-10, -3]);
        assert_eq!(update_ack_snr_filter([-10, -3], -11), [-11, -5]);
        assert_eq!(update_ack_snr_filter([20, 4], 10), [10, 6]);
    }

    #[test]
    fn bounded_beacon_parser_owns_dtim_count_and_period() {
        let mut beacon = [0_u8; 46];
        beacon[..2].copy_from_slice(&0x0080_u16.to_le_bytes());
        beacon[36..40].copy_from_slice(&[0, 2, b'a', b'p']);
        beacon[40..46].copy_from_slice(&[5, 4, 0, 2, 0, 0]);
        assert_eq!(beacon_dtim(&beacon), Some((40, 0, 2)));
        beacon[42] = 1;
        assert_eq!(beacon_dtim(&beacon), Some((40, 1, 2)));
        beacon[42] = 2;
        assert_eq!(beacon_dtim(&beacon), None);
        beacon[41] = 3;
        assert_eq!(beacon_dtim(&beacon), None);
    }

    #[test]
    fn rust_tx_done_registry_owns_fifo_and_supported_callbacks() {
        let mut first = [0_usize; 8];
        let mut second = [0_usize; 8];
        let first = first.as_mut_ptr().cast::<u8>();
        let second = second.as_mut_ptr().cast::<u8>();
        let mut registry = StrictTxDoneRegistry::empty();
        let power_save = supported_callback_index(CALLBACK_AP_POWER_SAVE).unwrap();
        registry.callbacks[power_save] = 0x1234;

        unsafe {
            assert!(registry.append(first));
            assert!(registry.append(second));
            assert_eq!(
                first.add(FRAME_NEXT_OFFSET).cast::<*mut u8>().read(),
                second
            );
            assert_eq!(registry.pop_front(), first);
            assert!(first
                .add(FRAME_NEXT_OFFSET)
                .cast::<*mut u8>()
                .read()
                .is_null());
            assert_eq!(registry.pop_front(), second);
            assert!(registry.pop_front().is_null());
        }
        assert_eq!(registry.callback(CALLBACK_AP_POWER_SAVE), Some(0x1234));
        assert_eq!(registry.callback(31), None);
    }

    #[test]
    fn only_ap_direction_deauthentication_is_completion_only() {
        assert!(is_ap_deauthentication_completion(0x00c0, 0x0114_0000));
        assert!(is_ap_deauthentication_completion(0x00c0, 0x0414_0000));
        assert!(!is_ap_deauthentication_completion(0x00c0, 0x0004_0000));
        assert!(!is_ap_deauthentication_completion(0x00a0, 0x0114_0000));
    }

    #[test]
    fn only_measured_terminal_ap_addba_completions_are_noops() {
        let callbacks = (1 << CALLBACK_MGMT) | (1 << CALLBACK_ADDBA_RESPONSE);
        for (descriptor_security, hardware_status) in
            [(0x0104_0000, 1), (0x0114_0000, 1), (0x0214_0000, 2)]
        {
            for frame_control in [0x00d0, 0x08d0] {
                for layout in [0x2000, 0x2732, 0x2733, 0x2734, 0x2fff] {
                    assert!(is_ap_addba_response_completion_layout(
                        frame_control,
                        0x20,
                        0x0d,
                        layout,
                        0xc00b_402c,
                        0,
                        descriptor_security,
                        callbacks,
                        hardware_status,
                    ));
                }
            }
        }
        assert!(!is_ap_addba_response_completion_layout(
            0x18d0,
            0x20,
            0x0d,
            0x2732,
            0xc00b_402c,
            0,
            0x0114_0000,
            callbacks,
            1,
        ));
        assert!(!is_ap_addba_response_completion_layout(
            0x00d0,
            0x20,
            0x0d,
            0x3732,
            0xc00b_402c,
            0,
            0x0114_0000,
            callbacks,
            1,
        ));
        assert!(!is_ap_addba_response_completion_layout(
            0x00d0,
            0x20,
            0x0d,
            0x2732,
            0xc00b_402c,
            0,
            0x0114_0000,
            callbacks,
            2,
        ));
        assert!(!is_ap_addba_response_completion_layout(
            0x00d0,
            0x20,
            0x0d,
            0x2732,
            0xc00b_402c,
            0,
            0x0114_0000,
            1 << CALLBACK_MGMT,
            1,
        ));
    }

    #[cfg(feature = "hil-vendor-tx")]
    #[test]
    fn locates_payload_after_optional_qos_and_ccmp_headers() {
        assert_eq!(ieee80211_data_header_len(0x0208), 24);
        assert_eq!(ieee80211_data_header_len(0x0288), 26);
        assert_eq!(ieee80211_data_header_len(0x0388), 32);
        assert_eq!(ieee80211_data_header_len(0x8388), 36);
    }
}
