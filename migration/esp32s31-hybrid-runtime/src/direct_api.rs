//! Allocation-free, bounded replacements for selected upper Wi-Fi APIs.
//!
//! The pinned `esp_wifi_stop` wrapper allocates a 24-byte ioctl command and,
//! after its first stop phase, may retry up to 500 times with an OSI delay
//! between attempts. The strict cold-start path calls it only while the radio
//! state is not started, before changing mode from NULL to STA or AP. In that
//! state the vendor process returns `ESP_ERR_WIFI_NOT_STARTED`, which the
//! wrapper converts to success.
//!
//! This interposition reproduces only that qualified cold behavior. It performs
//! one volatile state read and returns. An active-radio call is rejected and
//! counted; stopping a running radio belongs in an asynchronous Rust lifecycle
//! future, not in this synchronous ABI.

use core::sync::atomic::{AtomicU32, Ordering};

const API_REQUEST_SIZE: usize = 24;
const API_REQUEST_ARGUMENT_OFFSET: usize = 8;
const CONFIG_REQUEST_SIZE: usize = 208;
const CONFIG_REQUEST_PAYLOAD_OFFSET: usize = 20;
const WIFI_CONFIG_SIZE: usize = 184;
#[cfg(all(
    target_arch = "riscv32",
    any(
        feature = "rust-direct-cold-stop",
        feature = "rust-direct-set-max-tx-power",
        feature = "rust-direct-set-country-nvs-free"
    )
))]
const WIFI_STATE_OFFSET: usize = 0x1f5;
const WIFI_STATE_STARTED: u8 = 2;
const ESP_OK: i32 = 0;
const ESP_ERR_INVALID_ARG: i32 = 0x102;
const ESP_ERR_WIFI_NOT_INIT: i32 = 0x3001;
const ESP_ERR_WIFI_NOT_STARTED: i32 = 0x3002;
const ESP_ERR_WIFI_IF: i32 = 0x3004;
const ESP_ERR_WIFI_STATE: i32 = 0x3006;
const ESP_ERR_WIFI_NVS: i32 = 0x3008;
const ESP_ERR_NOT_SUPPORTED: i32 = 0x106;
const MAX_PS_TYPE: u32 = 2;

static CALLS: AtomicU32 = AtomicU32::new(0);
static PRESTART_SUCCESSES: AtomicU32 = AtomicU32::new(0);
static ACTIVE_REJECTIONS: AtomicU32 = AtomicU32::new(0);
static LAST_STATE: AtomicU32 = AtomicU32::new(0);
static SET_MODE_CALLS: AtomicU32 = AtomicU32::new(0);
static SET_MODE_NOT_INITIALIZED: AtomicU32 = AtomicU32::new(0);
static SET_MODE_LAST_MODE: AtomicU32 = AtomicU32::new(0);
static SET_MODE_LAST_RESULT: AtomicU32 = AtomicU32::new(0);
static SET_PS_CALLS: AtomicU32 = AtomicU32::new(0);
static SET_PS_NOT_INITIALIZED: AtomicU32 = AtomicU32::new(0);
static SET_PS_INVALID_ARGUMENTS: AtomicU32 = AtomicU32::new(0);
static SET_PS_LAST_TYPE: AtomicU32 = AtomicU32::new(0);
static SET_PS_LAST_RESULT: AtomicU32 = AtomicU32::new(0);
static REG_RXCB_CALLS: AtomicU32 = AtomicU32::new(0);
static REG_RXCB_NOT_INITIALIZED: AtomicU32 = AtomicU32::new(0);
static REG_RXCB_INVALID_INTERFACES: AtomicU32 = AtomicU32::new(0);
static REG_RXCB_LAST_INTERFACE: AtomicU32 = AtomicU32::new(0);
static REG_RXCB_LAST_RESULT: AtomicU32 = AtomicU32::new(0);
static REG_MGMT_FRAME_CALLS: AtomicU32 = AtomicU32::new(0);
static REG_MGMT_FRAME_NOT_INITIALIZED: AtomicU32 = AtomicU32::new(0);
static REG_MGMT_FRAME_LAST_MASK: AtomicU32 = AtomicU32::new(0);
static REG_MGMT_FRAME_LAST_CONTEXT: AtomicU32 = AtomicU32::new(0);
static REG_MGMT_FRAME_LAST_RESULT: AtomicU32 = AtomicU32::new(0);
static SET_MAX_TX_POWER_CALLS: AtomicU32 = AtomicU32::new(0);
static SET_MAX_TX_POWER_NOT_INITIALIZED: AtomicU32 = AtomicU32::new(0);
static SET_MAX_TX_POWER_NOT_STARTED: AtomicU32 = AtomicU32::new(0);
static SET_MAX_TX_POWER_INVALID_ARGUMENTS: AtomicU32 = AtomicU32::new(0);
static SET_MAX_TX_POWER_LAST_POWER: AtomicU32 = AtomicU32::new(0);
static SET_MAX_TX_POWER_LAST_RESULT: AtomicU32 = AtomicU32::new(0);
static SET_COUNTRY_CALLS: AtomicU32 = AtomicU32::new(0);
static SET_COUNTRY_NOT_INITIALIZED: AtomicU32 = AtomicU32::new(0);
static SET_COUNTRY_ACTIVE_REJECTIONS: AtomicU32 = AtomicU32::new(0);
static SET_COUNTRY_NVS_REJECTIONS: AtomicU32 = AtomicU32::new(0);
static SET_COUNTRY_INVALID_ARGUMENTS: AtomicU32 = AtomicU32::new(0);
static SET_COUNTRY_PUBLICATIONS: AtomicU32 = AtomicU32::new(0);
static SET_COUNTRY_LAST_CODE: AtomicU32 = AtomicU32::new(0);
static SET_COUNTRY_LAST_RESULT: AtomicU32 = AtomicU32::new(0);
static SET_CONFIG_CALLS: AtomicU32 = AtomicU32::new(0);
static SET_CONFIG_NOT_INITIALIZED: AtomicU32 = AtomicU32::new(0);
static SET_CONFIG_INVALID_INTERFACES: AtomicU32 = AtomicU32::new(0);
static SET_CONFIG_INVALID_ARGUMENTS: AtomicU32 = AtomicU32::new(0);
static SET_CONFIG_LAST_INTERFACE: AtomicU32 = AtomicU32::new(0);
static SET_CONFIG_LAST_RESULT: AtomicU32 = AtomicU32::new(0);
static SET_PROTOCOLS_CALLS: AtomicU32 = AtomicU32::new(0);
static SET_PROTOCOLS_NOT_INITIALIZED: AtomicU32 = AtomicU32::new(0);
static SET_PROTOCOLS_ACTIVE_IDEMPOTENT_SUCCESSES: AtomicU32 = AtomicU32::new(0);
static SET_PROTOCOLS_ACTIVE_REJECTIONS: AtomicU32 = AtomicU32::new(0);
static SET_PROTOCOLS_INVALID_INTERFACES: AtomicU32 = AtomicU32::new(0);
static SET_PROTOCOLS_INVALID_ARGUMENTS: AtomicU32 = AtomicU32::new(0);
static SET_PROTOCOLS_PUBLICATIONS: AtomicU32 = AtomicU32::new(0);
static SET_PROTOCOLS_LAST_INTERFACE: AtomicU32 = AtomicU32::new(0);
static SET_PROTOCOLS_LAST_BITMAPS: AtomicU32 = AtomicU32::new(0);
static SET_PROTOCOLS_LAST_RESULT: AtomicU32 = AtomicU32::new(0);
static SET_PROMISCUOUS_CALLS: AtomicU32 = AtomicU32::new(0);
static SET_PROMISCUOUS_NOT_INITIALIZED: AtomicU32 = AtomicU32::new(0);
static SET_PROMISCUOUS_IDEMPOTENT_SUCCESSES: AtomicU32 = AtomicU32::new(0);
static SET_PROMISCUOUS_TRANSITION_REJECTIONS: AtomicU32 = AtomicU32::new(0);
static SET_PROMISCUOUS_LAST_REQUESTED: AtomicU32 = AtomicU32::new(0);
static SET_PROMISCUOUS_LAST_STATE: AtomicU32 = AtomicU32::new(0);
static SET_PROMISCUOUS_LAST_RESULT: AtomicU32 = AtomicU32::new(0);
static SET_INACTIVE_TIME_CALLS: AtomicU32 = AtomicU32::new(0);
static SET_INACTIVE_TIME_NOT_INITIALIZED: AtomicU32 = AtomicU32::new(0);
static SET_INACTIVE_TIME_NOT_STARTED: AtomicU32 = AtomicU32::new(0);
static SET_INACTIVE_TIME_INVALID_ARGUMENTS: AtomicU32 = AtomicU32::new(0);
static SET_INACTIVE_TIME_INVALID_MODES: AtomicU32 = AtomicU32::new(0);
static SET_INACTIVE_TIME_PUBLICATIONS: AtomicU32 = AtomicU32::new(0);
static SET_INACTIVE_TIME_LAST_INTERFACE: AtomicU32 = AtomicU32::new(0);
static SET_INACTIVE_TIME_LAST_SECONDS: AtomicU32 = AtomicU32::new(0);
static SET_INACTIVE_TIME_LAST_RESULT: AtomicU32 = AtomicU32::new(0);

#[repr(C)]
struct WifiCountry {
    cc: [u8; 3],
    start_channel: u8,
    channel_count: u8,
    max_tx_power: i8,
    _padding: [u8; 2],
    policy: u32,
}

#[repr(C, align(4))]
struct ApiRequest {
    bytes: [u8; API_REQUEST_SIZE],
}

