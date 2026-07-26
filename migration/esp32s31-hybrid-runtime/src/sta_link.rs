//! Allocation-free asynchronous STA authentication boundary.
//!
//! The pinned blob's `ieee80211_send_mgmt` dispatcher reaches the complete
//! vendor STA state machine, including allocator, power-save, and retry
//! branches. This module instead owns open-system authentication and calls
//! only the finite management-buffer/TX leaves.

pub const OPEN_AUTH_DEFAULT_TIMEOUT_US: u32 = 500_000;
pub const OPEN_AUTH_DEFAULT_ATTEMPTS: u8 = 3;
pub const STA_ASSOC_DEFAULT_TIMEOUT_US: u32 = 500_000;
pub const STA_ASSOC_DEFAULT_ATTEMPTS: u8 = 3;

#[cfg(any(test, all(target_arch = "riscv32", feature = "strict-no-wait")))]
const SELECTED_RSN_IE_LEN: usize = 22;
#[cfg(any(test, all(target_arch = "riscv32", feature = "strict-no-wait")))]
const RSN_OUI: [u8; 3] = [0x00, 0x0f, 0xac];
#[cfg(any(test, all(target_arch = "riscv32", feature = "strict-no-wait")))]
const RSN_CIPHER_CCMP: u8 = 4;
#[cfg(any(test, all(target_arch = "riscv32", feature = "strict-no-wait")))]
const RSN_AKM_PSK: u8 = 2;
#[cfg(any(test, all(target_arch = "riscv32", feature = "strict-no-wait")))]
const RSN_CAPABILITY_MFPR: u16 = 1 << 6;
#[cfg(any(test, all(target_arch = "riscv32", feature = "strict-no-wait")))]
const HT20_CAPABILITY_IE: [u8; crate::scan::STRICT_SCAN_HT_CAPABILITY_IE_LEN] = [
    45, 26,
    // One-stream HT20 with short guard interval. Channel-width, STBC,
    // LDPC, large A-MSDU, and 40 MHz claims remain disabled until their
    // corresponding Rust-owned paths exist.
    0x20, 0x00, // Smallest advertised receive A-MPDU and no required MPDU spacing.
    0x00, // RX MCS 0..7.
    0xff, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    // RX highest rate is unspecified; TX MCS set is defined and equal to RX.
    0, 0, 0x01, 0, 0, 0, // HT extended capabilities, transmit beamforming, and ASEL.
    0, 0, 0, 0, 0, 0, 0,
];
#[cfg(any(test, all(target_arch = "riscv32", feature = "strict-no-wait")))]
const HE20_MCS9_CAPABILITY_IE: [u8; 24] = [
    255,
    22,
    crate::he::HE_CAPABILITIES_EXTENSION_ID,
    // Exact vendor-oracle HE MAC capability bytes.
    0x03,
    0x18,
    0x9c,
    0xca,
    0x10,
    0x80,
    // Exact one-stream, 20 MHz HE PHY capability bytes.
    0x00,
    0x10,
    0x8a,
    0x1b,
    0x0d,
    0xc0,
    0x1f,
    0x00,
    0x02,
    0x82,
    0x01,
    // RX/TX: NSS1 HE MCS0-9; NSS2-8 unsupported.
    0xfd,
    0xff,
    0xfd,
    0xff,
];
#[cfg(any(test, all(target_arch = "riscv32", feature = "strict-no-wait")))]
const WMM_INFORMATION_IE: [u8; 9] = [221, 7, 0x00, 0x50, 0xf2, 0x02, 0x00, 0x01, 0x00];

#[cfg(feature = "hil-he-association-oracle")]
fn request_he20_mcs9(access_point: &crate::scan::StrictScanRecord) -> bool {
    access_point.ht_capability_ie_present
        && crate::he::parse_he20_capabilities(access_point.he_capability_ie_bytes())
            .ok()
            .is_some_and(|capability| capability.supports_bidirectional_mcs9())
}

