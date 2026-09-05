//! Bounded multi-peer AP MLME and security ownership.

use core::fmt;

mod block_ack;
mod peer;
mod power_save;
mod security;

// Preserve the existing service-level imports while limits own their definition.
pub use crate::limits::{
    AP_MAX_CLIENTS, AP_TIM_VIRTUAL_BITMAP_OCTETS, AccessPointClientLimit,
    AccessPointClientLimitError,
};

use open_esp_radio_ieee80211::ap::{ApAssociationSecurityObservation, ApPowerSaveObservation};
use open_esp_radio_ieee80211::beacon::{
    TimAssociationId, TimBitmapError, TimVirtualBitmap, WPA2_PERSONAL_CCMP_PSK_RSN_IE,
};
use open_esp_radio_ieee80211::block_ack::{
    AddbaRequest, BlockAckAction, OperationalTxBlockAck, TxBlockAckAlarm, TxBlockAckConfig,
    TxBlockAckError, TxBlockAckResponse, TxBlockAckSession,
};
use open_esp_radio_ieee80211::ht::HtPeerCapabilities;
use open_esp_radio_ieee80211::security::WifiSecurityMode;
use open_esp_radio_wpa2::{
    AssociationSecurityBinding, OwnedEapolFrame, Pmk, Ptk, PtkContext,
    aes::{SoftwareAesKeyWrapError, software_aes128_key_wrap},
    ap::validate_wpa2_ap_rsn,
    frames::{
        OwnedAssociationSecurityIes, OwnedRsnIe, WPA2_PLAIN_KEY_DATA_CAPACITY, Wpa2FrameError,
        Wpa2Gtk, Wpa2PlainKeyData, Wpa2TxFrame, build_ap_action_frame,
    },
    retry::{Wpa2Retry, Wpa2RetryAction, Wpa2RetryAlarm, Wpa2RetryConfig, Wpa2RetryError},
    state::{
        PtkContext as Wpa2StatePtkContext, Wpa2ApAction, Wpa2ApPhase, Wpa2ApState, Wpa2StateError,
    },
};

pub const AP_STATUS_SUCCESS: u16 = 0;
pub const AP_STATUS_TOO_MANY_STATIONS: u16 = 17;
pub const AP_STATUS_UNSUPPORTED_RATES: u16 = 18;
pub const AP_STATUS_INVALID_RSN: u16 = 40;
pub const AP_ASSOCIATION_DEADLINE_MICROS: u64 = 15_000_000;
/// AP-owned response window for each four-way-handshake publication.
///
/// This follows the generic hostap authenticator policy: a 100-ms first
/// EAPOL-Key response window, 1-second subsequent windows, and four total
/// publications. An acknowledged Message 1 uses the subsequent window
/// immediately. It is protocol policy, not an ESP32 hardware fact.
pub const AP_WPA2_FIRST_RETRY_INTERVAL_MICROS: u32 = 100_000;
pub const AP_WPA2_SUBSEQUENT_RETRY_INTERVAL_MICROS: u32 = 1_000_000;
/// Retransmissions after the original M1 or M3 publication.
pub const AP_WPA2_RETRY_ATTEMPTS: u8 = 3;
pub const AP_TX_BLOCK_ACK_TID: u8 = 0;
/// Bounded AP downlink window used for both negotiation and aggregate
/// admission. AP and STA use the same 32-MPDU production aggregate contract;
/// duplex fairness must be provided by the common MAC transaction scheduler,
/// not by weakening the AP's negotiated BlockAck capability.
pub const AP_TX_BLOCK_ACK_WINDOW: u16 = 32;
pub const AP_TX_BLOCK_ACK_NEGOTIATION_TIMEOUT_MICROS: u32 = 100_000;

/// Validated inactivity policy for an associated SoftAP peer.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct AccessPointInactiveTimeout(u16);

impl AccessPointInactiveTimeout {
    pub const MIN_SECONDS: u16 = 10;
    pub const MAX_SECONDS: u16 = 3_600;
    pub const DEFAULT_SECONDS: u16 = 300;

