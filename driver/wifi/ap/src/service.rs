//! Bounded multi-peer AP MLME and security ownership.

use core::fmt;

use open_esp_radio_ieee80211::ap::ApPowerSaveObservation;
use open_esp_radio_ieee80211::beacon::WPA2_PERSONAL_CCMP_PSK_RSN_IE;
use open_esp_radio_ieee80211::block_ack::{
    AddbaRequest, BlockAckAction, OperationalTxBlockAck, TxBlockAckAlarm, TxBlockAckConfig,
    TxBlockAckError, TxBlockAckResponse, TxBlockAckSession,
};
use open_esp_radio_ieee80211::ht::HtPeerCapabilities;
use open_esp_radio_wpa2::{
    OwnedEapolFrame, Pmk, Ptk, PtkContext,
    aes::{SoftwareAesKeyWrapError, software_aes128_key_wrap},
    ap::validate_wpa2_ap_rsn,
    frames::{
        OwnedRsnIe, WPA2_PLAIN_KEY_DATA_CAPACITY, Wpa2FrameError, Wpa2Gtk, Wpa2PlainKeyData,
        Wpa2TxFrame, build_ap_action_frame,
    },
    retry::{Wpa2Retry, Wpa2RetryAction, Wpa2RetryAlarm, Wpa2RetryConfig, Wpa2RetryError},
    state::{
        PtkContext as Wpa2StatePtkContext, Wpa2ApAction, Wpa2ApPhase, Wpa2ApState, Wpa2StateError,
    },
};

/// Public encrypted-client ceiling for one AP epoch.
///
/// ESP32-S31 maps these clients to AIDs 1..=15 and hardware pairwise key
/// entries 8..=22. Higher values are rejected before radio ownership moves.
pub const AP_MAX_CLIENTS: usize = 15;
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
/// admission. Keeping the window at sixteen prevents one saturated downlink
/// TXOP from monopolizing the medium against the peer's concurrent uplink.
pub const AP_TX_BLOCK_ACK_WINDOW: u16 = 16;
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

/// Validated runtime admission limit for one AP epoch.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct AccessPointClientLimit(u8);

impl AccessPointClientLimit {
    pub const MAX: u8 = AP_MAX_CLIENTS as u8;

    pub const fn new(value: u8) -> Result<Self, AccessPointClientLimitError> {
        if value == 0 || value > Self::MAX {
            return Err(AccessPointClientLimitError { value });
        }
        Ok(Self(value))
    }

    pub const fn get(self) -> u8 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AccessPointClientLimitError {
    value: u8,
}

impl AccessPointClientLimitError {
    pub const fn value(self) -> u8 {
        self.value
    }
}

impl fmt::Display for AccessPointClientLimitError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "access-point client limit {} is outside 1..={}",
            self.value,
            AccessPointClientLimit::MAX,
        )
    }
}

impl core::error::Error for AccessPointClientLimitError {}

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
    peer: [u8; 6],
    association_id: u16,
    generation: u32,
    more_data: bool,
}

impl ApBufferedUnicastRelease {
    pub const fn peer(&self) -> [u8; 6] {
        self.peer
    }