#[repr(C, align(4))]
struct ConfigRequest {
    bytes: [u8; CONFIG_REQUEST_SIZE],
}

impl ApiRequest {
    unsafe fn with_byte_argument(argument: u8) -> core::mem::MaybeUninit<Self> {
        let mut request = core::mem::MaybeUninit::<Self>::uninit();
        request
            .as_mut_ptr()
            .cast::<u8>()
            .add(API_REQUEST_ARGUMENT_OFFSET)
            .write(argument);
        request
    }

    unsafe fn with_rx_callback(interface: u8, callback: u32) -> core::mem::MaybeUninit<Self> {
        let mut request = Self::with_byte_argument(interface);
        request
            .as_mut_ptr()
            .cast::<u8>()
            .add(12)
            .cast::<u32>()
            .write_unaligned(callback);
        request
    }

    unsafe fn with_mgmt_frame_registration(
        frame_subtype_mask: u32,
        context: u32,
    ) -> core::mem::MaybeUninit<Self> {
        let mut request = core::mem::MaybeUninit::<Self>::uninit();
        let bytes = request.as_mut_ptr().cast::<u8>();
        bytes
            .add(12)
            .cast::<u32>()
            .write_unaligned(frame_subtype_mask);
        bytes.add(20).cast::<u32>().write_unaligned(context);
        request
    }
}

impl ConfigRequest {
    unsafe fn initialize(
        request: &mut core::mem::MaybeUninit<Self>,
        interface: u8,
        config: *const core::ffi::c_void,
    ) {
        let bytes = request.as_mut_ptr().cast::<u8>();
        bytes.write_bytes(0, CONFIG_REQUEST_SIZE);
        bytes.write(11);
        bytes.add(API_REQUEST_ARGUMENT_OFFSET).write(interface);
        core::ptr::copy_nonoverlapping(
            config.cast::<u8>(),
            bytes.add(CONFIG_REQUEST_PAYLOAD_OFFSET),
            WIFI_CONFIG_SIZE,
        );
    }
}

/// Observation counters for the bounded cold-stop boundary.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DirectColdStopSnapshot {
    pub calls: u32,
    pub prestart_successes: u32,
    pub active_rejections: u32,
    pub last_state: u8,
}

/// Observation counters for direct set-mode process calls.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DirectSetModeSnapshot {
    pub calls: u32,
    pub not_initialized: u32,
    pub last_mode: u8,
    pub last_result: i32,
}

/// Observation counters for direct power-save process calls.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DirectSetPsSnapshot {
    pub calls: u32,
    pub not_initialized: u32,
    pub invalid_arguments: u32,
    pub last_ps_type: u8,
    pub last_result: i32,
}

/// Observation counters for direct RX callback registration.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DirectRegRxcbSnapshot {
    pub calls: u32,
    pub not_initialized: u32,
    pub invalid_interfaces: u32,
    pub last_interface: u8,
    pub last_result: i32,
}

/// Observation counters for direct management-frame registration.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DirectRegMgmtFrameSnapshot {
    pub calls: u32,
    pub not_initialized: u32,
    pub last_frame_subtype_mask: u32,
    pub last_context: usize,
    pub last_result: i32,
}

/// Observation counters for direct maximum-TX-power changes.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DirectSetMaxTxPowerSnapshot {
    pub calls: u32,
    pub not_initialized: u32,
    pub not_started: u32,
    pub invalid_arguments: u32,
    pub last_power: i8,
    pub last_result: i32,
}

/// Observation counters for the NVS-free pre-start country boundary.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DirectSetCountrySnapshot {
    pub calls: u32,
    pub not_initialized: u32,
    pub active_rejections: u32,
    pub nvs_rejections: u32,
    pub invalid_arguments: u32,
    pub publications: u32,
    pub last_country_code: [u8; 3],
    pub last_result: i32,
}

/// Observation counters for direct interface-configuration process calls.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DirectSetConfigSnapshot {
    pub calls: u32,
    pub not_initialized: u32,
    pub invalid_interfaces: u32,
    pub invalid_arguments: u32,
    pub last_interface: u8,
    pub last_result: i32,
}

/// Observation counters for NVS-free pre-start protocol publication.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DirectSetProtocolsSnapshot {
    pub calls: u32,
    pub not_initialized: u32,
    pub active_idempotent_successes: u32,
    pub active_rejections: u32,
    pub invalid_interfaces: u32,
    pub invalid_arguments: u32,
    pub publications: u32,
    pub last_interface: u8,
    pub last_2_4_ghz_bitmap: u16,
    pub last_5_ghz_bitmap: u16,
    pub last_result: i32,
}

/// Observation counters for the idempotent promiscuous-mode boundary.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DirectPromiscuousSnapshot {
    pub calls: u32,
    pub not_initialized: u32,
    pub idempotent_successes: u32,
    pub transition_rejections: u32,
    pub last_requested: bool,
    pub last_state: u8,
    pub last_result: i32,
}

/// Observation counters for direct NVS-free inactivity-time publication.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DirectSetInactiveTimeSnapshot {
    pub calls: u32,
    pub not_initialized: u32,
    pub not_started: u32,
    pub invalid_arguments: u32,
    pub invalid_modes: u32,
    pub publications: u32,
    pub last_interface: u8,
    pub last_seconds: u16,
    pub last_result: i32,
}

/// Return the current cold-stop interposition counters.
pub fn direct_cold_stop_snapshot() -> DirectColdStopSnapshot {
    DirectColdStopSnapshot {
        calls: CALLS.load(Ordering::Relaxed),
        prestart_successes: PRESTART_SUCCESSES.load(Ordering::Relaxed),
        active_rejections: ACTIVE_REJECTIONS.load(Ordering::Relaxed),
        last_state: LAST_STATE.load(Ordering::Relaxed) as u8,
    }
}

/// Return the current direct set-mode counters.
pub fn direct_set_mode_snapshot() -> DirectSetModeSnapshot {
    DirectSetModeSnapshot {
        calls: SET_MODE_CALLS.load(Ordering::Relaxed),
        not_initialized: SET_MODE_NOT_INITIALIZED.load(Ordering::Relaxed),
        last_mode: SET_MODE_LAST_MODE.load(Ordering::Relaxed) as u8,
        last_result: SET_MODE_LAST_RESULT.load(Ordering::Relaxed) as i32,
    }
}

/// Return the current direct power-save counters.
pub fn direct_set_ps_snapshot() -> DirectSetPsSnapshot {
    DirectSetPsSnapshot {
        calls: SET_PS_CALLS.load(Ordering::Relaxed),
        not_initialized: SET_PS_NOT_INITIALIZED.load(Ordering::Relaxed),
        invalid_arguments: SET_PS_INVALID_ARGUMENTS.load(Ordering::Relaxed),
        last_ps_type: SET_PS_LAST_TYPE.load(Ordering::Relaxed) as u8,
        last_result: SET_PS_LAST_RESULT.load(Ordering::Relaxed) as i32,
    }
}

/// Return the current direct RX callback registration counters.
pub fn direct_reg_rxcb_snapshot() -> DirectRegRxcbSnapshot {
    DirectRegRxcbSnapshot {
        calls: REG_RXCB_CALLS.load(Ordering::Relaxed),
        not_initialized: REG_RXCB_NOT_INITIALIZED.load(Ordering::Relaxed),
        invalid_interfaces: REG_RXCB_INVALID_INTERFACES.load(Ordering::Relaxed),
        last_interface: REG_RXCB_LAST_INTERFACE.load(Ordering::Relaxed) as u8,
        last_result: REG_RXCB_LAST_RESULT.load(Ordering::Relaxed) as i32,
    }
}

/// Return the current direct management-frame registration counters.
pub fn direct_reg_mgmt_frame_snapshot() -> DirectRegMgmtFrameSnapshot {
    DirectRegMgmtFrameSnapshot {
        calls: REG_MGMT_FRAME_CALLS.load(Ordering::Relaxed),
        not_initialized: REG_MGMT_FRAME_NOT_INITIALIZED.load(Ordering::Relaxed),
        last_frame_subtype_mask: REG_MGMT_FRAME_LAST_MASK.load(Ordering::Relaxed),
        last_context: REG_MGMT_FRAME_LAST_CONTEXT.load(Ordering::Relaxed) as usize,
        last_result: REG_MGMT_FRAME_LAST_RESULT.load(Ordering::Relaxed) as i32,
    }
}

/// Return the current direct maximum-TX-power counters.
pub fn direct_set_max_tx_power_snapshot() -> DirectSetMaxTxPowerSnapshot {
    DirectSetMaxTxPowerSnapshot {
        calls: SET_MAX_TX_POWER_CALLS.load(Ordering::Relaxed),
        not_initialized: SET_MAX_TX_POWER_NOT_INITIALIZED.load(Ordering::Relaxed),
        not_started: SET_MAX_TX_POWER_NOT_STARTED.load(Ordering::Relaxed),
        invalid_arguments: SET_MAX_TX_POWER_INVALID_ARGUMENTS.load(Ordering::Relaxed),
        last_power: SET_MAX_TX_POWER_LAST_POWER.load(Ordering::Relaxed) as u8 as i8,
        last_result: SET_MAX_TX_POWER_LAST_RESULT.load(Ordering::Relaxed) as i32,
    }
}

