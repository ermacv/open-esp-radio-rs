//! Static WPA2-Personal AP association boundary for the pinned S31 ABI.

use core::sync::atomic::{AtomicU8, AtomicUsize, Ordering};

use crate::{
    channel::{BoundedChannel, Receive},
    wpa2_frames::OwnedRsnIe,
};

pub const WPA2_AP_ASSOC_CAPACITY: usize = 8;

const RSN_ELEMENT_ID: u8 = 0x30;
const RSN_VERSION: u16 = 1;
const RSN_CAPABILITY_MFPR: u16 = 1 << 6;
const RSN_CAPABILITY_MFPC: u16 = 1 << 7;
const RSN_OUI: [u8; 3] = [0x00, 0x0f, 0xac];
const RSN_CIPHER_CCMP: u8 = 4;
const RSN_AKM_PSK: u8 = 2;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Wpa2ApRsnError {
    Malformed,
    CapacityExceeded,
    UnsupportedVersion,
    UnsupportedGroupCipher,
    UnsupportedPairwiseCipher,
    UnsupportedAkm,
    ManagementFrameProtectionUnsupported,
    PmkidCachingUnsupported,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Wpa2ApPeerEvent {
    Associated {
        peer: [u8; 6],
        rsn_ie: OwnedRsnIe,
        reassociation: bool,
    },
    Removed {
        peer: [u8; 6],
    },
}

static EVENTS: BoundedChannel<Wpa2ApPeerEvent, WPA2_AP_ASSOC_CAPACITY> = BoundedChannel::new();
static REJECTED: AtomicUsize = AtomicUsize::new(0);
#[cfg_attr(
    target_arch = "riscv32",
    link_section = ".critical.bss.wifi_strict.ap_join_diagnostics"
)]
static JOIN_ATTEMPTS: AtomicUsize = AtomicUsize::new(0);
#[cfg_attr(
    target_arch = "riscv32",
    link_section = ".critical.bss.wifi_strict.ap_join_diagnostics"
)]
static JOIN_ACCEPTED: AtomicUsize = AtomicUsize::new(0);
#[cfg_attr(
    target_arch = "riscv32",
    link_section = ".critical.bss.wifi_strict.ap_join_diagnostics"
)]
static JOIN_LAST_RSN_LEN: AtomicUsize = AtomicUsize::new(0);
#[cfg_attr(
    target_arch = "riscv32",
    link_section = ".critical.bss.wifi_strict.ap_join_diagnostics"
)]
static JOIN_LAST_ERROR: AtomicU8 = AtomicU8::new(0);
#[cfg_attr(
    target_arch = "riscv32",
    link_section = ".critical.bss.wifi_strict.ap_join_diagnostics"
)]
static JOIN_LAST_RSN_PREFIX: [AtomicU8; 32] = [const { AtomicU8::new(0) }; 32];

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Wpa2ApJoinSnapshot {
    pub attempts: usize,
    pub accepted: usize,
    pub last_rsn_len: usize,
    pub last_error: Option<Wpa2ApRsnError>,
    pub last_rsn_prefix: [u8; 32],
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ManagementTxRejectionSnapshot {
    pub count: usize,
    pub captured: bool,
    pub reason: u32,
    pub subtype: u8,
    pub node: usize,
    pub buffer: usize,
    pub layout: u16,
    pub header_len: u16,
    pub body_len: u16,
    pub raw_frame_control: u16,
    pub frame_control: u16,
    pub raw_word0: u32,
    pub raw_word1: u32,
    pub body_word0: u32,
    pub body_word1: u32,
    pub category: u8,
    pub action: u8,
    pub interface_mode: u32,
    pub node_flags: u32,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DeferredApManagementSnapshot {
    pub captured: usize,
    pub coalesced: usize,
    pub submitted: usize,
    pub cancelled: usize,
    pub dropped: usize,
    pub occupied: usize,
    pub capacity: usize,
}

pub const AP_ASSOCIATION_RESPONSE_CAPTURE_CAPACITY: usize = 256;
const STRICT_AP_ASSOCIATION_RESPONSE_BODY_LEN: usize = 103;
const STRICT_AP_BGN_RATES: [u8; 12] = [
    0x8b, 0x96, 0x82, 0x84, 0x0c, 0x18, 0x30, 0x60, 0x6c, 0x12, 0x24, 0x48,
];
const STRICT_AP_BGN_HT20_ASSOCIATION_RESPONSE: [u8; STRICT_AP_ASSOCIATION_RESPONSE_BODY_LEN] = [
    0x31, 0x04, 0x00, 0x00, 0x01, 0xc0, 0x01, 0x08, 0x8b, 0x96, 0x82, 0x84, 0x0c, 0x18, 0x30, 0x60,
    0x32, 0x04, 0x6c, 0x12, 0x24, 0x48, 0x2a, 0x01, 0x00, 0x2d, 0x1a, 0x6e, 0x11, 0x00, 0xff, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x3d, 0x16, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xdd, 0x18, 0x00,
    0x50, 0xf2, 0x02, 0x01, 0x01, 0x04, 0x00, 0x03, 0xa4, 0x00, 0x00, 0x27, 0xa4, 0x00, 0x00, 0x42,
    0x43, 0x5e, 0x00, 0x62, 0x32, 0x2f, 0x00,
];

fn write_strict_ap_bgn_ht20_association_response(
    body: &mut [u8; STRICT_AP_ASSOCIATION_RESPONSE_BODY_LEN],
    status: u16,
    association_id: u16,
    primary_channel: u8,
) -> bool {
    if !(1..=13).contains(&primary_channel) || (status == 0 && association_id & 0x3fff == 0) {
        return false;
    }
    body.copy_from_slice(&STRICT_AP_BGN_HT20_ASSOCIATION_RESPONSE);
    body[2..4].copy_from_slice(&status.to_le_bytes());
    body[4..6].copy_from_slice(&if status == 0 { association_id } else { 0 }.to_le_bytes());
    // First byte of the HT Operation element.
    body[55] = primary_channel;
    true
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ApAssociationResponseSnapshot {
    pub captured: bool,
    pub subtype: u8,
    pub status: u16,
    pub body_len: usize,
    pub truncated: bool,
    pub body: [u8; AP_ASSOCIATION_RESPONSE_CAPTURE_CAPACITY],
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ApAssociationRejectionSnapshot {
    pub count: usize,
    pub reason: u32,
    pub node: usize,
    pub interface: usize,
    pub node_flags: u32,
    pub node_state: u8,
    pub association_id: u16,
    pub interface_mode: u32,
    pub interface_state: u8,
    pub interface_flags: u32,
    pub interface_capabilities: u32,
    pub interface_options: u32,
}

impl ApAssociationResponseSnapshot {
    const fn empty() -> Self {
        Self {
            captured: false,
            subtype: 0,
            status: 0,
            body_len: 0,
            truncated: false,
            body: [0; AP_ASSOCIATION_RESPONSE_CAPTURE_CAPACITY],
        }
    }
}

fn rsn_error_code(error: Wpa2ApRsnError) -> u8 {
    match error {
        Wpa2ApRsnError::Malformed => 1,
        Wpa2ApRsnError::CapacityExceeded => 2,
        Wpa2ApRsnError::UnsupportedVersion => 3,
        Wpa2ApRsnError::UnsupportedGroupCipher => 4,
        Wpa2ApRsnError::UnsupportedPairwiseCipher => 5,
        Wpa2ApRsnError::UnsupportedAkm => 6,
        Wpa2ApRsnError::ManagementFrameProtectionUnsupported => 7,
        Wpa2ApRsnError::PmkidCachingUnsupported => 8,
    }
}

fn rsn_error_from_code(code: u8) -> Option<Wpa2ApRsnError> {
    match code {
        1 => Some(Wpa2ApRsnError::Malformed),
        2 => Some(Wpa2ApRsnError::CapacityExceeded),
        3 => Some(Wpa2ApRsnError::UnsupportedVersion),
        4 => Some(Wpa2ApRsnError::UnsupportedGroupCipher),
        5 => Some(Wpa2ApRsnError::UnsupportedPairwiseCipher),
        6 => Some(Wpa2ApRsnError::UnsupportedAkm),
        7 => Some(Wpa2ApRsnError::ManagementFrameProtectionUnsupported),
        8 => Some(Wpa2ApRsnError::PmkidCachingUnsupported),
        _ => None,
    }
}

fn record_join_rsn(bytes: &[u8], error: Option<Wpa2ApRsnError>) {
    JOIN_ATTEMPTS.fetch_add(1, Ordering::Relaxed);
    JOIN_LAST_RSN_LEN.store(bytes.len(), Ordering::Relaxed);
    for (index, destination) in JOIN_LAST_RSN_PREFIX.iter().enumerate() {
        destination.store(bytes.get(index).copied().unwrap_or(0), Ordering::Relaxed);
    }
    JOIN_LAST_ERROR.store(error.map(rsn_error_code).unwrap_or(0), Ordering::Release);
}

pub fn wpa2_ap_join_snapshot() -> Wpa2ApJoinSnapshot {
    Wpa2ApJoinSnapshot {
        attempts: JOIN_ATTEMPTS.load(Ordering::Acquire),
        accepted: JOIN_ACCEPTED.load(Ordering::Acquire),
        last_rsn_len: JOIN_LAST_RSN_LEN.load(Ordering::Acquire),
        last_error: rsn_error_from_code(JOIN_LAST_ERROR.load(Ordering::Acquire)),
        last_rsn_prefix: core::array::from_fn(|index| {
            JOIN_LAST_RSN_PREFIX[index].load(Ordering::Acquire)
        }),
    }
}

fn read_u16(bytes: &[u8], offset: &mut usize) -> Result<u16, Wpa2ApRsnError> {
    let value = bytes
        .get(*offset..*offset + 2)
        .ok_or(Wpa2ApRsnError::Malformed)?;
    *offset += 2;
    Ok(u16::from_le_bytes([value[0], value[1]]))
}

fn read_suite(bytes: &[u8], offset: &mut usize) -> Result<[u8; 4], Wpa2ApRsnError> {
    let suite = bytes
        .get(*offset..*offset + 4)
        .ok_or(Wpa2ApRsnError::Malformed)?;
    *offset += 4;
    Ok([suite[0], suite[1], suite[2], suite[3]])
}

fn supported_suite(suite: [u8; 4], selector: u8) -> bool {
    suite[..3] == RSN_OUI && suite[3] == selector
}

/// Validate the allocation-free WPA2-PSK/CCMP subset implemented by the Rust
/// state machine and return an owned association IE.
struct ValidatedWpa2Rsn {
    owned: OwnedRsnIe,
    capabilities: u16,
}

fn validate_wpa2_ap_rsn_with_capabilities(
    bytes: &[u8],
) -> Result<ValidatedWpa2Rsn, Wpa2ApRsnError> {
    let owned = OwnedRsnIe::try_copy(bytes).map_err(|error| match error {
        crate::wpa2_frames::Wpa2FrameError::CapacityExceeded => Wpa2ApRsnError::CapacityExceeded,
        _ => Wpa2ApRsnError::Malformed,
    })?;
    if bytes.first() != Some(&RSN_ELEMENT_ID) {
        return Err(Wpa2ApRsnError::Malformed);
    }

    let body = &bytes[2..];
    let mut offset = 0;
    if read_u16(body, &mut offset)? != RSN_VERSION {
        return Err(Wpa2ApRsnError::UnsupportedVersion);
    }
    if !supported_suite(read_suite(body, &mut offset)?, RSN_CIPHER_CCMP) {
        return Err(Wpa2ApRsnError::UnsupportedGroupCipher);
    }

    let pairwise_count = usize::from(read_u16(body, &mut offset)?);
    if pairwise_count == 0 {
        return Err(Wpa2ApRsnError::UnsupportedPairwiseCipher);
    }
    let mut pairwise_ccmp = false;
    for _ in 0..pairwise_count {
        pairwise_ccmp |= supported_suite(read_suite(body, &mut offset)?, RSN_CIPHER_CCMP);
    }
    if !pairwise_ccmp {
        return Err(Wpa2ApRsnError::UnsupportedPairwiseCipher);
    }

    let akm_count = usize::from(read_u16(body, &mut offset)?);
    if akm_count == 0 {
        return Err(Wpa2ApRsnError::UnsupportedAkm);
    }
    let mut psk = false;
    for _ in 0..akm_count {
        psk |= supported_suite(read_suite(body, &mut offset)?, RSN_AKM_PSK);
    }
    if !psk {
        return Err(Wpa2ApRsnError::UnsupportedAkm);
    }

    let capabilities = if offset < body.len() {
        let capabilities = read_u16(body, &mut offset)?;
        if capabilities & RSN_CAPABILITY_MFPR != 0 {
            return Err(Wpa2ApRsnError::ManagementFrameProtectionUnsupported);
        }
        capabilities
    } else {
        0
    };
    // A zero PMKID count is a standard optional suffix and does not request
    // PMKSA caching. Nonzero lists cannot be honored by this fixed-state
    // authenticator and are rejected before association succeeds.
    if offset < body.len() && read_u16(body, &mut offset)? != 0 {
        return Err(Wpa2ApRsnError::PmkidCachingUnsupported);
    }
    // A group-management cipher is rejected here. It is unnecessary when the
    // AP does not advertise MFPC, even if the station reports that capability.
    if offset != body.len() {
        return Err(Wpa2ApRsnError::Malformed);
    }
    Ok(ValidatedWpa2Rsn {
        owned,
        capabilities,
    })
}

pub fn validate_wpa2_ap_rsn(bytes: &[u8]) -> Result<OwnedRsnIe, Wpa2ApRsnError> {
    validate_wpa2_ap_rsn_with_capabilities(bytes).map(|validated| validated.owned)
}

pub fn try_receive_wpa2_ap_event() -> Option<Wpa2ApPeerEvent> {
    EVENTS.try_receive()
}

pub fn receive_wpa2_ap_event() -> Receive<'static, Wpa2ApPeerEvent, WPA2_AP_ASSOC_CAPACITY> {
    EVENTS.receive()
}

pub fn rejected_wpa2_ap_events() -> usize {
    REJECTED.load(Ordering::Acquire)
}

const fn is_bounded_ap_addba_response_layout(
    subtype: u8,
    layout: u16,
    header_len: u16,
    body_len: u16,
    category: u8,
    action: u8,
) -> bool {
    subtype == 0xd0
        && layout == 0
        && header_len == 24
        && body_len == 9
        && category == 3
        && action == 1
}

const fn updated_tim_bitmap_byte(current: u8, association_id: u16, set: bool) -> u8 {
    let mask = 1_u8 << (association_id & 7);
    if set {
        current | mask
    } else {
        current & !mask
    }
}

#[cfg(target_arch = "riscv32")]
mod target {
    use core::{
        cell::UnsafeCell,
        ffi::c_void,
        mem, ptr,
        sync::atomic::{AtomicBool, AtomicU8, Ordering},
        task::{Context, Poll},
    };

    use super::*;
    use crate::queue::WakerCell;

    const WPA_AP_JOIN_OFFSET: usize = 0x24;
    const WPA_AP_REMOVE_OFFSET: usize = 0x28;
    const WPA_AP_INIT_OFFSET: usize = 0x1c;
    const WPA_AP_DEINIT_OFFSET: usize = 0x20;
    const WPA_AP_GET_RSN_OFFSET: usize = 0x2c;
    const WPA_AP_SPP_OFFSET: usize = 0x34;
    const PINNED_STATION_SIZE: usize = 0x28;
    const PINNED_STATION_MAC_OFFSET: usize = 8;
    const CCMP_DRIVER_SELECTOR: u8 = 4;
    const WLAN_STATUS_SUCCESS: u16 = 0;
    const WLAN_STATUS_AP_UNABLE_TO_HANDLE_NEW_STA: u16 = 17;
    const WLAN_STATUS_INVALID_IE: u16 = 40;
    const DEFERRED_AP_ACTION_BODY_LEN: usize = 9;
    const DEFERRED_SLOT_EMPTY: u8 = 0;
    const DEFERRED_SLOT_WRITING: u8 = 1;
    const DEFERRED_SLOT_READY: u8 = 2;

    type InitCallback = unsafe extern "C" fn() -> *mut c_void;
    type DeinitCallback = unsafe extern "C" fn(*mut c_void) -> bool;
    type JoinCallback = unsafe extern "C" fn(*mut WpaStationJoinParam) -> bool;
    type RemoveCallback = unsafe extern "C" fn(*const u8) -> bool;
    type GetRsnCallback = unsafe extern "C" fn(*mut usize) -> *mut u8;
    type SppCallback = unsafe extern "C" fn(*mut c_void, *mut bool, *mut bool);

    #[repr(C)]
    struct WpaStationJoinParam {
        station: *mut *mut c_void,
        bssid: *const u8,
        wpa_ie: *const u8,
        rsnxe: *const u8,
        pmf_enable: *mut bool,
        pairwise_cipher: *mut u8,
        rsn_selection_ie: *mut u8,
        owe_dhie: *mut u8,
        subtype: i32,
        rsnxe_len: u16,
        wpa_ie_len: u8,
        owe_dh_len: u8,
    }

    #[repr(C, align(4))]
    struct PinnedStation {
        bytes: [u8; PINNED_STATION_SIZE],
    }

    struct PeerSlot {
        claimed: AtomicBool,
        association_epoch: AtomicUsize,
        association_id: AtomicUsize,
        station: UnsafeCell<PinnedStation>,
    }

    impl PeerSlot {
        const fn new() -> Self {
            Self {
                claimed: AtomicBool::new(false),
                association_epoch: AtomicUsize::new(0),
                association_id: AtomicUsize::new(0),
                station: UnsafeCell::new(PinnedStation {
                    bytes: [0; PINNED_STATION_SIZE],
                }),
            }
        }
    }

    unsafe impl Sync for PeerSlot {}

    struct ApRsnStorage(UnsafeCell<[u8; crate::wpa2_frames::WPA2_RSN_IE_CAPACITY]>);

    impl ApRsnStorage {
        const fn new() -> Self {
            Self(UnsafeCell::new(
                [0; crate::wpa2_frames::WPA2_RSN_IE_CAPACITY],
            ))
        }
    }

    unsafe impl Sync for ApRsnStorage {}

    struct StaticByte(UnsafeCell<u8>);

    unsafe impl Sync for StaticByte {}

    static PEERS: [PeerSlot; WPA2_AP_ASSOC_CAPACITY] =
        [const { PeerSlot::new() }; WPA2_AP_ASSOC_CAPACITY];
    static NEXT_ASSOCIATION_EPOCH: AtomicUsize = AtomicUsize::new(1);
    static AP_RSN: ApRsnStorage = ApRsnStorage::new();
    static AP_RSN_LEN: AtomicUsize = AtomicUsize::new(0);
    static AP_CONTEXT: StaticByte = StaticByte(UnsafeCell::new(0));
    static CALLBACKS_INSTALLED: AtomicBool = AtomicBool::new(false);
    #[repr(u32)]
    enum ManagementTxRejectionReason {
        WrongHart = 1,
        OutsideRadioContext = 2,
        NullNode = 3,
        NullBuffer = 4,
        UnsupportedSubtype = 5,
        MeshEnabled = 6,
        OffHomeChannel = 7,
        NullInterface = 8,
        UnsupportedInterfaceMode = 9,
        ApNodeState = 10,
        PtiWrongHart = 11,
        PtiNullBuffer = 12,
        PtiMissingDescriptor = 13,
        PtiEventOutOfRange = 14,
    }

    struct ManagementTxRejectionDiagnostics {
        count: AtomicUsize,
        captured: AtomicBool,
        reason: AtomicUsize,
        subtype: AtomicU8,
        node: AtomicUsize,
        buffer: AtomicUsize,
        layout: AtomicUsize,
        header_len: AtomicUsize,
        body_len: AtomicUsize,
        raw_frame_control: AtomicUsize,
        frame_control: AtomicUsize,
        raw_word0: AtomicUsize,
        raw_word1: AtomicUsize,
        body_word0: AtomicUsize,
        body_word1: AtomicUsize,
        category: AtomicU8,
        action: AtomicU8,
        interface_mode: AtomicUsize,
        node_flags: AtomicUsize,
    }

    impl ManagementTxRejectionDiagnostics {
        const fn new() -> Self {
            Self {
                count: AtomicUsize::new(0),
                captured: AtomicBool::new(false),
                reason: AtomicUsize::new(0),
                subtype: AtomicU8::new(0),
                node: AtomicUsize::new(0),
                buffer: AtomicUsize::new(0),
                layout: AtomicUsize::new(0),
                header_len: AtomicUsize::new(0),
                body_len: AtomicUsize::new(0),
                raw_frame_control: AtomicUsize::new(0),
                frame_control: AtomicUsize::new(0),
                raw_word0: AtomicUsize::new(0),
                raw_word1: AtomicUsize::new(0),
                body_word0: AtomicUsize::new(0),
                body_word1: AtomicUsize::new(0),
                category: AtomicU8::new(0),
                action: AtomicU8::new(0),
                interface_mode: AtomicUsize::new(u32::MAX as usize),
                node_flags: AtomicUsize::new(0),
            }
        }
    }

    #[link_section = ".critical.bss.wifi_strict.management_tx_rejection"]
    static MANAGEMENT_TX_REJECTION: ManagementTxRejectionDiagnostics =
        ManagementTxRejectionDiagnostics::new();
    static PTI_REJECTED_BUFFER: AtomicUsize = AtomicUsize::new(0);

    #[derive(Clone, Copy)]
    struct DeferredApManagement {
        peer: [u8; 6],
        body: [u8; DEFERRED_AP_ACTION_BODY_LEN],
        association_epoch: usize,
        active_epoch: usize,
        ps_poll_epoch: usize,
        removal_epoch: usize,
    }

    impl DeferredApManagement {
        const fn empty() -> Self {
            Self {
                peer: [0; 6],
                body: [0; DEFERRED_AP_ACTION_BODY_LEN],
                association_epoch: 0,
                active_epoch: 0,
                ps_poll_epoch: 0,
                removal_epoch: 0,
            }
        }
    }

    struct DeferredApManagementSlot {
        state: AtomicU8,
        value: UnsafeCell<DeferredApManagement>,
    }

    impl DeferredApManagementSlot {
        const fn new() -> Self {
            Self {
                state: AtomicU8::new(DEFERRED_SLOT_EMPTY),
                value: UnsafeCell::new(DeferredApManagement::empty()),
            }
        }
    }

    // Capture and consumption both run on the serialized radio-owner stack.
    // The atomic state still makes publication order explicit and prevents a
    // future non-radio diagnostic reader from observing a partially written
    // command.
    unsafe impl Sync for DeferredApManagementSlot {}

    #[link_section = ".critical.bss.wifi_strict.deferred_ap_management"]
    static DEFERRED_AP_MANAGEMENT: [DeferredApManagementSlot; WPA2_AP_ASSOC_CAPACITY] =
        [const { DeferredApManagementSlot::new() }; WPA2_AP_ASSOC_CAPACITY];
    static DEFERRED_AP_MANAGEMENT_READY: WakerCell = WakerCell::new();
    static DEFERRED_AP_MANAGEMENT_CAPTURED: AtomicUsize = AtomicUsize::new(0);
    static DEFERRED_AP_MANAGEMENT_COALESCED: AtomicUsize = AtomicUsize::new(0);
    static DEFERRED_AP_MANAGEMENT_SUBMITTED: AtomicUsize = AtomicUsize::new(0);
    static DEFERRED_AP_MANAGEMENT_CANCELLED: AtomicUsize = AtomicUsize::new(0);
    static DEFERRED_AP_MANAGEMENT_DROPPED: AtomicUsize = AtomicUsize::new(0);

    struct ApAssociationResponseCapture {
        state: AtomicU8,
        value: UnsafeCell<ApAssociationResponseSnapshot>,
    }

    impl ApAssociationResponseCapture {
        const fn new() -> Self {
            Self {
                state: AtomicU8::new(0),
                value: UnsafeCell::new(ApAssociationResponseSnapshot::empty()),
            }
        }
    }

    // The serialized radio owner writes the first complete response, then
    // publishes state=2 with Release. Diagnostic readers only copy after an
    // Acquire load observes that state.
    unsafe impl Sync for ApAssociationResponseCapture {}

    #[link_section = ".critical.bss.wifi_strict.ap_assoc_response_capture"]
    static AP_ASSOCIATION_RESPONSE_CAPTURE: ApAssociationResponseCapture =
        ApAssociationResponseCapture::new();

    #[repr(u32)]
    enum AssociationRejectionReason {
        UnsupportedSubtype = 1,
        UnsupportedStatus = 2,
        NodeUnavailable = 3,
        InterfaceUnavailable = 4,
        InterfaceMode = 5,
        InterfaceState = 6,
        InterfaceFlags = 7,
        InterfaceCapabilities = 8,
        InterfaceOptions = 9,
        NodeState = 10,
        NodeFlags = 11,
        NodePowerSaveFlags = 12,
        RateCount = 13,
        RateValue = 14,
        ChannelUnavailable = 15,
        FrameAllocation = 17,
        ResponseParameters = 18,
        ManagementOutput = 19,
        BandwidthStateUnavailable = 20,
        UnsupportedBandwidth = 21,
    }

    struct AssociationRejectionDiagnostics {
        count: AtomicUsize,
        reason: AtomicUsize,
        node: AtomicUsize,
        interface: AtomicUsize,
        node_flags: AtomicUsize,
        node_state: AtomicU8,
        association_id: AtomicUsize,
        interface_mode: AtomicUsize,
        interface_state: AtomicU8,
        interface_flags: AtomicUsize,
        interface_capabilities: AtomicUsize,
        interface_options: AtomicUsize,
    }

    impl AssociationRejectionDiagnostics {
        const fn new() -> Self {
            Self {
                count: AtomicUsize::new(0),
                reason: AtomicUsize::new(0),
                node: AtomicUsize::new(0),
                interface: AtomicUsize::new(0),
                node_flags: AtomicUsize::new(0),
                node_state: AtomicU8::new(0),
                association_id: AtomicUsize::new(0),
                interface_mode: AtomicUsize::new(0),
                interface_state: AtomicU8::new(0),
                interface_flags: AtomicUsize::new(0),
                interface_capabilities: AtomicUsize::new(0),
                interface_options: AtomicUsize::new(0),
            }
        }
    }

    #[link_section = ".critical.bss.wifi_strict.ap_assoc_rejection"]
    static AP_ASSOCIATION_REJECTION: AssociationRejectionDiagnostics =
        AssociationRejectionDiagnostics::new();

    unsafe extern "C" {
        static mut g_ic: u8;
        static mut g_wifi_nvs: *mut u8;
        static mut wpa_cb: *mut c_void;
        fn hostap_init() -> *mut c_void;
        fn hostap_deinit(context: *mut c_void) -> bool;
        fn wpa_ap_remove(peer: *const u8) -> bool;
        fn wpa_ap_get_wpa_ie(length: *mut usize) -> *mut u8;
        fn wpa_ap_get_peer_spp_msg(station: *mut c_void, capable: *mut bool, required: *mut bool);
        fn __esp_hostap_sta_join(join: *mut WpaStationJoinParam) -> bool;
        fn __esp_hostap_sta_join_end();
        fn cnx_node_search(peer: *const u8) -> *mut u8;
        fn ieee80211_getmgtframe(
            body: *mut *mut u8,
            header_length: u32,
            body_length: u32,
        ) -> *mut u8;
        fn ieee80211_set_tx_desc(
            node: *mut u8,
            buffer: *mut u8,
            rate_policy: u32,
            tid: u32,
            flags: u32,
        );
        #[link_name = "ieee80211_set_tx_pti"]
        fn linked_ieee80211_set_tx_pti(buffer: *mut u8, packet_type: u32);
        fn __real_ieee80211_set_tx_pti(buffer: *mut u8, packet_type: u32);
        static mut coex_pti_tab: [u8; 48];
        #[link_name = "ieee80211_mgmt_output"]
        fn linked_ieee80211_mgmt_output(node: *mut u8, buffer: *mut u8, subtype: u8) -> i32;
        fn __real_ieee80211_mgmt_output(node: *mut u8, buffer: *mut u8, subtype: u8) -> i32;
        #[cfg(not(feature = "strict-no-wait"))]
        fn chm_is_at_home_channel() -> bool;
        #[cfg(not(feature = "strict-no-wait"))]
        fn chm_get_home_channel() -> *const u8;
        fn ic_tx_pkt(buffer: *mut u8) -> i32;
        fn esf_buf_recycle(frame: *mut c_void);
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub enum Wpa2ApInstallError {
        SupplicantNotInitialized,
        InvalidRsn(Wpa2ApRsnError),
        ManagementFrameProtectionUnsupported,
        UnexpectedJoinSize(usize),
        UnexpectedInitCallback(usize),
        UnexpectedDeinitCallback(usize),
        UnexpectedJoinCallback(usize),
        UnexpectedRemoveCallback(usize),
        UnexpectedGetRsnCallback(usize),
        UnexpectedSppCallback(usize),
    }

    #[inline]
    unsafe fn is_at_home_channel() -> bool {
        #[cfg(feature = "strict-no-wait")]
        {
            crate::channel_switch::is_at_home_channel()
        }
        #[cfg(not(feature = "strict-no-wait"))]
        {
            chm_is_at_home_channel()
        }
    }

    unsafe fn callback_slot<T>(callbacks: *mut c_void, offset: usize) -> *mut T {
        callbacks.cast::<u8>().add(offset).cast::<T>()
    }

    unsafe fn station_mac(slot: &PeerSlot) -> [u8; 6] {
        let mut peer = [0; 6];
        ptr::copy_nonoverlapping(
            slot.station
                .get()
                .cast::<u8>()
                .add(PINNED_STATION_MAC_OFFSET),
            peer.as_mut_ptr(),
            peer.len(),
        );
        peer
    }

    unsafe fn claim_peer(peer: [u8; 6]) -> Option<*mut c_void> {
        let association_id = peer_node_association_id(&peer)?;
        for slot in &PEERS {
            if slot.claimed.load(Ordering::Acquire) && station_mac(slot) == peer {
                // A reassociation replaces the previous generation in place.
                // Wake its deferred owners before publishing the new epoch so
                // none can inherit readiness from the replacement session.
                crate::ap_power_save::observe_peer_removed(&peer);
                slot.association_epoch.store(
                    next_association_epoch(),
                    Ordering::Release,
                );
                slot.association_id
                    .store(usize::from(association_id), Ordering::Release);
                return Some(slot.station.get().cast());
            }
        }
        for slot in &PEERS {
            if slot
                .claimed
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                ptr::write_bytes(slot.station.get().cast::<u8>(), 0, PINNED_STATION_SIZE);
                ptr::copy_nonoverlapping(
                    peer.as_ptr(),
                    slot.station
                        .get()
                        .cast::<u8>()
                        .add(PINNED_STATION_MAC_OFFSET),
                    peer.len(),
                );
                slot.association_epoch.store(
                    next_association_epoch(),
                    Ordering::Release,
                );
                slot.association_id
                    .store(usize::from(association_id), Ordering::Release);
                return Some(slot.station.get().cast());
            }
        }
        None
    }

    unsafe fn peer_node_association_id(peer: &[u8; 6]) -> Option<u16> {
        let node = cnx_node_search(peer.as_ptr());
        if node.is_null() {
            return None;
        }
        let association_id = node.add(0x26).cast::<u16>().read_unaligned() & 0x3fff;
        (association_id != 0).then_some(association_id)
    }

    fn next_association_epoch() -> usize {
        let epoch = NEXT_ASSOCIATION_EPOCH.fetch_add(1, Ordering::Relaxed);
        if epoch == 0 {
            NEXT_ASSOCIATION_EPOCH.fetch_add(1, Ordering::Relaxed)
        } else {
            epoch
        }
    }

    unsafe fn release_peer(peer: &[u8; 6]) -> bool {
        for slot in &PEERS {
            if slot.claimed.load(Ordering::Acquire) && station_mac(slot) == *peer {
                ptr::write_bytes(slot.station.get().cast::<u8>(), 0, PINNED_STATION_SIZE);
                slot.association_id.store(0, Ordering::Release);
                slot.association_epoch.store(0, Ordering::Release);
                slot.claimed.store(false, Ordering::Release);
                return true;
            }
        }
        false
    }

    #[no_mangle]
    unsafe extern "C" fn __esp_wifi_async_wpa2_ap_init() -> *mut c_void {
        if AP_RSN_LEN.load(Ordering::Acquire) == 0 {
            ptr::null_mut()
        } else {
            AP_CONTEXT.0.get().cast()
        }
    }

    #[no_mangle]
    unsafe extern "C" fn __esp_wifi_async_wpa2_ap_deinit(context: *mut c_void) -> bool {
        if !ptr::eq(context, AP_CONTEXT.0.get().cast()) {
            return false;
        }
        for slot in &PEERS {
            ptr::write_bytes(slot.station.get().cast::<u8>(), 0, PINNED_STATION_SIZE);
            slot.claimed.store(false, Ordering::Release);
        }
        true
    }

    #[no_mangle]
    unsafe extern "C" fn __esp_wifi_async_wpa2_ap_get_rsn(length: *mut usize) -> *mut u8 {
        let rsn_length = AP_RSN_LEN.load(Ordering::Acquire);
        if length.is_null() || rsn_length == 0 {
            return ptr::null_mut();
        }
        length.write(rsn_length);
        AP_RSN.0.get().cast::<u8>()
    }

    unsafe fn send_association_response(peer: [u8; 6], subtype: i32, status: u16) -> bool {
        let Ok(subtype @ (0x10 | 0x30)) = u8::try_from(subtype) else {
            record_association_rejection(
                AssociationRejectionReason::UnsupportedSubtype,
                ptr::null_mut(),
            );
            return false;
        };
        if u8::try_from(status).is_err() {
            record_association_rejection(
                AssociationRejectionReason::UnsupportedStatus,
                ptr::null_mut(),
            );
            return false;
        }
        let node = cnx_node_search(peer.as_ptr());
        if node.is_null() || node.cast::<*mut u8>().read().is_null() {
            record_association_rejection(AssociationRejectionReason::NodeUnavailable, node);
            return false;
        }
        let Some(buffer) = construct_strict_ap_association_response(node, status) else {
            return false;
        };
        capture_ap_association_response(buffer, subtype, status);
        if status != 0 {
            // Exact side effect of the pinned `ieee80211_send_mgmt` response
            // branch: clear the association-pending bit after an error.
            let flags = node.add(0x0c).cast::<u32>();
            flags.write(flags.read() & 0xfdff_ffff);
        }
        ieee80211_set_tx_desc(node, buffer, 7, 0, 0);
        linked_ieee80211_set_tx_pti(buffer, 6);
        if linked_ieee80211_mgmt_output(node, buffer, subtype) != 0 {
            record_association_rejection(AssociationRejectionReason::ManagementOutput, node);
            return false;
        }
        true
    }

    unsafe fn record_association_rejection(reason: AssociationRejectionReason, node: *mut u8) {
        let interface = if node.is_null() {
            ptr::null_mut()
        } else {
            node.cast::<*mut u8>().read()
        };
        AP_ASSOCIATION_REJECTION
            .node
            .store(node as usize, Ordering::Relaxed);
        AP_ASSOCIATION_REJECTION
            .interface
            .store(interface as usize, Ordering::Relaxed);
        if !node.is_null() {
            AP_ASSOCIATION_REJECTION.node_flags.store(
                node.add(0x0c).cast::<u32>().read_unaligned() as usize,
                Ordering::Relaxed,
            );
            AP_ASSOCIATION_REJECTION
                .node_state
                .store(node.add(0x31).read(), Ordering::Relaxed);
            AP_ASSOCIATION_REJECTION.association_id.store(
                node.add(0x26).cast::<u16>().read_unaligned() as usize,
                Ordering::Relaxed,
            );
        }
        if !interface.is_null() {
            AP_ASSOCIATION_REJECTION.interface_mode.store(
                interface.add(0x138).cast::<u32>().read_unaligned() as usize,
                Ordering::Relaxed,
            );
            AP_ASSOCIATION_REJECTION
                .interface_state
                .store(interface.add(0x154).read(), Ordering::Relaxed);
            AP_ASSOCIATION_REJECTION.interface_flags.store(
                interface.add(0x144).cast::<u32>().read_unaligned() as usize,
                Ordering::Relaxed,
            );
            AP_ASSOCIATION_REJECTION.interface_capabilities.store(
                interface.add(0xa4).cast::<u32>().read_unaligned() as usize,
                Ordering::Relaxed,
            );
            AP_ASSOCIATION_REJECTION.interface_options.store(
                interface.add(0x228).cast::<u32>().read_unaligned() as usize,
                Ordering::Relaxed,
            );
        }
        AP_ASSOCIATION_REJECTION
            .reason
            .store(reason as usize, Ordering::Release);
        AP_ASSOCIATION_REJECTION
            .count
            .fetch_add(1, Ordering::Release);
    }

    unsafe fn reject_association_construction(
        reason: AssociationRejectionReason,
        node: *mut u8,
    ) -> Option<*mut u8> {
        record_association_rejection(reason, node);
        None
    }

    unsafe fn construct_strict_ap_association_response(
        node: *mut u8,
        status: u16,
    ) -> Option<*mut u8> {
        let interface = node.cast::<*mut u8>().read();
        if interface.is_null() {
            return reject_association_construction(
                AssociationRejectionReason::InterfaceUnavailable,
                node,
            );
        }
        if interface.add(0x138).cast::<u32>().read_unaligned() != 1 {
            return reject_association_construction(
                AssociationRejectionReason::InterfaceMode,
                node,
            );
        }
        if interface.add(0x154).read().wrapping_sub(2) > 1 {
            return reject_association_construction(
                AssociationRejectionReason::InterfaceState,
                node,
            );
        }
        if interface.add(0x144).cast::<u32>().read_unaligned() & 0x0080_0000 == 0 {
            return reject_association_construction(
                AssociationRejectionReason::InterfaceFlags,
                node,
            );
        }
        if interface.add(0xa4).cast::<u32>().read_unaligned() & 0x0000_2000 == 0 {
            return reject_association_construction(
                AssociationRejectionReason::InterfaceCapabilities,
                node,
            );
        }
        if interface.add(0x228).cast::<u32>().read_unaligned() & 1 != 0 {
            return reject_association_construction(
                AssociationRejectionReason::InterfaceOptions,
                node,
            );
        }
        if node.add(0x31).read().wrapping_sub(2) > 1 {
            return reject_association_construction(AssociationRejectionReason::NodeState, node);
        }
        let node_flags = node.add(0x0c).cast::<u32>().read_unaligned();
        if node_flags & 0x42 != 0x42 {
            return reject_association_construction(AssociationRejectionReason::NodeFlags, node);
        }
        if node_flags & 0xc0 == 0xc0 {
            return reject_association_construction(
                AssociationRejectionReason::NodePowerSaveFlags,
                node,
            );
        }
        if node.add(0x73).read() != STRICT_AP_BGN_RATES.len() as u8 {
            return reject_association_construction(AssociationRejectionReason::RateCount, node);
        }
        for (index, expected) in STRICT_AP_BGN_RATES.iter().copied().enumerate() {
            if node.add(0x74 + index).read() != expected {
                return reject_association_construction(
                    AssociationRejectionReason::RateValue,
                    node,
                );
            }
        }

        // In strict mode the home selector belongs to the Rust channel
        // resource adopted before handoff. The non-strict profile retains the
        // pinned finite getter. The second byte is only a secondary-channel
        // candidate: net80211 can recompute it after a station leaves even
        // while the configured AP bandwidth remains 20 MHz.
        #[cfg(feature = "strict-no-wait")]
        let primary_channel = crate::channel_switch::home_channel().map(|channel| channel[0]);
        #[cfg(not(feature = "strict-no-wait"))]
        let primary_channel = {
            let channel = chm_get_home_channel();
            (!channel.is_null()).then(|| channel.read())
        };
        let Some(primary_channel) = primary_channel else {
            return reject_association_construction(
                AssociationRejectionReason::ChannelUnavailable,
                node,
            );
        };
        // The authoritative AP bandwidth returned by
        // `wifi_get_bw_process` is the byte at g_wifi_nvs+0x3fb.
        let wifi_nvs = ptr::addr_of!(g_wifi_nvs).read();
        if wifi_nvs.is_null() {
            return reject_association_construction(
                AssociationRejectionReason::BandwidthStateUnavailable,
                node,
            );
        }
        // `wifi_bandwidth_t::WIFI_BW20` is encoded as one on this ABI.
        if wifi_nvs.add(0x3fb).read() != 1 {
            return reject_association_construction(
                AssociationRejectionReason::UnsupportedBandwidth,
                node,
            );
        }
        let association_id = node.add(0x26).cast::<u16>().read_unaligned();
        let mut body = ptr::null_mut();
        let buffer = ieee80211_getmgtframe(
            &mut body,
            24,
            STRICT_AP_ASSOCIATION_RESPONSE_BODY_LEN as u32,
        );
        if buffer.is_null() || body.is_null() {
            return reject_association_construction(
                AssociationRejectionReason::FrameAllocation,
                node,
            );
        }
        let response = &mut *body.cast::<[u8; STRICT_AP_ASSOCIATION_RESPONSE_BODY_LEN]>();
        if !write_strict_ap_bgn_ht20_association_response(
            response,
            status,
            association_id,
            primary_channel,
        ) {
            esf_buf_recycle(buffer.cast());
            return reject_association_construction(
                AssociationRejectionReason::ResponseParameters,
                node,
            );
        }
        buffer
            .add(0x14)
            .cast::<u32>()
            .write_unaligned(((STRICT_AP_ASSOCIATION_RESPONSE_BODY_LEN as u32) << 16) | 24);
        Some(buffer)
    }

    unsafe fn capture_ap_association_response(buffer: *mut u8, subtype: u8, status: u16) {
        if AP_ASSOCIATION_RESPONSE_CAPTURE
            .state
            .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }

        let mut snapshot = ApAssociationResponseSnapshot {
            captured: true,
            subtype,
            status,
            ..ApAssociationResponseSnapshot::empty()
        };
        let header_len = usize::from(buffer.add(0x14).cast::<u16>().read_unaligned());
        let body_len = usize::from(buffer.add(0x16).cast::<u16>().read_unaligned());
        let first_buffer = buffer.add(4).cast::<*mut u8>().read_unaligned();
        let header = if first_buffer.is_null() {
            ptr::null_mut()
        } else {
            first_buffer.add(4).cast::<*mut u8>().read_unaligned()
        };
        if header_len == 24 && !header.is_null() {
            snapshot.body_len = body_len;
            snapshot.truncated = body_len > snapshot.body.len();
            let copy_len = body_len.min(snapshot.body.len());
            ptr::copy_nonoverlapping(
                header.add(header_len),
                snapshot.body.as_mut_ptr(),
                copy_len,
            );
        }
        AP_ASSOCIATION_RESPONSE_CAPTURE
            .value
            .get()
            .write(snapshot);
        AP_ASSOCIATION_RESPONSE_CAPTURE
            .state
            .store(2, Ordering::Release);
    }

    pub(crate) fn management_link_wrappers_active() -> bool {
        ptr::eq(
            linked_ieee80211_mgmt_output as *const (),
            __wrap_ieee80211_mgmt_output as *const (),
        ) && ptr::eq(
            linked_ieee80211_set_tx_pti as *const (),
            __wrap_ieee80211_set_tx_pti as *const (),
        )
    }

    fn rejected_buffer_key(buffer: *mut u8) -> usize {
        if buffer.is_null() {
            1
        } else {
            buffer as usize
        }
    }

    unsafe fn is_bounded_ap_addba_response(buffer: *mut u8, subtype: u8) -> bool {
        if buffer.is_null() {
            return false;
        }
        let layout = buffer.add(0x24).cast::<u16>().read_unaligned();
        let header_len = buffer.add(0x14).cast::<u16>().read_unaligned();
        let body_len = buffer.add(0x16).cast::<u16>().read_unaligned();
        if subtype != 0xd0 || layout != 0 || header_len != 24 || body_len != 9 {
            return false;
        }
        let first_buffer = buffer.add(4).cast::<*mut u8>().read_unaligned();
        if first_buffer.is_null() {
            return false;
        }
        let header = first_buffer.add(4).cast::<*mut u8>().read_unaligned();
        if header.is_null() {
            return false;
        }
        is_bounded_ap_addba_response_layout(
            subtype,
            layout,
            header_len,
            body_len,
            header.add(24).read(),
            header.add(25).read(),
        )
    }

    unsafe fn ap_addba_response_body(buffer: *mut u8) -> Option<*mut u8> {
        if !is_bounded_ap_addba_response(buffer, 0xd0) {
            return None;
        }
        let first_buffer = buffer.add(4).cast::<*mut u8>().read_unaligned();
        let header = first_buffer.add(4).cast::<*mut u8>().read_unaligned();
        let body = header.add(24);
        (body.add(3).read() == 1 && body.add(4).read() == 0).then_some(body)
    }

    unsafe fn try_defer_ap_addba_response(node: *mut u8, buffer: *mut u8) -> bool {
        let Some(body) = ap_addba_response_body(buffer) else {
            return false;
        };
        let mut peer = [0_u8; 6];
        ptr::copy_nonoverlapping(node.add(4), peer.as_mut_ptr(), peer.len());
        if peer[0] & 1 != 0 {
            return false;
        }
        let Some(association_epoch) = wpa2_ap_peer_association_epoch(&peer) else {
            return false;
        };
        let mut owned_body = [0_u8; DEFERRED_AP_ACTION_BODY_LEN];
        ptr::copy_nonoverlapping(body, owned_body.as_mut_ptr(), owned_body.len());

        for slot in &DEFERRED_AP_MANAGEMENT {
            if slot.state.load(Ordering::Acquire) == DEFERRED_SLOT_READY
                && (*slot.value.get()).peer == peer
            {
                // Keep one command per peer, but refresh its dialog token and
                // parameters from the newest request. The original readiness
                // baseline remains authoritative: repeated sleeping traffic
                // must not turn into a synthetic retry edge.
                (*slot.value.get()).body = owned_body;
                (*slot.value.get()).association_epoch = association_epoch;
                DEFERRED_AP_MANAGEMENT_COALESCED.fetch_add(1, Ordering::Relaxed);
                esf_buf_recycle(buffer.cast());
                return true;
            }
        }

        let Some(slot) = DEFERRED_AP_MANAGEMENT.iter().find(|slot| {
            slot.state
                .compare_exchange(
                    DEFERRED_SLOT_EMPTY,
                    DEFERRED_SLOT_WRITING,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                )
                .is_ok()
        }) else {
            return false;
        };
        slot.value.get().write(DeferredApManagement {
            peer,
            body: owned_body,
            association_epoch,
            active_epoch: crate::ap_power_save::active_epoch(&peer),
            ps_poll_epoch: crate::ap_power_save::ps_poll_epoch(&peer),
            removal_epoch: crate::ap_power_save::removal_epoch(&peer),
        });
        slot.state.store(DEFERRED_SLOT_READY, Ordering::Release);
        strict_update_ap_tim(node, true);
        crate::ap_power_save::record_deferred_transmit(&peer);
        DEFERRED_AP_MANAGEMENT_CAPTURED.fetch_add(1, Ordering::Relaxed);
        esf_buf_recycle(buffer.cast());
        DEFERRED_AP_MANAGEMENT_READY.wake();
        true
    }

    fn release_deferred_slot(slot: &DeferredApManagementSlot) {
        unsafe { slot.value.get().write(DeferredApManagement::empty()) };
        slot.state.store(DEFERRED_SLOT_EMPTY, Ordering::Release);
    }

    pub(crate) unsafe fn strict_update_ap_tim(node: *mut u8, set: bool) -> bool {
        if node.is_null() {
            return false;
        }

        // Exact finite body of the pinned `ieee80211_set_tim` leaf. The AID
        // lives at node+0x26. Its low three bits select a bit in the 2048-byte
        // virtual TIM bitmap at g_ic+0x1b7; the remaining 11 bits select the
        // byte. The preceding byte's bit zero mirrors the BSS/self-node TIM.
        let association_id = node.add(0x26).cast::<u16>().read_unaligned();
        let bitmap =
            ptr::addr_of_mut!(g_ic).add(0x1b7 + usize::from((association_id >> 3) & 0x07ff));
        let previous = bitmap.read();
        let updated = updated_tim_bitmap_byte(previous, association_id, set);
        if updated == previous {
            return false;
        }
        bitmap.write(updated);

        let interface = crate::net80211_state::access_point_interface()
            .map(|interface| interface.as_ptr())
            .unwrap_or(ptr::null_mut());
        if !interface.is_null() && interface.add(0xec).cast::<*mut u8>().read_unaligned() == node {
            let bss_tim = ptr::addr_of_mut!(g_ic).add(0x1b6);
            let flags = bss_tim.read();
            bss_tim.write(if set { flags | 1 } else { flags & !1 });
        }
        true
    }

    unsafe fn strict_deferred_ap_action_output(node: *mut u8, buffer: *mut u8) -> i32 {
        if !crate::critical::on_strict_wifi_hart()
            || !crate::context::in_radio_context()
            || node.is_null()
            || buffer.is_null()
            || node.add(4).read() & 1 != 0
            || !crate::net80211_state::ordinary_sta_ap_profile()
            || !is_at_home_channel()
        {
            if !buffer.is_null() {
                esf_buf_recycle(buffer.cast());
            }
            return -1;
        }
        let interface = node.cast::<*mut u8>().read();
        if interface.is_null() || interface.add(0x138).cast::<u32>().read() != 1 {
            esf_buf_recycle(buffer.cast());
            return -1;
        }
        let Some(body) = ap_addba_response_body(buffer) else {
            esf_buf_recycle(buffer.cast());
            return -1;
        };
        let first_buffer = buffer.add(4).cast::<*mut u8>().read_unaligned();
        let header = first_buffer.add(4).cast::<*mut u8>().read_unaligned();
        let descriptor = buffer.add(0x34).cast::<*mut u8>().read_unaligned();
        if descriptor.is_null() {
            esf_buf_recycle(buffer.cast());
            return -1;
        }

        // Exact valid AP/action branch of the pinned `ieee80211_send_setup`.
        // The management frame has no DS bits. Address 1 is the peer; address
        // 2 and BSSID are the AP MAC stored by `wifi_get_macaddr(AP)`.
        header.cast::<u16>().write_unaligned(0x00d0);
        header.add(2).cast::<u16>().write_unaligned(0);
        ptr::copy_nonoverlapping(node.add(4), header.add(4), 6);
        let ap_mac = ptr::addr_of!(g_ic).add(0x214);
        ptr::copy_nonoverlapping(ap_mac, header.add(10), 6);
        ptr::copy_nonoverlapping(ap_mac, header.add(16), 6);
        let sequence = node.add(0xce).cast::<u16>().read_unaligned();
        node.add(0xce)
            .cast::<u16>()
            .write_unaligned(sequence.wrapping_add(1));
        buffer
            .add(0x24)
            .cast::<u16>()
            .write_unaligned(sequence & 0x0fff);
        header
            .add(22)
            .cast::<u16>()
            .write_unaligned(sequence << 4);

        // Preserve the nine-byte body across the header stores, then reproduce
        // the bounded management descriptor/length tail before `ic_tx_pkt`.
        debug_assert_eq!(body, header.add(24));
        let callbacks = descriptor.add(0x14).cast::<u32>();
        callbacks.write_unaligned(callbacks.read_unaligned() | 4 | (1 << 13));
        let packet_length = 24_u32 + DEFERRED_AP_ACTION_BODY_LEN as u32;
        let flags = first_buffer.cast::<u32>();
        flags.write_unaligned(
            (flags.read_unaligned() & 0xf000_3fff) | 0xc000_0000 | (packet_length << 14),
        );
        ic_tx_pkt(buffer)
    }

    unsafe fn submit_deferred_ap_management(command: DeferredApManagement) -> bool {
        let node = cnx_node_search(command.peer.as_ptr());
        if node.is_null() || node.cast::<*mut u8>().read().is_null() || node.add(4).read() & 1 != 0
        {
            return false;
        }
        let mut body = ptr::null_mut();
        let buffer = ieee80211_getmgtframe(&mut body, 24, DEFERRED_AP_ACTION_BODY_LEN as u32);
        if buffer.is_null() || body.is_null() {
            return false;
        }
        ptr::copy_nonoverlapping(command.body.as_ptr(), body, command.body.len());
        buffer
            .add(0x14)
            .cast::<u32>()
            .write_unaligned(((DEFERRED_AP_ACTION_BODY_LEN as u32) << 16) | 24);
        ieee80211_set_tx_desc(node, buffer, 7, 0, 0);
        let descriptor = buffer.add(0x34).cast::<*mut u8>().read_unaligned();
        if descriptor.is_null() {
            esf_buf_recycle(buffer.cast());
            return false;
        }
        let callbacks = descriptor.add(0x14).cast::<u32>();
        callbacks.write_unaligned(callbacks.read_unaligned() | (1 << 13));

        let result = strict_deferred_ap_action_output(node, buffer);
        if result == 0 {
            strict_update_ap_tim(node, false);
            true
        } else {
            false
        }
    }

    pub(crate) fn poll_deferred_ap_management(cx: &mut Context<'_>) -> bool {
        DEFERRED_AP_MANAGEMENT_READY.register(cx.waker());
        let mut handled = false;
        for slot in &DEFERRED_AP_MANAGEMENT {
            if slot.state.load(Ordering::Acquire) != DEFERRED_SLOT_READY {
                continue;
            }
            let command = unsafe { *slot.value.get() };
            if wpa2_ap_peer_association_epoch(&command.peer) != Some(command.association_epoch) {
                release_deferred_slot(slot);
                DEFERRED_AP_MANAGEMENT_CANCELLED.fetch_add(1, Ordering::Relaxed);
                handled = true;
                continue;
            }
            match crate::ap_power_save::poll_peer_edge(
                command.active_epoch,
                command.ps_poll_epoch,
                command.removal_epoch,
                &command.peer,
                cx,
            ) {
                Poll::Pending => continue,
                Poll::Ready(crate::ap_power_save::PeerEdge::Removed) => {
                    release_deferred_slot(slot);
                    DEFERRED_AP_MANAGEMENT_CANCELLED.fetch_add(1, Ordering::Relaxed);
                    handled = true;
                }
                Poll::Ready(crate::ap_power_save::PeerEdge::Retry) => {
                    let submitted = unsafe { submit_deferred_ap_management(command) };
                    release_deferred_slot(slot);
                    if submitted {
                        DEFERRED_AP_MANAGEMENT_SUBMITTED.fetch_add(1, Ordering::Relaxed);
                    } else {
                        DEFERRED_AP_MANAGEMENT_DROPPED.fetch_add(1, Ordering::Relaxed);
                    }
                    handled = true;
                }
            }
        }
        handled
    }

    unsafe fn record_management_tx_rejection(
        reason: ManagementTxRejectionReason,
        subtype: u8,
        node: *mut u8,
        buffer: *mut u8,
    ) {
        MANAGEMENT_TX_REJECTION
            .count
            .fetch_add(1, Ordering::Relaxed);
        // The radio owner is the sole writer. Preserve its first complete
        // rejection oracle so an async diagnostic reader cannot observe a
        // mixture of fields from two rapidly repeated management attempts.
        if MANAGEMENT_TX_REJECTION.captured.load(Ordering::Acquire) {
            return;
        }
        let mut layout = 0_u16;
        let mut header_len = 0_u16;
        let mut body_len = 0_u16;
        let mut raw_frame_control = 0_u16;
        let mut frame_control = 0_u16;
        let mut raw_word0 = 0_u32;
        let mut raw_word1 = 0_u32;
        let mut body_word0 = 0_u32;
        let mut body_word1 = 0_u32;
        let mut category = 0_u8;
        let mut action = 0_u8;
        if !buffer.is_null() {
            layout = buffer.add(0x24).cast::<u16>().read_unaligned();
            header_len = buffer.add(0x14).cast::<u16>().read_unaligned();
            body_len = buffer.add(0x16).cast::<u16>().read_unaligned();
            let first_buffer = buffer.add(4).cast::<*mut u8>().read_unaligned();
            if !first_buffer.is_null() {
                let mut header = first_buffer.add(4).cast::<*mut u8>().read_unaligned();
                if !header.is_null() {
                    raw_frame_control = header.cast::<u16>().read_unaligned();
                    raw_word0 = header.cast::<u32>().read_unaligned();
                    raw_word1 = header.add(4).cast::<u32>().read_unaligned();
                    if layout & 0x2000 != 0 {
                        header = header.add(8);
                    }
                    frame_control = header.cast::<u16>().read_unaligned();
                    // `ieee80211_mgmt_output` receives a buffer whose body has
                    // already been constructed, but `ieee80211_send_setup`
                    // has not replaced the 802.11 header yet. A recycled
                    // buffer can therefore still expose the previous frame
                    // control while the action body at the fixed 24-byte
                    // management-header offset is already authoritative.
                    if body_len >= 1 {
                        category = header.add(24).read();
                    }
                    if body_len >= 2 {
                        action = header.add(25).read();
                    }
                    if body_len >= 4 {
                        body_word0 = header.add(24).cast::<u32>().read_unaligned();
                    }
                    if body_len >= 8 {
                        body_word1 = header.add(28).cast::<u32>().read_unaligned();
                    }
                }
            }
        }
        let interface = if node.is_null() {
            ptr::null_mut()
        } else {
            node.cast::<*mut u8>().read_unaligned()
        };
        let interface_mode = if interface.is_null() {
            u32::MAX
        } else {
            interface.add(0x138).cast::<u32>().read_unaligned()
        };
        let node_flags = if node.is_null() {
            0
        } else {
            node.add(0x0c).cast::<u32>().read_unaligned()
        };

        MANAGEMENT_TX_REJECTION
            .subtype
            .store(subtype, Ordering::Relaxed);
        MANAGEMENT_TX_REJECTION
            .node
            .store(node as usize, Ordering::Relaxed);
        MANAGEMENT_TX_REJECTION
            .buffer
            .store(buffer as usize, Ordering::Relaxed);
        MANAGEMENT_TX_REJECTION
            .layout
            .store(usize::from(layout), Ordering::Relaxed);
        MANAGEMENT_TX_REJECTION
            .header_len
            .store(usize::from(header_len), Ordering::Relaxed);
        MANAGEMENT_TX_REJECTION
            .body_len
            .store(usize::from(body_len), Ordering::Relaxed);
        MANAGEMENT_TX_REJECTION
            .raw_frame_control
            .store(usize::from(raw_frame_control), Ordering::Relaxed);
        MANAGEMENT_TX_REJECTION
            .frame_control
            .store(usize::from(frame_control), Ordering::Relaxed);
        MANAGEMENT_TX_REJECTION
            .raw_word0
            .store(raw_word0 as usize, Ordering::Relaxed);
        MANAGEMENT_TX_REJECTION
            .raw_word1
            .store(raw_word1 as usize, Ordering::Relaxed);
        MANAGEMENT_TX_REJECTION
            .body_word0
            .store(body_word0 as usize, Ordering::Relaxed);
        MANAGEMENT_TX_REJECTION
            .body_word1
            .store(body_word1 as usize, Ordering::Relaxed);
        MANAGEMENT_TX_REJECTION
            .category
            .store(category, Ordering::Relaxed);
        MANAGEMENT_TX_REJECTION
            .action
            .store(action, Ordering::Relaxed);
        MANAGEMENT_TX_REJECTION
            .interface_mode
            .store(interface_mode as usize, Ordering::Relaxed);
        MANAGEMENT_TX_REJECTION
            .node_flags
            .store(node_flags as usize, Ordering::Relaxed);
        MANAGEMENT_TX_REJECTION
            .reason
            .store(reason as usize, Ordering::Release);
        MANAGEMENT_TX_REJECTION
            .captured
            .store(true, Ordering::Release);
    }

    unsafe fn reject_management_tx(
        reason: ManagementTxRejectionReason,
        subtype: u8,
        node: *mut u8,
        buffer: *mut u8,
    ) -> i32 {
        record_management_tx_rejection(reason, subtype, node, buffer);
        if !buffer.is_null() && crate::critical::on_strict_wifi_hart() {
            esf_buf_recycle(buffer.cast());
        }
        -1
    }

    /// Strict gate for the stock management-frame output routine.
    ///
    /// The accepted AP/STA subtypes take the ordinary fixed-channel branch.
    /// Mesh, NAN, power-save buffering, robust management, disconnect, and
    /// off-channel queuing are rejected before the vendor body is entered.
    ///
    /// # Safety
    ///
    /// `node` and `buffer` must be live objects from the pinned net80211 ABI,
    /// with ownership transferred according to `ieee80211_mgmt_output`.
    #[no_mangle]
    pub unsafe extern "C" fn __wrap_ieee80211_mgmt_output(
        node: *mut u8,
        buffer: *mut u8,
        subtype: u8,
    ) -> i32 {
        if !crate::critical::strict_wifi_hart_armed() {
            return __real_ieee80211_mgmt_output(node, buffer, subtype);
        }
        if PTI_REJECTED_BUFFER
            .compare_exchange(
                rejected_buffer_key(buffer),
                0,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
        {
            if !buffer.is_null() && crate::critical::on_strict_wifi_hart() {
                esf_buf_recycle(buffer.cast());
            }
            return -1;
        }
        let on_wifi_hart = crate::critical::on_strict_wifi_hart();
        let in_radio_context = crate::context::in_radio_context();
        let owned_action = crate::sta_link::is_owned_action_management(buffer, subtype);
        let ap_addba_response = if !owned_action
            && on_wifi_hart
            && in_radio_context
            && !buffer.is_null()
        {
            is_bounded_ap_addba_response(buffer, subtype)
        } else {
            false
        };
        let subtype_allowed = matches!(subtype, 0x00 | 0x10 | 0x20 | 0x30 | 0x40 | 0x50 | 0xb0)
            || owned_action
            || ap_addba_response;
        let rejection = if !on_wifi_hart {
            Some(ManagementTxRejectionReason::WrongHart)
        } else if !in_radio_context {
            Some(ManagementTxRejectionReason::OutsideRadioContext)
        } else if node.is_null() {
            Some(ManagementTxRejectionReason::NullNode)
        } else if buffer.is_null() {
            Some(ManagementTxRejectionReason::NullBuffer)
        } else if !subtype_allowed {
            Some(ManagementTxRejectionReason::UnsupportedSubtype)
        } else if !crate::net80211_state::ordinary_sta_ap_profile() {
            Some(ManagementTxRejectionReason::MeshEnabled)
        } else if !is_at_home_channel() {
            Some(ManagementTxRejectionReason::OffHomeChannel)
        } else {
            None
        };
        if let Some(reason) = rejection {
            return reject_management_tx(reason, subtype, node, buffer);
        }
        let interface = node.cast::<*mut u8>().read();
        if interface.is_null() {
            return reject_management_tx(
                ManagementTxRejectionReason::NullInterface,
                subtype,
                node,
                buffer,
            );
        }
        let mode = interface.add(0x138).cast::<u32>().read();
        if mode > 1 {
            return reject_management_tx(
                ManagementTxRejectionReason::UnsupportedInterfaceMode,
                subtype,
                node,
                buffer,
            );
        }
        if ap_addba_response && mode != 1 {
            return reject_management_tx(
                ManagementTxRejectionReason::UnsupportedSubtype,
                subtype,
                node,
                buffer,
            );
        }
        if mode == 1
            && (node.add(0x04).read() & 1 != 0
                || node.add(0x0c).cast::<u32>().read() & 0x10 != 0
                || node.add(0x2fe).read() != 0)
        {
            if ap_addba_response
                && node.add(0x04).read() & 1 == 0
                && try_defer_ap_addba_response(node, buffer)
            {
                return 0;
            }
            return reject_management_tx(
                ManagementTxRejectionReason::ApNodeState,
                subtype,
                node,
                buffer,
            );
        }
        #[cfg(feature = "hil-rx-ampdu")]
        let mut rx_ampdu_accepted_peer = None;
        #[cfg(feature = "hil-rx-ampdu")]
        if ap_addba_response {
            if let Some(body) = ap_addba_response_body(buffer) {
                let mut peer = [0_u8; 6];
                ptr::copy_nonoverlapping(node.add(4), peer.as_mut_ptr(), peer.len());
                let body =
                    core::slice::from_raw_parts_mut(body, DEFERRED_AP_ACTION_BODY_LEN);
                if crate::rx_ampdu_ap::try_accept_response(peer, body) {
                    rx_ampdu_accepted_peer = Some(peer);
                }
            }
        }
        let result = __real_ieee80211_mgmt_output(node, buffer, subtype);
        #[cfg(feature = "hil-rx-ampdu")]
        if result != 0 {
            if let Some(peer) = rx_ampdu_accepted_peer {
                crate::rx_ampdu_ap::rollback_failed_response(peer);
            }
        }
        result
    }

    /// Replace the OSI-table coexistence PTI call with its exact finite leaf.
    ///
    /// # Safety
    ///
    /// `buffer` must point to a live pinned ESF object with a writable TX
    /// descriptor at offset `0x34`.
    #[no_mangle]
    pub unsafe extern "C" fn __wrap_ieee80211_set_tx_pti(buffer: *mut u8, event: u32) {
        if !crate::critical::strict_wifi_hart_armed() {
            __real_ieee80211_set_tx_pti(buffer, event);
            return;
        }
        PTI_REJECTED_BUFFER.store(0, Ordering::Release);
        if !crate::critical::on_strict_wifi_hart() {
            record_management_tx_rejection(
                ManagementTxRejectionReason::PtiWrongHart,
                event as u8,
                ptr::null_mut(),
                buffer,
            );
            PTI_REJECTED_BUFFER.store(rejected_buffer_key(buffer), Ordering::Release);
            return;
        }
        if buffer.is_null() {
            record_management_tx_rejection(
                ManagementTxRejectionReason::PtiNullBuffer,
                event as u8,
                ptr::null_mut(),
                buffer,
            );
            PTI_REJECTED_BUFFER.store(rejected_buffer_key(buffer), Ordering::Release);
            return;
        }
        let descriptor = buffer.add(0x34).cast::<*mut u8>().read();
        if descriptor.is_null() {
            record_management_tx_rejection(
                ManagementTxRejectionReason::PtiMissingDescriptor,
                event as u8,
                ptr::null_mut(),
                buffer,
            );
            PTI_REJECTED_BUFFER.store(rejected_buffer_key(buffer), Ordering::Release);
            return;
        }
        if event >= 48 {
            record_management_tx_rejection(
                ManagementTxRejectionReason::PtiEventOutOfRange,
                event as u8,
                ptr::null_mut(),
                buffer,
            );
            PTI_REJECTED_BUFFER.store(rejected_buffer_key(buffer), Ordering::Release);
            return;
        }
        let event_index = event as usize;
        // Exact success branch of the pinned 0x1e-byte
        // `coex_core_pti_get`: one indexed byte read from its exported table.
        let pti = ptr::read_volatile(
            ptr::addr_of_mut!(coex_pti_tab)
                .cast::<u8>()
                .add(event_index),
        );
        descriptor.add(0x20).write(pti);
        descriptor.add(0x22).cast::<u16>().write(1);
    }

    #[no_mangle]
    unsafe extern "C" fn __esp_wifi_async_wpa2_ap_join(join: *mut WpaStationJoinParam) -> bool {
        if join.is_null() || !crate::context::in_radio_context() {
            REJECTED.fetch_add(1, Ordering::Relaxed);
            return false;
        }
        let join = &mut *join;
        if join.station.is_null()
            || join.bssid.is_null()
            || join.wpa_ie.is_null()
            || join.pmf_enable.is_null()
            || join.pairwise_cipher.is_null()
            || !join.rsnxe.is_null()
            || join.rsnxe_len != 0
            || !join.rsn_selection_ie.is_null()
            || !join.owe_dhie.is_null()
            || join.owe_dh_len != 0
        {
            REJECTED.fetch_add(1, Ordering::Relaxed);
            return false;
        }

        let mut peer = [0; 6];
        peer.copy_from_slice(core::slice::from_raw_parts(join.bssid, 6));
        let rsn_bytes = core::slice::from_raw_parts(join.wpa_ie, usize::from(join.wpa_ie_len));
        let rsn_ie = match validate_wpa2_ap_rsn(rsn_bytes) {
            Ok(rsn_ie) => {
                record_join_rsn(rsn_bytes, None);
                rsn_ie
            }
            Err(error) => {
                record_join_rsn(rsn_bytes, Some(error));
                let _ = send_association_response(peer, join.subtype, WLAN_STATUS_INVALID_IE);
                REJECTED.fetch_add(1, Ordering::Relaxed);
                return false;
            }
        };

        if EVENTS.len() >= WPA2_AP_ASSOC_CAPACITY {
            let _ = send_association_response(
                peer,
                join.subtype,
                WLAN_STATUS_AP_UNABLE_TO_HANDLE_NEW_STA,
            );
            REJECTED.fetch_add(1, Ordering::Relaxed);
            return false;
        }
        let Some(station) = claim_peer(peer) else {
            let _ = send_association_response(
                peer,
                join.subtype,
                WLAN_STATUS_AP_UNABLE_TO_HANDLE_NEW_STA,
            );
            REJECTED.fetch_add(1, Ordering::Relaxed);
            return false;
        };
        if !send_association_response(peer, join.subtype, WLAN_STATUS_SUCCESS) {
            release_peer(&peer);
            REJECTED.fetch_add(1, Ordering::Relaxed);
            return false;
        }

        join.station.write(station);
        join.pmf_enable.write(false);
        join.pairwise_cipher.write(CCMP_DRIVER_SELECTOR);
        let event = Wpa2ApPeerEvent::Associated {
            peer,
            rsn_ie,
            reassociation: join.subtype == 0x30,
        };
        if EVENTS.try_send(event).is_err() {
            // The callback is the sole producer in the serialized radio
            // context, so only violated integration assumptions reach here.
            release_peer(&peer);
            REJECTED.fetch_add(1, Ordering::Relaxed);
            return false;
        }
        JOIN_ACCEPTED.fetch_add(1, Ordering::Relaxed);
        true
    }

    #[no_mangle]
    unsafe extern "C" fn __esp_wifi_async_wpa2_ap_remove(peer: *const u8) -> bool {
        if peer.is_null() || !crate::context::in_radio_context() {
            REJECTED.fetch_add(1, Ordering::Relaxed);
            return false;
        }
        let mut owned_peer = [0; 6];
        owned_peer.copy_from_slice(core::slice::from_raw_parts(peer, 6));
        // Publish while the association generation is still available.
        // `ap_power_save` keys the removal edge by both MAC and generation so
        // an old disconnect cannot cancel traffic for a later reassociation.
        crate::ap_power_save::observe_peer_removed(&owned_peer);
        let existed = release_peer(&owned_peer);
        if EVENTS
            .try_send(Wpa2ApPeerEvent::Removed { peer: owned_peer })
            .is_err()
        {
            REJECTED.fetch_add(1, Ordering::Relaxed);
        }
        existed
    }

    #[no_mangle]
    unsafe extern "C" fn __esp_wifi_async_wpa2_ap_get_peer_spp_msg(
        _station: *mut c_void,
        capable: *mut bool,
        required: *mut bool,
    ) {
        if !capable.is_null() {
            capable.write(false);
        }
        if !required.is_null() {
            required.write(false);
        }
    }

    /// Patch the three AP callbacks that otherwise own heap-backed station
    /// state. Install after `esp_supplicant_init`, before starting AP RX.
    ///
    /// The companion `ld/esp32s31-wpa2-ap-locals.x` fragment is required.
    ///
    /// # Safety
    /// The pinned audited S31 archives must be linked. Installation must be
    /// serialized with supplicant init/deinit and no AP station may exist yet.
    pub unsafe fn install_async_wpa2_ap_callbacks(rsn_ie: &[u8]) -> Result<(), Wpa2ApInstallError> {
        const _: () = assert!(mem::size_of::<WpaStationJoinParam>() == 0x28);

        let validated_rsn = validate_wpa2_ap_rsn_with_capabilities(rsn_ie)
            .map_err(Wpa2ApInstallError::InvalidRsn)?;
        if validated_rsn.capabilities & RSN_CAPABILITY_MFPC != 0 {
            return Err(Wpa2ApInstallError::ManagementFrameProtectionUnsupported);
        }
        let owned_rsn = validated_rsn.owned;

        let join_size = (__esp_hostap_sta_join_end as *const () as usize)
            .wrapping_sub(__esp_hostap_sta_join as *const () as usize);
        // The archive audit pins the 0x114-byte input section. The linker can
        // shrink its call pairs to a 0xf8-byte final flash image.
        if join_size != 0x114 && join_size != 0xf8 {
            return Err(Wpa2ApInstallError::UnexpectedJoinSize(join_size));
        }
        let callbacks = ptr::addr_of!(wpa_cb).read();
        if callbacks.is_null() {
            return Err(Wpa2ApInstallError::SupplicantNotInitialized);
        }

        let init = callback_slot::<InitCallback>(callbacks, WPA_AP_INIT_OFFSET);
        let deinit = callback_slot::<DeinitCallback>(callbacks, WPA_AP_DEINIT_OFFSET);
        let join = callback_slot::<JoinCallback>(callbacks, WPA_AP_JOIN_OFFSET);
        let remove = callback_slot::<RemoveCallback>(callbacks, WPA_AP_REMOVE_OFFSET);
        let get_rsn = callback_slot::<GetRsnCallback>(callbacks, WPA_AP_GET_RSN_OFFSET);
        let spp = callback_slot::<SppCallback>(callbacks, WPA_AP_SPP_OFFSET);
        let current_init = init.read() as usize;
        let current_deinit = deinit.read() as usize;
        let current_join = join.read() as usize;
        let current_remove = remove.read() as usize;
        let current_get_rsn = get_rsn.read() as usize;
        let current_spp = spp.read() as usize;
        if current_init != hostap_init as InitCallback as usize
            && current_init != __esp_wifi_async_wpa2_ap_init as InitCallback as usize
        {
            return Err(Wpa2ApInstallError::UnexpectedInitCallback(current_init));
        }
        if current_deinit != hostap_deinit as DeinitCallback as usize
            && current_deinit != __esp_wifi_async_wpa2_ap_deinit as DeinitCallback as usize
        {
            return Err(Wpa2ApInstallError::UnexpectedDeinitCallback(current_deinit));
        }
        if current_join != __esp_hostap_sta_join as JoinCallback as usize
            && current_join != __esp_wifi_async_wpa2_ap_join as JoinCallback as usize
        {
            return Err(Wpa2ApInstallError::UnexpectedJoinCallback(current_join));
        }
        if current_remove != wpa_ap_remove as RemoveCallback as usize
            && current_remove != __esp_wifi_async_wpa2_ap_remove as RemoveCallback as usize
        {
            return Err(Wpa2ApInstallError::UnexpectedRemoveCallback(current_remove));
        }
        if current_get_rsn != wpa_ap_get_wpa_ie as GetRsnCallback as usize
            && current_get_rsn != __esp_wifi_async_wpa2_ap_get_rsn as GetRsnCallback as usize
        {
            return Err(Wpa2ApInstallError::UnexpectedGetRsnCallback(
                current_get_rsn,
            ));
        }
        if current_spp != wpa_ap_get_peer_spp_msg as SppCallback as usize
            && current_spp != __esp_wifi_async_wpa2_ap_get_peer_spp_msg as SppCallback as usize
        {
            return Err(Wpa2ApInstallError::UnexpectedSppCallback(current_spp));
        }

        let rsn = owned_rsn.as_bytes();
        ptr::copy_nonoverlapping(rsn.as_ptr(), AP_RSN.0.get().cast::<u8>(), rsn.len());
        AP_RSN_LEN.store(rsn.len(), Ordering::Release);

        init.write(__esp_wifi_async_wpa2_ap_init);
        deinit.write(__esp_wifi_async_wpa2_ap_deinit);
        join.write(__esp_wifi_async_wpa2_ap_join);
        remove.write(__esp_wifi_async_wpa2_ap_remove);
        get_rsn.write(__esp_wifi_async_wpa2_ap_get_rsn);
        spp.write(__esp_wifi_async_wpa2_ap_get_peer_spp_msg);
        CALLBACKS_INSTALLED.store(true, Ordering::Release);
        Ok(())
    }

    pub fn async_wpa2_ap_callbacks_installed() -> bool {
        CALLBACKS_INSTALLED.load(Ordering::Acquire)
    }

    pub fn management_tx_rejection_snapshot() -> ManagementTxRejectionSnapshot {
        ManagementTxRejectionSnapshot {
            count: MANAGEMENT_TX_REJECTION.count.load(Ordering::Acquire),
            captured: MANAGEMENT_TX_REJECTION.captured.load(Ordering::Acquire),
            reason: MANAGEMENT_TX_REJECTION.reason.load(Ordering::Acquire) as u32,
            subtype: MANAGEMENT_TX_REJECTION.subtype.load(Ordering::Acquire),
            node: MANAGEMENT_TX_REJECTION.node.load(Ordering::Acquire),
            buffer: MANAGEMENT_TX_REJECTION.buffer.load(Ordering::Acquire),
            layout: MANAGEMENT_TX_REJECTION.layout.load(Ordering::Acquire) as u16,
            header_len: MANAGEMENT_TX_REJECTION.header_len.load(Ordering::Acquire) as u16,
            body_len: MANAGEMENT_TX_REJECTION.body_len.load(Ordering::Acquire) as u16,
            raw_frame_control: MANAGEMENT_TX_REJECTION
                .raw_frame_control
                .load(Ordering::Acquire) as u16,
            frame_control: MANAGEMENT_TX_REJECTION
                .frame_control
                .load(Ordering::Acquire) as u16,
            raw_word0: MANAGEMENT_TX_REJECTION.raw_word0.load(Ordering::Acquire) as u32,
            raw_word1: MANAGEMENT_TX_REJECTION.raw_word1.load(Ordering::Acquire) as u32,
            body_word0: MANAGEMENT_TX_REJECTION.body_word0.load(Ordering::Acquire) as u32,
            body_word1: MANAGEMENT_TX_REJECTION.body_word1.load(Ordering::Acquire) as u32,
            category: MANAGEMENT_TX_REJECTION.category.load(Ordering::Acquire),
            action: MANAGEMENT_TX_REJECTION.action.load(Ordering::Acquire),
            interface_mode: MANAGEMENT_TX_REJECTION
                .interface_mode
                .load(Ordering::Acquire) as u32,
            node_flags: MANAGEMENT_TX_REJECTION.node_flags.load(Ordering::Acquire) as u32,
        }
    }

    pub fn deferred_ap_management_snapshot() -> DeferredApManagementSnapshot {
        DeferredApManagementSnapshot {
            captured: DEFERRED_AP_MANAGEMENT_CAPTURED.load(Ordering::Acquire),
            coalesced: DEFERRED_AP_MANAGEMENT_COALESCED.load(Ordering::Acquire),
            submitted: DEFERRED_AP_MANAGEMENT_SUBMITTED.load(Ordering::Acquire),
            cancelled: DEFERRED_AP_MANAGEMENT_CANCELLED.load(Ordering::Acquire),
            dropped: DEFERRED_AP_MANAGEMENT_DROPPED.load(Ordering::Acquire),
            occupied: DEFERRED_AP_MANAGEMENT
                .iter()
                .filter(|slot| slot.state.load(Ordering::Acquire) == DEFERRED_SLOT_READY)
                .count(),
            capacity: WPA2_AP_ASSOC_CAPACITY,
        }
    }

    pub fn ap_association_response_snapshot() -> ApAssociationResponseSnapshot {
        if AP_ASSOCIATION_RESPONSE_CAPTURE.state.load(Ordering::Acquire) != 2 {
            ApAssociationResponseSnapshot::empty()
        } else {
            unsafe { *AP_ASSOCIATION_RESPONSE_CAPTURE.value.get() }
        }
    }

    pub fn ap_association_rejection_snapshot() -> ApAssociationRejectionSnapshot {
        ApAssociationRejectionSnapshot {
            count: AP_ASSOCIATION_REJECTION.count.load(Ordering::Acquire),
            reason: AP_ASSOCIATION_REJECTION.reason.load(Ordering::Acquire) as u32,
            node: AP_ASSOCIATION_REJECTION.node.load(Ordering::Acquire),
            interface: AP_ASSOCIATION_REJECTION.interface.load(Ordering::Acquire),
            node_flags: AP_ASSOCIATION_REJECTION.node_flags.load(Ordering::Acquire) as u32,
            node_state: AP_ASSOCIATION_REJECTION.node_state.load(Ordering::Acquire),
            association_id: AP_ASSOCIATION_REJECTION
                .association_id
                .load(Ordering::Acquire) as u16,
            interface_mode: AP_ASSOCIATION_REJECTION
                .interface_mode
                .load(Ordering::Acquire) as u32,
            interface_state: AP_ASSOCIATION_REJECTION
                .interface_state
                .load(Ordering::Acquire),
            interface_flags: AP_ASSOCIATION_REJECTION
                .interface_flags
                .load(Ordering::Acquire) as u32,
            interface_capabilities: AP_ASSOCIATION_REJECTION
                .interface_capabilities
                .load(Ordering::Acquire) as u32,
            interface_options: AP_ASSOCIATION_REJECTION
                .interface_options
                .load(Ordering::Acquire) as u32,
        }
    }

    pub(crate) fn wpa2_ap_peer_association_epoch(peer: &[u8; 6]) -> Option<usize> {
        PEERS.iter().find_map(|slot| {
            (slot.claimed.load(Ordering::Acquire) && unsafe { station_mac(slot) == *peer })
                .then(|| slot.association_epoch.load(Ordering::Acquire))
        })
    }

    pub(crate) fn wpa2_ap_peer_association_id(peer: &[u8; 6]) -> Option<u16> {
        PEERS.iter().find_map(|slot| {
            if !slot.claimed.load(Ordering::Acquire) || unsafe { station_mac(slot) != *peer } {
                return None;
            }
            let association_id = slot.association_id.load(Ordering::Acquire) as u16;
            (association_id != 0).then_some(association_id)
        })
    }
}

#[cfg(target_arch = "riscv32")]
pub use target::{
    ap_association_rejection_snapshot, ap_association_response_snapshot,
    async_wpa2_ap_callbacks_installed, deferred_ap_management_snapshot,
    install_async_wpa2_ap_callbacks, management_tx_rejection_snapshot, Wpa2ApInstallError,
};
#[cfg(target_arch = "riscv32")]
pub(crate) use target::{
    management_link_wrappers_active, poll_deferred_ap_management, strict_update_ap_tim,
    wpa2_ap_peer_association_epoch, wpa2_ap_peer_association_id,
};

#[cfg(test)]
mod tests {
    use super::*;

    fn rsn(pairwise: u8, akm: u8, capabilities: u16) -> [u8; 22] {
        let mut ie = [0_u8; 22];
        ie[0] = 0x30;
        ie[1] = 20;
        ie[2..4].copy_from_slice(&1_u16.to_le_bytes());
        ie[4..8].copy_from_slice(&[0x00, 0x0f, 0xac, 4]);
        ie[8..10].copy_from_slice(&1_u16.to_le_bytes());
        ie[10..14].copy_from_slice(&[0x00, 0x0f, 0xac, pairwise]);
        ie[14..16].copy_from_slice(&1_u16.to_le_bytes());
        ie[16..20].copy_from_slice(&[0x00, 0x0f, 0xac, akm]);
        ie[20..22].copy_from_slice(&capabilities.to_le_bytes());
        ie
    }

    #[test]
    fn accepts_wpa2_psk_ccmp() {
        let ie = rsn(4, 2, 0);
        assert_eq!(validate_wpa2_ap_rsn(&ie).unwrap().as_bytes(), &ie);
    }

    #[test]
    fn accepts_optional_station_mfp_capability() {
        let ie = rsn(4, 2, RSN_CAPABILITY_MFPC);
        assert_eq!(validate_wpa2_ap_rsn(&ie).unwrap().as_bytes(), &ie);
    }

    #[test]
    fn rejects_non_ccmp_non_psk_and_required_pmf() {
        assert_eq!(
            validate_wpa2_ap_rsn(&rsn(2, 2, 0)),
            Err(Wpa2ApRsnError::UnsupportedPairwiseCipher)
        );
        assert_eq!(
            validate_wpa2_ap_rsn(&rsn(4, 8, 0)),
            Err(Wpa2ApRsnError::UnsupportedAkm)
        );
        assert_eq!(
            validate_wpa2_ap_rsn(&rsn(4, 2, RSN_CAPABILITY_MFPR)),
            Err(Wpa2ApRsnError::ManagementFrameProtectionUnsupported)
        );
    }

    #[test]
    fn accepts_explicit_zero_pmkid_count_and_rejects_nonzero_lists() {
        let mut zero_pmkid = [0_u8; 24];
        zero_pmkid[..22].copy_from_slice(&rsn(4, 2, 0));
        zero_pmkid[1] = 22;
        assert_eq!(
            validate_wpa2_ap_rsn(&zero_pmkid).unwrap().as_bytes(),
            &zero_pmkid
        );

        zero_pmkid[22..24].copy_from_slice(&1_u16.to_le_bytes());
        assert_eq!(
            validate_wpa2_ap_rsn(&zero_pmkid),
            Err(Wpa2ApRsnError::PmkidCachingUnsupported)
        );
    }

    #[test]
    fn rejects_truncated_suite_lists() {
        let mut ie = rsn(4, 2, 0);
        ie[8..10].copy_from_slice(&2_u16.to_le_bytes());
        assert_eq!(validate_wpa2_ap_rsn(&ie), Err(Wpa2ApRsnError::Malformed));
    }

    #[test]
    fn admits_only_the_measured_ap_addba_response_layout() {
        assert!(is_bounded_ap_addba_response_layout(0xd0, 0, 24, 9, 3, 1));
        assert!(!is_bounded_ap_addba_response_layout(0xd0, 0, 24, 9, 3, 0));
        assert!(!is_bounded_ap_addba_response_layout(0xd0, 0, 24, 9, 3, 2));
        assert!(!is_bounded_ap_addba_response_layout(
            0xd0, 0x2000, 24, 9, 3, 1
        ));
        assert!(!is_bounded_ap_addba_response_layout(0xd0, 0, 24, 8, 3, 1));
    }

    #[test]
    fn tim_bitmap_update_matches_aid_bit_selection() {
        assert_eq!(updated_tim_bitmap_byte(0, 0, true), 0x01);
        assert_eq!(updated_tim_bitmap_byte(0, 7, true), 0x80);
        assert_eq!(updated_tim_bitmap_byte(0, 8, true), 0x01);
        assert_eq!(updated_tim_bitmap_byte(0xa5, 10, true), 0xa5);
        assert_eq!(updated_tim_bitmap_byte(0xa5, 8, false), 0xa4);
        assert_eq!(updated_tim_bitmap_byte(0xa5, 15, false), 0x25);
    }

    #[test]
    fn strict_bgn_ht20_association_response_matches_hardware_oracle() {
        let mut body = [0; STRICT_AP_ASSOCIATION_RESPONSE_BODY_LEN];
        assert!(write_strict_ap_bgn_ht20_association_response(
            &mut body, 0, 0xc001, 1
        ));
        assert_eq!(body, STRICT_AP_BGN_HT20_ASSOCIATION_RESPONSE);
        assert_eq!(
            &body[6..16],
            &[1, 8, 0x8b, 0x96, 0x82, 0x84, 12, 24, 48, 96]
        );
        assert_eq!(&body[16..22], &[50, 4, 0x6c, 0x12, 0x24, 0x48]);
        assert_eq!(&body[25..27], &[45, 26]);
        assert_eq!(&body[53..55], &[61, 22]);
        assert_eq!(&body[77..80], &[221, 24, 0]);
    }

    #[test]
    fn strict_association_response_owns_status_aid_and_channel() {
        let mut body = [0; STRICT_AP_ASSOCIATION_RESPONSE_BODY_LEN];
        assert!(write_strict_ap_bgn_ht20_association_response(
            &mut body, 17, 0xc123, 11
        ));
        assert_eq!(&body[2..4], &17_u16.to_le_bytes());
        assert_eq!(&body[4..6], &[0, 0]);
        assert_eq!(body[55], 11);
        assert!(!write_strict_ap_bgn_ht20_association_response(
            &mut body, 0, 0, 1
        ));
        assert!(!write_strict_ap_bgn_ht20_association_response(
            &mut body, 0, 1, 14
        ));
    }
}