#[cfg(not(feature = "hil-he-association-oracle"))]
fn request_he20_mcs9(_access_point: &crate::scan::StrictScanRecord) -> bool {
    false
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaAssocSecurityError {
    MissingRsn,
    MalformedRsn,
    UnsupportedVersion,
    UnsupportedGroupCipher,
    UnsupportedPairwiseCipher,
    UnsupportedAkm,
    ManagementFrameProtectionRequired,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaAssocError {
    Busy,
    InvalidAccessPoint,
    QueueFull,
    InterfaceUnavailable,
    ManagementBufferUnavailable,
    RequestTooLarge,
    TxRejected(i32),
    TimerUnavailable,
    Timeout,
    Status(u16),
    InvalidAssociationId(u16),
    Security(StaAssocSecurityError),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StaAssociation {
    capability_info: u16,
    association_id: u16,
    security_ies: Option<crate::wpa2_frames::OwnedAssociationSecurityIes>,
}

impl StaAssociation {
    pub const fn capability_info(&self) -> u16 {
        self.capability_info
    }

    pub const fn association_id(&self) -> u16 {
        self.association_id
    }

    pub fn security_ies(&self) -> Option<&crate::wpa2_frames::OwnedAssociationSecurityIes> {
        self.security_ies.as_ref()
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct StaAssocSnapshot {
    pub attempts: u32,
    pub submitted: u32,
    pub tx_done: u32,
    pub responses: u32,
    pub timeouts: u32,
    pub last_frame_control: u16,
    pub last_hardware_status: u8,
    pub last_descriptor_status: u32,
    pub last_status: u16,
    pub last_association_id: u16,
    pub last_request_body_len: u16,
    pub ht_requested: bool,
    pub ht_negotiated: bool,
    pub wmm_negotiated: bool,
    pub ht_mcs_count: u8,
    pub fixed_ht20_rate: Option<u8>,
    pub he_requested: bool,
    pub he_capability_len: u16,
    pub he_operation_len: u16,
    pub he_bidirectional_mcs9: bool,
    pub he_bss_color: Option<u8>,
    pub he_peer_state_applied: bool,
    pub addba_requests: u32,
    pub addba_declines_submitted: u32,
    pub addba_accepted_submitted: u32,
    pub addba_deferred: u32,
    pub addba_deferred_dispatched: u32,
    pub action_tx_done: u32,
    pub tx_addba_submitted: u32,
    pub tx_addba_responses: u32,
    pub tx_addba_accepted: u32,
    pub tx_addba_rejected: u32,
    pub tx_addba_timeouts: u32,
    pub tx_addba_last_status: u16,
    pub tx_addba_window: u16,
}

#[cfg(any(test, all(target_arch = "riscv32", feature = "strict-no-wait")))]
fn association_response_ie(frame: &[u8], id: u8) -> Option<&[u8]> {
    let mut offset = 30_usize;
    while offset + 2 <= frame.len() {
        let element_id = frame[offset];
        let length = usize::from(frame[offset + 1]);
        let end = offset.checked_add(2 + length)?;
        if end > frame.len() {
            return None;
        }
        if element_id == id {
            return Some(&frame[offset..end]);
        }
        offset = end;
    }
    None
}

#[cfg(any(test, all(target_arch = "riscv32", feature = "strict-no-wait")))]
fn association_response_extension_ie(frame: &[u8], extension_id: u8) -> Option<&[u8]> {
    let mut offset = 30_usize;
    while offset + 3 <= frame.len() {
        let length = usize::from(frame[offset + 1]);
        let end = offset.checked_add(2 + length)?;
        if end > frame.len() {
            return None;
        }
        if frame[offset] == 255 && length >= 1 && frame[offset + 2] == extension_id {
            return Some(&frame[offset..end]);
        }
        offset = end;
    }
    None
}

#[cfg(any(test, all(target_arch = "riscv32", feature = "strict-no-wait")))]
#[derive(Clone, Copy, Debug)]
struct SelectedRsn {
    len: u8,
    bytes: [u8; crate::scan::STRICT_SCAN_RSN_IE_CAPACITY],
}

#[cfg(any(test, all(target_arch = "riscv32", feature = "strict-no-wait")))]
impl SelectedRsn {
    const EMPTY: Self = Self {
        len: 0,
        bytes: [0; crate::scan::STRICT_SCAN_RSN_IE_CAPACITY],
    };

    fn as_bytes(&self) -> &[u8] {
        &self.bytes[..usize::from(self.len)]
    }
}

#[cfg(any(test, all(target_arch = "riscv32", feature = "strict-no-wait")))]
fn read_rsn_u16(bytes: &[u8], offset: &mut usize) -> Result<u16, StaAssocSecurityError> {
    let value = bytes
        .get(*offset..*offset + 2)
        .ok_or(StaAssocSecurityError::MalformedRsn)?;
    *offset += 2;
    Ok(u16::from_le_bytes([value[0], value[1]]))
}

#[cfg(any(test, all(target_arch = "riscv32", feature = "strict-no-wait")))]
fn read_rsn_suite(bytes: &[u8], offset: &mut usize) -> Result<[u8; 4], StaAssocSecurityError> {
    let value = bytes
        .get(*offset..*offset + 4)
        .ok_or(StaAssocSecurityError::MalformedRsn)?;
    *offset += 4;
    Ok([value[0], value[1], value[2], value[3]])
}

#[cfg(any(test, all(target_arch = "riscv32", feature = "strict-no-wait")))]
fn is_rsn_suite(suite: [u8; 4], selector: u8) -> bool {
    suite[..3] == RSN_OUI && suite[3] == selector
}

#[cfg(any(test, all(target_arch = "riscv32", feature = "strict-no-wait")))]
fn select_wpa2_psk_rsn(
    access_point: &crate::scan::StrictScanRecord,
) -> Result<SelectedRsn, StaAssocSecurityError> {
    if !access_point.privacy && access_point.rsn_ie_len == 0 {
        return Ok(SelectedRsn::EMPTY);
    }
    let rsn = access_point.rsn_ie_bytes();
    if rsn.len() < 2 || rsn[0] != 48 || usize::from(rsn[1]) + 2 != rsn.len() {
        return Err(if rsn.is_empty() {
            StaAssocSecurityError::MissingRsn
        } else {
            StaAssocSecurityError::MalformedRsn
        });
    }
    let body = &rsn[2..];
    let mut offset = 0;
    if read_rsn_u16(body, &mut offset)? != 1 {
        return Err(StaAssocSecurityError::UnsupportedVersion);
    }
    if !is_rsn_suite(read_rsn_suite(body, &mut offset)?, RSN_CIPHER_CCMP) {
        return Err(StaAssocSecurityError::UnsupportedGroupCipher);
    }
    let pairwise_count = usize::from(read_rsn_u16(body, &mut offset)?);
    let mut has_ccmp = false;
    for _ in 0..pairwise_count {
        has_ccmp |= is_rsn_suite(read_rsn_suite(body, &mut offset)?, RSN_CIPHER_CCMP);
    }
    if !has_ccmp {
        return Err(StaAssocSecurityError::UnsupportedPairwiseCipher);
    }
    let akm_count = usize::from(read_rsn_u16(body, &mut offset)?);
    let mut has_psk = false;
    for _ in 0..akm_count {
        has_psk |= is_rsn_suite(read_rsn_suite(body, &mut offset)?, RSN_AKM_PSK);
    }
    if !has_psk {
        return Err(StaAssocSecurityError::UnsupportedAkm);
    }
    if offset < body.len() {
        let capabilities = read_rsn_u16(body, &mut offset)?;
        if capabilities & RSN_CAPABILITY_MFPR != 0 {
            return Err(StaAssocSecurityError::ManagementFrameProtectionRequired);
        }
    }

    let mut selected = SelectedRsn::EMPTY;
    selected.len = SELECTED_RSN_IE_LEN as u8;
    selected.bytes[..SELECTED_RSN_IE_LEN].copy_from_slice(&[
        48,
        20,
        1,
        0,
        0x00,
        0x0f,
        0xac,
        RSN_CIPHER_CCMP,
        1,
        0,
        0x00,
        0x0f,
        0xac,
        RSN_CIPHER_CCMP,
        1,
        0,
        0x00,
        0x0f,
        0xac,
        RSN_AKM_PSK,
        0,
        0,
    ]);
    Ok(selected)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaAuthError {
    Busy,
    InvalidAccessPoint,
    QueueFull,
    InterfaceUnavailable,
    ManagementBufferUnavailable,
    TxRejected(i32),
    TimerUnavailable,
    Timeout,
    Status(u16),
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct StaAuthSnapshot {
    pub attempts: u32,
    pub submitted: u32,
    pub tx_done: u32,
    pub responses: u32,
    pub timeouts: u32,
    pub last_frame_control: u16,
    pub last_hardware_status: u8,
    pub last_descriptor_status: u32,
    pub last_node_rate_count: u8,
    pub last_node_first_rate: u8,
}

#[cfg(all(target_arch = "riscv32", feature = "strict-no-wait"))]
mod target {
    use core::{
        cell::UnsafeCell,
        ffi::c_void,
        ptr,
        sync::atomic::{AtomicU32, AtomicU8, AtomicUsize, Ordering},
    };

    use super::*;
    use crate::{
        interrupt::InterruptSignal,
        scan::StrictScanRecord,
        timer::RawOsiTimer,
        tx_ampdu::{TxBlockAckAlarm, TxBlockAckConfig, TxBlockAckResponse, TxBlockAckSession},
    };

    pub(crate) const STA_AUTH_EVENT: u32 = u32::MAX - 12;
    pub(crate) const STA_ASSOC_EVENT: u32 = u32::MAX - 13;

    const PHASE_IDLE: u8 = 0;
    const PHASE_ARMING: u8 = 1;
    const PHASE_WAITING: u8 = 2;
    const PHASE_COMPLETE: u8 = 3;

    const RESULT_PENDING: u32 = 0;
    const RESULT_OK: u32 = 1;
    const RESULT_TIMEOUT: u32 = 2;
    const RESULT_INTERFACE_UNAVAILABLE: u32 = 3;
    const RESULT_BUFFER_UNAVAILABLE: u32 = 4;
    const RESULT_TIMER_UNAVAILABLE: u32 = 5;
    const RESULT_TX_REJECTED: u32 = 0x4000_0000;
    const RESULT_STATUS: u32 = 0x8000_0000;

    const VENDOR_NODE_LEN: usize = 0x606;
    const AUTH_BODY_LEN: usize = 6;
    const MANAGEMENT_HEADER_LEN: u32 = 24;
    const AUTH_SUBTYPE: u8 = 0xb0;
    const AUTH_PTI: u32 = 6;
    const MANAGEMENT_RATE_POLICY: u32 = 7;
    const ASSOC_SUBTYPE: u8 = 0x00;
    const ASSOC_PTI: u32 = 6;
    const ASSOC_BODY_CAPACITY: usize = 160;
    const ASSOC_FIXED_BODY_LEN: usize = 4;
    const ASSOC_CAPABILITY_MASK: u16 = 0x0431;
    const ASSOC_LISTEN_INTERVAL: u16 = 1;
    const TX_BLOCK_ACK_TID: u8 = 7;
    const TX_BLOCK_ACK_TIMEOUT_US: u32 = 100_000;
    const TX_BLOCK_ACK_CONFIG: TxBlockAckConfig = TxBlockAckConfig {
        tid: TX_BLOCK_ACK_TID,
        window: crate::tx_ampdu::TX_BLOCK_ACK_MAX_WINDOW,
        timeout_tu: 0,
        negotiation_timeout_us: TX_BLOCK_ACK_TIMEOUT_US,
        amsdu: false,
    };
    const TX_BLOCK_ACK_SESSION_INITIAL: TxBlockAckSession =
        match TxBlockAckSession::new(TX_BLOCK_ACK_CONFIG) {
            Ok(session) => session,
            Err(_) => panic!("fixed TX BlockAck config must be valid"),
        };

    #[derive(Clone, Copy)]
    struct AuthConfig {
        local: [u8; 6],
        bssid: [u8; 6],
        channel: u8,
        timeout_us: u32,
        supported_rates: [u8; 8],
        supported_rates_len: u8,
        extended_rates: [u8; crate::scan::STRICT_SCAN_EXTENDED_RATES_CAPACITY],
        extended_rates_len: u8,
    }

    impl AuthConfig {
        const EMPTY: Self = Self {
            local: [0; 6],
            bssid: [0; 6],
            channel: 0,
            timeout_us: 0,
            supported_rates: [0; 8],
            supported_rates_len: 0,
            extended_rates: [0; crate::scan::STRICT_SCAN_EXTENDED_RATES_CAPACITY],
            extended_rates_len: 0,
        };
    }

    #[derive(Clone, Copy)]
    struct AssocConfig {
        local: [u8; 6],
        access_point: StrictScanRecord,
        selected_rsn: SelectedRsn,
        timeout_us: u32,
    }

    impl AssocConfig {
        const EMPTY: Self = Self {
            local: [0; 6],
            access_point: StrictScanRecord::EMPTY,
            selected_rsn: SelectedRsn::EMPTY,
            timeout_us: 0,
        };
    }

    struct ConfigCell(UnsafeCell<AuthConfig>);
    unsafe impl Sync for ConfigCell {}

    struct AssocConfigCell(UnsafeCell<AssocConfig>);
    unsafe impl Sync for AssocConfigCell {}

    #[repr(C, align(4))]
    struct NodeCell(UnsafeCell<[u8; VENDOR_NODE_LEN]>);
    unsafe impl Sync for NodeCell {}

    struct TimerCell(UnsafeCell<RawOsiTimer>);
    unsafe impl Sync for TimerCell {}

    struct TxBlockAckCell(UnsafeCell<TxBlockAckSession>);
    unsafe impl Sync for TxBlockAckCell {}

    #[derive(Clone, Copy)]
    struct PendingRxAddba {
        peer: [u8; 6],
        body: [u8; crate::tx_ampdu::ADDBA_ACTION_BODY_LEN],
    }

    impl PendingRxAddba {
        const EMPTY: Self = Self {
            peer: [0; 6],
            body: [0; crate::tx_ampdu::ADDBA_ACTION_BODY_LEN],
        };
    }

    struct PendingRxAddbaCell(UnsafeCell<PendingRxAddba>);
    unsafe impl Sync for PendingRxAddbaCell {}

    static CONFIG: ConfigCell = ConfigCell(UnsafeCell::new(AuthConfig::EMPTY));
    static ASSOC_CONFIG: AssocConfigCell = AssocConfigCell(UnsafeCell::new(AssocConfig::EMPTY));
    #[unsafe(link_section = ".critical.bss.wifi_strict.sta_node")]
    static NODE: NodeCell = NodeCell(UnsafeCell::new([0; VENDOR_NODE_LEN]));
    static TIMER: TimerCell = TimerCell(UnsafeCell::new(RawOsiTimer {
        next: ptr::null_mut(),
        expire: 0,
        period: 0,
        callback: None,
        argument: ptr::null_mut(),
    }));
    static PHASE: AtomicU8 = AtomicU8::new(PHASE_IDLE);
    static RESULT: AtomicU32 = AtomicU32::new(RESULT_PENDING);
    static SIGNAL: InterruptSignal = InterruptSignal::new();
    static ATTEMPTS: AtomicU32 = AtomicU32::new(0);
    static SUBMITTED: AtomicU32 = AtomicU32::new(0);
    static TX_DONE: AtomicU32 = AtomicU32::new(0);
    static RESPONSES: AtomicU32 = AtomicU32::new(0);
    static TIMEOUTS: AtomicU32 = AtomicU32::new(0);
    static LAST_FRAME_CONTROL: AtomicU32 = AtomicU32::new(0);
    static LAST_HARDWARE_STATUS: AtomicU32 = AtomicU32::new(0);
    static LAST_DESCRIPTOR_STATUS: AtomicU32 = AtomicU32::new(0);
    static LAST_NODE_RATE_COUNT: AtomicU32 = AtomicU32::new(0);
    static LAST_NODE_FIRST_RATE: AtomicU32 = AtomicU32::new(0);

    static ASSOC_TIMER: TimerCell = TimerCell(UnsafeCell::new(RawOsiTimer {
        next: ptr::null_mut(),
        expire: 0,
        period: 0,
        callback: None,
        argument: ptr::null_mut(),
    }));
    #[unsafe(link_section = ".critical.bss.wifi_strict.tx_block_ack")]
    static TX_BLOCK_ACK_SESSION: TxBlockAckCell =
        TxBlockAckCell(UnsafeCell::new(TX_BLOCK_ACK_SESSION_INITIAL));
    #[unsafe(link_section = ".critical.bss.wifi_strict.tx_block_ack")]
    static TX_BLOCK_ACK_TIMER: TimerCell = TimerCell(UnsafeCell::new(RawOsiTimer {
        next: ptr::null_mut(),
        expire: 0,
        period: 0,
        callback: None,
        argument: ptr::null_mut(),
    }));
    static ASSOC_PHASE: AtomicU8 = AtomicU8::new(PHASE_IDLE);
    static ASSOC_RESULT: AtomicU32 = AtomicU32::new(RESULT_PENDING);
    static ASSOC_SIGNAL: InterruptSignal = InterruptSignal::new();
    static RESET_SIGNAL: InterruptSignal = InterruptSignal::new();
    static ASSOC_ATTEMPTS: AtomicU32 = AtomicU32::new(0);
    static ASSOC_SUBMITTED: AtomicU32 = AtomicU32::new(0);
    static ASSOC_TX_DONE: AtomicU32 = AtomicU32::new(0);
    static ASSOC_RESPONSES: AtomicU32 = AtomicU32::new(0);
    static ASSOC_TIMEOUTS: AtomicU32 = AtomicU32::new(0);
    static ASSOC_LAST_FRAME_CONTROL: AtomicU32 = AtomicU32::new(0);
    static ASSOC_LAST_HARDWARE_STATUS: AtomicU32 = AtomicU32::new(0);
    static ASSOC_LAST_DESCRIPTOR_STATUS: AtomicU32 = AtomicU32::new(0);
    static ASSOC_LAST_CAPABILITY: AtomicU32 = AtomicU32::new(0);
    static ASSOC_LAST_STATUS: AtomicU32 = AtomicU32::new(0);
    static ASSOC_LAST_ID: AtomicU32 = AtomicU32::new(0);
    static ASSOC_LAST_BODY_LEN: AtomicU32 = AtomicU32::new(0);
    static ASSOC_HT_REQUESTED: AtomicU32 = AtomicU32::new(0);
    static ASSOC_HT_NEGOTIATED: AtomicU32 = AtomicU32::new(0);
    static ASSOC_WMM_NEGOTIATED: AtomicU32 = AtomicU32::new(0);
    static ASSOC_HT_MCS_COUNT: AtomicU32 = AtomicU32::new(0);
    static ASSOC_FIXED_HT20_RATE: AtomicU32 = AtomicU32::new(u32::MAX);
    static ASSOC_HE_REQUESTED: AtomicU32 = AtomicU32::new(0);
    static ASSOC_HE_CAPABILITY_LEN: AtomicU32 = AtomicU32::new(0);
    static ASSOC_HE_OPERATION_LEN: AtomicU32 = AtomicU32::new(0);
    static ASSOC_HE_BIDIRECTIONAL_MCS9: AtomicU32 = AtomicU32::new(0);
    static ASSOC_HE_BSS_COLOR: AtomicU32 = AtomicU32::new(u32::MAX);
    static ASSOC_HE_PEER_STATE_APPLIED: AtomicU32 = AtomicU32::new(0);
    static ADDBA_REQUESTS: AtomicU32 = AtomicU32::new(0);
    static ADDBA_DECLINES_SUBMITTED: AtomicU32 = AtomicU32::new(0);
    static ADDBA_ACCEPTED_SUBMITTED: AtomicU32 = AtomicU32::new(0);
    static ADDBA_DEFERRED: AtomicU32 = AtomicU32::new(0);
    static ADDBA_DEFERRED_DISPATCHED: AtomicU32 = AtomicU32::new(0);
    static ACTION_TX_DONE: AtomicU32 = AtomicU32::new(0);
    static OWNED_ACTION_BUFFER: AtomicUsize = AtomicUsize::new(0);
    static OWNED_RX_ADDBA_RESPONSE: AtomicU8 = AtomicU8::new(0);
    static OWNED_RX_ADDBA_ACCEPTED: AtomicU8 = AtomicU8::new(0);
    static PENDING_RX_ADDBA_STATE: AtomicU8 = AtomicU8::new(0);
    #[unsafe(link_section = ".critical.bss.wifi_strict.rx_addba")]
    static PENDING_RX_ADDBA: PendingRxAddbaCell =
        PendingRxAddbaCell(UnsafeCell::new(PendingRxAddba::EMPTY));
    static TX_ADDBA_SUBMITTED: AtomicU32 = AtomicU32::new(0);
    static TX_ADDBA_SESSION_ATTEMPTS: AtomicU32 = AtomicU32::new(0);
    static TX_ADDBA_RESPONSES: AtomicU32 = AtomicU32::new(0);
    static TX_ADDBA_ACCEPTED: AtomicU32 = AtomicU32::new(0);
    static TX_ADDBA_REJECTED: AtomicU32 = AtomicU32::new(0);
    static TX_ADDBA_TIMEOUTS: AtomicU32 = AtomicU32::new(0);
    static TX_ADDBA_LAST_STATUS: AtomicU32 = AtomicU32::new(0);
    static TX_ADDBA_WINDOW: AtomicU32 = AtomicU32::new(0);
    static TX_ADDBA_ALARM_GENERATION: AtomicU32 = AtomicU32::new(0);
    const TX_ADDBA_MAX_ATTEMPTS: u32 = 3;

    unsafe extern "C" {
        static mut g_ic: u8;
        static mut g_per_conn_trc: u8;
        fn ieee80211_getmgtframe(
            body: *mut *mut u8,
            header_length: u32,
            body_length: u32,
        ) -> *mut u8;
        fn esf_buf_recycle(buffer: *mut c_void);
        fn ieee80211_set_tx_desc(
            node: *mut u8,
            buffer: *mut u8,
            rate_policy: u32,
            tid: u32,
            flags: u32,
        );
        #[link_name = "ieee80211_set_tx_pti"]
        fn linked_ieee80211_set_tx_pti(buffer: *mut u8, packet_type: u32);
        #[link_name = "ieee80211_mgmt_output"]
        fn linked_ieee80211_mgmt_output(node: *mut u8, buffer: *mut u8, subtype: u8) -> i32;
    }
    const PHY_RATE_MCS7_SGI: u32 = 0x21;

    fn station_interface() -> *mut u8 {
        crate::net80211_state::station_interface()
            .map(|interface| interface.as_ptr())
            .unwrap_or(ptr::null_mut())
    }

    unsafe fn set_default_sta_fixed_rate(rate: u8) -> bool {
        // `trc_init` installs the three allocation-backed default contexts at
        // g_per_conn_trc + 0x4c/0x50/0x54. Strict takeover happens only after
        // that initialization. Interface 0 uses the first pointer. `rcGetSched`
        // consumes flag bit 0 and byte +8 as its fixed-rate fast path.
        let trc = ptr::addr_of_mut!(g_per_conn_trc)
            .add(0x4c)
            .cast::<*mut u8>()
            .read();
        if trc.is_null() {
            return false;
        }
        trc.add(8).write(rate);
        trc.add(9).write(rate);
        let flags = trc.add(0x0c).cast::<u16>();
        flags.write_unaligned((flags.read_unaligned() & !0x03) | 0x01);
        true
    }

    fn decode_result(result: u32) -> Result<(), StaAuthError> {
        match result {
            RESULT_OK => Ok(()),
            RESULT_TIMEOUT => Err(StaAuthError::Timeout),
            RESULT_INTERFACE_UNAVAILABLE => Err(StaAuthError::InterfaceUnavailable),
            RESULT_BUFFER_UNAVAILABLE => Err(StaAuthError::ManagementBufferUnavailable),
            RESULT_TIMER_UNAVAILABLE => Err(StaAuthError::TimerUnavailable),
            value if value & RESULT_STATUS != 0 => Err(StaAuthError::Status(value as u16)),
            value if value & RESULT_TX_REJECTED != 0 => Err(StaAuthError::TxRejected(i32::from(
                (value & 0xffff) as u16 as i16,
            ))),
            value => Err(StaAuthError::TxRejected(value as i32)),
        }
    }

    fn complete(result: u32) {
        if PHASE
            .compare_exchange(
                PHASE_WAITING,
                PHASE_COMPLETE,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
        {
            RESULT.store(result, Ordering::Release);
            SIGNAL.notify_from_isr();
        }
    }

    pub fn sta_auth_snapshot() -> StaAuthSnapshot {
        StaAuthSnapshot {
            attempts: ATTEMPTS.load(Ordering::Acquire),
            submitted: SUBMITTED.load(Ordering::Acquire),
            tx_done: TX_DONE.load(Ordering::Acquire),
            responses: RESPONSES.load(Ordering::Acquire),
            timeouts: TIMEOUTS.load(Ordering::Acquire),
            last_frame_control: LAST_FRAME_CONTROL.load(Ordering::Acquire) as u16,
            last_hardware_status: LAST_HARDWARE_STATUS.load(Ordering::Acquire) as u8,
            last_descriptor_status: LAST_DESCRIPTOR_STATUS.load(Ordering::Acquire),
            last_node_rate_count: LAST_NODE_RATE_COUNT.load(Ordering::Acquire) as u8,
            last_node_first_rate: LAST_NODE_FIRST_RATE.load(Ordering::Acquire) as u8,
        }
    }

    fn complete_assoc(result: u32) {
        if ASSOC_PHASE
            .compare_exchange(
                PHASE_WAITING,
                PHASE_COMPLETE,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
        {
            ASSOC_RESULT.store(result, Ordering::Release);
            ASSOC_SIGNAL.notify_from_isr();
        }
    }

    pub fn sta_assoc_snapshot() -> StaAssocSnapshot {
        StaAssocSnapshot {
            attempts: ASSOC_ATTEMPTS.load(Ordering::Acquire),
            submitted: ASSOC_SUBMITTED.load(Ordering::Acquire),
            tx_done: ASSOC_TX_DONE.load(Ordering::Acquire),
            responses: ASSOC_RESPONSES.load(Ordering::Acquire),
            timeouts: ASSOC_TIMEOUTS.load(Ordering::Acquire),
            last_frame_control: ASSOC_LAST_FRAME_CONTROL.load(Ordering::Acquire) as u16,
            last_hardware_status: ASSOC_LAST_HARDWARE_STATUS.load(Ordering::Acquire) as u8,
            last_descriptor_status: ASSOC_LAST_DESCRIPTOR_STATUS.load(Ordering::Acquire),
            last_status: ASSOC_LAST_STATUS.load(Ordering::Acquire) as u16,
            last_association_id: ASSOC_LAST_ID.load(Ordering::Acquire) as u16,
            last_request_body_len: ASSOC_LAST_BODY_LEN.load(Ordering::Acquire) as u16,
            ht_requested: ASSOC_HT_REQUESTED.load(Ordering::Acquire) != 0,
            ht_negotiated: ASSOC_HT_NEGOTIATED.load(Ordering::Acquire) != 0,
            wmm_negotiated: ASSOC_WMM_NEGOTIATED.load(Ordering::Acquire) != 0,
            ht_mcs_count: ASSOC_HT_MCS_COUNT.load(Ordering::Acquire) as u8,
            fixed_ht20_rate: match ASSOC_FIXED_HT20_RATE.load(Ordering::Acquire) {
                u32::MAX => None,
                rate => Some(rate as u8),
            },
            he_requested: ASSOC_HE_REQUESTED.load(Ordering::Acquire) != 0,
            he_capability_len: ASSOC_HE_CAPABILITY_LEN.load(Ordering::Acquire) as u16,
            he_operation_len: ASSOC_HE_OPERATION_LEN.load(Ordering::Acquire) as u16,
            he_bidirectional_mcs9: ASSOC_HE_BIDIRECTIONAL_MCS9.load(Ordering::Acquire) != 0,
            he_bss_color: match ASSOC_HE_BSS_COLOR.load(Ordering::Acquire) {
                u32::MAX => None,
                color => Some(color as u8),
            },
            he_peer_state_applied: ASSOC_HE_PEER_STATE_APPLIED.load(Ordering::Acquire) != 0,
            addba_requests: ADDBA_REQUESTS.load(Ordering::Acquire),
            addba_declines_submitted: ADDBA_DECLINES_SUBMITTED.load(Ordering::Acquire),
            addba_accepted_submitted: ADDBA_ACCEPTED_SUBMITTED.load(Ordering::Acquire),
            addba_deferred: ADDBA_DEFERRED.load(Ordering::Acquire),
            addba_deferred_dispatched: ADDBA_DEFERRED_DISPATCHED.load(Ordering::Acquire),
            action_tx_done: ACTION_TX_DONE.load(Ordering::Acquire),
            tx_addba_submitted: TX_ADDBA_SUBMITTED.load(Ordering::Acquire),
            tx_addba_responses: TX_ADDBA_RESPONSES.load(Ordering::Acquire),
            tx_addba_accepted: TX_ADDBA_ACCEPTED.load(Ordering::Acquire),
            tx_addba_rejected: TX_ADDBA_REJECTED.load(Ordering::Acquire),
            tx_addba_timeouts: TX_ADDBA_TIMEOUTS.load(Ordering::Acquire),
            tx_addba_last_status: TX_ADDBA_LAST_STATUS.load(Ordering::Acquire) as u16,
            tx_addba_window: TX_ADDBA_WINDOW.load(Ordering::Acquire) as u16,
        }
    }

    fn decode_assoc_result(
        result: u32,
        selected_rsn: SelectedRsn,
    ) -> Result<StaAssociation, StaAssocError> {
        match result {
            RESULT_OK => {
                let association_id = ASSOC_LAST_ID.load(Ordering::Acquire) as u16;
                if association_id == 0 || association_id > 0x3fff {
                    return Err(StaAssocError::InvalidAssociationId(association_id));
                }
                let security_ies = if selected_rsn.len == 0 {
                    None
                } else {
                    let rsn: crate::wpa2_frames::OwnedRsnIe =
                        crate::wpa2_frames::OwnedRsnIe::try_copy(selected_rsn.as_bytes()).map_err(
                            |_| StaAssocError::Security(StaAssocSecurityError::MalformedRsn),
                        )?;
                    Some(
                        crate::wpa2_frames::OwnedAssociationSecurityIes::try_copy(&rsn, &[])
                            .map_err(|_| {
                                StaAssocError::Security(StaAssocSecurityError::MalformedRsn)
                            })?,
                    )
                };
                Ok(StaAssociation {
                    capability_info: ASSOC_LAST_CAPABILITY.load(Ordering::Acquire) as u16,
                    association_id,
                    security_ies,
                })
            }
            RESULT_TIMEOUT => Err(StaAssocError::Timeout),
            RESULT_INTERFACE_UNAVAILABLE => Err(StaAssocError::InterfaceUnavailable),
            RESULT_BUFFER_UNAVAILABLE => Err(StaAssocError::ManagementBufferUnavailable),
            RESULT_TIMER_UNAVAILABLE => Err(StaAssocError::TimerUnavailable),
            value if value & RESULT_STATUS != 0 => Err(StaAssocError::Status(value as u16)),
            value if value & RESULT_TX_REJECTED != 0 => Err(StaAssocError::TxRejected(i32::from(
                (value & 0xffff) as u16 as i16,
            ))),
            value => Err(StaAssocError::TxRejected(value as i32)),
        }
    }

    async fn associate_attempt(
        access_point: &StrictScanRecord,
        local: [u8; 6],
        selected_rsn: SelectedRsn,
        timeout_us: u32,
    ) -> Result<StaAssociation, StaAssocError> {
        if PHASE.load(Ordering::Acquire) != PHASE_IDLE {
            return Err(StaAssocError::Busy);
        }
        ASSOC_PHASE
            .compare_exchange(
                PHASE_IDLE,
                PHASE_ARMING,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .map_err(|_| StaAssocError::Busy)?;
        unsafe {
            ASSOC_CONFIG.0.get().write(AssocConfig {
                local,
                access_point: *access_point,
                selected_rsn,
                timeout_us,
            });
        }
        ASSOC_RESULT.store(RESULT_PENDING, Ordering::Relaxed);
        let observed = ASSOC_SIGNAL.generation();
        ASSOC_PHASE.store(PHASE_WAITING, Ordering::Release);
        if !crate::adapter::enqueue_internal_event(crate::event::PpEvent {
            kind: STA_ASSOC_EVENT,
            argument: ptr::null_mut(),
        }) {
            ASSOC_PHASE.store(PHASE_IDLE, Ordering::Release);
            return Err(StaAssocError::QueueFull);
        }
        ASSOC_SIGNAL.wait_after(observed).await;
        let result = ASSOC_RESULT.load(Ordering::Acquire);
        ASSOC_PHASE.store(PHASE_IDLE, Ordering::Release);
        decode_assoc_result(result, selected_rsn)
    }

    /// Associate with an already authenticated AP using bounded async retries.
    pub async fn associate_sta(
        access_point: &StrictScanRecord,
        local: [u8; 6],
        timeout_us: u32,
        attempts: u8,
    ) -> Result<StaAssociation, StaAssocError> {
        if !(1..=13).contains(&access_point.channel)
            || access_point.ssid_len == 0
            || access_point.bssid == [0; 6]
            || local == [0; 6]
            || timeout_us == 0
            || attempts == 0
        {
            return Err(StaAssocError::InvalidAccessPoint);
        }
        let selected_rsn = select_wpa2_psk_rsn(access_point).map_err(StaAssocError::Security)?;
        let mut remaining = attempts;
        loop {
            match associate_attempt(access_point, local, selected_rsn, timeout_us).await {
                Err(StaAssocError::Timeout) if remaining > 1 => remaining -= 1,
                result => return result,
            }
        }
    }

    async fn authenticate_attempt(
        access_point: &StrictScanRecord,
        local: [u8; 6],
        timeout_us: u32,
    ) -> Result<(), StaAuthError> {
        if ASSOC_PHASE.load(Ordering::Acquire) != PHASE_IDLE {
            return Err(StaAuthError::Busy);
        }
        PHASE
            .compare_exchange(
                PHASE_IDLE,
                PHASE_ARMING,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .map_err(|_| StaAuthError::Busy)?;
        unsafe {
            CONFIG.0.get().write(AuthConfig {
                local,
                bssid: access_point.bssid,
                channel: access_point.channel,
                timeout_us,
                supported_rates: access_point.supported_rates,
                supported_rates_len: access_point.supported_rates_len,
                extended_rates: access_point.extended_supported_rates,
                extended_rates_len: access_point.extended_supported_rates_len,
            });
        }
        RESULT.store(RESULT_PENDING, Ordering::Relaxed);
        let observed = SIGNAL.generation();
        PHASE.store(PHASE_WAITING, Ordering::Release);
        if !crate::adapter::enqueue_internal_event(crate::event::PpEvent {
            kind: STA_AUTH_EVENT,
            argument: ptr::null_mut(),
        }) {
            PHASE.store(PHASE_IDLE, Ordering::Release);
            return Err(StaAuthError::QueueFull);
        }
        SIGNAL.wait_after(observed).await;
        let result = RESULT.load(Ordering::Acquire);
        PHASE.store(PHASE_IDLE, Ordering::Release);
        decode_result(result)
    }

    /// Authenticate with an AP using bounded async retries.
    ///
    /// Each timeout is a one-shot runtime timer. No retry polls state, delays a
    /// task, blocks an executor thread, or enters the vendor STA state machine.
    pub async fn authenticate_open(
        access_point: &StrictScanRecord,
        local: [u8; 6],
        timeout_us: u32,
        attempts: u8,
    ) -> Result<(), StaAuthError> {
        if !(1..=13).contains(&access_point.channel)
            || access_point.bssid == [0; 6]
            || local == [0; 6]
            || timeout_us == 0
            || attempts == 0
        {
            return Err(StaAuthError::InvalidAccessPoint);
        }
        let mut remaining = attempts;
        loop {
            match authenticate_attempt(access_point, local, timeout_us).await {
                Err(StaAuthError::Timeout) if remaining > 1 => remaining -= 1,
                result => return result,
            }
        }
    }

    unsafe fn initialize_static_node(config: AuthConfig) -> Option<*mut u8> {
        let ic = ptr::addr_of_mut!(g_ic);
        let interface = station_interface();
        if interface.is_null() || interface.add(0x138).cast::<u32>().read() != 0 {
            return None;
        }
        let node = NODE.0.get().cast::<u8>();
        ptr::write_bytes(node, 0, VENDOR_NODE_LEN);
        node.cast::<*mut u8>().write(interface);
        ptr::copy_nonoverlapping(config.bssid.as_ptr(), node.add(4), 6);
        node.add(0xab).write(config.channel);
        node.add(0xac).write(0);
        node.add(0x134).write(4);
        initialize_node_rates(node, interface, config);

        interface.add(0xe4).cast::<*mut u8>().write(node);
        ptr::copy_nonoverlapping(config.bssid.as_ptr(), interface.add(0x9c), 6);
        ptr::copy_nonoverlapping(config.local.as_ptr(), ic.add(0x21a), 6);
        crate::scan::enable_sta_link_rx_policy();
        Some(node)
    }

    unsafe fn initialize_node_rates(node: *mut u8, interface: *const u8, config: AuthConfig) {
        // Exact bounded subset of the pinned `ieee80211_setup_rates` leaf.
        // The interface advertises at most sixteen local legacy rates at
        // 0x156; only rates present in either AP IE are copied to node+0x74.
        let local_length = usize::from(interface.add(0x155).read()).min(16);
        let supported_length = usize::from(config.supported_rates_len).min(8);
        let extended_length = usize::from(config.extended_rates_len)
            .min(crate::scan::STRICT_SCAN_EXTENDED_RATES_CAPACITY);
        let mut output_length = 0_usize;
        let mut basic_count = 0_u8;
        let mut ordinary_count = 0_u8;
        let mut highest_basic = 0_u8;
        let mut highest_ordinary = 0_u8;

        for index in 0..local_length {
            let rate = interface.add(0x156 + index).read();
            let value = rate & 0x7f;
            let supported = config.supported_rates[..supported_length]
                .iter()
                .chain(config.extended_rates[..extended_length].iter())
                .any(|candidate| candidate & 0x7f == value);
            if !supported || output_length == 16 {
                continue;
            }
            node.add(0x74 + output_length).write(rate);
            output_length += 1;
            if rate & 0x80 != 0 {
                basic_count = basic_count.saturating_add(1);
                highest_basic = highest_basic.max(value);
            } else {
                ordinary_count = ordinary_count.saturating_add(1);
                highest_ordinary = highest_ordinary.max(rate);
            }
        }
        node.add(0x73).write(output_length as u8);
        LAST_NODE_RATE_COUNT.store(output_length as u32, Ordering::Relaxed);
        LAST_NODE_FIRST_RATE.store(
            if output_length == 0 {
                0
            } else {
                u32::from(node.add(0x74).read())
            },
            Ordering::Relaxed,
        );
        node.add(0x2ec).write(highest_basic);
        node.add(0x2ed).write(highest_ordinary);
        if basic_count != 4 {
            node.add(0x2f1).write(1);
        }
        if ordinary_count == 1 || highest_ordinary == 0x6c {
            node.add(0x2f2).write(1);
        }
    }

    fn association_body_len(
        access_point: &StrictScanRecord,
        node_rate_count: usize,
        selected_rsn: SelectedRsn,
    ) -> Option<usize> {
        let supported = node_rate_count.min(8);
        let extended = node_rate_count.saturating_sub(supported);
        ASSOC_FIXED_BODY_LEN
            .checked_add(2 + usize::from(access_point.ssid_len))?
            .checked_add(2 + supported)?
            .checked_add(if extended == 0 { 0 } else { 2 + extended })?
            .checked_add(usize::from(selected_rsn.len))
            .and_then(|length| {
                let ht_length = access_point
                    .ht_capability_ie_present
                    .then_some(HT20_CAPABILITY_IE.len() + WMM_INFORMATION_IE.len())
                    .unwrap_or(0);
                let he_length = request_he20_mcs9(access_point)
                    .then_some(HE20_MCS9_CAPABILITY_IE.len())
                    .unwrap_or(0);
                ht_length.checked_add(he_length)?.checked_add(length)
            })
            .filter(|length| *length <= ASSOC_BODY_CAPACITY)
    }

    unsafe fn write_association_body(
        body: *mut u8,
        body_len: usize,
        node: *const u8,
        config: AssocConfig,
    ) -> Result<(), StaAssocError> {
        let mut offset = 0_usize;
        let capability = (config.access_point.capability_info & ASSOC_CAPABILITY_MASK) | 1;
        body.add(offset)
            .cast::<u16>()
            .write_unaligned(capability.to_le());
        offset += 2;
        body.add(offset)
            .cast::<u16>()
            .write_unaligned(ASSOC_LISTEN_INTERVAL.to_le());
        offset += 2;

        let ssid_len = usize::from(config.access_point.ssid_len);
        body.add(offset).write(0);
        body.add(offset + 1).write(ssid_len as u8);
        ptr::copy_nonoverlapping(
            config.access_point.ssid.as_ptr(),
            body.add(offset + 2),
            ssid_len,
        );
        offset += 2 + ssid_len;

        let rate_count = usize::from(node.add(0x73).read()).min(16);
        let supported = rate_count.min(8);
        body.add(offset).write(1);
        body.add(offset + 1).write(supported as u8);
        ptr::copy_nonoverlapping(node.add(0x74), body.add(offset + 2), supported);
        offset += 2 + supported;
        if rate_count > supported {
            let extended = rate_count - supported;
            body.add(offset).write(50);
            body.add(offset + 1).write(extended as u8);
            ptr::copy_nonoverlapping(node.add(0x74 + supported), body.add(offset + 2), extended);
            offset += 2 + extended;
        }
        let selected_rsn = config.selected_rsn.as_bytes();
        ptr::copy_nonoverlapping(selected_rsn.as_ptr(), body.add(offset), selected_rsn.len());
        offset += selected_rsn.len();
        if config.access_point.ht_capability_ie_present {
            ptr::copy_nonoverlapping(
                HT20_CAPABILITY_IE.as_ptr(),
                body.add(offset),
                HT20_CAPABILITY_IE.len(),
            );
            offset += HT20_CAPABILITY_IE.len();
            if request_he20_mcs9(&config.access_point) {
                ptr::copy_nonoverlapping(
                    HE20_MCS9_CAPABILITY_IE.as_ptr(),
                    body.add(offset),
                    HE20_MCS9_CAPABILITY_IE.len(),
                );
                offset += HE20_MCS9_CAPABILITY_IE.len();
            }
            ptr::copy_nonoverlapping(
                WMM_INFORMATION_IE.as_ptr(),
                body.add(offset),
                WMM_INFORMATION_IE.len(),
            );
            offset += WMM_INFORMATION_IE.len();
        }
        if offset != body_len {
            return Err(StaAssocError::RequestTooLarge);
        }
        Ok(())
    }

    pub(crate) fn is_owned_action_management(buffer: *mut u8, subtype: u8) -> bool {
        subtype == 0xd0
            && !buffer.is_null()
            && OWNED_ACTION_BUFFER.load(Ordering::Acquire) == buffer as usize
    }

    pub(crate) unsafe fn complete_owned_action_management(frame: *mut u8) -> bool {
        if frame.is_null()
            || OWNED_ACTION_BUFFER
                .compare_exchange(
                    frame as usize,
                    0,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                )
                .is_err()
        {
            return false;
        }
        OWNED_RX_ADDBA_RESPONSE.store(0, Ordering::Release);
        let accepted_rx_addba = OWNED_RX_ADDBA_ACCEPTED.swap(0, Ordering::AcqRel) != 0;
        #[cfg(feature = "hil-rx-ampdu")]
        if accepted_rx_addba {
            let descriptor = frame.add(0x34).cast::<*mut u8>().read();
            if !descriptor.is_null() && descriptor.add(19).read() == 2 {
                let config = ASSOC_CONFIG.0.get().read();
                crate::rx_ampdu_ap::rollback_failed_response(config.access_point.bssid);
            }
        }
        #[cfg(not(feature = "hil-rx-ampdu"))]
        let _ = (frame, accepted_rx_addba);
        ACTION_TX_DONE.fetch_add(1, Ordering::Relaxed);
        dispatch_deferred_rx_addba();
        true
    }

    fn cancel_owned_action_management(buffer: *mut u8) {
        let _ = OWNED_ACTION_BUFFER.compare_exchange(
            buffer as usize,
            0,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
        OWNED_RX_ADDBA_RESPONSE.store(0, Ordering::Release);
        OWNED_RX_ADDBA_ACCEPTED.store(0, Ordering::Release);
    }

    unsafe extern "C" fn tx_addba_timeout(_argument: *mut c_void) {
        let _ = crate::adapter::cancel_internal_timer(TX_BLOCK_ACK_TIMER.0.get().cast());
        let alarm = TxBlockAckAlarm {
            generation: TX_ADDBA_ALARM_GENERATION.load(Ordering::Acquire),
            deadline_us: 0,
        };
        if (*TX_BLOCK_ACK_SESSION.0.get()).on_alarm(alarm) {
            TX_ADDBA_TIMEOUTS.fetch_add(1, Ordering::Relaxed);
            // A retry is driven exclusively by this expired async timer edge.
            // There is no delay loop or polling path, and the fixed attempt
            // bound keeps a non-responsive peer from retaining work forever.
            if TX_ADDBA_SESSION_ATTEMPTS.load(Ordering::Acquire) < TX_ADDBA_MAX_ATTEMPTS {
                let _ = start_sta_tx_block_ack();
            }
        }
    }

    /// Start a Rust-owned TX ADDBA negotiation after the controlled port is
    /// authorized. This only proves the management protocol and bounded async
    /// deadline. It deliberately does not enable the vendor aggregation
    /// scheduler or attach an allocation-backed vendor BA object.
    pub(crate) unsafe fn start_sta_tx_block_ack() -> bool {
        if !crate::critical::on_strict_wifi_hart()
            || !crate::context::in_radio_context()
            || ASSOC_HT_NEGOTIATED.load(Ordering::Acquire) == 0
            || ASSOC_WMM_NEGOTIATED.load(Ordering::Acquire) == 0
            || TX_ADDBA_SESSION_ATTEMPTS.load(Ordering::Acquire) >= TX_ADDBA_MAX_ATTEMPTS
            || OWNED_ACTION_BUFFER.load(Ordering::Acquire) != 0
        {
            return false;
        }
        let interface = station_interface();
        if interface.is_null() {
            return false;
        }
        let node = interface.add(0xe4).cast::<*mut u8>().read();
        if node != NODE.0.get().cast::<u8>() {
            return false;
        }
        // The pinned node stores the next 12-bit QoS sequence number at
        // node+0xae+2*TID. Rust snapshots it before constructing ADDBA.
        let starting_sequence = node
            .add(0xae + usize::from(TX_BLOCK_ACK_TID) * 2)
            .cast::<u16>()
            .read_unaligned()
            & 0x0fff;
        let session = &mut *TX_BLOCK_ACK_SESSION.0.get();
        if session.is_awaiting() || session.operational().is_some() {
            return false;
        }
        let Ok(request) = session.begin(starting_sequence, 0) else {
            return false;
        };

        let mut body = ptr::null_mut();
        let buffer = ieee80211_getmgtframe(
            &mut body,
            MANAGEMENT_HEADER_LEN,
            crate::tx_ampdu::ADDBA_ACTION_BODY_LEN as u32,
        );
        if buffer.is_null() || body.is_null() {
            session.stop();
            return false;
        }
        ptr::copy_nonoverlapping(
            request.body.as_ptr(),
            body,
            crate::tx_ampdu::ADDBA_ACTION_BODY_LEN,
        );
        buffer
            .add(0x14)
            .cast::<u16>()
            .write_unaligned(MANAGEMENT_HEADER_LEN as u16);
        ieee80211_set_tx_desc(node, buffer, MANAGEMENT_RATE_POLICY, 0, 0);
        linked_ieee80211_set_tx_pti(buffer, ASSOC_PTI);
        if OWNED_ACTION_BUFFER
            .compare_exchange(0, buffer as usize, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            session.stop();
            esf_buf_recycle(buffer.cast());
            return false;
        }
        if linked_ieee80211_mgmt_output(node, buffer, 0xd0) != 0 {
            session.stop();
            cancel_owned_action_management(buffer);
            return false;
        }
        TX_ADDBA_ALARM_GENERATION.store(request.alarm.generation, Ordering::Release);
        TX_ADDBA_SUBMITTED.fetch_add(1, Ordering::Relaxed);
        TX_ADDBA_SESSION_ATTEMPTS.fetch_add(1, Ordering::Relaxed);
        if !crate::adapter::schedule_internal_timer(
            TX_BLOCK_ACK_TIMER.0.get().cast(),
            tx_addba_timeout,
            ptr::null_mut(),
            TX_BLOCK_ACK_TIMEOUT_US,
        ) {
            session.stop();
            return false;
        }
        true
    }

    unsafe fn send_addba_response(node: *mut u8, peer: [u8; 6], request: &[u8]) -> bool {
        const ACTION_BODY_LEN: usize = 9;
        const STATUS_REQUEST_DECLINED: u16 = 37;
        if request.len() < 33 || OWNED_ACTION_BUFFER.load(Ordering::Acquire) != 0 {
            return false;
        }
        let dialog_token = request[26];
        let request_parameters = u16::from_le_bytes([request[27], request[28]]);
        let timeout = u16::from_le_bytes([request[29], request[30]]);
        // Preserve only immediate/delayed policy and TID. A declined response
        // advertises neither A-MSDU nor a receive reorder-buffer size.
        let response_parameters = request_parameters & 0x003e;

        let mut body = ptr::null_mut();
        let buffer =
            ieee80211_getmgtframe(&mut body, MANAGEMENT_HEADER_LEN, ACTION_BODY_LEN as u32);
        if buffer.is_null() || body.is_null() {
            return false;
        }
        body.write(3); // Block Ack category.
        body.add(1).write(1); // ADDBA response.
        body.add(2).write(dialog_token);
        body.add(3)
            .cast::<u16>()
            .write_unaligned(STATUS_REQUEST_DECLINED.to_le());
        body.add(5)
            .cast::<u16>()
            .write_unaligned(response_parameters.to_le());
        body.add(7).cast::<u16>().write_unaligned(timeout.to_le());
        buffer
            .add(0x14)
            .cast::<u16>()
            .write_unaligned(MANAGEMENT_HEADER_LEN as u16);
        ieee80211_set_tx_desc(node, buffer, MANAGEMENT_RATE_POLICY, 0, 0);
        linked_ieee80211_set_tx_pti(buffer, ASSOC_PTI);
        if OWNED_ACTION_BUFFER
            .compare_exchange(0, buffer as usize, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            esf_buf_recycle(buffer.cast());
            return false;
        }
        #[cfg(feature = "hil-rx-ampdu")]
        let rx_ampdu_accepted = crate::rx_ampdu_ap::try_accept_sta_request(
            peer,
            &request[24..33],
            core::slice::from_raw_parts_mut(body, ACTION_BODY_LEN),
        );
        #[cfg(not(feature = "hil-rx-ampdu"))]
        let rx_ampdu_accepted = false;
        OWNED_RX_ADDBA_RESPONSE.store(1, Ordering::Release);
        if rx_ampdu_accepted {
            OWNED_RX_ADDBA_ACCEPTED.store(1, Ordering::Release);
        }
        let result = linked_ieee80211_mgmt_output(node, buffer, 0xd0);
        if result != 0 {
            #[cfg(feature = "hil-rx-ampdu")]
            if rx_ampdu_accepted {
                crate::rx_ampdu_ap::rollback_failed_response(peer);
            }
            cancel_owned_action_management(buffer);
            return false;
        }
        if rx_ampdu_accepted {
            ADDBA_ACCEPTED_SUBMITTED.fetch_add(1, Ordering::Relaxed);
        } else {
            ADDBA_DECLINES_SUBMITTED.fetch_add(1, Ordering::Relaxed);
        }
        true
    }

    unsafe fn defer_rx_addba(peer: [u8; 6], request: &[u8]) -> bool {
        if request.len() < 33
            || PENDING_RX_ADDBA_STATE
                .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
        {
            return false;
        }
        let pending = &mut *PENDING_RX_ADDBA.0.get();
        pending.peer = peer;
        pending
            .body
            .copy_from_slice(&request[24..24 + crate::tx_ampdu::ADDBA_ACTION_BODY_LEN]);
        PENDING_RX_ADDBA_STATE.store(2, Ordering::Release);
        ADDBA_DEFERRED.fetch_add(1, Ordering::Relaxed);
        true
    }

    unsafe fn dispatch_deferred_rx_addba() {
        if PENDING_RX_ADDBA_STATE
            .compare_exchange(2, 1, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }
        let pending = *PENDING_RX_ADDBA.0.get();
        let interface = station_interface();
        let node = if interface.is_null() {
            ptr::null_mut()
        } else {
            interface.add(0xe4).cast::<*mut u8>().read()
        };
        PENDING_RX_ADDBA_STATE.store(0, Ordering::Release);
        let request = {
            let mut frame = [0_u8; 24 + crate::tx_ampdu::ADDBA_ACTION_BODY_LEN];
            frame[24..].copy_from_slice(&pending.body);
            frame
        };
        if node != NODE.0.get().cast::<u8>()
            || !send_addba_response(node, pending.peer, &request)
        {
            let pending_slot = &mut *PENDING_RX_ADDBA.0.get();
            *pending_slot = pending;
            PENDING_RX_ADDBA_STATE.store(2, Ordering::Release);
            return;
        }
        ADDBA_DEFERRED_DISPATCHED.fetch_add(1, Ordering::Relaxed);
    }

    unsafe fn complete_tx_addba_request_ownership() {
        if OWNED_RX_ADDBA_RESPONSE.load(Ordering::Acquire) != 0 {
            return;
        }
        if OWNED_ACTION_BUFFER.swap(0, Ordering::AcqRel) == 0 {
            return;
        }
        OWNED_RX_ADDBA_ACCEPTED.store(0, Ordering::Release);
        dispatch_deferred_rx_addba();
    }

    /// Consume a peer ADDBA request before it can enter the vendor BlockAck
    /// state machine. Until Rust owns reorder buffers, it emits one explicit
    /// standards-level decline using the fixed management pool.
    pub(crate) fn ingest_management_action(frame: &[u8]) -> bool {
        if frame.len() < 33 || frame[0] & 0xfc != 0xd0 || frame[24] != 3 {
            return false;
        }
        let config = unsafe { ASSOC_CONFIG.0.get().read() };
        if frame[4..10] != config.local
            || frame[10..16] != config.access_point.bssid
            || frame[16..22] != config.access_point.bssid
        {
            return false;
        }
        if frame[25] == crate::tx_ampdu::ADDBA_RESPONSE_ACTION {
            let session = unsafe { &mut *TX_BLOCK_ACK_SESSION.0.get() };
            match session.on_response(&frame[24..33]) {
                Ok(TxBlockAckResponse::Operational(agreement)) => {
                    unsafe {
                        let _ = crate::adapter::cancel_internal_timer(
                            TX_BLOCK_ACK_TIMER.0.get().cast(),
                        );
                    }
                    TX_ADDBA_RESPONSES.fetch_add(1, Ordering::Relaxed);
                    TX_ADDBA_ACCEPTED.fetch_add(1, Ordering::Relaxed);
                    TX_ADDBA_LAST_STATUS.store(0, Ordering::Release);
                    TX_ADDBA_WINDOW.store(u32::from(agreement.window), Ordering::Release);
                    #[cfg(feature = "hil-ampdu-intercept")]
                    unsafe {
                        crate::tx_intercept::enable(agreement.window);
                    }
                }
                Ok(TxBlockAckResponse::Rejected(status)) => {
                    unsafe {
                        let _ = crate::adapter::cancel_internal_timer(
                            TX_BLOCK_ACK_TIMER.0.get().cast(),
                        );
                    }
                    TX_ADDBA_RESPONSES.fetch_add(1, Ordering::Relaxed);
                    TX_ADDBA_REJECTED.fetch_add(1, Ordering::Relaxed);
                    TX_ADDBA_LAST_STATUS.store(u32::from(status), Ordering::Release);
                    TX_ADDBA_WINDOW.store(0, Ordering::Release);
                }
                Err(_) => {}
            }
            unsafe { complete_tx_addba_request_ownership() };
            return true;
        }
        if frame[25] != crate::tx_ampdu::ADDBA_REQUEST_ACTION {
            return false;
        }
        ADDBA_REQUESTS.fetch_add(1, Ordering::Relaxed);
        let interface = station_interface();
        if interface.is_null() {
            return true;
        }
        let node = unsafe { interface.add(0xe4).cast::<*mut u8>().read() };
        if node != NODE.0.get().cast::<u8>() {
            return true;
        }
        if OWNED_ACTION_BUFFER.load(Ordering::Acquire) != 0 {
            let _ = unsafe { defer_rx_addba(config.access_point.bssid, frame) };
        } else if !unsafe { send_addba_response(node, config.access_point.bssid, frame) } {
            let _ = unsafe { defer_rx_addba(config.access_point.bssid, frame) };
        }
        true
    }

    pub(crate) unsafe fn dispatch_assoc_tx() {
        ASSOC_ATTEMPTS.fetch_add(1, Ordering::Relaxed);
        if ASSOC_PHASE.load(Ordering::Acquire) != PHASE_WAITING
            || !crate::critical::on_strict_wifi_hart()
            || !crate::context::in_radio_context()
        {
            complete_assoc(RESULT_INTERFACE_UNAVAILABLE);
            return;
        }
        let config = ASSOC_CONFIG.0.get().read();
        let node_config = AuthConfig {
            local: config.local,
            bssid: config.access_point.bssid,
            channel: config.access_point.channel,
            timeout_us: config.timeout_us,
            supported_rates: config.access_point.supported_rates,
            supported_rates_len: config.access_point.supported_rates_len,
            extended_rates: config.access_point.extended_supported_rates,
            extended_rates_len: config.access_point.extended_supported_rates_len,
        };
        let Some(node) = initialize_static_node(node_config) else {
            complete_assoc(RESULT_INTERFACE_UNAVAILABLE);
            return;
        };
        ASSOC_HT_REQUESTED.store(
            u32::from(config.access_point.ht_capability_ie_present),
            Ordering::Relaxed,
        );
        ASSOC_HE_REQUESTED.store(
            u32::from(request_he20_mcs9(&config.access_point)),
            Ordering::Relaxed,
        );
        ASSOC_HT_NEGOTIATED.store(0, Ordering::Relaxed);
        ASSOC_WMM_NEGOTIATED.store(0, Ordering::Relaxed);
        ASSOC_HT_MCS_COUNT.store(0, Ordering::Relaxed);
        ASSOC_HE_CAPABILITY_LEN.store(0, Ordering::Relaxed);
        ASSOC_HE_OPERATION_LEN.store(0, Ordering::Relaxed);
        ASSOC_HE_BIDIRECTIONAL_MCS9.store(0, Ordering::Relaxed);
        ASSOC_HE_BSS_COLOR.store(u32::MAX, Ordering::Relaxed);
        ASSOC_HE_PEER_STATE_APPLIED.store(0, Ordering::Relaxed);
        let node_rate_count = usize::from(node.add(0x73).read()).min(16);
        let Some(body_len) =
            association_body_len(&config.access_point, node_rate_count, config.selected_rsn)
        else {
            complete_assoc(RESULT_BUFFER_UNAVAILABLE);
            return;
        };
        let mut body = ptr::null_mut();
        let buffer = ieee80211_getmgtframe(&mut body, MANAGEMENT_HEADER_LEN, body_len as u32);
        if buffer.is_null() || body.is_null() {
            complete_assoc(RESULT_BUFFER_UNAVAILABLE);
            return;
        }
        if write_association_body(body, body_len, node, config).is_err() {
            complete_assoc(RESULT_BUFFER_UNAVAILABLE);
            return;
        }
        buffer
            .add(0x14)
            .cast::<u16>()
            .write_unaligned(MANAGEMENT_HEADER_LEN as u16);
        ASSOC_LAST_BODY_LEN.store(body_len as u32, Ordering::Relaxed);
        ieee80211_set_tx_desc(node, buffer, MANAGEMENT_RATE_POLICY, 0, 0);
        linked_ieee80211_set_tx_pti(buffer, ASSOC_PTI);
        let tx = linked_ieee80211_mgmt_output(node, buffer, ASSOC_SUBTYPE);
        if tx != 0 {
            complete_assoc(RESULT_TX_REJECTED | u32::from(tx as u16));
            return;
        }
        ASSOC_SUBMITTED.fetch_add(1, Ordering::Relaxed);
        if !crate::adapter::schedule_internal_timer(
            ASSOC_TIMER.0.get().cast(),
            assoc_timeout,
            ptr::null_mut(),
            config.timeout_us,
        ) {
            complete_assoc(RESULT_TIMER_UNAVAILABLE);
        }
    }

    unsafe extern "C" fn assoc_timeout(_argument: *mut c_void) {
        let _ = crate::adapter::cancel_internal_timer(ASSOC_TIMER.0.get().cast());
        ASSOC_TIMEOUTS.fetch_add(1, Ordering::Relaxed);
        complete_assoc(RESULT_TIMEOUT);
    }

    pub(crate) unsafe fn dispatch_auth_tx() {
        ATTEMPTS.fetch_add(1, Ordering::Relaxed);
        if PHASE.load(Ordering::Acquire) != PHASE_WAITING
            || !crate::critical::on_strict_wifi_hart()
            || !crate::context::in_radio_context()
        {
            complete(RESULT_INTERFACE_UNAVAILABLE);
            return;
        }
        let config = CONFIG.0.get().read();
        let Some(node) = initialize_static_node(config) else {
            complete(RESULT_INTERFACE_UNAVAILABLE);
            return;
        };
        let mut body = ptr::null_mut();
        let buffer = ieee80211_getmgtframe(&mut body, MANAGEMENT_HEADER_LEN, AUTH_BODY_LEN as u32);
        if buffer.is_null() || body.is_null() {
            complete(RESULT_BUFFER_UNAVAILABLE);
            return;
        }
        // `ieee80211_getmgtframe` stores only the body length at +0x16. The
        // pinned vendor auth constructor separately publishes the reserved
        // 802.11 header length at +0x14 before descriptor construction.
        buffer
            .add(0x14)
            .cast::<u16>()
            .write_unaligned(MANAGEMENT_HEADER_LEN as u16);
        // Open System algorithm, transaction 1, status success/reserved zero.
        body.cast::<u16>().write_unaligned(0);
        body.add(2).cast::<u16>().write_unaligned(1);
        body.add(4).cast::<u16>().write_unaligned(0);
        ieee80211_set_tx_desc(node, buffer, MANAGEMENT_RATE_POLICY, 0, 0);
        linked_ieee80211_set_tx_pti(buffer, AUTH_PTI);
        let tx = linked_ieee80211_mgmt_output(node, buffer, AUTH_SUBTYPE);
        if tx != 0 {
            complete(RESULT_TX_REJECTED | u32::from(tx as u16));
            return;
        }
        SUBMITTED.fetch_add(1, Ordering::Relaxed);
        if !crate::adapter::schedule_internal_timer(
            TIMER.0.get().cast(),
            auth_timeout,
            ptr::null_mut(),
            config.timeout_us,
        ) {
            complete(RESULT_TIMER_UNAVAILABLE);
        }
    }

    unsafe extern "C" fn auth_timeout(_argument: *mut c_void) {
        let _ = crate::adapter::cancel_internal_timer(TIMER.0.get().cast());
        TIMEOUTS.fetch_add(1, Ordering::Relaxed);
        complete(RESULT_TIMEOUT);
    }

    pub(crate) fn management_tx_done(
        frame_control: u16,
        hardware_status: u8,
        descriptor_status: u32,
    ) {
        match frame_control & 0x00fc {
            0x00b0 => {
                LAST_FRAME_CONTROL.store(u32::from(frame_control), Ordering::Relaxed);
                LAST_HARDWARE_STATUS.store(u32::from(hardware_status), Ordering::Relaxed);
                LAST_DESCRIPTOR_STATUS.store(descriptor_status, Ordering::Relaxed);
                TX_DONE.fetch_add(1, Ordering::Release);
            }
            0x0000 => {
                ASSOC_LAST_FRAME_CONTROL.store(u32::from(frame_control), Ordering::Relaxed);
                ASSOC_LAST_HARDWARE_STATUS.store(u32::from(hardware_status), Ordering::Relaxed);
                ASSOC_LAST_DESCRIPTOR_STATUS.store(descriptor_status, Ordering::Relaxed);
                ASSOC_TX_DONE.fetch_add(1, Ordering::Release);
            }
            _ => {}
        }
    }

    pub(crate) fn observe_management(frame: &[u8]) {
        if frame.len() < 30 {
            return;
        }
        let frame_control = u16::from_le_bytes([frame[0], frame[1]]);
        match frame_control & 0x00fc {
            0x00b0 if PHASE.load(Ordering::Acquire) == PHASE_WAITING => {
                observe_auth_response(frame)
            }
            0x0010 if ASSOC_PHASE.load(Ordering::Acquire) == PHASE_WAITING => {
                observe_assoc_response(frame)
            }
            _ => {}
        }
    }

    fn observe_auth_response(frame: &[u8]) {
        let config = unsafe { CONFIG.0.get().read() };
        if frame[4..10] != config.local
            || frame[10..16] != config.bssid
            || frame[16..22] != config.bssid
            || u16::from_le_bytes([frame[24], frame[25]]) != 0
            || u16::from_le_bytes([frame[26], frame[27]]) != 2
        {
            return;
        }
        let status = u16::from_le_bytes([frame[28], frame[29]]);
        RESPONSES.fetch_add(1, Ordering::Relaxed);
        unsafe {
            let _ = crate::adapter::cancel_internal_timer(TIMER.0.get().cast());
        }
        if status == 0 {
            complete(RESULT_OK);
        } else {
            complete(RESULT_STATUS | u32::from(status));
        }
    }

    fn association_response_has_wmm(frame: &[u8]) -> bool {
        let mut offset = 30_usize;
        while offset + 2 <= frame.len() {
            let length = usize::from(frame[offset + 1]);
            let Some(end) = offset.checked_add(2 + length) else {
                return false;
            };
            if end > frame.len() {
                return false;
            }
            let element = &frame[offset..end];
            if element[0] == 221 && length >= 6 && element[2..6] == [0x00, 0x50, 0xf2, 0x02] {
                return true;
            }
            offset = end;
        }
        false
    }

    unsafe fn apply_static_ht_capability(node: *mut u8, element: &[u8]) -> u8 {
        if element.len() != crate::scan::STRICT_SCAN_HT_CAPABILITY_IE_LEN
            || element[0] != 45
            || element[1] != 26
        {
            return 0;
        }
        let capability = u16::from_le_bytes([element[2], element[3]]);
        let mut flags = node.add(0x0c).cast::<u32>().read();
        flags |= 0x40;
        if capability & 0x20 != 0 {
            flags |= 0x8000;
        }
        node.add(0x0c).cast::<u32>().write(flags);
        node.add(0x15c).cast::<u16>().write_unaligned(capability);
        node.add(0x15e).write(element[4]);
        // `rcUpdateAMPDUParam` normally derives this hardware protection
        // spacing while the vendor connection state machine installs the
        // peer. Strict association bypasses that state machine, so reproduce
        // its finite density mapping from the AP's negotiated HT Parameters
        // byte. `mac_tx_set_htsig` later copies the 10-bit value into all
        // three protection-register fields.
        node.add(0x82).cast::<u16>().write_unaligned(
            crate::tx_ampdu::basic_ht_ampdu_protection_spacing(element[4]),
        );

        // Pinned `ieee80211_setup_htrates` stores a count followed by an
        // explicit MCS-index list. The strict local profile owns one spatial
        // stream, so only the eight finite bits in the first peer MCS byte can
        // enter the intersection.
        ptr::write_bytes(node.add(0x163), 0, 128);
        let mut count = 0_u8;
        for mcs in 0_u8..8 {
            if element[5] & (1 << mcs) != 0 {
                node.add(0x164 + usize::from(count)).write(mcs);
                count += 1;
            }
        }
        node.add(0x163).write(count);
        node.add(0x2f3).write(u8::from(count != 0));
        count
    }

    fn observe_assoc_response(frame: &[u8]) {
        let config = unsafe { ASSOC_CONFIG.0.get().read() };
        if frame[4..10] != config.local
            || frame[10..16] != config.access_point.bssid
            || frame[16..22] != config.access_point.bssid
        {
            return;
        }
        let capability = u16::from_le_bytes([frame[24], frame[25]]);
        let status = u16::from_le_bytes([frame[26], frame[27]]);
        let association_id = u16::from_le_bytes([frame[28], frame[29]]) & 0x3fff;
        let ht_capability = association_response_ie(frame, 45);
        let he_capability =
            association_response_extension_ie(frame, crate::he::HE_CAPABILITIES_EXTENSION_ID);
        let he_operation =
            association_response_extension_ie(frame, crate::he::HE_OPERATION_EXTENSION_ID);
        let he_bidirectional_mcs9 = he_capability
            .and_then(|element| crate::he::parse_he20_capabilities(element).ok())
            .is_some_and(|capability| capability.supports_bidirectional_mcs9());
        let he_bss_color = he_operation
            .and_then(|element| crate::he::parse_he20_operation(element).ok())
            .map(|operation| operation.bss_color);
        // HT stations use QoS data service. The bounded S31 RX prefix may end
        // before the response's trailing WMM parameter element, but an AP
        // returning an HT Capability after accepting our explicit WMM request
        // has necessarily negotiated the QoS data path. We still recognize
        // the element directly whenever it is present.
        let wmm = association_response_has_wmm(frame) || ht_capability.is_some();
        ASSOC_LAST_CAPABILITY.store(u32::from(capability), Ordering::Relaxed);
        ASSOC_LAST_STATUS.store(u32::from(status), Ordering::Relaxed);
        ASSOC_LAST_ID.store(u32::from(association_id), Ordering::Relaxed);
        ASSOC_HE_CAPABILITY_LEN.store(
            he_capability.map_or(0, |element| element.len()) as u32,
            Ordering::Relaxed,
        );
        ASSOC_HE_OPERATION_LEN.store(
            he_operation.map_or(0, |element| element.len()) as u32,
            Ordering::Relaxed,
        );
        ASSOC_HE_BIDIRECTIONAL_MCS9.store(u32::from(he_bidirectional_mcs9), Ordering::Relaxed);
        ASSOC_HE_BSS_COLOR.store(he_bss_color.map_or(u32::MAX, u32::from), Ordering::Relaxed);
        ASSOC_RESPONSES.fetch_add(1, Ordering::Relaxed);
        unsafe {
            let _ = crate::adapter::cancel_internal_timer(ASSOC_TIMER.0.get().cast());
        }
        if status == 0 {
            if unsafe {
                commit_static_association(
                    association_id,
                    ht_capability,
                    he_capability,
                    he_operation,
                    wmm,
                )
            } {
                complete_assoc(RESULT_OK);
            } else {
                complete_assoc(RESULT_INTERFACE_UNAVAILABLE);
            }
        } else {
            complete_assoc(RESULT_STATUS | u32::from(status));
        }
    }

    unsafe fn commit_static_association(
        association_id: u16,
        ht_capability: Option<&[u8]>,
        he_capability: Option<&[u8]>,
        he_operation: Option<&[u8]>,
        wmm: bool,
    ) -> bool {
        if association_id == 0 || association_id > 0x3fff {
            return false;
        }
        let interface = station_interface();
        if interface.is_null() {
            return false;
        }
        let node = interface.add(0xe4).cast::<*mut u8>().read();
        if node != NODE.0.get().cast::<u8>() {
            return false;
        }

        // Pinned `ieee80211_search_node` admits STA data only in state RUN
        // (interface+0x98 == 5). `ni_associd` is the 16-bit field at node+0x26.
        // These are the only net80211 facts needed after our Rust-owned
        // association response transition; no vendor connection callback or
        // supplicant state machine is entered.
        let mcs_count = ht_capability
            .map(|element| apply_static_ht_capability(node, element))
            .unwrap_or(0);
        if wmm {
            let flags = node.add(0x0c).cast::<u32>().read();
            node.add(0x0c).cast::<u32>().write(flags | 0x02);
        }
        #[cfg(feature = "hil-he-association-oracle")]
        if ASSOC_HE_REQUESTED.load(Ordering::Acquire) != 0 {
            let Some((he_capability, he_operation)) = he_capability.zip(he_operation) else {
                return false;
            };
            if !apply_static_he_peer_state(node, association_id, he_capability, he_operation) {
                return false;
            }
            ASSOC_HE_PEER_STATE_APPLIED.store(1, Ordering::Release);
        }
        #[cfg(not(feature = "hil-he-association-oracle"))]
        let _ = (he_capability, he_operation);
        ASSOC_HT_NEGOTIATED.store(u32::from(mcs_count != 0), Ordering::Release);
        ASSOC_WMM_NEGOTIATED.store(u32::from(wmm), Ordering::Release);
        ASSOC_HT_MCS_COUNT.store(u32::from(mcs_count), Ordering::Release);
        if mcs_count >= 8 {
            // Diagnostic first policy: prove the lower PP/LMAC path can use
            // negotiated HT independently of the vendor connection/runtime
            // state machine. Rust mutates only the already initialized
            // interface-0 default TRC context before the first data frame.
            let applied = set_default_sta_fixed_rate(PHY_RATE_MCS7_SGI as u8);
            ASSOC_FIXED_HT20_RATE.store(
                if applied { PHY_RATE_MCS7_SGI } else { u32::MAX },
                Ordering::Release,
            );
        } else {
            ASSOC_FIXED_HT20_RATE.store(u32::MAX, Ordering::Release);
        }
        node.add(0x26).cast::<u16>().write_unaligned(association_id);
        interface.add(0x98).cast::<u32>().write(5);
        true
    }

    #[cfg(feature = "hil-he-association-oracle")]
    #[link_section = ".rwtext.wifi_strict.he_peer"]
    unsafe fn apply_static_he_peer_state(
        node: *mut u8,
        association_id: u16,
        capability: &[u8],
        operation: &[u8],
    ) -> bool {
        let Ok(state) = crate::he::parse_he20_peer_state(capability, operation) else {
            return false;
        };
        if crate::he::program_he20_peer_hardware(state).is_err() {
            return false;
        }
        let minimum_mpdu_start_spacing = node.add(0x15e).read() >> 2 & 0x07;
        let bssid_index = node.add(0x383).read();
        if crate::he::program_he20_association_hardware(
            association_id,
            minimum_mpdu_start_spacing,
            bssid_index,
        )
        .is_err()
        {
            return false;
        }

        // Exact bounded capability/operation stores recovered from the pinned
        // parsers. Do not publish their HE TX-selection flag (0x0020_0000)
        // yet: `ieee80211_set_tx_desc` turns it into the descriptor HE bit,
        // whose PPDU formatter is intentionally still fail-closed. The
        // receive-side MMIO state and peer bytes remain installed while this
        // HIL keeps the qualified outbound HT path.
        node.add(0x2ef).write(state.max_rate_code);
        ptr::copy_nonoverlapping(
            state.capability_prefix.as_ptr(),
            node.add(0x33c),
            state.capability_prefix.len(),
        );
        node.add(0x354).write(state.packet_padding_eight_us);
        node.add(0x355).write(state.operation_parameters as u8);
        node.add(0x356)
            .write((state.operation_parameters >> 8) as u8);
        node.add(0x357)
            .write((state.operation_parameters >> 16) as u8);
        node.add(0x358).write(state.bss_color_information);
        node.add(0x35a)
            .cast::<u16>()
            .write_unaligned(state.basic_mcs_nss_map);
        let mut operation_state = node.add(0x35c).cast::<u16>().read_unaligned() & !0x07ff;
        if let Some(threshold) = state.rts_threshold {
            operation_state |= threshold;
        }
        if state.extended_range_single_user {
            operation_state |= 1 << 10;
        }
        node.add(0x35c)
            .cast::<u16>()
            .write_unaligned(operation_state);
        true
    }

    /// Validate the non-key half of the strict STA teardown before the radio
    /// owner mutates any hardware key state.
    pub(crate) unsafe fn can_reset_static_sta_link() -> bool {
        if !crate::critical::on_strict_wifi_hart()
            || !crate::context::in_radio_context()
            || PHASE.load(Ordering::Acquire) != PHASE_IDLE
            || ASSOC_PHASE.load(Ordering::Acquire) != PHASE_IDLE
            || OWNED_ACTION_BUFFER.load(Ordering::Acquire) != 0
        {
            return false;
        }
        #[cfg(feature = "hil-ampdu-intercept")]
        if !crate::tx_intercept::can_reset_sta_link() {
            return false;
        }
        let interface = station_interface();
        if interface.is_null() {
            return false;
        }
        let node = interface.add(0xe4).cast::<*mut u8>().read();
        node.is_null() || node == NODE.0.get().cast::<u8>()
    }

    /// Clear the Rust-owned association state after key ownership preflight.
    ///
    /// # Safety
    /// `can_reset_static_sta_link` must have returned true in the same
    /// serialized radio-owner command and all STA hardware keys must already
    /// be disabled.
    pub(crate) unsafe fn reset_static_sta_link() {
        debug_assert!(can_reset_static_sta_link());
        let _ = crate::adapter::cancel_internal_timer(TIMER.0.get().cast());
        let _ = crate::adapter::cancel_internal_timer(ASSOC_TIMER.0.get().cast());
        let _ = crate::adapter::cancel_internal_timer(TX_BLOCK_ACK_TIMER.0.get().cast());
        (*TX_BLOCK_ACK_SESSION.0.get()).stop();
        #[cfg(feature = "hil-ampdu-intercept")]
        crate::tx_intercept::reset_sta_link();
        TX_ADDBA_SESSION_ATTEMPTS.store(0, Ordering::Release);
        TX_ADDBA_LAST_STATUS.store(0, Ordering::Release);
        TX_ADDBA_WINDOW.store(0, Ordering::Release);
        TX_ADDBA_ALARM_GENERATION.store(0, Ordering::Release);
        ASSOC_HT_REQUESTED.store(0, Ordering::Release);
        ASSOC_HT_NEGOTIATED.store(0, Ordering::Release);
        ASSOC_WMM_NEGOTIATED.store(0, Ordering::Release);
        ASSOC_HT_MCS_COUNT.store(0, Ordering::Release);
        ASSOC_FIXED_HT20_RATE.store(u32::MAX, Ordering::Release);
        ASSOC_HE_REQUESTED.store(0, Ordering::Release);
        ASSOC_HE_CAPABILITY_LEN.store(0, Ordering::Release);
        ASSOC_HE_OPERATION_LEN.store(0, Ordering::Release);
        ASSOC_HE_BIDIRECTIONAL_MCS9.store(0, Ordering::Release);
        ASSOC_HE_BSS_COLOR.store(u32::MAX, Ordering::Release);
        ASSOC_HE_PEER_STATE_APPLIED.store(0, Ordering::Release);
        let associated_peer = ASSOC_CONFIG.0.get().read().access_point.bssid;
        #[cfg(feature = "hil-rx-ampdu")]
        crate::rx_ampdu_ap::remove_peer(associated_peer);
        OWNED_RX_ADDBA_ACCEPTED.store(0, Ordering::Release);
        OWNED_RX_ADDBA_RESPONSE.store(0, Ordering::Release);
        PENDING_RX_ADDBA_STATE.store(0, Ordering::Release);
        CONFIG.0.get().write(AuthConfig::EMPTY);
        ASSOC_CONFIG.0.get().write(AssocConfig::EMPTY);

        let interface = station_interface();
        let node = interface.add(0xe4).cast::<*mut u8>().read();
        interface.add(0xe4).cast::<*mut u8>().write(ptr::null_mut());
        interface.add(0x98).cast::<u32>().write(0);
        ptr::write_bytes(interface.add(0x9c), 0, 6);
        if node == NODE.0.get().cast::<u8>() {
            ptr::write_bytes(node, 0, VENDOR_NODE_LEN);
        }
        RESET_SIGNAL.notify_from_isr();
    }

    /// Snapshot the reset completion generation before enqueueing
    /// `Wpa2IoCommand::ResetStaLink`.
    pub fn sta_link_reset_generation() -> usize {
        RESET_SIGNAL.generation()
    }

    /// Await the exact radio-owner reset completion edge without polling.
    pub async fn wait_sta_link_reset_after(observed: usize) {
        RESET_SIGNAL.wait_after(observed).await;
    }
}

#[cfg(all(target_arch = "riscv32", feature = "strict-no-wait"))]
pub use target::associate_sta;
#[cfg(all(target_arch = "riscv32", feature = "strict-no-wait"))]
pub use target::authenticate_open;
#[cfg(all(target_arch = "riscv32", feature = "strict-no-wait"))]
pub use target::sta_assoc_snapshot;
#[cfg(all(target_arch = "riscv32", feature = "strict-no-wait"))]
pub use target::sta_auth_snapshot;
#[cfg(all(target_arch = "riscv32", feature = "strict-no-wait"))]
pub(crate) use target::{can_reset_static_sta_link, reset_static_sta_link};
#[cfg(all(target_arch = "riscv32", feature = "strict-no-wait"))]
pub(crate) use target::{
    complete_owned_action_management, dispatch_assoc_tx, dispatch_auth_tx,
    ingest_management_action, is_owned_action_management, management_tx_done, observe_management,
    start_sta_tx_block_ack, STA_ASSOC_EVENT, STA_AUTH_EVENT,
};
#[cfg(all(target_arch = "riscv32", feature = "strict-no-wait"))]
pub use target::{sta_link_reset_generation, wait_sta_link_reset_after};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::StrictScanRecord;

    #[test]
    fn open_auth_response_layout_is_unambiguous() {
        let mut frame = [0_u8; 30];
        frame[0] = 0xb0;
        frame[24..26].copy_from_slice(&0_u16.to_le_bytes());
        frame[26..28].copy_from_slice(&2_u16.to_le_bytes());
        frame[28..30].copy_from_slice(&17_u16.to_le_bytes());
        assert_eq!(frame[0] & 0xfc, 0xb0);
        assert_eq!(u16::from_le_bytes([frame[24], frame[25]]), 0);
        assert_eq!(u16::from_le_bytes([frame[26], frame[27]]), 2);
        assert_eq!(u16::from_le_bytes([frame[28], frame[29]]), 17);
    }

    fn access_point_with_rsn(akms: &[[u8; 4]], capabilities: u16) -> StrictScanRecord {
        let mut record = StrictScanRecord::EMPTY;
        record.ssid[..4].copy_from_slice(b"test");
        record.ssid_len = 4;
        record.bssid = [1, 2, 3, 4, 5, 6];
        record.channel = 6;
        record.privacy = true;
        let mut offset = 2;
        record.rsn_ie[offset..offset + 2].copy_from_slice(&1_u16.to_le_bytes());
        offset += 2;
        record.rsn_ie[offset..offset + 4].copy_from_slice(&[0, 0x0f, 0xac, 4]);
        offset += 4;
        record.rsn_ie[offset..offset + 2].copy_from_slice(&1_u16.to_le_bytes());
        offset += 2;
        record.rsn_ie[offset..offset + 4].copy_from_slice(&[0, 0x0f, 0xac, 4]);
        offset += 4;
        record.rsn_ie[offset..offset + 2].copy_from_slice(&(akms.len() as u16).to_le_bytes());
        offset += 2;
        for akm in akms {
            record.rsn_ie[offset..offset + 4].copy_from_slice(akm);
            offset += 4;
        }
        record.rsn_ie[offset..offset + 2].copy_from_slice(&capabilities.to_le_bytes());
        offset += 2;
        record.rsn_ie[0] = 48;
        record.rsn_ie[1] = (offset - 2) as u8;
        record.rsn_ie_len = offset as u8;
        record
    }

    #[test]
    fn mixed_wpa2_wpa3_ap_is_narrowed_to_wpa2_psk_ccmp() {
        let record = access_point_with_rsn(&[[0, 0x0f, 0xac, 8], [0, 0x0f, 0xac, 2]], 0x80);
        let selected = select_wpa2_psk_rsn(&record).unwrap();
        assert_eq!(selected.as_bytes().len(), SELECTED_RSN_IE_LEN);
        assert_eq!(&selected.as_bytes()[8..14], &[1, 0, 0, 0x0f, 0xac, 4]);
        assert_eq!(&selected.as_bytes()[14..20], &[1, 0, 0, 0x0f, 0xac, 2]);
        assert_eq!(&selected.as_bytes()[20..22], &[0, 0]);
    }

    #[test]
    fn required_management_frame_protection_is_rejected() {
        let record = access_point_with_rsn(&[[0, 0x0f, 0xac, 2]], RSN_CAPABILITY_MFPR);
        assert_eq!(
            select_wpa2_psk_rsn(&record).unwrap_err(),
            StaAssocSecurityError::ManagementFrameProtectionRequired
        );
    }

    #[test]
    fn open_ap_needs_no_security_ie() {
        let record = StrictScanRecord {
            privacy: false,
            ..StrictScanRecord::EMPTY
        };
        assert!(select_wpa2_psk_rsn(&record).unwrap().as_bytes().is_empty());
    }

    #[test]
    fn association_response_fixed_fields_are_unambiguous() {
        let mut frame = [0_u8; 30];
        frame[0] = 0x10;
        frame[24..26].copy_from_slice(&0x0431_u16.to_le_bytes());
        frame[26..28].copy_from_slice(&0_u16.to_le_bytes());
        frame[28..30].copy_from_slice(&0xc02a_u16.to_le_bytes());
        assert_eq!(frame[0] & 0xfc, 0x10);
        assert_eq!(u16::from_le_bytes([frame[26], frame[27]]), 0);
        assert_eq!(u16::from_le_bytes([frame[28], frame[29]]) & 0x3fff, 42);
    }

    #[test]
    fn association_response_extension_elements_are_bounded_and_parsed() {
        let mut frame = [0_u8; 63];
        frame[0] = 0x10;
        frame[24..26].copy_from_slice(&0x0431_u16.to_le_bytes());
        frame[28..30].copy_from_slice(&0xc02a_u16.to_le_bytes());

        let capability = &mut frame[30..54];
        capability[..3].copy_from_slice(&[255, 22, crate::he::HE_CAPABILITIES_EXTENSION_ID]);
        capability[20..22].copy_from_slice(&0xfffd_u16.to_le_bytes());
        capability[22..24].copy_from_slice(&0xfffd_u16.to_le_bytes());
        frame[54..63].copy_from_slice(&[
            255,
            7,
            crate::he::HE_OPERATION_EXTENSION_ID,
            0,
            0,
            0,
            0xc5,
            0xfd,
            0xff,
        ]);

        let capability =
            association_response_extension_ie(&frame, crate::he::HE_CAPABILITIES_EXTENSION_ID)
                .unwrap();
        assert!(crate::he::parse_he20_capabilities(capability)
            .unwrap()
            .supports_bidirectional_mcs9());
        let operation =
            association_response_extension_ie(&frame, crate::he::HE_OPERATION_EXTENSION_ID)
                .unwrap();
        assert_eq!(
            crate::he::parse_he20_operation(operation)
                .unwrap()
                .bss_color,
            5
        );
    }

    #[test]
    fn vendor_oracle_he20_capability_is_one_stream_mcs9() {
        let capability = crate::he::parse_he20_capabilities(&HE20_MCS9_CAPABILITY_IE).unwrap();
        assert_eq!(capability.receive_nss1, crate::he::HeMcsNssSupport::Mcs0To9);
        assert_eq!(
            capability.transmit_nss1,
            crate::he::HeMcsNssSupport::Mcs0To9
        );
        assert!(capability.supports_bidirectional_mcs9());
    }

    #[test]
    fn association_response_extension_parser_rejects_truncated_tail() {
        let mut frame = [0_u8; 34];
        frame[30..34].copy_from_slice(&[255, 22, crate::he::HE_CAPABILITIES_EXTENSION_ID, 0]);
        assert_eq!(
            association_response_extension_ie(&frame, crate::he::HE_CAPABILITIES_EXTENSION_ID),
            None
        );
    }
}