/// Return the current NVS-free pre-start country counters.
pub fn direct_set_country_snapshot() -> DirectSetCountrySnapshot {
    let code = SET_COUNTRY_LAST_CODE.load(Ordering::Relaxed).to_le_bytes();
    DirectSetCountrySnapshot {
        calls: SET_COUNTRY_CALLS.load(Ordering::Relaxed),
        not_initialized: SET_COUNTRY_NOT_INITIALIZED.load(Ordering::Relaxed),
        active_rejections: SET_COUNTRY_ACTIVE_REJECTIONS.load(Ordering::Relaxed),
        nvs_rejections: SET_COUNTRY_NVS_REJECTIONS.load(Ordering::Relaxed),
        invalid_arguments: SET_COUNTRY_INVALID_ARGUMENTS.load(Ordering::Relaxed),
        publications: SET_COUNTRY_PUBLICATIONS.load(Ordering::Relaxed),
        last_country_code: [code[0], code[1], code[2]],
        last_result: SET_COUNTRY_LAST_RESULT.load(Ordering::Relaxed) as i32,
    }
}

/// Return the current direct interface-configuration counters.
pub fn direct_set_config_snapshot() -> DirectSetConfigSnapshot {
    DirectSetConfigSnapshot {
        calls: SET_CONFIG_CALLS.load(Ordering::Relaxed),
        not_initialized: SET_CONFIG_NOT_INITIALIZED.load(Ordering::Relaxed),
        invalid_interfaces: SET_CONFIG_INVALID_INTERFACES.load(Ordering::Relaxed),
        invalid_arguments: SET_CONFIG_INVALID_ARGUMENTS.load(Ordering::Relaxed),
        last_interface: SET_CONFIG_LAST_INTERFACE.load(Ordering::Relaxed) as u8,
        last_result: SET_CONFIG_LAST_RESULT.load(Ordering::Relaxed) as i32,
    }
}

/// Return the current NVS-free protocol-publication counters.
pub fn direct_set_protocols_snapshot() -> DirectSetProtocolsSnapshot {
    let bitmaps = SET_PROTOCOLS_LAST_BITMAPS.load(Ordering::Relaxed);
    DirectSetProtocolsSnapshot {
        calls: SET_PROTOCOLS_CALLS.load(Ordering::Relaxed),
        not_initialized: SET_PROTOCOLS_NOT_INITIALIZED.load(Ordering::Relaxed),
        active_idempotent_successes: SET_PROTOCOLS_ACTIVE_IDEMPOTENT_SUCCESSES
            .load(Ordering::Relaxed),
        active_rejections: SET_PROTOCOLS_ACTIVE_REJECTIONS.load(Ordering::Relaxed),
        invalid_interfaces: SET_PROTOCOLS_INVALID_INTERFACES.load(Ordering::Relaxed),
        invalid_arguments: SET_PROTOCOLS_INVALID_ARGUMENTS.load(Ordering::Relaxed),
        publications: SET_PROTOCOLS_PUBLICATIONS.load(Ordering::Relaxed),
        last_interface: SET_PROTOCOLS_LAST_INTERFACE.load(Ordering::Relaxed) as u8,
        last_2_4_ghz_bitmap: bitmaps as u16,
        last_5_ghz_bitmap: (bitmaps >> 16) as u16,
        last_result: SET_PROTOCOLS_LAST_RESULT.load(Ordering::Relaxed) as i32,
    }
}

/// Return the current idempotent promiscuous-mode counters.
pub fn direct_promiscuous_snapshot() -> DirectPromiscuousSnapshot {
    DirectPromiscuousSnapshot {
        calls: SET_PROMISCUOUS_CALLS.load(Ordering::Relaxed),
        not_initialized: SET_PROMISCUOUS_NOT_INITIALIZED.load(Ordering::Relaxed),
        idempotent_successes: SET_PROMISCUOUS_IDEMPOTENT_SUCCESSES.load(Ordering::Relaxed),
        transition_rejections: SET_PROMISCUOUS_TRANSITION_REJECTIONS.load(Ordering::Relaxed),
        last_requested: SET_PROMISCUOUS_LAST_REQUESTED.load(Ordering::Relaxed) != 0,
        last_state: SET_PROMISCUOUS_LAST_STATE.load(Ordering::Relaxed) as u8,
        last_result: SET_PROMISCUOUS_LAST_RESULT.load(Ordering::Relaxed) as i32,
    }
}

/// Return the current direct inactivity-time publication counters.
pub fn direct_set_inactive_time_snapshot() -> DirectSetInactiveTimeSnapshot {
    DirectSetInactiveTimeSnapshot {
        calls: SET_INACTIVE_TIME_CALLS.load(Ordering::Relaxed),
        not_initialized: SET_INACTIVE_TIME_NOT_INITIALIZED.load(Ordering::Relaxed),
        not_started: SET_INACTIVE_TIME_NOT_STARTED.load(Ordering::Relaxed),
        invalid_arguments: SET_INACTIVE_TIME_INVALID_ARGUMENTS.load(Ordering::Relaxed),
        invalid_modes: SET_INACTIVE_TIME_INVALID_MODES.load(Ordering::Relaxed),
        publications: SET_INACTIVE_TIME_PUBLICATIONS.load(Ordering::Relaxed),
        last_interface: SET_INACTIVE_TIME_LAST_INTERFACE.load(Ordering::Relaxed) as u8,
        last_seconds: SET_INACTIVE_TIME_LAST_SECONDS.load(Ordering::Relaxed) as u16,
        last_result: SET_INACTIVE_TIME_LAST_RESULT.load(Ordering::Relaxed) as i32,
    }
}

fn classify_cold_stop(state: u8) -> i32 {
    if state < WIFI_STATE_STARTED {
        ESP_OK
    } else {
        ESP_ERR_WIFI_NOT_STARTED
    }
}

fn validate_ps_type(ps_type: u32) -> Result<u8, i32> {
    if ps_type <= MAX_PS_TYPE {
        Ok(ps_type as u8)
    } else {
        Err(ESP_ERR_INVALID_ARG)
    }
}

fn validate_max_tx_power(power: i8) -> Result<u8, i32> {
    const MIN_POWER: u8 = 8;
    const MAX_POWER: u8 = 84;

    let power = power as u8;
    if (MIN_POWER..=MAX_POWER).contains(&power) {
        Ok(power)
    } else {
        Err(ESP_ERR_INVALID_ARG)
    }
}

fn country_window_valid(
    requested_start: u8,
    requested_count: u8,
    allowed_start: u8,
    allowed_last: u8,
) -> bool {
    requested_start >= allowed_start
        && requested_start <= allowed_last
        && requested_count != 0
        && u16::from(requested_start) + u16::from(requested_count) <= 15
}

fn normalized_operating_class(value: u8) -> u8 {
    match value {
        b' ' | b'I' | b'O' | b'X' => value,
        _ => b' ',
    }
}

fn interface_enabled_by_mode(interface: u32, mode: u8) -> bool {
    match interface {
        0 => mode & !2 == 1,
        1 => matches!(mode, 2 | 3),
        2 => mode & !2 == 4,
        _ => false,
    }
}

// Keep this pure selector as an explicit final-ELF proof boundary. The strict
// application audit verifies that it is call-free, finite and reached directly
// by the public wrapper.
#[inline(never)]
fn select_2_4_ghz_protocol(
    bitmap: u16,
    supports_2_4_ghz: bool,
    ax_disabled: bool,
) -> Result<(u8, u8), i32> {
    const PROTOCOL_11B: u16 = 1 << 0;
    const PROTOCOL_11G: u16 = 1 << 1;
    const PROTOCOL_11N: u16 = 1 << 2;
    const PROTOCOL_LR: u16 = 1 << 3;
    const PROTOCOL_11A_OR_11AC: u16 = (1 << 4) | (1 << 5);
    const PROTOCOL_11AX: u16 = 1 << 6;
    const KNOWN_PROTOCOLS: u16 = 0x7f;

    if bitmap == 0
        || (supports_2_4_ghz
            && (bitmap & PROTOCOL_11A_OR_11AC != 0 || bitmap & !KNOWN_PROTOCOLS != 0))
        || (ax_disabled && bitmap & PROTOCOL_11AX != 0)
    {
        return Err(ESP_ERR_INVALID_ARG);
    }

    let primary = if bitmap & PROTOCOL_11AX != 0 {
        7
    } else if bitmap & PROTOCOL_11N != 0 {
        3
    } else if bitmap & PROTOCOL_11G != 0 {
        2
    } else if bitmap & PROTOCOL_11B != 0 {
        1
    } else if bitmap == PROTOCOL_LR {
        4
    } else {
        return Err(ESP_ERR_INVALID_ARG);
    };
    Ok((primary, u8::from(bitmap & PROTOCOL_LR != 0)))
}

fn classify_promiscuous_request(requested: bool, current: u8) -> i32 {
    if current == u8::from(requested) {
        ESP_OK
    } else {
        ESP_ERR_WIFI_STATE
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InactiveTimeTarget {
    Station,
    AccessPoint,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InactiveTimeSelectionError {
    InvalidArgument,
    InvalidMode,
}

// Keep validation as a separate final-ELF proof boundary. The strict
// application audit requires this selector to remain call-free and acyclic.
#[inline(never)]
fn select_inactive_time_target(
    interface: u32,
    seconds: u16,
    mode: u8,
) -> Result<InactiveTimeTarget, InactiveTimeSelectionError> {
    match interface {
        0 if seconds > 2 => {
            if mode & !2 == 1 {
                Ok(InactiveTimeTarget::Station)
            } else {
                Err(InactiveTimeSelectionError::InvalidMode)
            }
        }
        1 if seconds > 9 => {
            if matches!(mode, 2 | 3) {
                Ok(InactiveTimeTarget::AccessPoint)
            } else {
                Err(InactiveTimeSelectionError::InvalidMode)
            }
        }
        _ => Err(InactiveTimeSelectionError::InvalidArgument),
    }
}

#[cfg(all(
    target_arch = "riscv32",
    any(
        feature = "rust-direct-cold-stop",
        feature = "rust-direct-set-max-tx-power",
        feature = "rust-direct-set-country-nvs-free",
        feature = "rust-direct-set-protocols-nvs-free",
        feature = "rust-direct-promiscuous-idempotent",
        feature = "rust-direct-set-inactive-time-nvs-free"
    )
))]
unsafe extern "C" {
    static g_ic: u8;
}