    pub const fn new(seconds: u16) -> Result<Self, AccessPointInactiveTimeoutError> {
        if seconds < Self::MIN_SECONDS || seconds > Self::MAX_SECONDS {
            return Err(AccessPointInactiveTimeoutError { seconds });
        }
        Ok(Self(seconds))
    }

    pub const fn seconds(self) -> u16 {
        self.0
    }

    pub const fn micros(self) -> u64 {
        self.0 as u64 * 1_000_000
    }
}

impl Default for AccessPointInactiveTimeout {
    fn default() -> Self {
        Self(Self::DEFAULT_SECONDS)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AccessPointInactiveTimeoutError {
    seconds: u16,
}

impl AccessPointInactiveTimeoutError {
    pub const fn seconds(self) -> u16 {
        self.seconds
    }
}

impl fmt::Display for AccessPointInactiveTimeoutError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "access-point inactivity timeout {} is outside {}..={} seconds",
            self.seconds,
            AccessPointInactiveTimeout::MIN_SECONDS,
            AccessPointInactiveTimeout::MAX_SECONDS,
        )
    }
}

impl core::error::Error for AccessPointInactiveTimeoutError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApPeerPhase {
    Authenticated,
    Securing,
    Authorized,
    Closing,
}

/// AP-visible power-management state for one associated peer.
///
/// This state follows the peer's most recently admitted PM bit. It does not
/// imply that a frame has already been moved into or out of the caller-owned
/// downlink queue.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ApPeerPowerState {
    #[default]
    Active,
    Sleeping,
}

/// Whether a newly arrived unicast frame may be transmitted immediately or
/// must remain in the caller-owned AP power-save queue.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApDownlinkDisposition {
    TransmitNow,
    Buffer,
}

/// Non-reusable identity of one associated AP peer.
///
/// The association ID names a bounded peer-table slot, but that slot can be
/// reused after teardown. The epoch fences retained data and scheduler state
/// from a later association which happens to reuse the same slot or address.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ApAssociationIdentity {
    address: [u8; 6],
    association_id: u16,
    association_epoch: u32,
}

impl ApAssociationIdentity {
    pub const fn new(
        address: [u8; 6],
        association_id: u16,
        association_epoch: u32,
    ) -> Option<Self> {
        if address[0] & 1 != 0
            || association_id == 0
            || association_id as usize > AP_MAX_CLIENTS
            || association_epoch == 0
        {
            return None;
        }
        Some(Self {
            address,
            association_id,
            association_epoch,
        })
    }

    pub const fn address(self) -> [u8; 6] {
        self.address
    }

    pub const fn association_id(self) -> u16 {
        self.association_id
    }

    pub const fn association_epoch(self) -> u32 {
        self.association_epoch
    }
}

/// Ownership decision for one authorized unicast downlink.
///
/// Keeping the generation-bound identity beside the power-save decision lets
/// a caller retain the payload without rescanning the peer table or reducing
/// its owner to a reusable MAC address.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ApDownlinkAdmission {
    identity: ApAssociationIdentity,
    disposition: ApDownlinkDisposition,
}

impl ApDownlinkAdmission {
    pub const fn identity(self) -> ApAssociationIdentity {
        self.identity
    }

    pub const fn disposition(self) -> ApDownlinkDisposition {
        self.disposition
    }
}

/// Semantic result of one admitted PM-bit or PS-Poll observation.
///
/// `ReleaseOne` reserves exactly one buffered-frame count until
/// [`AccessPointService::complete_buffered_unicast_release`] commits or rolls
/// the reservation back. The token is deliberately non-`Copy`.
#[derive(Debug, Eq, PartialEq)]
pub enum ApPowerSaveAction {
    None,
    StateChanged {
        peer: [u8; 6],
        state: ApPeerPowerState,
        buffered_frames: u16,
    },
    ReleaseOne(ApBufferedUnicastRelease),
}

/// Affine reservation for one caller-owned buffered unicast frame.
#[derive(Debug, Eq, PartialEq)]
pub struct ApBufferedUnicastRelease {
    identity: ApAssociationIdentity,
    more_data: bool,
}

impl ApBufferedUnicastRelease {
    pub const fn peer(&self) -> [u8; 6] {
        self.identity.address()
    }

    pub const fn association_id(&self) -> u16 {
        self.identity.association_id()
    }