    pub const fn association_id(&self) -> u16 {
        self.association_id
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
    phase: ApPeerPhase,
    wpa2: Option<Wpa2ApState>,
    pending_ptk: Option<Ptk>,
    wpa2_retry: Wpa2Retry,
    wpa2_retry_alarm: Option<Wpa2RetryAlarm>,
    maximum_legacy_rate_500kbps: u8,
    ht: Option<HtPeerCapabilities>,
    qos_supported: bool,
    tx_block_ack: TxBlockAckSession,
    power_state: ApPeerPowerState,
    buffered_unicast_frames: u16,
    buffered_release_in_flight: bool,
    buffered_release_generation: u32,
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
    const fn authenticated(address: [u8; 6], association_id: u16, now_micros: u64) -> Self {
        Self {
            address,
            association_id,
            phase: ApPeerPhase::Authenticated,
            wpa2: None,
            pending_ptk: None,
            wpa2_retry: new_ap_wpa2_retry(),
            wpa2_retry_alarm: None,
            // Authentication precedes rate negotiation. Keep the universally
            // compatible 1-Mbit/s value until Association succeeds.
            maximum_legacy_rate_500kbps: 2,
            ht: None,
            qos_supported: false,
            tx_block_ack: new_ap_tx_block_ack(),
            power_state: ApPeerPowerState::Active,
            buffered_unicast_frames: 0,
            buffered_release_in_flight: false,
            buffered_release_generation: 0,
            last_activity_micros: now_micros,
            deadline_micros: now_micros.saturating_add(AP_ASSOCIATION_DEADLINE_MICROS),
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
        amsdu: false,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AccessPointServiceStatus {
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
    pmk: Pmk,
    gtk: Wpa2Gtk,
    peer_storage: Option<&'peers mut AccessPointPeerStorage>,
    client_limit: AccessPointClientLimit,
    inactive_timeout: AccessPointInactiveTimeout,
    next_management_sequence: u16,
    next_data_sequence: u16,
    next_qos_sequences: [u16; 8],
    status_revision: u32,
    associated_count: u8,
    authorized_count: u8,
    buffered_group_frames: u16,
    buffered_group_release_generation: u32,
    buffered_group_release_in_flight: bool,
    smallest_operational_tx_block_ack_window: Option<u16>,
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
            pmk,
            gtk,
            peer_storage: Some(peer_storage),
            client_limit,
            inactive_timeout,
            next_management_sequence: 0,
            next_data_sequence: 0,
            next_qos_sequences: [0; 8],
            status_revision: 0,
            associated_count: 0,
            authorized_count: 0,
            buffered_group_frames: 0,
            buffered_group_release_generation: 0,
            buffered_group_release_in_flight: false,
            smallest_operational_tx_block_ack_window: None,
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

    /// Decide ownership of a newly arrived downlink unicast frame.
    ///
    /// The service never stores the frame itself. A `Buffer` result requires
    /// the caller to retain the frame first and only then call
    /// [`Self::commit_buffered_unicast`].
    pub fn downlink_disposition(
        &self,
        peer: [u8; 6],
    ) -> Result<ApDownlinkDisposition, ApServiceError> {
        let peer = self.checked_peer(peer)?;
        if peer.phase != ApPeerPhase::Authorized {
            return Err(ApServiceError::WrongPeerPhase);
        }
        Ok(match peer.power_state {
            ApPeerPowerState::Active => ApDownlinkDisposition::TransmitNow,
            ApPeerPowerState::Sleeping => ApDownlinkDisposition::Buffer,
        })
    }

    /// Commit one frame already retained by the caller's per-peer queue.
    pub fn commit_buffered_unicast(&mut self, peer: [u8; 6]) -> Result<u16, ApServiceError> {
        let buffered = {
            let peer = self.checked_peer_mut(peer)?;
            if peer.phase != ApPeerPhase::Authorized
                || peer.power_state != ApPeerPowerState::Sleeping
            {
                return Err(ApServiceError::WrongPeerPhase);
            }
            peer.buffered_unicast_frames = peer
                .buffered_unicast_frames
                .checked_add(1)
                .ok_or(ApServiceError::BufferedTrafficOverflow)?;
            peer.buffered_unicast_frames
        };
        self.status_revision = self.status_revision.wrapping_add(1);
        Ok(buffered)
    }

    /// Reserve one buffered unicast frame for an awake peer or a PS-Poll.
    ///
    /// Forgetting the returned token intentionally leaves the peer blocked:
    /// a second dequeue cannot overtake an unaccounted first frame.
    pub fn begin_buffered_unicast_release(
        &mut self,
        peer: [u8; 6],
    ) -> Result<Option<ApBufferedUnicastRelease>, ApServiceError> {
        let token = {
            let peer = self.checked_peer_mut(peer)?;
            if peer.phase != ApPeerPhase::Authorized {
                return Err(ApServiceError::WrongPeerPhase);
            }
            if peer.buffered_release_in_flight {
                return Err(ApServiceError::BufferedReleaseInFlight);
            }
            if peer.buffered_unicast_frames == 0 {
                return Ok(None);
            }
            peer.buffered_release_generation = peer
                .buffered_release_generation
                .checked_add(1)
                .ok_or(ApServiceError::BufferedTrafficOverflow)?;
            peer.buffered_release_in_flight = true;
            ApBufferedUnicastRelease {
                peer: peer.address,
                association_id: peer.association_id,
                generation: peer.buffered_release_generation,
                more_data: peer.buffered_unicast_frames > 1,
            }
        };
        self.status_revision = self.status_revision.wrapping_add(1);
        Ok(Some(token))
    }

    /// Resolve one reserved queue release after the caller either transmitted
    /// the exact retained frame or returned it to the same queue.
    pub fn complete_buffered_unicast_release(
        &mut self,
        release: ApBufferedUnicastRelease,
        delivered: bool,
    ) -> Result<u16, ApServiceError> {
        let remaining = {
            let peer = self.checked_peer_mut(release.peer)?;
            if peer.association_id != release.association_id {
                return Err(ApServiceError::AssociationIdMismatch);
            }
            if !peer.buffered_release_in_flight
                || peer.buffered_release_generation != release.generation
            {
                return Err(ApServiceError::StaleBufferedRelease);
            }
            if delivered {
                peer.buffered_unicast_frames = peer
                    .buffered_unicast_frames
                    .checked_sub(1)
                    .ok_or(ApServiceError::NoBufferedTraffic)?;
            }
            peer.buffered_release_in_flight = false;
            peer.buffered_unicast_frames
        };
        self.status_revision = self.status_revision.wrapping_add(1);
        Ok(remaining)
    }

    /// Apply one parsed peer PM edge after the caller has validated that the
    /// frame belongs to this AP. PS-Poll reserves, but does not consume, one
    /// caller-owned buffered frame.
    pub fn observe_power_save(
        &mut self,
        observation: ApPowerSaveObservation,
        now_micros: u64,
    ) -> Result<ApPowerSaveAction, ApServiceError> {
        let inactive_timeout_micros = self.inactive_timeout.micros();
        match observation {
            ApPowerSaveObservation::Sleeping { peer } | ApPowerSaveObservation::Active { peer } => {
                let requested = if matches!(observation, ApPowerSaveObservation::Sleeping { .. }) {
                    ApPeerPowerState::Sleeping
                } else {
                    ApPeerPowerState::Active
                };
                let (changed, buffered_frames) = {
                    let existing = self.checked_peer_mut(peer)?;
                    if existing.phase != ApPeerPhase::Authorized {
                        return Err(ApServiceError::WrongPeerPhase);
                    }
                    let changed = existing.power_state != requested;
                    existing.power_state = requested;
                    existing.last_activity_micros = now_micros;
                    existing.deadline_micros = now_micros.saturating_add(inactive_timeout_micros);
                    (changed, existing.buffered_unicast_frames)
                };
                if !changed {
                    return Ok(ApPowerSaveAction::None);
                }
                self.status_revision = self.status_revision.wrapping_add(1);
                Ok(ApPowerSaveAction::StateChanged {
                    peer,
                    state: requested,
                    buffered_frames,
                })
            }
            ApPowerSaveObservation::PsPoll {
                peer,
                association_id,
            } => {
                {
                    let existing = self.checked_peer_mut(peer)?;
                    if existing.phase != ApPeerPhase::Authorized
                        || existing.power_state != ApPeerPowerState::Sleeping
                    {
                        return Err(ApServiceError::WrongPeerPhase);
                    }
                    if existing.association_id != association_id {
                        return Err(ApServiceError::AssociationIdMismatch);
                    }
                    existing.last_activity_micros = now_micros;
                    existing.deadline_micros = now_micros.saturating_add(inactive_timeout_micros);
                }
                Ok(match self.begin_buffered_unicast_release(peer)? {
                    Some(release) => ApPowerSaveAction::ReleaseOne(release),
                    None => ApPowerSaveAction::None,
                })
            }
        }
    }

    /// Two-octet TIM virtual bitmap for the public AP AID range 1..=15.
    pub fn unicast_tim_bitmap(&self) -> u16 {
        self.storage()
            .peers
            .iter()
            .flatten()
            .filter(|peer| {
                peer.power_state == ApPeerPowerState::Sleeping
                    && peer.buffered_unicast_frames != 0
                    && (1..=AP_MAX_CLIENTS as u16).contains(&peer.association_id)
            })
            .fold(0_u16, |bitmap, peer| {
                bitmap | (1_u16 << peer.association_id)
            })
    }

    pub const fn buffered_group_frames(&self) -> u16 {
        self.buffered_group_frames
    }

    pub const fn group_traffic_pending(&self) -> bool {
        self.buffered_group_frames != 0
    }

    /// Decide ownership of a newly arrived multicast/broadcast frame.
    ///
    /// Group traffic is retained whenever at least one authorized station has
    /// announced PM=1. The caller must retain the payload first and call
    /// [`Self::commit_buffered_group`] only after that ownership transfer
    /// succeeds.
    pub fn group_downlink_disposition(&self) -> ApDownlinkDisposition {
        if self.storage().peers.iter().flatten().any(|peer| {
            peer.phase == ApPeerPhase::Authorized && peer.power_state == ApPeerPowerState::Sleeping
        }) {
            ApDownlinkDisposition::Buffer
        } else {
            ApDownlinkDisposition::TransmitNow
        }
    }

    /// Commit one multicast/broadcast frame already retained by the caller.
    pub fn commit_buffered_group(&mut self) -> Result<u16, ApServiceError> {
        self.buffered_group_frames = self
            .buffered_group_frames
            .checked_add(1)
            .ok_or(ApServiceError::BufferedTrafficOverflow)?;
        self.status_revision = self.status_revision.wrapping_add(1);
        Ok(self.buffered_group_frames)
    }

    /// Reserve the oldest caller-owned group frame after a successful DTIM
    /// beacon publication advertised group traffic.
    ///
    /// The DTIM publication edge is intentionally owned by the caller. This
    /// service cannot infer it from a timer or from the current TIM phase.
    pub fn begin_buffered_group_release(
        &mut self,
    ) -> Result<Option<ApBufferedGroupRelease>, ApServiceError> {
        if self.buffered_group_release_in_flight {
            return Err(ApServiceError::BufferedReleaseInFlight);
        }
        if self.buffered_group_frames == 0 {
            return Ok(None);
        }
        self.buffered_group_release_generation = self
            .buffered_group_release_generation
            .checked_add(1)
            .ok_or(ApServiceError::BufferedTrafficOverflow)?;
        self.buffered_group_release_in_flight = true;
        self.status_revision = self.status_revision.wrapping_add(1);
        Ok(Some(ApBufferedGroupRelease {
            generation: self.buffered_group_release_generation,
            more_data: self.buffered_group_frames > 1,
        }))
    }

    /// Resolve one affine group release after its exact retained payload
    /// reached terminal hardware publication or was restored to the queue.
    ///
    /// `delivered` means terminal publication success. Group-addressed MPDUs
    /// have no acknowledgement, so this API never manufactures ACK evidence.
    pub fn complete_buffered_group_release(
        &mut self,
        release: ApBufferedGroupRelease,
        delivered: bool,
    ) -> Result<u16, ApServiceError> {
        if !self.buffered_group_release_in_flight
            || release.generation != self.buffered_group_release_generation
        {
            return Err(ApServiceError::StaleBufferedRelease);
        }
        if delivered {
            self.buffered_group_frames = self
                .buffered_group_frames
                .checked_sub(1)
                .ok_or(ApServiceError::NoBufferedTraffic)?;
        }
        self.buffered_group_release_in_flight = false;
        self.status_revision = self.status_revision.wrapping_add(1);
        Ok(self.buffered_group_frames)
    }

    /// Account one group frame only after its DTIM-scoped publication has
    /// reached a terminal success. Failed frames remain advertised.
    pub fn complete_buffered_group(&mut self, delivered: bool) -> Result<u16, ApServiceError> {
        if self.buffered_group_release_in_flight {
            return Err(ApServiceError::BufferedReleaseInFlight);
        }
        if delivered {
            self.buffered_group_frames = self
                .buffered_group_frames
                .checked_sub(1)
                .ok_or(ApServiceError::NoBufferedTraffic)?;
            self.status_revision = self.status_revision.wrapping_add(1);
        }
        Ok(self.buffered_group_frames)
    }

    /// Clear the portable advertisement count at a caller-owned queue-drop
    /// boundary such as AP stop.
    ///
    /// The returned count tells the caller exactly how many retained payload
    /// owners it must drop. An in-flight affine release must be rolled back
    /// before this operation is legal.
    pub fn discard_buffered_groups(&mut self) -> Result<u16, ApServiceError> {
        if self.buffered_group_release_in_flight {
            return Err(ApServiceError::BufferedReleaseInFlight);
        }
        let discarded = self.buffered_group_frames;
        if discarded != 0 {
            self.buffered_group_frames = 0;
            self.status_revision = self.status_revision.wrapping_add(1);
        }
        Ok(discarded)
    }

    pub fn status(&self) -> AccessPointServiceStatus {
        let mut peers = [None; AP_MAX_CLIENTS];
        for (destination, source) in peers.iter_mut().zip(self.storage().peers.iter()) {
            *destination = source.as_ref().map(|peer| ApPeerStatus {
                address: peer.address,
                association_id: peer.association_id,
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

    pub fn next_management_sequence(&mut self) -> u16 {
        let sequence = self.next_management_sequence;
        self.next_management_sequence = (sequence + 1) & 0x0fff;
        sequence
    }

    /// Consume the non-QoS data sequence space used by the initial EAPOL and
    /// legacy data path. Per-TID sequence spaces are introduced with QoS.
    pub fn next_data_sequence(&mut self) -> u16 {
        let sequence = self.next_data_sequence;
        self.next_data_sequence = (sequence + 1) & 0x0fff;
        sequence
    }

    pub const fn current_data_sequence(&self) -> u16 {
        self.next_data_sequence
    }

    pub fn next_qos_sequence(&mut self, tid: u8) -> Option<u16> {
        let sequence = self.next_qos_sequences.get_mut(usize::from(tid))?;
        let current = *sequence;
        *sequence = (current + 1) & 0x0fff;
        Some(current)
    }

    pub fn current_qos_sequence(&self, tid: u8) -> Option<u16> {
        self.next_qos_sequences.get(usize::from(tid)).copied()
    }

    pub fn authenticate_open(&mut self, peer: [u8; 6], now_micros: u64) -> ApMlmeAction {
        let (status, changed) = if let Some(index) = self.peer_index(peer) {
            let association_id = self.storage().peers[index]
                .as_ref()
                .expect("peer index resolves an occupied entry")
                .association_id;
            self.advance_peer_generation();
            self.storage_mut().peers[index] =
                Some(ApPeer::authenticated(peer, association_id, now_micros));
            (AP_STATUS_SUCCESS, true)
        } else if self.occupied_count() >= self.client_limit.get() {
            (AP_STATUS_TOO_MANY_STATIONS, false)
        } else if let Some(index) = self.storage().peers.iter().position(Option::is_none) {
            let association_id = u16::try_from(index + 1).expect("fifteen AIDs fit u16");
            self.advance_peer_generation();
            self.storage_mut().peers[index] =
                Some(ApPeer::authenticated(peer, association_id, now_micros));
            (AP_STATUS_SUCCESS, true)
        } else {
            (AP_STATUS_TOO_MANY_STATIONS, false)
        };
        if changed {
            self.revise_status();
        }
        ApMlmeAction::AuthenticationResponse { peer, status }
    }

    pub fn associate_wpa2(
        &mut self,
        peer: [u8; 6],
        rsn_ie: &[u8],
        capabilities: ApAssociationCapabilities,
        authenticator_nonce: [u8; 32],
        initial_replay_counter: u64,
        now_micros: u64,
    ) -> Result<ApMlmeAction, ApServiceError> {
        let access_point = self.address;
        let inactive_timeout_micros = self.inactive_timeout.micros();
        let existing = self.checked_peer_mut(peer)?;
        if existing.phase != ApPeerPhase::Authenticated {
            return Err(ApServiceError::WrongPeerPhase);
        }
        if validate_wpa2_ap_rsn(rsn_ie).is_err() {
            return Ok(ApMlmeAction::AssociationResponse {
                peer,
                status: AP_STATUS_INVALID_RSN,
                association_id: None,
            });
        }
        if capabilities.maximum_legacy_rate_500kbps == 0 {
            return Ok(ApMlmeAction::AssociationResponse {
                peer,
                status: AP_STATUS_UNSUPPORTED_RATES,
                association_id: None,
            });
        }
        let wpa2 = Wpa2ApState::new(
            access_point,
            peer,
            authenticator_nonce,
            initial_replay_counter,
        )?;
        existing.phase = ApPeerPhase::Securing;
        existing.wpa2 = Some(wpa2);
        existing.maximum_legacy_rate_500kbps = capabilities.maximum_legacy_rate_500kbps;
        existing.ht = capabilities.ht;
        existing.qos_supported = capabilities.qos_supported;
        existing.last_activity_micros = now_micros;
        existing.deadline_micros = now_micros.saturating_add(inactive_timeout_micros);
        let association_id = existing.association_id;
        self.revise_status();
        Ok(ApMlmeAction::AssociationResponse {
            peer,
            status: AP_STATUS_SUCCESS,
            association_id: Some(association_id),
        })
    }

    /// Begin the AP-originated TID-0 TX BlockAck negotiation for one
    /// authorized HT peer. The agreement remains owned by that peer entry.
    pub fn begin_tx_block_ack(
        &mut self,
        peer: [u8; 6],
        now_micros: u64,
    ) -> Result<Option<AddbaRequest>, ApServiceError> {
        let starting_sequence = self
            .current_qos_sequence(AP_TX_BLOCK_ACK_TID)
            .expect("AP data TID is representable");
        let peer = self.checked_peer_mut(peer)?;
        if peer.phase != ApPeerPhase::Authorized || peer.ht.is_none() || !peer.qos_supported {
            return Ok(None);
        }
        if peer.tx_block_ack.operational().is_some() || peer.tx_block_ack.is_awaiting() {
            return Ok(None);
        }
        Ok(Some(
            peer.tx_block_ack.begin(starting_sequence, now_micros)?,
        ))
    }

    pub fn on_tx_block_ack_action(
        &mut self,
        peer: [u8; 6],
        action: BlockAckAction,
    ) -> Result<Option<TxBlockAckResponse>, ApServiceError> {
        let peer = self.checked_peer_mut(peer)?;
        match action {
            BlockAckAction::AddbaResponse { .. } => {
                let response = peer.tx_block_ack.on_response_action(action)?;
                self.revise_status();
                Ok(Some(response))
            }
            // This owner represents only AP-originated TX aggregation. A
            // peer-originated DELBA (`initiator = true`) terminates the
            // independent peer -> AP agreement and must not revoke our
            // AP -> peer session. The recipient clears this TX agreement
            // with `initiator = false`.
            BlockAckAction::Delba {
                tid,
                initiator: false,
                ..
            } if tid == AP_TX_BLOCK_ACK_TID => {
                peer.tx_block_ack.stop();
                self.revise_status();
                Ok(None)
            }
            _ => Ok(None),
        }
    }

    pub fn on_tx_block_ack_alarm(
        &mut self,
        peer: [u8; 6],
        alarm: TxBlockAckAlarm,
    ) -> Result<bool, ApServiceError> {
        let peer = self.checked_peer_mut(peer)?;
        Ok(peer.tx_block_ack.on_alarm(alarm))
    }

    /// Signal that the successful Association Response reached TX complete.
    pub fn begin_wpa2(&self, peer: [u8; 6]) -> Result<ApMlmeAction, ApServiceError> {
        let existing = self.checked_peer(peer)?;
        if existing.phase != ApPeerPhase::Securing {
            return Err(ApServiceError::WrongPeerPhase);
        }
        Ok(ApMlmeAction::BeginWpa2 { peer })
    }

    pub fn wpa2_mut(&mut self, peer: [u8; 6]) -> Result<&mut Wpa2ApState, ApServiceError> {
        let existing = self.checked_peer_mut(peer)?;
        existing.wpa2.as_mut().ok_or(ApServiceError::WrongPeerPhase)
    }

    pub fn wpa2_authorized(&self, peer: [u8; 6]) -> Result<bool, ApServiceError> {
        let existing = self.checked_peer(peer)?;
        Ok(existing.wpa2.as_ref().map(Wpa2ApState::phase) == Some(Wpa2ApPhase::Authorized))
    }

    pub fn derive_ptk(&self, context: PtkContext) -> Ptk {
        self.pmk.derive_ptk(context)
    }

    /// Build Message 1 only after the successful Association Response reached
    /// TX complete. The AP state retains the replay/nonce transaction.
    pub fn begin_wpa2_frame<const N: usize>(
        &self,
        peer: [u8; 6],
    ) -> Result<Wpa2TxFrame<N>, ApWpa2Error> {
        let existing = self.checked_peer(peer)?;
        let state = existing
            .wpa2
            .as_ref()
            .ok_or(ApServiceError::WrongPeerPhase)?;
        let Wpa2ApAction::Transmit(transmit) = state.message1(false)? else {
            return Err(ApWpa2Error::UnexpectedAction);
        };
        Ok(build_ap_action_frame(state, transmit, [0; 8], &[])?)
    }

    /// Bind a terminal EAPOL-Key TX completion to the generic finite retry
    /// owner. A new handshake message replaces the previous response window;
    /// completion of a retransmission keeps the alarm already advanced by the
    /// timer edge that produced it.
    pub fn observe_wpa2_transmit(
        &mut self,
        peer: [u8; 6],
        retransmission: bool,
        acknowledged: bool,
        now_micros: u64,
    ) -> Result<bool, ApWpa2Error> {
        let inactive_timeout_micros = self.inactive_timeout.micros();
        let transmit = self
            .checked_peer(peer)?
            .wpa2
            .as_ref()
            .ok_or(ApServiceError::WrongPeerPhase)?
            .retry_transmit()?;
        let existing = self.checked_peer_mut(peer)?;
        let stage_changed = existing.wpa2_retry.pending_message() != Some(transmit.message);
        let armed = stage_changed || !retransmission;
        if armed {
            existing.wpa2_retry.cancel();
            let mut alarm = existing.wpa2_retry.arm(transmit, now_micros)?;
            // hostapd extends only the acknowledged initial M1 window. M3
            // retains the short first timeout, then uses the subsequent one.
            if acknowledged
                && transmit.message == open_esp_radio_wpa2::state::Wpa2TxMessage::PairwiseMessage1
            {
                alarm = existing
                    .wpa2_retry
                    .defer_first_after_ack(now_micros)?
                    .expect("freshly armed WPA2 retry has a first window");
            }
            existing.wpa2_retry_alarm = Some(alarm);
        }
        existing.last_activity_micros = now_micros;
        existing.deadline_micros = now_micros.saturating_add(inactive_timeout_micros);
        Ok(armed)
    }

    pub fn next_wpa2_retry_deadline(&self) -> Option<u64> {
        self.storage()
            .peers
            .iter()
            .flatten()
            .filter_map(|peer| peer.wpa2_retry_alarm.map(|alarm| alarm.deadline_us))
            .min()
    }

    /// Consume at most one due authenticator retry edge.
    pub fn take_due_wpa2_retry<const N: usize>(
        &mut self,
        now_micros: u64,
    ) -> Result<ApWpa2RetryProgress<N>, ApWpa2Error> {
        let Some(index) = self.storage().peers.iter().position(|peer| {
            peer.as_ref()
                .and_then(|peer| peer.wpa2_retry_alarm)
                .is_some_and(|alarm| alarm.deadline_us <= now_micros)
        }) else {
            return Ok(ApWpa2RetryProgress::None);
        };
        let (peer_address, action) = {
            let peer = self.storage_mut().peers[index]
                .as_mut()
                .expect("due WPA2 retry belongs to an occupied peer");
            let alarm = peer
                .wpa2_retry_alarm
                .take()
                .expect("due WPA2 retry retains its alarm");
            let action = peer.wpa2_retry.on_alarm(alarm, now_micros)?;
            (peer.address, action)
        };
        match action {
            Wpa2RetryAction::Stale => Ok(ApWpa2RetryProgress::None),
            Wpa2RetryAction::Transmit { frame, next_alarm } => {
                self.checked_peer_mut(peer_address)?.wpa2_retry_alarm = Some(next_alarm);
                let frame = match frame.message {
                    open_esp_radio_wpa2::state::Wpa2TxMessage::PairwiseMessage1 => {
                        let state = self
                            .checked_peer(peer_address)?
                            .wpa2
                            .as_ref()
                            .ok_or(ApServiceError::WrongPeerPhase)?;
                        build_ap_action_frame(state, frame, [0; 8], &[])?
                    }
                    open_esp_radio_wpa2::state::Wpa2TxMessage::PairwiseMessage3 => {
                        let ApWpa2Progress::Transmit(frame) =
                            self.build_pending_transmit(peer_address, frame)?
                        else {
                            return Err(ApWpa2Error::UnexpectedAction);
                        };
                        frame
                    }
                    _ => return Err(ApWpa2Error::UnexpectedAction),
                };
                Ok(ApWpa2RetryProgress::Transmit {
                    peer: peer_address,
                    frame,
                })
            }
            Wpa2RetryAction::Exhausted => {
                let peer = self.checked_peer_mut(peer_address)?;
                let close = ApPeerClose {
                    peer: peer.address,
                    kind: ApPeerCloseKind::Wpa2HandshakeTimeout,
                    was_associated: true,
                    maximum_legacy_rate_500kbps: peer.maximum_legacy_rate_500kbps,
                };
                peer.phase = ApPeerPhase::Closing;
                self.revise_status();
                Ok(ApWpa2RetryProgress::Close(close))
            }
        }
    }

    /// Advance the bounded authenticator state through Message 2 or Message 4.
    ///
    /// PTK derivation, MIC verification and GTK wrapping are pure bounded
    /// operations here. Hardware key installation remains an explicit later
    /// edge in the chip AP engine.
    pub fn on_eapol<const N: usize>(
        &mut self,
        peer: [u8; 6],
        frame: OwnedEapolFrame<N>,
    ) -> Result<ApWpa2Progress<N>, ApWpa2Error> {
        let action = self
            .checked_peer_mut(peer)?
            .wpa2
            .as_mut()
            .ok_or(ApServiceError::WrongPeerPhase)?
            .on_frame(frame)?;
        match action {
            Wpa2ApAction::None => Ok(ApWpa2Progress::None),
            Wpa2ApAction::DerivePtk {
                ticket,
                context,
                message2,
            } => self.complete_message2(peer, ticket, context, message2),
            Wpa2ApAction::VerifyMessage4Mic { ticket, message4 } => {
                let valid = {
                    let ptk = self
                        .checked_peer(peer)?
                        .pending_ptk
                        .as_ref()
                        .ok_or(ApWpa2Error::MissingPairwiseKey)?;
                    message4.key_frame().verify_mic(ptk)
                };
                let action = self
                    .checked_peer_mut(peer)?
                    .wpa2
                    .as_mut()
                    .ok_or(ApServiceError::WrongPeerPhase)?
                    .complete_message4_mic(ticket, message4, valid)?;
                match action {
                    Wpa2ApAction::AuthorizePeer => {
                        let existing = self.checked_peer_mut(peer)?;
                        existing.wpa2_retry.cancel();
                        existing.wpa2_retry_alarm = None;
                        Ok(ApWpa2Progress::AuthorizePeer)
                    }
                    Wpa2ApAction::DeauthenticatePeer => Ok(ApWpa2Progress::DeauthenticatePeer),
                    _ => Err(ApWpa2Error::UnexpectedAction),
                }
            }
            Wpa2ApAction::Transmit(transmit) => self.build_pending_transmit(peer, transmit),
            Wpa2ApAction::DeauthenticatePeer => Ok(ApWpa2Progress::DeauthenticatePeer),
            _ => Err(ApWpa2Error::UnexpectedAction),
        }
    }

    fn complete_message2<const N: usize>(
        &mut self,
        peer: [u8; 6],
        ticket: open_esp_radio_wpa2::state::Wpa2Ticket,
        context: Wpa2StatePtkContext,
        message2: OwnedEapolFrame<N>,
    ) -> Result<ApWpa2Progress<N>, ApWpa2Error> {
        let ptk = self.pmk.derive_ptk(PtkContext {
            authenticator_address: context.authenticator_address,
            supplicant_address: context.supplicant_address,
            authenticator_nonce: context.authenticator_nonce,
            supplicant_nonce: context.supplicant_nonce,
        });
        let action = self
            .checked_peer_mut(peer)?
            .wpa2
            .as_mut()
            .ok_or(ApServiceError::WrongPeerPhase)?
            .complete_ptk(ticket, message2, true)?;
        let Wpa2ApAction::VerifyMessage2Mic { ticket, message2 } = action else {
            return Err(ApWpa2Error::UnexpectedAction);
        };
        let valid = message2.key_frame().verify_mic(&ptk);
        let action = self
            .checked_peer_mut(peer)?
            .wpa2
            .as_mut()
            .ok_or(ApServiceError::WrongPeerPhase)?
            .complete_message2_mic(ticket, message2, valid)?;
        let ticket = match action {
            Wpa2ApAction::PrepareMessage3 { ticket } => ticket,
            Wpa2ApAction::DeauthenticatePeer => {
                return Ok(ApWpa2Progress::DeauthenticatePeer);
            }
            _ => return Err(ApWpa2Error::UnexpectedAction),
        };

        let authenticator_rsn = OwnedRsnIe::<64>::try_copy(&WPA2_PERSONAL_CCMP_PSK_RSN_IE)?;
        let plain =
            Wpa2PlainKeyData::<WPA2_PLAIN_KEY_DATA_CAPACITY>::build(&authenticator_rsn, &self.gtk)?;
        let wrapped = software_aes128_key_wrap(ptk.kek(), plain.as_bytes())?;
        let action = self
            .checked_peer_mut(peer)?
            .wpa2
            .as_mut()
            .ok_or(ApServiceError::WrongPeerPhase)?
            .complete_message3_preparation::<N>(ticket, true)?;
        let Wpa2ApAction::Transmit(transmit) = action else {
            return Err(ApWpa2Error::UnexpectedAction);
        };
        let state = self
            .checked_peer(peer)?
            .wpa2
            .as_ref()
            .ok_or(ApServiceError::WrongPeerPhase)?;
        let response =
            build_ap_action_frame(state, transmit, [0; 8], wrapped.as_bytes())?.authenticate(&ptk);
        let existing = self.checked_peer_mut(peer)?;
        existing.pending_ptk = Some(ptk);
        // Valid M2 closes the Message-1 response window. Message 3 receives a
        // fresh schedule only after its own terminal TX completion.
        existing.wpa2_retry.cancel();
        existing.wpa2_retry_alarm = None;
        Ok(ApWpa2Progress::Transmit(response))
    }

    fn build_pending_transmit<const N: usize>(
        &self,
        peer: [u8; 6],
        transmit: open_esp_radio_wpa2::state::Wpa2Transmit,
    ) -> Result<ApWpa2Progress<N>, ApWpa2Error> {
        let existing = self.checked_peer(peer)?;
        let state = existing
            .wpa2
            .as_ref()
            .ok_or(ApServiceError::WrongPeerPhase)?;
        let ptk = existing
            .pending_ptk
            .as_ref()
            .ok_or(ApWpa2Error::MissingPairwiseKey)?;
        let authenticator_rsn = OwnedRsnIe::<64>::try_copy(&WPA2_PERSONAL_CCMP_PSK_RSN_IE)?;
        let plain =
            Wpa2PlainKeyData::<WPA2_PLAIN_KEY_DATA_CAPACITY>::build(&authenticator_rsn, &self.gtk)?;
        let wrapped = software_aes128_key_wrap(ptk.kek(), plain.as_bytes())?;
        let response =
            build_ap_action_frame(state, transmit, [0; 8], wrapped.as_bytes())?.authenticate(ptk);
        Ok(ApWpa2Progress::Transmit(response))
    }

    pub fn pending_ptk(&self, peer: [u8; 6]) -> Result<&Ptk, ApServiceError> {
        self.checked_peer(peer)?
            .pending_ptk
            .as_ref()
            .ok_or(ApServiceError::WrongPeerPhase)
    }

    pub const fn gtk(&self) -> &Wpa2Gtk {
        &self.gtk
    }

    pub fn authorize(&mut self, peer: [u8; 6], now_micros: u64) -> Result<(), ApServiceError> {
        let inactive_timeout_micros = self.inactive_timeout.micros();
        let existing = self.checked_peer_mut(peer)?;
        if existing.wpa2.as_ref().map(Wpa2ApState::phase) != Some(Wpa2ApPhase::Authorized) {
            return Err(ApServiceError::WrongPeerPhase);
        }
        existing.phase = ApPeerPhase::Authorized;
        existing.pending_ptk = None;
        existing.wpa2_retry.cancel();
        existing.wpa2_retry_alarm = None;
        existing.last_activity_micros = now_micros;
        existing.deadline_micros = now_micros.saturating_add(inactive_timeout_micros);
        self.revise_status();
        Ok(())
    }

    pub fn observe_activity(
        &mut self,
        peer: [u8; 6],
        now_micros: u64,
    ) -> Result<(), ApServiceError> {
        let inactive_timeout_micros = self.inactive_timeout.micros();
        let existing = self.checked_peer_mut(peer)?;
        if !matches!(
            existing.phase,
            ApPeerPhase::Securing | ApPeerPhase::Authorized
        ) {
            return Err(ApServiceError::WrongPeerPhase);
        }
        existing.last_activity_micros = now_micros;
        existing.deadline_micros = now_micros.saturating_add(inactive_timeout_micros);
        Ok(())
    }

    pub fn next_peer_deadline(&self) -> Option<u64> {
        self.storage()
            .peers
            .iter()
            .flatten()
            .filter(|peer| peer.phase != ApPeerPhase::Closing)
            .map(|peer| peer.deadline_micros)
            .min()
    }

    pub fn begin_due_peer_close(&mut self, now_micros: u64) -> Option<ApPeerClose> {
        let index = self.storage().peers.iter().position(|peer| {
            peer.as_ref().is_some_and(|peer| {
                peer.phase != ApPeerPhase::Closing && peer.deadline_micros <= now_micros
            })
        })?;
        let peer = self.storage_mut().peers[index].as_mut()?;
        let was_associated = matches!(peer.phase, ApPeerPhase::Securing | ApPeerPhase::Authorized);
        let close = ApPeerClose {
            peer: peer.address,
            kind: if was_associated {
                ApPeerCloseKind::InactivityTimeout
            } else {
                ApPeerCloseKind::AuthenticationTimeout
            },
            was_associated,
            maximum_legacy_rate_500kbps: peer.maximum_legacy_rate_500kbps,
        };
        peer.phase = ApPeerPhase::Closing;
        self.revise_status();
        Some(close)
    }

    pub fn begin_wpa2_failure_close(
        &mut self,
        peer_address: [u8; 6],
    ) -> Result<ApPeerClose, ApServiceError> {
        let peer = self.checked_peer_mut(peer_address)?;
        if peer.phase != ApPeerPhase::Securing {
            return Err(ApServiceError::WrongPeerPhase);
        }
        peer.wpa2_retry.cancel();
        peer.wpa2_retry_alarm = None;
        let close = ApPeerClose {
            peer: peer.address,
            kind: ApPeerCloseKind::Wpa2HandshakeFailure,
            was_associated: true,
            maximum_legacy_rate_500kbps: peer.maximum_legacy_rate_500kbps,
        };
        peer.phase = ApPeerPhase::Closing;
        self.revise_status();
        Ok(close)
    }

    pub fn begin_stop_peer(&mut self) -> Option<ApPeerClose> {
        let index = self.storage().peers.iter().position(|peer| {
            peer.as_ref()
                .is_some_and(|peer| peer.phase != ApPeerPhase::Closing)
        })?;
        let peer = self.storage_mut().peers[index].as_mut()?;
        let was_associated = matches!(peer.phase, ApPeerPhase::Securing | ApPeerPhase::Authorized);
        let close = ApPeerClose {
            peer: peer.address,
            kind: ApPeerCloseKind::AccessPointStop,
            was_associated,
            maximum_legacy_rate_500kbps: peer.maximum_legacy_rate_500kbps,
        };
        peer.phase = ApPeerPhase::Closing;
        self.revise_status();
        Some(close)
    }

    pub fn remove_peer(&mut self, peer: [u8; 6]) -> Result<ApMlmeAction, ApServiceError> {
        let index = self.peer_index(peer).ok_or(ApServiceError::UnknownPeer)?;
        self.storage_mut().peers[index] = None;
        self.advance_peer_generation();
        self.revise_status();
        Ok(ApMlmeAction::PeerRemoved { peer })
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
mod tests {
    use super::*;
    use open_esp_radio_ieee80211::{
        beacon::WPA2_PERSONAL_CCMP_PSK_RSN_IE,
        channel::WifiChannel,
        ht::{ht_capability_ie, ht_peer_capabilities},
    };
    use open_esp_radio_wpa2::{
        EapolKeyMessage, OwnedEapolFrame, PtkContext, Wpa2Interface,
        aes::software_aes128_key_unwrap,
        frames::{OwnedRsnIe, Wpa2Gtk, Wpa2TxFrame, parse_gtk_key_data},
        state::{Wpa2ApAction, Wpa2Ticket},
    };

    const AP: [u8; 6] = [0x02, 0, 0, 0, 0, 1];
    const PEER: [u8; 6] = [0x02, 0, 0, 0, 0, 2];
    const OTHER: [u8; 6] = [0x02, 0, 0, 0, 0, 3];
    const WPA2_RSN: [u8; 22] = [
        0x30, 20, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 2, 0, 0,
    ];
    const SUPPLICANT_RSN: [u8; 22] = [
        0x30, 20, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 2, 0x0c, 0,
    ];
    fn ht_capabilities() -> ApAssociationCapabilities {
        ApAssociationCapabilities {
            maximum_legacy_rate_500kbps: 108,
            ht: ht_peer_capabilities(&ht_capability_ie(WifiChannel::mhz20(6).unwrap())),
            qos_supported: true,
        }
    }

    const LEGACY_CAPABILITIES: ApAssociationCapabilities = ApAssociationCapabilities {
        maximum_legacy_rate_500kbps: 108,
        ht: None,
        qos_supported: false,
    };

    fn service(storage: &mut AccessPointPeerStorage) -> AccessPointService<'_> {
        AccessPointService::new(
            AP,
            Pmk::derive(b"password", b"test-ap").unwrap(),
            Wpa2Gtk::new(1, true, [0x55; 16]).unwrap(),
            AccessPointClientLimit::new(2).unwrap(),
            AccessPointInactiveTimeout::default(),
            storage,
        )
    }

    #[test]
    fn runtime_limit_is_enforced_before_hardware_moves() {
        let mut storage = AccessPointPeerStorage::new();
        let mut service = service(&mut storage);
        assert_eq!(
            service.authenticate_open(PEER, 0),
            ApMlmeAction::AuthenticationResponse {
                peer: PEER,
                status: AP_STATUS_SUCCESS,
            }
        );
        assert_eq!(
            service.authenticate_open(OTHER, 0),
            ApMlmeAction::AuthenticationResponse {
                peer: OTHER,
                status: AP_STATUS_SUCCESS,
            }
        );
        let third = [0x02, 0, 0, 0, 0, 4];
        assert_eq!(
            service.authenticate_open(third, 0),
            ApMlmeAction::AuthenticationResponse {
                peer: third,
                status: AP_STATUS_TOO_MANY_STATIONS,
            }
        );
        assert_eq!(service.associated_count(), 0);
        assert_eq!(service.peers().count(), 2);
        assert_eq!(service.peer_status(PEER).unwrap().association_id, 1);
        assert_eq!(service.peer_status(OTHER).unwrap().association_id, 2);
    }

    #[test]
    fn peer_binding_rejects_a_reused_slot_generation() {
        let mut storage = AccessPointPeerStorage::new();
        let mut service = service(&mut storage);
        service.authenticate_open(PEER, 0);
        let first = service.bind_peer(PEER).unwrap();
        assert_eq!(service.bound_peer_status(first).unwrap().address, PEER);

        service.remove_peer(PEER).unwrap();
        service.authenticate_open(OTHER, 1);
        let second = service.bind_peer(OTHER).unwrap();
        assert_eq!(service.bound_peer_status(first), None);
        assert_eq!(service.bound_peer_status(second).unwrap().address, OTHER);
        assert_ne!(first, second);
    }

    #[test]
    fn client_limit_rejects_zero_and_values_above_the_owned_tables() {
        assert_eq!(AccessPointClientLimit::new(0).unwrap_err().value(), 0,);
        assert_eq!(AccessPointClientLimit::new(16).unwrap_err().value(), 16,);
        assert_eq!(AccessPointClientLimit::new(15).unwrap().get(), 15);
    }

    #[test]
    fn inactivity_timeout_is_bounded_and_defaults_to_vendor_policy() {
        assert_eq!(AccessPointInactiveTimeout::default().seconds(), 300);
        assert_eq!(AccessPointInactiveTimeout::new(9).unwrap_err().seconds(), 9);
        assert_eq!(AccessPointInactiveTimeout::new(10).unwrap().seconds(), 10);
        assert_eq!(
            AccessPointInactiveTimeout::new(3_600).unwrap().seconds(),
            3_600
        );
        assert_eq!(
            AccessPointInactiveTimeout::new(3_601)
                .unwrap_err()
                .seconds(),
            3_601
        );
    }

    #[test]
    fn authenticated_peer_expires_at_the_recovered_fifteen_second_frontier() {
        let mut storage = AccessPointPeerStorage::new();
        let mut service = service(&mut storage);
        service.authenticate_open(PEER, 1_000);
        assert_eq!(service.next_peer_deadline(), Some(15_001_000));
        assert_eq!(service.begin_due_peer_close(15_000_999), None);
        assert_eq!(
            service.begin_due_peer_close(15_001_000),
            Some(ApPeerClose {
                peer: PEER,
                kind: ApPeerCloseKind::AuthenticationTimeout,
                was_associated: false,
                maximum_legacy_rate_500kbps: 2,
            })
        );
        assert_eq!(
            service.peer_status(PEER).unwrap().phase,
            ApPeerPhase::Closing
        );
        assert_eq!(service.associated_count(), 0);
    }

    #[test]
    fn associated_activity_refreshes_the_configured_inactivity_frontier() {
        let mut storage = AccessPointPeerStorage::new();
        let mut service = AccessPointService::new(
            AP,
            Pmk::derive(b"password", b"test-ap").unwrap(),
            Wpa2Gtk::new(1, true, [0x55; 16]).unwrap(),
            AccessPointClientLimit::new(2).unwrap(),
            AccessPointInactiveTimeout::new(10).unwrap(),
            &mut storage,
        );
        service.authenticate_open(PEER, 0);
        service
            .associate_wpa2(PEER, &WPA2_RSN, ht_capabilities(), [7; 32], 9, 2_000)
            .unwrap();
        assert_eq!(service.next_peer_deadline(), Some(10_002_000));
        service.observe_activity(PEER, 5_000_000).unwrap();
        assert_eq!(service.next_peer_deadline(), Some(15_000_000));
        assert_eq!(service.begin_due_peer_close(14_999_999), None);
        assert_eq!(
            service.begin_due_peer_close(15_000_000),
            Some(ApPeerClose {
                peer: PEER,
                kind: ApPeerCloseKind::InactivityTimeout,
                was_associated: true,
                maximum_legacy_rate_500kbps: 108,
            })
        );
        assert_eq!(service.associated_count(), 0);
    }

    #[test]
    fn association_owns_a_bounded_wpa2_state() {
        let mut storage = AccessPointPeerStorage::new();
        let mut service = service(&mut storage);
        service.authenticate_open(PEER, 0);
        assert_eq!(
            service
                .associate_wpa2(PEER, &WPA2_RSN, ht_capabilities(), [7; 32], 9, 1)
                .unwrap(),
            ApMlmeAction::AssociationResponse {
                peer: PEER,
                status: AP_STATUS_SUCCESS,
                association_id: Some(1),
            }
        );
        assert_eq!(
            service.peer_status(PEER).unwrap().phase,
            ApPeerPhase::Securing
        );
        assert_eq!(
            service
                .peer_status(PEER)
                .unwrap()
                .maximum_legacy_rate_500kbps,
            108
        );
        assert_eq!(service.associated_count(), 1);
        assert_eq!(
            service.begin_wpa2(PEER).unwrap(),
            ApMlmeAction::BeginWpa2 { peer: PEER }
        );
        assert!(matches!(
            service.wpa2_mut(PEER).unwrap().message1(false).unwrap(),
            Wpa2ApAction::Transmit(_)
        ));
        let _ticket_type_is_owned: Option<Wpa2Ticket> = None;
    }

    #[test]
    fn association_rejects_a_peer_without_a_common_legacy_rate() {
        let mut storage = AccessPointPeerStorage::new();
        let mut service = service(&mut storage);
        service.authenticate_open(PEER, 0);
        assert_eq!(
            service
                .associate_wpa2(
                    PEER,
                    &WPA2_RSN,
                    ApAssociationCapabilities {
                        maximum_legacy_rate_500kbps: 0,
                        ht: None,
                        qos_supported: false,
                    },
                    [7; 32],
                    9,
                    1,
                )
                .unwrap(),
            ApMlmeAction::AssociationResponse {
                peer: PEER,
                status: AP_STATUS_UNSUPPORTED_RATES,
                association_id: None,
            }
        );
        assert_eq!(
            service.peer_status(PEER).unwrap().phase,
            ApPeerPhase::Authenticated
        );
    }

    #[test]
    fn complete_four_way_handshake_retains_ptk_until_hardware_authorization() {
        const ANONCE: [u8; 32] = [7; 32];
        const SNONCE: [u8; 32] = [8; 32];

        let mut storage = AccessPointPeerStorage::new();
        let mut service = service(&mut storage);
        service.authenticate_open(PEER, 0);
        // Supplicants may add their own RSN capabilities. Message 3 must not
        // reflect those bytes back: it authenticates the AP's beacon RSN IE.
        service
            .associate_wpa2(PEER, &SUPPLICANT_RSN, ht_capabilities(), ANONCE, 9, 1)
            .unwrap();
        let message1 = service.begin_wpa2_frame::<512>(PEER).unwrap();
        assert_eq!(
            message1.key_frame().message(),
            EapolKeyMessage::PairwiseMessage1
        );
        assert!(!message1.retransmission());
        service
            .observe_wpa2_transmit(PEER, false, true, 10)
            .unwrap();
        assert_eq!(service.next_wpa2_retry_deadline(), Some(1_000_010));
        let ApWpa2RetryProgress::Transmit {
            peer: retried_peer,
            frame: retried_message1,
        } = service.take_due_wpa2_retry::<512>(1_000_010).unwrap()
        else {
            panic!("Message 1 response timeout must retransmit")
        };
        assert_eq!(retried_peer, PEER);
        assert!(retried_message1.retransmission());
        assert_eq!(retried_message1.as_bytes(), message1.as_bytes());

        let ptk = Pmk::derive(b"password", b"test-ap")
            .unwrap()
            .derive_ptk(PtkContext {
                authenticator_address: AP,
                supplicant_address: PEER,
                authenticator_nonce: ANONCE,
                supplicant_nonce: SNONCE,
            });
        let rsn = OwnedRsnIe::<64>::try_copy(&SUPPLICANT_RSN).unwrap();
        let message2 = Wpa2TxFrame::<512>::message2(AP, 9, SNONCE, &rsn)
            .unwrap()
            .authenticate(&ptk);
        let message2 =
            OwnedEapolFrame::<512>::try_copy(Wpa2Interface::AccessPoint, PEER, message2.as_bytes())
                .unwrap();
        let ApWpa2Progress::Transmit(message3) = service.on_eapol(PEER, message2).unwrap() else {
            panic!("message 2 must produce message 3");
        };
        assert_eq!(
            message3.key_frame().message(),
            EapolKeyMessage::PairwiseMessage3
        );
        assert!(message3.key_frame().verify_mic(&ptk));
        let plaintext = software_aes128_key_unwrap(ptk.kek(), message3.key_frame().key_data())
            .expect("AP wrapped its Message 3 key data");
        assert!(
            parse_gtk_key_data(plaintext.as_bytes(), &WPA2_PERSONAL_CCMP_PSK_RSN_IE, &[],).is_ok()
        );
        assert_eq!(service.next_wpa2_retry_deadline(), None);
        service
            .observe_wpa2_transmit(PEER, false, true, 2_000_000)
            .unwrap();
        assert_eq!(service.next_wpa2_retry_deadline(), Some(2_100_000));
        let ApWpa2RetryProgress::Transmit {
            peer: retried_peer,
            frame: retried_message3,
        } = service.take_due_wpa2_retry::<512>(2_100_000).unwrap()
        else {
            panic!("Message 3 response timeout must retransmit")
        };
        assert_eq!(retried_peer, PEER);
        assert!(retried_message3.retransmission());
        assert_eq!(retried_message3.as_bytes(), message3.as_bytes());
        assert!(matches!(
            parse_gtk_key_data(plaintext.as_bytes(), &SUPPLICANT_RSN, &[]),
            Err(Wpa2FrameError::RsnIeMismatch)
        ));

        let message4 = Wpa2TxFrame::<512>::message4(AP, 10)
            .unwrap()
            .authenticate(&ptk);
        let message4 =
            OwnedEapolFrame::<512>::try_copy(Wpa2Interface::AccessPoint, PEER, message4.as_bytes())
                .unwrap();
        assert!(matches!(
            service.on_eapol(PEER, message4).unwrap(),
            ApWpa2Progress::AuthorizePeer
        ));
        assert_eq!(service.next_wpa2_retry_deadline(), None);
        assert!(service.pending_ptk(PEER).is_ok());
        service.authorize(PEER, 2).unwrap();
        assert_eq!(
            service.peer_status(PEER).unwrap().phase,
            ApPeerPhase::Authorized
        );
        assert_eq!(
            service.pending_ptk(PEER).err(),
            Some(ApServiceError::WrongPeerPhase)
        );
    }

    #[test]
    fn exhausted_pairwise_update_count_closes_the_peer() {
        let mut storage = AccessPointPeerStorage::new();
        let mut service = service(&mut storage);
        service.authenticate_open(PEER, 0);
        service
            .associate_wpa2(PEER, &WPA2_RSN, ht_capabilities(), [7; 32], 9, 1)
            .unwrap();
        service.begin_wpa2_frame::<512>(PEER).unwrap();
        service.observe_wpa2_transmit(PEER, false, true, 0).unwrap();

        for deadline in [1_000_000, 2_000_000, 3_000_000] {
            assert!(matches!(
                service.take_due_wpa2_retry::<512>(deadline).unwrap(),
                ApWpa2RetryProgress::Transmit { peer: PEER, .. }
            ));
        }
        assert!(matches!(
            service.take_due_wpa2_retry::<512>(4_000_000).unwrap(),
            ApWpa2RetryProgress::Close(ApPeerClose {
                peer: PEER,
                kind: ApPeerCloseKind::Wpa2HandshakeTimeout,
                was_associated: true,
                ..
            })
        ));
        assert_eq!(
            service.peer_status(PEER).unwrap().phase,
            ApPeerPhase::Closing
        );
    }

    #[test]
    fn invalid_rsn_does_not_open_the_controlled_port() {
        let mut storage = AccessPointPeerStorage::new();
        let mut service = service(&mut storage);
        service.authenticate_open(PEER, 0);
        assert_eq!(
            service.associate_wpa2(PEER, &[0x30, 0], LEGACY_CAPABILITIES, [7; 32], 9, 1,),
            Ok(ApMlmeAction::AssociationResponse {
                peer: PEER,
                status: AP_STATUS_INVALID_RSN,
                association_id: None,
            })
        );
        assert_eq!(
            service.peer_status(PEER).unwrap().phase,
            ApPeerPhase::Authenticated
        );
    }

    #[test]
    fn management_sequence_wraps_at_twelve_bits() {
        let mut storage = AccessPointPeerStorage::new();
        let mut service = service(&mut storage);
        for expected in 0..=0x0fff {
            assert_eq!(service.next_management_sequence(), expected);
        }
        assert_eq!(service.next_management_sequence(), 0);
    }

    #[test]
    fn all_fifteen_aids_are_stable_and_reused_after_removal() {
        let mut storage = AccessPointPeerStorage::new();
        let mut service = AccessPointService::new(
            AP,
            Pmk::derive(b"password", b"test-ap").unwrap(),
            Wpa2Gtk::new(1, true, [0x55; 16]).unwrap(),
            AccessPointClientLimit::new(15).unwrap(),
            AccessPointInactiveTimeout::default(),
            &mut storage,
        );
        for suffix in 1..=15_u8 {
            let peer = [0x02, 0, 0, 0, 1, suffix];
            assert_eq!(
                service.authenticate_open(peer, 0),
                ApMlmeAction::AuthenticationResponse {
                    peer,
                    status: AP_STATUS_SUCCESS,
                }
            );
            assert_eq!(
                service.peer_status(peer).unwrap().association_id,
                u16::from(suffix)
            );
            assert!(matches!(
                service
                    .associate_wpa2(
                        peer,
                        &WPA2_RSN,
                        ht_capabilities(),
                        [suffix; 32],
                        u64::from(suffix),
                        1,
                    )
                    .unwrap(),
                ApMlmeAction::AssociationResponse {
                    status: AP_STATUS_SUCCESS,
                    association_id: Some(_),
                    ..
                }
            ));
        }
        assert_eq!(service.associated_count(), 15);
        let overflow = [0x02, 0, 0, 0, 2, 1];
        assert_eq!(
            service.authenticate_open(overflow, 0),
            ApMlmeAction::AuthenticationResponse {
                peer: overflow,
                status: AP_STATUS_TOO_MANY_STATIONS,
            }
        );
        let released = [0x02, 0, 0, 0, 1, 7];
        service.remove_peer(released).unwrap();
        assert_eq!(
            service.authenticate_open(overflow, 0),
            ApMlmeAction::AuthenticationResponse {
                peer: overflow,
                status: AP_STATUS_SUCCESS,
            }
        );
        assert_eq!(service.peer_status(overflow).unwrap().association_id, 7);
    }

    #[test]
    fn bounded_peer_table_has_an_explicit_memory_ceiling() {
        // The service itself travels through the typed lifecycle, while all
        // fifteen WPA2 state machines remain in caller-owned static storage.
        assert!(core::mem::size_of::<AccessPointService<'_>>() <= 256);
        // Fifteen independently negotiated TX BlockAck sessions add 104
        // bytes beyond the former WPA2-only 4-KiB ceiling. Keep that cost
        // explicit instead of moving agreement state to a global singleton.
        assert!(
            core::mem::size_of::<AccessPointPeerStorage>() <= 4_224,
            "peer storage size {}",
            core::mem::size_of::<AccessPointPeerStorage>()
        );
    }

    #[test]
    fn tx_block_ack_is_owned_by_the_exact_authorized_ht_peer() {
        let mut storage = AccessPointPeerStorage::new();
        let mut service = service(&mut storage);
        service.authenticate_open(PEER, 1);
        service
            .associate_wpa2(PEER, &WPA2_RSN, ht_capabilities(), [7; 32], 9, 1)
            .unwrap();
        service.checked_peer_mut(PEER).unwrap().phase = ApPeerPhase::Authorized;

        service.authenticate_open(OTHER, 1);
        service
            .associate_wpa2(OTHER, &WPA2_RSN, LEGACY_CAPABILITIES, [8; 32], 10, 1)
            .unwrap();
        service.checked_peer_mut(OTHER).unwrap().phase = ApPeerPhase::Authorized;

        let request = service.begin_tx_block_ack(PEER, 100).unwrap().unwrap();
        assert_eq!(service.smallest_operational_tx_block_ack_window(), None);
        assert!(service.begin_tx_block_ack(PEER, 101).unwrap().is_none());
        assert!(service.begin_tx_block_ack(OTHER, 101).unwrap().is_none());
        let response = BlockAckAction::AddbaResponse {
            dialog_token: request.dialog_token,
            status: 0,
            tid: AP_TX_BLOCK_ACK_TID,
            immediate: true,
            amsdu: false,
            window: AP_TX_BLOCK_ACK_WINDOW,
            timeout_tu: 0,
        };
        assert!(matches!(
            service.on_tx_block_ack_action(PEER, response),
            Ok(Some(TxBlockAckResponse::Operational(
                OperationalTxBlockAck {
                    tid: AP_TX_BLOCK_ACK_TID,
                    window: AP_TX_BLOCK_ACK_WINDOW,
                    ..
                }
            )))
        ));
        assert!(service.peer_status(PEER).unwrap().tx_block_ack.is_some());
        assert!(service.peer_status(OTHER).unwrap().tx_block_ack.is_none());
        assert_eq!(
            service.smallest_operational_tx_block_ack_window(),
            Some(AP_TX_BLOCK_ACK_WINDOW)
        );

        service
            .on_tx_block_ack_action(
                PEER,
                BlockAckAction::Delba {
                    tid: AP_TX_BLOCK_ACK_TID,
                    initiator: true,
                    reason: 37,
                },
            )
            .unwrap();
        assert!(
            service.peer_status(PEER).unwrap().tx_block_ack.is_some(),
            "peer-originated RX DELBA cannot revoke the AP-originated TX agreement"
        );
        assert_eq!(
            service.smallest_operational_tx_block_ack_window(),
            Some(AP_TX_BLOCK_ACK_WINDOW)
        );

        service
            .on_tx_block_ack_action(
                PEER,
                BlockAckAction::Delba {
                    tid: AP_TX_BLOCK_ACK_TID,
                    initiator: false,
                    reason: 37,
                },
            )
            .unwrap();
        assert!(service.peer_status(PEER).unwrap().tx_block_ack.is_none());
        assert_eq!(service.smallest_operational_tx_block_ack_window(), None);
    }
}