#[cfg(all(target_arch = "riscv32", feature = "rust-direct-set-country-nvs-free"))]
unsafe extern "C" {
    static g_wifi_menuconfig: u8;
    fn ieee80211_regdomain_get_country_info(
        requested: *const WifiCountry,
        normalized: *mut WifiCountry,
        allowed_last: *mut u8,
    ) -> i32;
}

#[cfg(all(
    target_arch = "riscv32",
    any(
        feature = "rust-direct-set-country-nvs-free",
        feature = "rust-direct-set-protocols-nvs-free",
        feature = "rust-direct-set-inactive-time-nvs-free"
    )
))]
unsafe extern "C" {
    static mut g_wifi_nvs: *mut u8;
}

#[cfg(all(
    target_arch = "riscv32",
    any(
        feature = "rust-direct-set-mode",
        feature = "rust-direct-set-ps",
        feature = "rust-direct-reg-rxcb",
        feature = "rust-direct-reg-mgmt-frame",
        feature = "rust-direct-set-max-tx-power",
        feature = "rust-direct-set-country-nvs-free",
        feature = "rust-direct-set-protocols-nvs-free",
        feature = "rust-direct-promiscuous-idempotent",
        feature = "rust-direct-set-inactive-time-nvs-free"
    )
))]
unsafe extern "C" {
    fn wifi_init_completed() -> i32;
}

#[cfg(all(target_arch = "riscv32", feature = "rust-direct-reg-rxcb"))]
unsafe extern "C" {
    fn wifi_set_rxcb_process(request: *mut core::ffi::c_void) -> i32;
}

#[cfg(all(target_arch = "riscv32", feature = "rust-direct-reg-mgmt-frame"))]
unsafe extern "C" {
    fn wifi_register_mgmt_frame(request: *mut core::ffi::c_void) -> i32;
}

#[cfg(all(target_arch = "riscv32", feature = "rust-direct-set-max-tx-power"))]
unsafe extern "C" {
    fn wifi_set_max_tpw(request: *mut core::ffi::c_void) -> i32;
}

#[cfg(all(target_arch = "riscv32", feature = "rust-direct-set-mode"))]
unsafe extern "C" {
    fn wifi_set_mode_process(request: *mut core::ffi::c_void) -> i32;
}

#[cfg(all(target_arch = "riscv32", feature = "rust-direct-set-ps"))]
unsafe extern "C" {
    fn wifi_set_ps_process(request: *mut core::ffi::c_void) -> i32;
}

#[cfg(all(target_arch = "riscv32", feature = "rust-direct-set-config"))]
unsafe extern "C" {
    fn wifi_set_config_process(request: *mut core::ffi::c_void) -> i32;
}

#[cfg(all(
    target_arch = "riscv32",
    feature = "rust-direct-set-protocols-nvs-free"
))]
unsafe extern "C" {
    fn ieee80211_protocol_attach(interface_state: *mut u8, band: u8, protocol: u32);
}

/// Replace only the qualified pre-start use of `esp_wifi_stop`.
///
/// This function is linked through `--wrap=esp_wifi_stop`. It deliberately
/// does not delegate active-radio calls to `__real_esp_wifi_stop`, because that
/// body contains the forbidden delay/retry loop.
#[cfg(all(target_arch = "riscv32", feature = "rust-direct-cold-stop"))]
#[no_mangle]
pub unsafe extern "C" fn __wrap_esp_wifi_stop() -> i32 {
    let state = core::ptr::read_volatile(core::ptr::addr_of!(g_ic).add(WIFI_STATE_OFFSET));
    CALLS.fetch_add(1, Ordering::Relaxed);
    LAST_STATE.store(u32::from(state), Ordering::Relaxed);
    let result = classify_cold_stop(state);
    if result == ESP_OK {
        PRESTART_SUCCESSES.fetch_add(1, Ordering::Relaxed);
    } else {
        ACTIVE_REJECTIONS.fetch_add(1, Ordering::Relaxed);
    }
    result
}

/// Invoke the pinned set-mode process with its exact request layout on stack.
///
/// The process remains the vendor run-to-completion state transition. This
/// wrapper removes only the allocator plus `ieee80211_ioctl` envelope. Strict
/// integration guarantees that upper API calls are serialized by the single
/// Rust radio owner.
#[cfg(all(target_arch = "riscv32", feature = "rust-direct-set-mode"))]
#[no_mangle]
pub unsafe extern "C" fn __wrap_esp_wifi_set_mode(mode: u32) -> i32 {
    SET_MODE_CALLS.fetch_add(1, Ordering::Relaxed);
    SET_MODE_LAST_MODE.store(mode, Ordering::Relaxed);
    if wifi_init_completed() == 0 {
        SET_MODE_NOT_INITIALIZED.fetch_add(1, Ordering::Relaxed);
        SET_MODE_LAST_RESULT.store(ESP_ERR_WIFI_NOT_INIT as u32, Ordering::Relaxed);
        return ESP_ERR_WIFI_NOT_INIT;
    }
    let mut request = ApiRequest::with_byte_argument(mode as u8);
    let result = wifi_set_mode_process(request.as_mut_ptr().cast());
    SET_MODE_LAST_RESULT.store(result as u32, Ordering::Relaxed);
    result
}

/// Invoke the pinned power-save process with its exact request layout on stack.
///
/// The process reads only byte 8 and delegates finite state changes and timer
/// rearming to the already patched asynchronous OSI timer table. Valid values
/// are the vendor ABI's NONE, MIN_MODEM and MAX_MODEM variants (`0..=2`).
#[cfg(all(target_arch = "riscv32", feature = "rust-direct-set-ps"))]
#[no_mangle]
pub unsafe extern "C" fn __wrap_esp_wifi_set_ps(ps_type: u32) -> i32 {
    SET_PS_CALLS.fetch_add(1, Ordering::Relaxed);
    SET_PS_LAST_TYPE.store(ps_type, Ordering::Relaxed);
    if wifi_init_completed() == 0 {
        SET_PS_NOT_INITIALIZED.fetch_add(1, Ordering::Relaxed);
        SET_PS_LAST_RESULT.store(ESP_ERR_WIFI_NOT_INIT as u32, Ordering::Relaxed);
        return ESP_ERR_WIFI_NOT_INIT;
    }
    let ps_type = match validate_ps_type(ps_type) {
        Ok(ps_type) => ps_type,
        Err(error) => {
            SET_PS_INVALID_ARGUMENTS.fetch_add(1, Ordering::Relaxed);
            SET_PS_LAST_RESULT.store(error as u32, Ordering::Relaxed);
            return error;
        }
    };
    let mut request = ApiRequest::with_byte_argument(ps_type);
    let result = wifi_set_ps_process(request.as_mut_ptr().cast());
    SET_PS_LAST_RESULT.store(result as u32, Ordering::Relaxed);
    result
}

/// Register an RX callback through the pinned finite interface dispatcher.
#[cfg(all(target_arch = "riscv32", feature = "rust-direct-reg-rxcb"))]
#[no_mangle]
pub unsafe extern "C" fn __wrap_esp_wifi_internal_reg_rxcb(interface: u32, callback: usize) -> i32 {
    const MAX_INTERFACE: u32 = 2;
    const ESP_ERR_WIFI_IF: i32 = 0x3004;

    REG_RXCB_CALLS.fetch_add(1, Ordering::Relaxed);
    REG_RXCB_LAST_INTERFACE.store(interface, Ordering::Relaxed);
    if wifi_init_completed() == 0 {
        REG_RXCB_NOT_INITIALIZED.fetch_add(1, Ordering::Relaxed);
        REG_RXCB_LAST_RESULT.store(ESP_ERR_WIFI_NOT_INIT as u32, Ordering::Relaxed);
        return ESP_ERR_WIFI_NOT_INIT;
    }
    if interface > MAX_INTERFACE {
        REG_RXCB_INVALID_INTERFACES.fetch_add(1, Ordering::Relaxed);
        REG_RXCB_LAST_RESULT.store(ESP_ERR_WIFI_IF as u32, Ordering::Relaxed);
        return ESP_ERR_WIFI_IF;
    }
    let mut request = ApiRequest::with_rx_callback(interface as u8, callback as u32);
    let result = wifi_set_rxcb_process(request.as_mut_ptr().cast());
    REG_RXCB_LAST_RESULT.store(result as u32, Ordering::Relaxed);
    result
}

/// Publish the management-frame subtype mask and callback context directly.
///
/// The pinned process leaf reads only request words 12 and 20, stores them in
/// the vendor control block, and returns success. The Rust radio owner
/// serializes registration with all other upper API state transitions.
#[cfg(all(target_arch = "riscv32", feature = "rust-direct-reg-mgmt-frame"))]
#[no_mangle]
pub unsafe extern "C" fn __wrap_esp_wifi_register_mgmt_frame_internal(
    frame_subtype_mask: u32,
    context: usize,
) -> i32 {
    REG_MGMT_FRAME_CALLS.fetch_add(1, Ordering::Relaxed);
    REG_MGMT_FRAME_LAST_MASK.store(frame_subtype_mask, Ordering::Relaxed);
    REG_MGMT_FRAME_LAST_CONTEXT.store(context as u32, Ordering::Relaxed);
    if wifi_init_completed() == 0 {
        REG_MGMT_FRAME_NOT_INITIALIZED.fetch_add(1, Ordering::Relaxed);
        REG_MGMT_FRAME_LAST_RESULT.store(ESP_ERR_WIFI_NOT_INIT as u32, Ordering::Relaxed);
        return ESP_ERR_WIFI_NOT_INIT;
    }
    let mut request = ApiRequest::with_mgmt_frame_registration(frame_subtype_mask, context as u32);
    let result = wifi_register_mgmt_frame(request.as_mut_ptr().cast());
    REG_MGMT_FRAME_LAST_RESULT.store(result as u32, Ordering::Relaxed);
    result
}