    pub const fn identity(&self) -> ApAssociationIdentity {
        self.identity
    }

    /// Value for the 802.11 More Data bit on the released MPDU.
    pub const fn more_data(&self) -> bool {
        self.more_data
    }
}

/// Affine reservation for one caller-owned buffered group frame.
///
/// The AP service owns only the advertised count. The caller must retain the
/// exact multicast/broadcast payload before committing it, then bind the
/// oldest retained payload to this token after a successfully published DTIM
/// beacon. Dropping the token deliberately leaves the release blocked.
#[derive(Debug, Eq, PartialEq)]
pub struct ApBufferedGroupRelease {
    generation: u32,
    more_data: bool,
}

impl ApBufferedGroupRelease {
    /// Value for the 802.11 More Data bit on this group-addressed MPDU.
    pub const fn more_data(&self) -> bool {
        self.more_data
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApPeerCloseKind {
    AuthenticationTimeout,
    Wpa2HandshakeFailure,
    Wpa2HandshakeTimeout,
    InactivityTimeout,
    AccessPointStop,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ApPeerClose {
    pub peer: [u8; 6],
    pub kind: ApPeerCloseKind,
    pub was_associated: bool,
    pub maximum_legacy_rate_500kbps: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ApAssociationCapabilities {
    pub maximum_legacy_rate_500kbps: u8,
    pub ht: Option<HtPeerCapabilities>,
    pub qos_supported: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApMlmeAction {
    AuthenticationResponse {
        peer: [u8; 6],
        status: u16,
    },
    AssociationResponse {
        peer: [u8; 6],
        status: u16,
        association_id: Option<u16>,
    },
    BeginWpa2 {
        peer: [u8; 6],
    },
    PeerRemoved {
        peer: [u8; 6],
    },
    None,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApServiceError {
    UnknownPeer,
    WrongPeerPhase,
    SecurityModeMismatch,
    AssociationIdMismatch,
    BufferedTrafficOverflow,
    NoBufferedTraffic,
    BufferedReleaseInFlight,
    StaleBufferedRelease,
    Wpa2(Wpa2StateError),
    BlockAck(TxBlockAckError),
}

impl From<TxBlockAckError> for ApServiceError {
    fn from(error: TxBlockAckError) -> Self {
        Self::BlockAck(error)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApWpa2Error {
    Service(ApServiceError),
    Frame(Wpa2FrameError),
    KeyWrap(SoftwareAesKeyWrapError),
    MissingPairwiseKey,
    UnexpectedAction,
    Retry(Wpa2RetryError),
}

impl From<Wpa2RetryError> for ApWpa2Error {
    fn from(error: Wpa2RetryError) -> Self {
        Self::Retry(error)
    }
}

impl From<ApServiceError> for ApWpa2Error {
    fn from(error: ApServiceError) -> Self {
        Self::Service(error)
    }
}

impl From<Wpa2StateError> for ApWpa2Error {
    fn from(error: Wpa2StateError) -> Self {
        Self::Service(ApServiceError::Wpa2(error))
    }
}

impl From<Wpa2FrameError> for ApWpa2Error {
    fn from(error: Wpa2FrameError) -> Self {
        Self::Frame(error)
    }
}

impl From<SoftwareAesKeyWrapError> for ApWpa2Error {
    fn from(error: SoftwareAesKeyWrapError) -> Self {
        Self::KeyWrap(error)
    }
}

pub enum ApWpa2Progress<const N: usize> {
    None,
    Transmit(Wpa2TxFrame<N>),
    AuthorizePeer,
    DeauthenticatePeer,
}

pub enum ApWpa2RetryProgress<const N: usize> {
    None,
    Transmit {
        peer: [u8; 6],
        frame: Wpa2TxFrame<N>,
    },
    Close(ApPeerClose),
}

impl From<Wpa2StateError> for ApServiceError {
    fn from(error: Wpa2StateError) -> Self {
        Self::Wpa2(error)
    }
}

struct ApPeer {
    address: [u8; 6],
    association_id: u16,
    association_epoch: u32,
    phase: ApPeerPhase,
    wpa2: Option<Wpa2ApState>,
    association_security_binding: Option<AssociationSecurityBinding>,
    pending_ptk: Option<Ptk>,
    wpa2_retry: Wpa2Retry,
    wpa2_retry_alarm: Option<Wpa2RetryAlarm>,
    maximum_legacy_rate_500kbps: u8,
    ht: Option<HtPeerCapabilities>,
    qos_supported: bool,
    /// Independent QoS sequence spaces for this receiver's TIDs. TX BlockAck
    /// and receiver reorder state are peer+TID agreements; sharing these
    /// counters across AP clients creates artificial holes whenever the
    /// scheduler switches peers.
    next_qos_sequences: [u16; 8],
    tx_block_ack: TxBlockAckSession,
    power_state: ApPeerPowerState,
    buffered_unicast_frames: u16,
    buffered_release_in_flight: bool,
    last_activity_micros: u64,
    deadline_micros: u64,
}

/// Caller-owned storage for all per-client AP protocol and key state.
///
/// This is intentionally separate from [`AccessPointService`]. On embedded
/// targets the table is large enough that constructing or moving it through a
/// cooperative task stack is unsafe; the radio integration gives it a stable
/// static address instead.
pub struct AccessPointPeerStorage {
    peers: [Option<ApPeer>; AP_MAX_CLIENTS],
    generation: u32,
}

impl AccessPointPeerStorage {
    pub const fn new() -> Self {
        Self {
            peers: [const { None }; AP_MAX_CLIENTS],
            generation: 0,
        }
    }
}

impl Default for AccessPointPeerStorage {
    fn default() -> Self {
        Self::new()
    }
}

impl ApPeer {
    const fn authenticated(
        address: [u8; 6],
        association_id: u16,
        association_epoch: u32,
        now_micros: u64,
    ) -> Self {
        Self {
            address,
            association_id,
            association_epoch,
            phase: ApPeerPhase::Authenticated,
            wpa2: None,
            association_security_binding: None,
            pending_ptk: None,
            wpa2_retry: new_ap_wpa2_retry(),
            wpa2_retry_alarm: None,
            // Authentication precedes rate negotiation. Keep the universally
            // compatible 1-Mbit/s value until Association succeeds.
            maximum_legacy_rate_500kbps: 2,
            ht: None,
            qos_supported: false,
            next_qos_sequences: [0; 8],
            tx_block_ack: new_ap_tx_block_ack(),
            power_state: ApPeerPowerState::Active,
            buffered_unicast_frames: 0,
            buffered_release_in_flight: false,
            last_activity_micros: now_micros,
            deadline_micros: now_micros.saturating_add(AP_ASSOCIATION_DEADLINE_MICROS),
        }
    }

    const fn association_identity(&self) -> ApAssociationIdentity {
        ApAssociationIdentity {
            address: self.address,
            association_id: self.association_id,
            association_epoch: self.association_epoch,
        }
    }
}

/// O(1) identity of one peer-table generation.
///
/// A slot may be reused after disconnect, so its index alone is not authority.
/// Every bound access validates both generation and address before exposing
/// peer state to a hot data-path operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ApPeerBinding {
    index: u8,
    generation: u32,
    address: [u8; 6],
}

impl ApPeerBinding {
    pub const fn address(self) -> [u8; 6] {
        self.address
    }
}

const fn new_ap_tx_block_ack() -> TxBlockAckSession {
    match TxBlockAckSession::new(TxBlockAckConfig {
        tid: AP_TX_BLOCK_ACK_TID,
        window: AP_TX_BLOCK_ACK_WINDOW,
        timeout_tu: 0,
        negotiation_timeout_us: AP_TX_BLOCK_ACK_NEGOTIATION_TIMEOUT_MICROS,
        // Baseline 3,839-byte A-MSDU construction and AP RX decapsulation are
        // both source-owned. The operational agreement still keeps this bit
        // false unless the peer echoes support in its ADDBA response.
        amsdu: true,
    }) {
        Ok(session) => session,
        Err(_) => panic!("valid AP TX BlockAck policy"),
    }
}

const fn new_ap_wpa2_retry() -> Wpa2Retry {
    match Wpa2Retry::new(Wpa2RetryConfig {
        first_interval_us: AP_WPA2_FIRST_RETRY_INTERVAL_MICROS,
        subsequent_interval_us: AP_WPA2_SUBSEQUENT_RETRY_INTERVAL_MICROS,
        attempts: AP_WPA2_RETRY_ATTEMPTS,
    }) {
        Ok(retry) => retry,
        Err(_) => panic!("valid AP WPA2 retry policy"),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ApPeerStatus {
    pub address: [u8; 6],
    pub association_id: u16,
    /// Non-reusable identity of this AID assignment. Reauthentication keeps
    /// the bounded AID slot but advances this epoch so receive-side duplicate
    /// history cannot cross association ownership.
    pub association_epoch: u32,
    pub phase: ApPeerPhase,
    pub maximum_legacy_rate_500kbps: u8,
    pub ht: Option<HtPeerCapabilities>,
    pub qos_supported: bool,
    pub tx_block_ack: Option<OperationalTxBlockAck>,
    pub power_state: ApPeerPowerState,
    pub buffered_unicast_frames: u16,
    pub buffered_release_in_flight: bool,
    pub last_activity_micros: u64,
    pub deadline_micros: u64,
}

impl ApPeerStatus {
    pub const fn association_identity(self) -> ApAssociationIdentity {
        ApAssociationIdentity {
            address: self.address,
            association_id: self.association_id,
            association_epoch: self.association_epoch,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AccessPointServiceStatus {
    pub security: WifiSecurityMode,
    pub client_limit: AccessPointClientLimit,
    pub associated: u8,
    pub authorized: u8,
    pub buffered_group_frames: u16,
    pub peers: [Option<ApPeerStatus>; AP_MAX_CLIENTS],
}

/// Complete portable owner for one AP service epoch.
///
/// Dropping this value clears the PMK and GTK through their zeroize-on-drop
/// implementations. A chip runtime still must clear its typed hardware slots
/// before it may classify the corresponding physical owner as stopped.
pub struct AccessPointService<'peers> {
    address: [u8; 6],
    security: AccessPointSecurityMaterial,
    peer_storage: Option<&'peers mut AccessPointPeerStorage>,
    client_limit: AccessPointClientLimit,
    inactive_timeout: AccessPointInactiveTimeout,
    next_management_sequence: u16,
    next_data_sequence: u16,
    status_revision: u32,
    associated_count: u8,
    authorized_count: u8,
    buffered_group_frames: u16,
    buffered_group_release_generation: u32,
    buffered_group_release_in_flight: bool,
    smallest_operational_tx_block_ack_window: Option<u16>,
}

/// Credential ownership for one AP epoch. Open deliberately has no PMK, GTK
/// or placeholder key bytes that could be installed by a later generic path.
pub enum AccessPointSecurityMaterial {
    Open,
    Wpa2Personal { pmk: Pmk, gtk: Wpa2Gtk },
}

impl<'peers> AccessPointService<'peers> {
    pub fn new(
        address: [u8; 6],
        pmk: Pmk,
        gtk: Wpa2Gtk,
        client_limit: AccessPointClientLimit,
        inactive_timeout: AccessPointInactiveTimeout,
        peer_storage: &'peers mut AccessPointPeerStorage,
    ) -> Self {
        peer_storage.peers.fill_with(|| None);
        peer_storage.generation = peer_storage
            .generation
            .checked_add(1)
            .expect("AP peer generation space is not reusable");
        Self {
            address,
            security: AccessPointSecurityMaterial::Wpa2Personal { pmk, gtk },
            peer_storage: Some(peer_storage),
            client_limit,
            inactive_timeout,
            next_management_sequence: 0,
            next_data_sequence: 0,
            status_revision: 0,
            associated_count: 0,
            authorized_count: 0,
            buffered_group_frames: 0,
            buffered_group_release_generation: 0,
            buffered_group_release_in_flight: false,
            smallest_operational_tx_block_ack_window: None,
        }
    }

    pub fn new_open(
        address: [u8; 6],
        client_limit: AccessPointClientLimit,
        inactive_timeout: AccessPointInactiveTimeout,
        peer_storage: &'peers mut AccessPointPeerStorage,
    ) -> Self {
        peer_storage.peers.fill_with(|| None);
        peer_storage.generation = peer_storage
            .generation
            .checked_add(1)
            .expect("AP peer generation space is not reusable");
        Self {
            address,
            security: AccessPointSecurityMaterial::Open,
            peer_storage: Some(peer_storage),
            client_limit,
            inactive_timeout,
            next_management_sequence: 0,
            next_data_sequence: 0,
            status_revision: 0,
            associated_count: 0,
            authorized_count: 0,
            buffered_group_frames: 0,
            buffered_group_release_generation: 0,
            buffered_group_release_in_flight: false,
            smallest_operational_tx_block_ack_window: None,
        }
    }

    pub const fn security_mode(&self) -> WifiSecurityMode {
        match &self.security {
            AccessPointSecurityMaterial::Open => WifiSecurityMode::Open,
            AccessPointSecurityMaterial::Wpa2Personal { .. } => WifiSecurityMode::Wpa2Personal,
        }
    }

    pub const fn address(&self) -> [u8; 6] {
        self.address
    }

    pub const fn client_limit(&self) -> AccessPointClientLimit {
        self.client_limit
    }

    pub fn peer_status(&self, address: [u8; 6]) -> Option<ApPeerStatus> {
        self.storage()
            .peers
            .iter()
            .flatten()
            .find(|peer| peer.address == address)
            .map(|peer| ApPeerStatus {
                address: peer.address,
                association_id: peer.association_id,
                association_epoch: peer.association_epoch,
                phase: peer.phase,
                maximum_legacy_rate_500kbps: peer.maximum_legacy_rate_500kbps,
                ht: peer.ht,
                qos_supported: peer.qos_supported,
                tx_block_ack: peer.tx_block_ack.operational(),
                power_state: peer.power_state,
                buffered_unicast_frames: peer.buffered_unicast_frames,
                buffered_release_in_flight: peer.buffered_release_in_flight,
                last_activity_micros: peer.last_activity_micros,
                deadline_micros: peer.deadline_micros,
            })
    }

    /// Resolve a peer address once at aggregate admission. Subsequent MPDUs
    /// use [`Self::bound_peer_status`] instead of rescanning the table.
    pub fn bind_peer(&self, address: [u8; 6]) -> Option<ApPeerBinding> {
        let index = self.peer_index(address)?;
        self.storage().peers[index].as_ref()?;
        Some(ApPeerBinding {
            index: u8::try_from(index).ok()?,
            generation: self.storage().generation,
            address,
        })
    }

    pub fn bound_peer_status(&self, binding: ApPeerBinding) -> Option<ApPeerStatus> {
        let peer = self.bound_peer(binding)?;
        Some(ApPeerStatus {
            address: peer.address,
            association_id: peer.association_id,
            association_epoch: peer.association_epoch,
            phase: peer.phase,
            maximum_legacy_rate_500kbps: peer.maximum_legacy_rate_500kbps,
            ht: peer.ht,
            qos_supported: peer.qos_supported,
            tx_block_ack: peer.tx_block_ack.operational(),
            power_state: peer.power_state,
            buffered_unicast_frames: peer.buffered_unicast_frames,
            buffered_release_in_flight: peer.buffered_release_in_flight,
            last_activity_micros: peer.last_activity_micros,
            deadline_micros: peer.deadline_micros,
        })
    }

    pub fn peers(&self) -> impl Iterator<Item = ApPeerStatus> + '_ {
        self.storage()
            .peers
            .iter()
            .flatten()
            .map(|peer| ApPeerStatus {
                address: peer.address,
                association_id: peer.association_id,
                association_epoch: peer.association_epoch,
                phase: peer.phase,
                maximum_legacy_rate_500kbps: peer.maximum_legacy_rate_500kbps,
                ht: peer.ht,
                qos_supported: peer.qos_supported,
                tx_block_ack: peer.tx_block_ack.operational(),
                power_state: peer.power_state,
                buffered_unicast_frames: peer.buffered_unicast_frames,
                buffered_release_in_flight: peer.buffered_release_in_flight,
                last_activity_micros: peer.last_activity_micros,
                deadline_micros: peer.deadline_micros,
            })
    }

    pub fn has_operational_tx_block_ack(&self) -> bool {
        self.smallest_operational_tx_block_ack_window().is_some()
    }

    /// Smallest currently operational downlink Block Ack window.
    ///
    /// A scheduler choosing a batch before it inspects the destination peer
    /// must not wait for more frames than any operational peer can admit. The
    /// per-peer agreement remains authoritative when the aggregate is built.
    pub fn smallest_operational_tx_block_ack_window(&self) -> Option<u16> {
        self.smallest_operational_tx_block_ack_window
    }

    pub const fn associated_count(&self) -> u8 {
        self.associated_count
    }

    pub const fn authorized_count(&self) -> u8 {
        self.authorized_count
    }

    #[inline(always)]
    pub fn is_authorized(&self, address: [u8; 6]) -> bool {
        self.storage()
            .peers
            .iter()
            .flatten()
            .any(|peer| peer.address == address && peer.phase == ApPeerPhase::Authorized)
    }

    pub fn status(&self) -> AccessPointServiceStatus {
        let mut peers = [None; AP_MAX_CLIENTS];
        for (destination, source) in peers.iter_mut().zip(self.storage().peers.iter()) {
            *destination = source.as_ref().map(|peer| ApPeerStatus {
                address: peer.address,
                association_id: peer.association_id,
                association_epoch: peer.association_epoch,
                phase: peer.phase,
                maximum_legacy_rate_500kbps: peer.maximum_legacy_rate_500kbps,
                ht: peer.ht,
                qos_supported: peer.qos_supported,
                tx_block_ack: peer.tx_block_ack.operational(),
                power_state: peer.power_state,
                buffered_unicast_frames: peer.buffered_unicast_frames,
                buffered_release_in_flight: peer.buffered_release_in_flight,
                last_activity_micros: peer.last_activity_micros,
                deadline_micros: peer.deadline_micros,
            });
        }
        AccessPointServiceStatus {
            security: self.security_mode(),
            client_limit: self.client_limit,
            associated: self.associated_count,
            authorized: self.authorized_count,
            buffered_group_frames: self.buffered_group_frames,
            peers,
        }
    }

    /// Monotonic public peer-table revision for cheap change detection.
    pub const fn status_revision(&self) -> u32 {
        self.status_revision
    }

    /// Refresh summaries on the rare peer-state mutation, keeping scheduler
    /// and link-state queries O(1) on the saturated data path.
    fn revise_status(&mut self) {
        let mut associated = 0_u8;
        let mut authorized = 0_u8;
        let mut smallest_window = None;
        for peer in self.storage().peers.iter().flatten() {
            if matches!(peer.phase, ApPeerPhase::Securing | ApPeerPhase::Authorized) {
                associated = associated.saturating_add(1);
            }
            if peer.phase == ApPeerPhase::Authorized {
                authorized = authorized.saturating_add(1);
            }
            if let Some(agreement) = peer.tx_block_ack.operational() {
                smallest_window = Some(smallest_window.map_or(agreement.window, |current: u16| {
                    current.min(agreement.window)
                }));
            }
        }
        self.associated_count = associated;
        self.authorized_count = authorized;
        self.smallest_operational_tx_block_ack_window = smallest_window;
        self.status_revision = self.status_revision.wrapping_add(1);
    }

    fn checked_peer(&self, peer: [u8; 6]) -> Result<&ApPeer, ApServiceError> {
        let index = self.peer_index(peer).ok_or(ApServiceError::UnknownPeer)?;
        self.storage().peers[index]
            .as_ref()
            .ok_or(ApServiceError::UnknownPeer)
    }

    fn checked_peer_mut(&mut self, peer: [u8; 6]) -> Result<&mut ApPeer, ApServiceError> {
        let index = self.peer_index(peer).ok_or(ApServiceError::UnknownPeer)?;
        self.storage_mut().peers[index]
            .as_mut()
            .ok_or(ApServiceError::UnknownPeer)
    }

    fn peer_index(&self, peer: [u8; 6]) -> Option<usize> {
        self.storage()
            .peers
            .iter()
            .position(|existing| existing.as_ref().is_some_and(|value| value.address == peer))
    }

    fn bound_association(&self, identity: ApAssociationIdentity) -> Option<&ApPeer> {
        let index = usize::from(identity.association_id.checked_sub(1)?);
        self.storage().peers.get(index)?.as_ref().filter(|peer| {
            peer.address == identity.address
                && peer.association_id == identity.association_id
                && peer.association_epoch == identity.association_epoch
        })
    }

    fn bound_association_mut(&mut self, identity: ApAssociationIdentity) -> Option<&mut ApPeer> {
        let index = usize::from(identity.association_id.checked_sub(1)?);
        self.storage_mut()
            .peers
            .get_mut(index)?
            .as_mut()
            .filter(|peer| {
                peer.address == identity.address
                    && peer.association_id == identity.association_id
                    && peer.association_epoch == identity.association_epoch
            })
    }

    /// Validate a retained association identity without scanning the peer
    /// table. This is the queue-generation fence used immediately before a
    /// caller releases old payload ownership into a new radio transaction.
    pub fn association_is_current(&self, identity: ApAssociationIdentity) -> bool {
        self.bound_association(identity)
            .is_some_and(|peer| peer.phase == ApPeerPhase::Authorized)
    }

    /// Return the current authorized state after the same O(1) identity
    /// validation when the caller also needs power-save or BA metadata.
    pub fn bound_authorized_peer_status(
        &self,
        identity: ApAssociationIdentity,
    ) -> Option<ApPeerStatus> {
        let peer = self.bound_association(identity)?;
        (peer.phase == ApPeerPhase::Authorized).then(|| ApPeerStatus {
            address: peer.address,
            association_id: peer.association_id,
            association_epoch: peer.association_epoch,
            phase: peer.phase,
            maximum_legacy_rate_500kbps: peer.maximum_legacy_rate_500kbps,
            ht: peer.ht,
            qos_supported: peer.qos_supported,
            tx_block_ack: peer.tx_block_ack.operational(),
            power_state: peer.power_state,
            buffered_unicast_frames: peer.buffered_unicast_frames,
            buffered_release_in_flight: peer.buffered_release_in_flight,
            last_activity_micros: peer.last_activity_micros,
            deadline_micros: peer.deadline_micros,
        })
    }

    fn bound_peer(&self, binding: ApPeerBinding) -> Option<&ApPeer> {
        if self.storage().generation != binding.generation {
            return None;
        }
        self.storage()
            .peers
            .get(usize::from(binding.index))?
            .as_ref()
            .filter(|peer| peer.address == binding.address)
    }

    fn bound_peer_mut(&mut self, binding: ApPeerBinding) -> Option<&mut ApPeer> {
        if self.storage().generation != binding.generation {
            return None;
        }
        self.storage_mut()
            .peers
            .get_mut(usize::from(binding.index))?
            .as_mut()
            .filter(|peer| peer.address == binding.address)
    }

    fn advance_peer_generation(&mut self) {
        let generation = self
            .storage()
            .generation
            .checked_add(1)
            .expect("AP peer generation space is not reusable");
        self.storage_mut().generation = generation;
    }

    fn occupied_count(&self) -> u8 {
        self.storage().peers.iter().flatten().count() as u8
    }

    /// End the service epoch, clear every per-peer secret and return the
    /// caller-owned table for a later AP materialization.
    pub fn into_peer_storage(mut self) -> &'peers mut AccessPointPeerStorage {
        let storage = self
            .peer_storage
            .take()
            .expect("an active AP service owns peer storage");
        storage.peers.fill_with(|| None);
        storage
    }

    fn storage(&self) -> &AccessPointPeerStorage {
        self.peer_storage
            .as_deref()
            .expect("an active AP service owns peer storage")
    }

    fn storage_mut(&mut self) -> &mut AccessPointPeerStorage {
        self.peer_storage
            .as_deref_mut()
            .expect("an active AP service owns peer storage")
    }
}

impl Drop for AccessPointService<'_> {
    fn drop(&mut self) {
        // Static placement must not retain pairwise protocol/key state into a
        // later AP epoch. Replacing every entry runs the WPA2/PTK destructors.
        if let Some(storage) = self.peer_storage.as_deref_mut() {
            storage.peers.fill_with(|| None);
        }
    }
}

#[cfg(test)]
mod tests;