/// Set maximum TX power through the pinned finite PHY process.
///
/// The public initialization, started-state and value-range checks are
/// preserved. The process reads only byte 8, publishes the PHY limit and
/// rebuilds the fixed 43-entry hardware power table.
#[cfg(all(target_arch = "riscv32", feature = "rust-direct-set-max-tx-power"))]
#[no_mangle]
pub unsafe extern "C" fn __wrap_esp_wifi_set_max_tx_power(power: i8) -> i32 {
    SET_MAX_TX_POWER_CALLS.fetch_add(1, Ordering::Relaxed);
    SET_MAX_TX_POWER_LAST_POWER.store(power as u8 as u32, Ordering::Relaxed);
    if wifi_init_completed() == 0 {
        SET_MAX_TX_POWER_NOT_INITIALIZED.fetch_add(1, Ordering::Relaxed);
        SET_MAX_TX_POWER_LAST_RESULT.store(ESP_ERR_WIFI_NOT_INIT as u32, Ordering::Relaxed);
        return ESP_ERR_WIFI_NOT_INIT;
    }
    let state = core::ptr::read_volatile(core::ptr::addr_of!(g_ic).add(WIFI_STATE_OFFSET));
    if state < WIFI_STATE_STARTED {
        SET_MAX_TX_POWER_NOT_STARTED.fetch_add(1, Ordering::Relaxed);
        SET_MAX_TX_POWER_LAST_RESULT.store(ESP_ERR_WIFI_NOT_STARTED as u32, Ordering::Relaxed);
        return ESP_ERR_WIFI_NOT_STARTED;
    }
    let power = match validate_max_tx_power(power) {
        Ok(power) => power,
        Err(error) => {
            SET_MAX_TX_POWER_INVALID_ARGUMENTS.fetch_add(1, Ordering::Relaxed);
            SET_MAX_TX_POWER_LAST_RESULT.store(error as u32, Ordering::Relaxed);
            return error;
        }
    };
    let mut request = ApiRequest::with_byte_argument(power);
    let result = wifi_set_max_tpw(request.as_mut_ptr().cast());
    SET_MAX_TX_POWER_LAST_RESULT.store(result as u32, Ordering::Relaxed);
    result
}

/// Publish a validated country before radio start without vendor NVS.
///
/// This reproduces the pinned pre-start branch only. Active-radio changes are
/// rejected for the Rust async owner to sequence, and enabling vendor NVS is a
/// hard configuration error. No `wifi_nvs_set`, `wifi_nvs_commit`, stop/start
/// or ioctl function is called.
#[cfg(all(target_arch = "riscv32", feature = "rust-direct-set-country-nvs-free"))]
#[no_mangle]
pub unsafe extern "C" fn __wrap_esp_wifi_set_country(country: *const core::ffi::c_void) -> i32 {
    const WIFI_NVS_COUNTRY_OFFSET: usize = 0x404;
    const WIFI_NVS_COUNTRY_MAX_POWER_OFFSET: usize = WIFI_NVS_COUNTRY_OFFSET + 5;
    const WIFI_NVS_COUNTRY_PADDING_OFFSET: usize = WIFI_NVS_COUNTRY_OFFSET + 6;
    const WIFI_NVS_COUNTRY_POLICY_OFFSET: usize = WIFI_NVS_COUNTRY_OFFSET + 8;
    const WIFI_MENUCONFIG_NVS_ENABLE_OFFSET: usize = 0x24;
    const WIFI_COUNTRY_CHANGED_OFFSET: usize = 0x226;

    SET_COUNTRY_CALLS.fetch_add(1, Ordering::Relaxed);
    if wifi_init_completed() == 0 {
        SET_COUNTRY_NOT_INITIALIZED.fetch_add(1, Ordering::Relaxed);
        SET_COUNTRY_LAST_RESULT.store(ESP_ERR_WIFI_NOT_INIT as u32, Ordering::Relaxed);
        return ESP_ERR_WIFI_NOT_INIT;
    }
    if country.is_null() {
        SET_COUNTRY_INVALID_ARGUMENTS.fetch_add(1, Ordering::Relaxed);
        SET_COUNTRY_LAST_RESULT.store(ESP_ERR_INVALID_ARG as u32, Ordering::Relaxed);
        return ESP_ERR_INVALID_ARG;
    }
    let country = country.cast::<WifiCountry>();
    let country_code = country.cast::<u8>().cast::<u32>().read_unaligned() & 0x00ff_ffff;
    SET_COUNTRY_LAST_CODE.store(country_code, Ordering::Relaxed);
    let state = core::ptr::read_volatile(core::ptr::addr_of!(g_ic).add(WIFI_STATE_OFFSET));
    if state >= WIFI_STATE_STARTED {
        SET_COUNTRY_ACTIVE_REJECTIONS.fetch_add(1, Ordering::Relaxed);
        SET_COUNTRY_LAST_RESULT.store(ESP_ERR_WIFI_STATE as u32, Ordering::Relaxed);
        return ESP_ERR_WIFI_STATE;
    }
    let nvs_enabled = core::ptr::read_volatile(
        core::ptr::addr_of!(g_wifi_menuconfig)
            .add(WIFI_MENUCONFIG_NVS_ENABLE_OFFSET)
            .cast::<u32>(),
    );
    if nvs_enabled != 0 {
        SET_COUNTRY_NVS_REJECTIONS.fetch_add(1, Ordering::Relaxed);
        SET_COUNTRY_LAST_RESULT.store(ESP_ERR_WIFI_NVS as u32, Ordering::Relaxed);
        return ESP_ERR_WIFI_NVS;
    }

    let mut normalized = WifiCountry {
        cc: [0; 3],
        start_channel: 0,
        channel_count: 0,
        max_tx_power: 0,
        _padding: [0; 2],
        policy: 0,
    };
    let mut allowed_last = 0u8;
    if ieee80211_regdomain_get_country_info(country, &mut normalized, &mut allowed_last) < 0
        || !country_window_valid(
            (*country).start_channel,
            (*country).channel_count,
            normalized.start_channel,
            allowed_last,
        )
    {
        SET_COUNTRY_INVALID_ARGUMENTS.fetch_add(1, Ordering::Relaxed);
        SET_COUNTRY_LAST_RESULT.store(ESP_ERR_INVALID_ARG as u32, Ordering::Relaxed);
        return ESP_ERR_INVALID_ARG;
    }

    let config = core::ptr::addr_of!(g_wifi_nvs).read_volatile();
    if config.is_null() {
        SET_COUNTRY_NVS_REJECTIONS.fetch_add(1, Ordering::Relaxed);
        SET_COUNTRY_LAST_RESULT.store(ESP_ERR_WIFI_NVS as u32, Ordering::Relaxed);
        return ESP_ERR_WIFI_NVS;
    }
    let requested_word = country.cast::<u32>().read_unaligned();
    let requested_count = (*country).channel_count;
    let requested_policy = core::ptr::addr_of!((*country).policy).read_unaligned();
    let current_word = config
        .add(WIFI_NVS_COUNTRY_OFFSET)
        .cast::<u32>()
        .read_unaligned();
    let current_count = config.add(WIFI_NVS_COUNTRY_OFFSET + 4).read();
    let current_policy = config
        .add(WIFI_NVS_COUNTRY_POLICY_OFFSET)
        .cast::<u32>()
        .read_unaligned();
    if current_word == requested_word
        && current_count == requested_count
        && current_policy == requested_policy
    {
        SET_COUNTRY_LAST_RESULT.store(ESP_OK as u32, Ordering::Relaxed);
        return ESP_OK;
    }

    config
        .add(WIFI_NVS_COUNTRY_OFFSET)
        .cast::<u32>()
        .write_unaligned(requested_word);
    config
        .add(WIFI_NVS_COUNTRY_OFFSET + 2)
        .write(normalized_operating_class((*country).cc[2]));
    config
        .add(WIFI_NVS_COUNTRY_OFFSET + 4)
        .write(requested_count);
    config
        .add(WIFI_NVS_COUNTRY_MAX_POWER_OFFSET)
        .write(normalized.max_tx_power as u8);
    config
        .add(WIFI_NVS_COUNTRY_PADDING_OFFSET)
        .cast::<u16>()
        .write_unaligned(0);
    config
        .add(WIFI_NVS_COUNTRY_POLICY_OFFSET)
        .cast::<u32>()
        .write_unaligned(requested_policy);
    core::ptr::addr_of!(g_ic)
        .add(WIFI_COUNTRY_CHANGED_OFFSET)
        .cast_mut()
        .write_volatile(1);
    SET_COUNTRY_PUBLICATIONS.fetch_add(1, Ordering::Relaxed);
    SET_COUNTRY_LAST_RESULT.store(ESP_OK as u32, Ordering::Relaxed);
    ESP_OK
}

/// Apply one interface configuration through a fixed caller-stack request.
///
/// The public initialization, interface and pointer guards are preserved.
/// The pinned process is synchronous and consumes the copied payload before
/// returning. This boundary removes the allocator and `ieee80211_ioctl`; it
/// deliberately does not yet claim Rust ownership of the vendor configuration
/// globals touched by the process.
#[cfg(all(target_arch = "riscv32", feature = "rust-direct-set-config"))]
#[no_mangle]
pub unsafe extern "C" fn __wrap_esp_wifi_set_config(
    interface: u32,
    config: *mut core::ffi::c_void,
) -> i32 {
    const MAX_INTERFACE: u32 = 2;

    SET_CONFIG_CALLS.fetch_add(1, Ordering::Relaxed);
    SET_CONFIG_LAST_INTERFACE.store(interface, Ordering::Relaxed);
    if wifi_init_completed() == 0 {
        SET_CONFIG_NOT_INITIALIZED.fetch_add(1, Ordering::Relaxed);
        SET_CONFIG_LAST_RESULT.store(ESP_ERR_WIFI_NOT_INIT as u32, Ordering::Relaxed);
        return ESP_ERR_WIFI_NOT_INIT;
    }
    if interface > MAX_INTERFACE {
        SET_CONFIG_INVALID_INTERFACES.fetch_add(1, Ordering::Relaxed);
        SET_CONFIG_LAST_RESULT.store(ESP_ERR_WIFI_IF as u32, Ordering::Relaxed);
        return ESP_ERR_WIFI_IF;
    }
    if config.is_null() {
        SET_CONFIG_INVALID_ARGUMENTS.fetch_add(1, Ordering::Relaxed);
        SET_CONFIG_LAST_RESULT.store(ESP_ERR_INVALID_ARG as u32, Ordering::Relaxed);
        return ESP_ERR_INVALID_ARG;
    }

    let mut request = core::mem::MaybeUninit::<ConfigRequest>::uninit();
    ConfigRequest::initialize(&mut request, interface as u8, config);
    let result = wifi_set_config_process(request.as_mut_ptr().cast());
    SET_CONFIG_LAST_RESULT.store(result as u32, Ordering::Relaxed);
    result
}

/// Publish the selected 2.4 GHz protocol before radio start without NVS/ioctl.
///
/// The pinned public API reduces the bitmap to one primary PHY mode plus the
/// LR flag, then enters an allocator-backed ioctl whose process may stop and
/// restart an active interface and persists the same fields through NVS.
/// Strict initialization needs only the pre-start branch plus the HAL's
/// identical post-start reapplication used to refresh rate control. Rust
/// therefore owns the validation and fixed-state publication, accepts an
/// active idempotent request as a no-op, rejects active-radio changes, and
/// calls the finite protocol attach leaf only when the live interface state
/// actually changed.
#[cfg(all(
    target_arch = "riscv32",
    feature = "rust-direct-set-protocols-nvs-free"
))]
#[no_mangle]
pub unsafe extern "C" fn __wrap_esp_wifi_set_protocols(
    interface: u32,
    protocols: *mut core::ffi::c_void,
) -> i32 {
    const MAX_INTERFACE: u32 = 2;
    const WIFI_CAPABILITIES_OFFSET: usize = 0x59c;
    const WIFI_STA_PROTOCOL_OFFSET: usize = 0x9c;
    const WIFI_AP_PROTOCOL_OFFSET: usize = 0x3fa;
    const WIFI_NAN_PROTOCOL_OFFSET: usize = 0x51c;
    const WIFI_STA_LR_OFFSET: usize = 0x475;
    const WIFI_AP_LR_OFFSET: usize = 0x511;
    const WIFI_STA_STATE_OFFSET: usize = 0x10;
    const WIFI_AP_STATE_OFFSET: usize = 0x14;
    const WIFI_INTERFACE_PROTOCOL_OFFSET: usize = 0x154;
    const WIFI_IC_STA_PROTOCOL_OFFSET: usize = 0x2c0;
    const WIFI_IC_AP_PROTOCOL_OFFSET: usize = 0x2be;
    const WIFI_CONFIG_CHANGED_OFFSET: usize = 0x226;

    SET_PROTOCOLS_CALLS.fetch_add(1, Ordering::Relaxed);
    SET_PROTOCOLS_LAST_INTERFACE.store(interface, Ordering::Relaxed);
    if wifi_init_completed() == 0 {
        SET_PROTOCOLS_NOT_INITIALIZED.fetch_add(1, Ordering::Relaxed);
        SET_PROTOCOLS_LAST_RESULT.store(ESP_ERR_WIFI_NOT_INIT as u32, Ordering::Relaxed);
        return ESP_ERR_WIFI_NOT_INIT;
    }
    if interface > MAX_INTERFACE {
        SET_PROTOCOLS_INVALID_INTERFACES.fetch_add(1, Ordering::Relaxed);
        SET_PROTOCOLS_LAST_RESULT.store(ESP_ERR_WIFI_IF as u32, Ordering::Relaxed);
        return ESP_ERR_WIFI_IF;
    }
    if protocols.is_null() {
        SET_PROTOCOLS_INVALID_ARGUMENTS.fetch_add(1, Ordering::Relaxed);
        SET_PROTOCOLS_LAST_RESULT.store(ESP_ERR_INVALID_ARG as u32, Ordering::Relaxed);
        return ESP_ERR_INVALID_ARG;
    }

    let bitmaps = protocols.cast::<u32>().read_unaligned();
    SET_PROTOCOLS_LAST_BITMAPS.store(bitmaps, Ordering::Relaxed);
    let bitmap_2_4_ghz = bitmaps as u16;
    let bitmap_5_ghz = (bitmaps >> 16) as u16;
    if bitmap_5_ghz != 0 {
        SET_PROTOCOLS_INVALID_ARGUMENTS.fetch_add(1, Ordering::Relaxed);
        SET_PROTOCOLS_LAST_RESULT.store(ESP_ERR_NOT_SUPPORTED as u32, Ordering::Relaxed);
        return ESP_ERR_NOT_SUPPORTED;
    }

    let config = core::ptr::addr_of!(g_wifi_nvs).read_volatile();
    if config.is_null() {
        SET_PROTOCOLS_INVALID_ARGUMENTS.fetch_add(1, Ordering::Relaxed);
        SET_PROTOCOLS_LAST_RESULT.store(ESP_ERR_WIFI_NVS as u32, Ordering::Relaxed);
        return ESP_ERR_WIFI_NVS;
    }
    if !interface_enabled_by_mode(interface, config.read_volatile()) {
        SET_PROTOCOLS_INVALID_INTERFACES.fetch_add(1, Ordering::Relaxed);
        SET_PROTOCOLS_LAST_RESULT.store(ESP_ERR_INVALID_ARG as u32, Ordering::Relaxed);
        return ESP_ERR_INVALID_ARG;
    }

    let capabilities = config
        .add(WIFI_CAPABILITIES_OFFSET)
        .cast::<u32>()
        .read_unaligned();
    // The pinned ESP32-S31 HAL installs `_wifi_disable_ac_ax` as a constant
    // `false` leaf. Encode that target fact here instead of retaining an
    // indirect OSI call which the strict ELF audit cannot prove.
    let (primary, lr) =
        match select_2_4_ghz_protocol(bitmap_2_4_ghz, capabilities & 1 != 0, false) {
            Ok(selection) => selection,
            Err(error) => {
                SET_PROTOCOLS_INVALID_ARGUMENTS.fetch_add(1, Ordering::Relaxed);
                SET_PROTOCOLS_LAST_RESULT.store(error as u32, Ordering::Relaxed);
                return error;
            }
        };

    let ic = core::ptr::addr_of!(g_ic);
    let state = core::ptr::read_volatile(ic.add(WIFI_STATE_OFFSET));
    if state >= WIFI_STATE_STARTED {
        let unchanged = match interface {
            0 => {
                config.add(WIFI_STA_PROTOCOL_OFFSET).read() == primary
                    && ic.add(WIFI_IC_STA_PROTOCOL_OFFSET).read() == primary
                    && config.add(WIFI_STA_LR_OFFSET).read() == lr
            }
            1 => {
                config.add(WIFI_AP_PROTOCOL_OFFSET).read() == primary
                    && ic.add(WIFI_IC_AP_PROTOCOL_OFFSET).read() == primary
                    && config.add(WIFI_AP_LR_OFFSET).read() == lr
            }
            2 => config.add(WIFI_NAN_PROTOCOL_OFFSET).read() == primary,
            _ => unreachable!(),
        };
        if unchanged {
            SET_PROTOCOLS_ACTIVE_IDEMPOTENT_SUCCESSES.fetch_add(1, Ordering::Relaxed);
            SET_PROTOCOLS_LAST_RESULT.store(ESP_OK as u32, Ordering::Relaxed);
            return ESP_OK;
        }
        SET_PROTOCOLS_ACTIVE_REJECTIONS.fetch_add(1, Ordering::Relaxed);
        SET_PROTOCOLS_LAST_RESULT.store(ESP_ERR_WIFI_STATE as u32, Ordering::Relaxed);
        return ESP_ERR_WIFI_STATE;
    }

    match interface {
        0 => {
            let protocol_changed = config.add(WIFI_STA_PROTOCOL_OFFSET).read() != primary
                || ic.add(WIFI_IC_STA_PROTOCOL_OFFSET).read() != primary
                || config.add(WIFI_STA_LR_OFFSET).read() != lr;
            config.add(WIFI_STA_PROTOCOL_OFFSET).write(primary);
            config.add(WIFI_STA_LR_OFFSET).write(lr);
            ic.add(WIFI_IC_STA_PROTOCOL_OFFSET)
                .cast_mut()
                .write(primary);
            if protocol_changed {
                let interface_state = ic
                    .add(WIFI_STA_STATE_OFFSET)
                    .cast::<*mut u8>()
                    .read_unaligned();
                if !interface_state.is_null() {
                    interface_state
                        .add(WIFI_INTERFACE_PROTOCOL_OFFSET)
                        .write(primary);
                    ieee80211_protocol_attach(interface_state, 1, u32::from(primary));
                }
            }
        }
        1 => {
            let protocol_changed = config.add(WIFI_AP_PROTOCOL_OFFSET).read() != primary
                || ic.add(WIFI_IC_AP_PROTOCOL_OFFSET).read() != primary
                || config.add(WIFI_AP_LR_OFFSET).read() != lr;
            config.add(WIFI_AP_PROTOCOL_OFFSET).write(primary);
            config.add(WIFI_AP_LR_OFFSET).write(lr);
            ic.add(WIFI_IC_AP_PROTOCOL_OFFSET).cast_mut().write(primary);
            if protocol_changed {
                let interface_state = ic
                    .add(WIFI_AP_STATE_OFFSET)
                    .cast::<*mut u8>()
                    .read_unaligned();
                if !interface_state.is_null() {
                    interface_state
                        .add(WIFI_INTERFACE_PROTOCOL_OFFSET)
                        .write(primary);
                    ieee80211_protocol_attach(interface_state, 1, u32::from(primary));
                }
            }
        }
        2 => config.add(WIFI_NAN_PROTOCOL_OFFSET).write(primary),
        _ => unreachable!(),
    }
    ic.add(WIFI_CONFIG_CHANGED_OFFSET)
        .cast_mut()
        .write_volatile(1);
    SET_PROTOCOLS_PUBLICATIONS.fetch_add(1, Ordering::Relaxed);
    SET_PROTOCOLS_LAST_RESULT.store(ESP_OK as u32, Ordering::Relaxed);
    ESP_OK
}

/// Admit only an already-satisfied promiscuous-mode request.
///
/// The vendor process starts/stops Wi-Fi hardware and changes the virtual
/// interface when this byte changes. Those synchronous lifecycle operations
/// are outside this compatibility ABI. The strict AP/STA profile asks only to
/// confirm the already-disabled state, so the wrapper preserves the public
/// initialization guard, reads the exact state byte and fails closed on a
/// requested transition.
#[cfg(all(
    target_arch = "riscv32",
    feature = "rust-direct-promiscuous-idempotent"
))]
#[no_mangle]
pub unsafe extern "C" fn __wrap_esp_wifi_set_promiscuous(requested: bool) -> i32 {
    const WIFI_PROMISCUOUS_OFFSET: usize = 0x1f7;

    SET_PROMISCUOUS_CALLS.fetch_add(1, Ordering::Relaxed);
    SET_PROMISCUOUS_LAST_REQUESTED.store(u32::from(requested), Ordering::Relaxed);
    if wifi_init_completed() == 0 {
        SET_PROMISCUOUS_NOT_INITIALIZED.fetch_add(1, Ordering::Relaxed);
        SET_PROMISCUOUS_LAST_RESULT.store(ESP_ERR_WIFI_NOT_INIT as u32, Ordering::Relaxed);
        return ESP_ERR_WIFI_NOT_INIT;
    }
    let current =
        core::ptr::read_volatile(core::ptr::addr_of!(g_ic).add(WIFI_PROMISCUOUS_OFFSET));
    SET_PROMISCUOUS_LAST_STATE.store(u32::from(current), Ordering::Relaxed);
    let result = classify_promiscuous_request(requested, current);
    if result == ESP_OK {
        SET_PROMISCUOUS_IDEMPOTENT_SUCCESSES.fetch_add(1, Ordering::Relaxed);
    } else {
        SET_PROMISCUOUS_TRANSITION_REJECTIONS.fetch_add(1, Ordering::Relaxed);
    }
    SET_PROMISCUOUS_LAST_RESULT.store(result as u32, Ordering::Relaxed);
    result
}

/// Publish the started STA/AP inactivity timeout without IPC, ioctl or NVS.
///
/// The pinned public function entered `esp_wifi_ipc_internal` with a
/// caller-owned stack request. `wifi_ipc_process` synchronously invoked
/// `esp_wifi_set_inactive_time_local`, which validated the interface, timeout
/// and mode, wrote one halfword into the live interface and one into
/// `g_wifi_nvs`, then tail-called `wifi_nvs_set`. The strict target has no
/// persistent Wi-Fi NVS. This wrapper preserves the public guards and exact
/// RAM publications while deliberately omitting that persistence-only tail.
#[cfg(all(
    target_arch = "riscv32",
    feature = "rust-direct-set-inactive-time-nvs-free"
))]
#[no_mangle]
pub unsafe extern "C" fn __wrap_esp_wifi_set_inactive_time(
    interface: u32,
    seconds: u16,
) -> i32 {
    const WIFI_STA_STATE_OFFSET: usize = 0x10;
    const WIFI_AP_STATE_OFFSET: usize = 0x14;
    const WIFI_INTERFACE_INACTIVE_TIME_OFFSET: usize = 0x226;
    const WIFI_STA_INACTIVE_TIME_OFFSET: usize = 0x47a;
    const WIFI_AP_INACTIVE_TIME_OFFSET: usize = 0x516;

    SET_INACTIVE_TIME_CALLS.fetch_add(1, Ordering::Relaxed);
    SET_INACTIVE_TIME_LAST_INTERFACE.store(interface, Ordering::Relaxed);
    SET_INACTIVE_TIME_LAST_SECONDS.store(u32::from(seconds), Ordering::Relaxed);
    if wifi_init_completed() == 0 {
        SET_INACTIVE_TIME_NOT_INITIALIZED.fetch_add(1, Ordering::Relaxed);
        SET_INACTIVE_TIME_LAST_RESULT.store(ESP_ERR_WIFI_NOT_INIT as u32, Ordering::Relaxed);
        return ESP_ERR_WIFI_NOT_INIT;
    }

    let ic = core::ptr::addr_of!(g_ic);
    let state = ic.add(WIFI_STATE_OFFSET).read_volatile();
    if state < WIFI_STATE_STARTED {
        SET_INACTIVE_TIME_NOT_STARTED.fetch_add(1, Ordering::Relaxed);
        SET_INACTIVE_TIME_LAST_RESULT.store(ESP_ERR_WIFI_NOT_STARTED as u32, Ordering::Relaxed);
        return ESP_ERR_WIFI_NOT_STARTED;
    }

    let config = core::ptr::addr_of!(g_wifi_nvs).read_volatile();
    if config.is_null() {
        SET_INACTIVE_TIME_INVALID_MODES.fetch_add(1, Ordering::Relaxed);
        SET_INACTIVE_TIME_LAST_RESULT.store(ESP_ERR_WIFI_NVS as u32, Ordering::Relaxed);
        return ESP_ERR_WIFI_NVS;
    }
    let mode = config.read_volatile();
    let target = match select_inactive_time_target(interface, seconds, mode) {
        Ok(target) => target,
        Err(error) => {
            match error {
                InactiveTimeSelectionError::InvalidArgument => {
                    SET_INACTIVE_TIME_INVALID_ARGUMENTS.fetch_add(1, Ordering::Relaxed);
                }
                InactiveTimeSelectionError::InvalidMode => {
                    SET_INACTIVE_TIME_INVALID_MODES.fetch_add(1, Ordering::Relaxed);
                }
            }
            SET_INACTIVE_TIME_LAST_RESULT.store(ESP_ERR_INVALID_ARG as u32, Ordering::Relaxed);
            return ESP_ERR_INVALID_ARG;
        }
    };

    let (interface_state_offset, config_offset) = match target {
        InactiveTimeTarget::Station => {
            (WIFI_STA_STATE_OFFSET, WIFI_STA_INACTIVE_TIME_OFFSET)
        }
        InactiveTimeTarget::AccessPoint => {
            (WIFI_AP_STATE_OFFSET, WIFI_AP_INACTIVE_TIME_OFFSET)
        }
    };
    let interface_state = ic
        .add(interface_state_offset)
        .cast::<*mut u8>()
        .read_unaligned();
    if interface_state.is_null() {
        SET_INACTIVE_TIME_INVALID_MODES.fetch_add(1, Ordering::Relaxed);
        SET_INACTIVE_TIME_LAST_RESULT.store(ESP_ERR_WIFI_STATE as u32, Ordering::Relaxed);
        return ESP_ERR_WIFI_STATE;
    }
    interface_state
        .add(WIFI_INTERFACE_INACTIVE_TIME_OFFSET)
        .cast::<u16>()
        .write_unaligned(seconds);
    config
        .add(config_offset)
        .cast::<u16>()
        .write_unaligned(seconds);
    SET_INACTIVE_TIME_PUBLICATIONS.fetch_add(1, Ordering::Relaxed);
    SET_INACTIVE_TIME_LAST_RESULT.store(ESP_OK as u32, Ordering::Relaxed);
    ESP_OK
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prestart_states_are_finite_successes() {
        assert_eq!(classify_cold_stop(0), ESP_OK);
        assert_eq!(classify_cold_stop(1), ESP_OK);
    }

    #[test]
    fn active_and_unknown_states_fail_closed() {
        assert_eq!(classify_cold_stop(2), ESP_ERR_WIFI_NOT_STARTED);
        assert_eq!(classify_cold_stop(3), ESP_ERR_WIFI_NOT_STARTED);
        assert_eq!(classify_cold_stop(u8::MAX), ESP_ERR_WIFI_NOT_STARTED);
    }

    #[test]
    fn set_mode_request_has_exact_vendor_layout() {
        assert_eq!(core::mem::size_of::<ApiRequest>(), API_REQUEST_SIZE);
        assert_eq!(core::mem::align_of::<ApiRequest>(), 4);
        assert_eq!(core::mem::offset_of!(ApiRequest, bytes), 0);
        let request = unsafe { ApiRequest::with_byte_argument(3) };
        assert_eq!(
            unsafe {
                request
                    .as_ptr()
                    .cast::<u8>()
                    .add(API_REQUEST_ARGUMENT_OFFSET)
                    .read()
            },
            3
        );
    }

    #[test]
    fn set_config_request_has_exact_vendor_layout() {
        assert_eq!(core::mem::size_of::<ConfigRequest>(), CONFIG_REQUEST_SIZE);
        assert_eq!(core::mem::align_of::<ConfigRequest>(), 4);
        let config = [0x5au8; WIFI_CONFIG_SIZE];
        let mut request = core::mem::MaybeUninit::<ConfigRequest>::uninit();
        unsafe { ConfigRequest::initialize(&mut request, 2, config.as_ptr().cast()) };
        let bytes = unsafe {
            core::slice::from_raw_parts(request.as_ptr().cast::<u8>(), CONFIG_REQUEST_SIZE)
        };
        assert_eq!(bytes[0], 11);
        assert_eq!(bytes[API_REQUEST_ARGUMENT_OFFSET], 2);
        assert_eq!(
            &bytes[CONFIG_REQUEST_PAYLOAD_OFFSET..CONFIG_REQUEST_PAYLOAD_OFFSET + WIFI_CONFIG_SIZE],
            &config,
        );
        assert!(bytes[1..API_REQUEST_ARGUMENT_OFFSET]
            .iter()
            .all(|byte| *byte == 0));
        assert!(
            bytes[API_REQUEST_ARGUMENT_OFFSET + 1..CONFIG_REQUEST_PAYLOAD_OFFSET]
                .iter()
                .all(|byte| *byte == 0)
        );
        assert!(bytes[CONFIG_REQUEST_PAYLOAD_OFFSET + WIFI_CONFIG_SIZE..]
            .iter()
            .all(|byte| *byte == 0));
    }

    #[test]
    fn protocol_bitmap_selection_matches_the_pinned_priority() {
        assert_eq!(select_2_4_ghz_protocol(0x47, true, false), Ok((7, 0)));
        assert_eq!(select_2_4_ghz_protocol(0x0f, true, false), Ok((3, 1)));
        assert_eq!(select_2_4_ghz_protocol(0x03, true, false), Ok((2, 0)));
        assert_eq!(select_2_4_ghz_protocol(0x01, true, false), Ok((1, 0)));
        assert_eq!(select_2_4_ghz_protocol(0x08, true, false), Ok((4, 1)));
    }

    #[test]
    fn protocol_bitmap_validation_rejects_unsupported_shapes() {
        assert_eq!(
            select_2_4_ghz_protocol(0, true, false),
            Err(ESP_ERR_INVALID_ARG)
        );
        assert_eq!(
            select_2_4_ghz_protocol(0x10, true, false),
            Err(ESP_ERR_INVALID_ARG)
        );
        assert_eq!(
            select_2_4_ghz_protocol(0x40, true, true),
            Err(ESP_ERR_INVALID_ARG)
        );
        assert_eq!(
            select_2_4_ghz_protocol(0x80, true, false),
            Err(ESP_ERR_INVALID_ARG)
        );
    }

    #[test]
    fn protocol_interfaces_follow_the_selected_wifi_mode() {
        assert!(interface_enabled_by_mode(0, 1));
        assert!(interface_enabled_by_mode(0, 3));
        assert!(!interface_enabled_by_mode(0, 2));
        assert!(interface_enabled_by_mode(1, 2));
        assert!(interface_enabled_by_mode(1, 3));
        assert!(!interface_enabled_by_mode(1, 1));
        assert!(interface_enabled_by_mode(2, 4));
        assert!(interface_enabled_by_mode(2, 6));
        assert!(!interface_enabled_by_mode(3, 3));
    }

    #[test]
    fn promiscuous_boundary_accepts_only_an_already_satisfied_request() {
        assert_eq!(classify_promiscuous_request(false, 0), ESP_OK);
        assert_eq!(classify_promiscuous_request(true, 1), ESP_OK);
        assert_eq!(
            classify_promiscuous_request(true, 0),
            ESP_ERR_WIFI_STATE
        );
        assert_eq!(
            classify_promiscuous_request(false, 1),
            ESP_ERR_WIFI_STATE
        );
        assert_eq!(
            classify_promiscuous_request(false, 2),
            ESP_ERR_WIFI_STATE
        );
    }

    #[test]
    fn inactivity_time_validation_matches_the_pinned_sta_ap_boundaries() {
        assert_eq!(
            select_inactive_time_target(0, 3, 1),
            Ok(InactiveTimeTarget::Station)
        );
        assert_eq!(
            select_inactive_time_target(0, u16::MAX, 3),
            Ok(InactiveTimeTarget::Station)
        );
        assert_eq!(
            select_inactive_time_target(1, 10, 2),
            Ok(InactiveTimeTarget::AccessPoint)
        );
        assert_eq!(
            select_inactive_time_target(1, u16::MAX, 3),
            Ok(InactiveTimeTarget::AccessPoint)
        );
        assert_eq!(
            select_inactive_time_target(0, 2, 1),
            Err(InactiveTimeSelectionError::InvalidArgument)
        );
        assert_eq!(
            select_inactive_time_target(1, 9, 2),
            Err(InactiveTimeSelectionError::InvalidArgument)
        );
        assert_eq!(
            select_inactive_time_target(2, 10, 4),
            Err(InactiveTimeSelectionError::InvalidArgument)
        );
    }

    #[test]
    fn inactivity_time_rejects_an_interface_disabled_by_mode() {
        assert_eq!(
            select_inactive_time_target(0, 3, 2),
            Err(InactiveTimeSelectionError::InvalidMode)
        );
        assert_eq!(
            select_inactive_time_target(1, 10, 1),
            Err(InactiveTimeSelectionError::InvalidMode)
        );
        assert_eq!(
            select_inactive_time_target(1, 10, 4),
            Err(InactiveTimeSelectionError::InvalidMode)
        );
    }

    #[test]
    fn power_save_type_matches_the_vendor_public_validation() {
        assert_eq!(validate_ps_type(0), Ok(0));
        assert_eq!(validate_ps_type(1), Ok(1));
        assert_eq!(validate_ps_type(2), Ok(2));
        assert_eq!(validate_ps_type(3), Err(ESP_ERR_INVALID_ARG));
        assert_eq!(validate_ps_type(u32::MAX), Err(ESP_ERR_INVALID_ARG));
    }

    #[test]
    fn maximum_tx_power_matches_the_vendor_public_validation() {
        assert_eq!(validate_max_tx_power(7), Err(ESP_ERR_INVALID_ARG));
        assert_eq!(validate_max_tx_power(8), Ok(8));
        assert_eq!(validate_max_tx_power(84), Ok(84));
        assert_eq!(validate_max_tx_power(85), Err(ESP_ERR_INVALID_ARG));
        assert_eq!(validate_max_tx_power(-1), Err(ESP_ERR_INVALID_ARG));
        assert_eq!(validate_max_tx_power(i8::MIN), Err(ESP_ERR_INVALID_ARG));
    }

    #[test]
    fn country_request_layout_matches_the_vendor_abi() {
        assert_eq!(core::mem::size_of::<WifiCountry>(), 12);
        assert_eq!(core::mem::align_of::<WifiCountry>(), 4);
        assert_eq!(core::mem::offset_of!(WifiCountry, cc), 0);
        assert_eq!(core::mem::offset_of!(WifiCountry, start_channel), 3);
        assert_eq!(core::mem::offset_of!(WifiCountry, channel_count), 4);
        assert_eq!(core::mem::offset_of!(WifiCountry, max_tx_power), 5);
        assert_eq!(core::mem::offset_of!(WifiCountry, policy), 8);
    }

    #[test]
    fn country_channel_window_matches_the_vendor_validation() {
        assert!(country_window_valid(1, 13, 1, 13));
        assert!(country_window_valid(1, 11, 1, 11));
        assert!(!country_window_valid(0, 13, 1, 13));
        assert!(!country_window_valid(1, 0, 1, 13));
        assert!(country_window_valid(12, 3, 1, 13));
        assert!(!country_window_valid(13, 3, 1, 13));
        assert!(!country_window_valid(14, 1, 1, 13));
    }

    #[test]
    fn country_operating_class_is_restricted_to_vendor_values() {
        for value in [b' ', b'I', b'O', b'X'] {
            assert_eq!(normalized_operating_class(value), value);
        }
        for value in [0, b'D', b'Z', u8::MAX] {
            assert_eq!(normalized_operating_class(value), b' ');
        }
    }

    #[test]
    fn rx_callback_request_has_exact_vendor_fields() {
        let request = unsafe { ApiRequest::with_rx_callback(2, 0x1234_5678) };
        let bytes = request.as_ptr().cast::<u8>();
        assert_eq!(unsafe { bytes.add(API_REQUEST_ARGUMENT_OFFSET).read() }, 2);
        assert_eq!(
            unsafe { bytes.add(12).cast::<u32>().read_unaligned() },
            0x1234_5678
        );
    }

    #[test]
    fn management_frame_registration_has_exact_vendor_fields() {
        let request = unsafe { ApiRequest::with_mgmt_frame_registration(0x0000_080a, 0x1234_5678) };
        let bytes = request.as_ptr().cast::<u8>();
        assert_eq!(
            unsafe { bytes.add(12).cast::<u32>().read_unaligned() },
            0x0000_080a
        );
        assert_eq!(
            unsafe { bytes.add(20).cast::<u32>().read_unaligned() },
            0x1234_5678
        );
    }
}
