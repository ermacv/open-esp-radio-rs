//! Bounded multi-peer AP MLME and security ownership.

use core::fmt;

use open_esp_radio_ieee80211::beacon::WPA2_PERSONAL_CCMP_PSK_RSN_IE;
use open_esp_radio_wpa2::{
    OwnedEapolFrame, Pmk, Ptk, PtkContext,
    aes::{SoftwareAesKeyWrapError, software_aes128_key_wrap},
    ap::validate_wpa2_ap_rsn,
    frames::{
        OwnedRsnIe, WPA2_PLAIN_KEY_DATA_CAPACITY, Wpa2FrameError, Wpa2Gtk, Wpa2PlainKeyData,
        Wpa2TxFrame, build_ap_action_frame,
    },
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
pub const AP_STATUS_INVALID_RSN: u16 = 40;

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
    Wpa2(Wpa2StateError),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApWpa2Error {
    Service(ApServiceError),
    Frame(Wpa2FrameError),
    KeyWrap(SoftwareAesKeyWrapError),
    MissingPairwiseKey,
    UnexpectedAction,
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
}

/// Caller-owned storage for all per-client AP protocol and key state.
///
/// This is intentionally separate from [`AccessPointService`]. On embedded
/// targets the table is large enough that constructing or moving it through a
/// cooperative task stack is unsafe; the radio integration gives it a stable
/// static address instead.
pub struct AccessPointPeerStorage {
    peers: [Option<ApPeer>; AP_MAX_CLIENTS],
}

impl AccessPointPeerStorage {
    pub const fn new() -> Self {
        Self {
            peers: [const { None }; AP_MAX_CLIENTS],
        }
    }
}

impl Default for AccessPointPeerStorage {
    fn default() -> Self {
        Self::new()
    }
}

impl ApPeer {
    const fn authenticated(address: [u8; 6], association_id: u16) -> Self {
        Self {
            address,
            association_id,
            phase: ApPeerPhase::Authenticated,
            wpa2: None,
            pending_ptk: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ApPeerStatus {
    pub address: [u8; 6],
    pub association_id: u16,
    pub phase: ApPeerPhase,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AccessPointServiceStatus {
    pub client_limit: AccessPointClientLimit,
    pub associated: u8,
    pub authorized: u8,
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
    next_management_sequence: u16,
    next_data_sequence: u16,
    status_revision: u32,
}

impl<'peers> AccessPointService<'peers> {
    pub fn new(
        address: [u8; 6],
        pmk: Pmk,
        gtk: Wpa2Gtk,
        client_limit: AccessPointClientLimit,
        peer_storage: &'peers mut AccessPointPeerStorage,
    ) -> Self {
        peer_storage.peers.fill_with(|| None);
        Self {
            address,
            pmk,
            gtk,
            peer_storage: Some(peer_storage),
            client_limit,
            next_management_sequence: 0,
            next_data_sequence: 0,
            status_revision: 0,
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
            })
    }

    pub fn associated_count(&self) -> u8 {
        self.peers()
            .filter(|peer| peer.phase != ApPeerPhase::Authenticated)
            .count() as u8
    }

    pub fn authorized_count(&self) -> u8 {
        self.peers()
            .filter(|peer| peer.phase == ApPeerPhase::Authorized)
            .count() as u8
    }

    pub fn is_authorized(&self, address: [u8; 6]) -> bool {
        self.peer_status(address)
            .is_some_and(|peer| peer.phase == ApPeerPhase::Authorized)
    }

    pub fn status(&self) -> AccessPointServiceStatus {
        let mut peers = [None; AP_MAX_CLIENTS];
        for (destination, source) in peers.iter_mut().zip(self.storage().peers.iter()) {
            *destination = source.as_ref().map(|peer| ApPeerStatus {
                address: peer.address,
                association_id: peer.association_id,
                phase: peer.phase,
            });
        }
        AccessPointServiceStatus {
            client_limit: self.client_limit,
            associated: self.associated_count(),
            authorized: self.authorized_count(),
            peers,
        }
    }

    /// Monotonic public peer-table revision for cheap change detection.
    pub const fn status_revision(&self) -> u32 {
        self.status_revision
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

    pub fn authenticate_open(&mut self, peer: [u8; 6]) -> ApMlmeAction {
        let (status, changed) = if let Some(index) = self.peer_index(peer) {
            let association_id = self.storage().peers[index]
                .as_ref()
                .expect("peer index resolves an occupied entry")
                .association_id;
            self.storage_mut().peers[index] = Some(ApPeer::authenticated(peer, association_id));
            (AP_STATUS_SUCCESS, true)
        } else if self.occupied_count() >= self.client_limit.get() {
            (AP_STATUS_TOO_MANY_STATIONS, false)
        } else if let Some(index) = self.storage().peers.iter().position(Option::is_none) {
            let association_id = u16::try_from(index + 1).expect("fifteen AIDs fit u16");
            self.storage_mut().peers[index] = Some(ApPeer::authenticated(peer, association_id));
            (AP_STATUS_SUCCESS, true)
        } else {
            (AP_STATUS_TOO_MANY_STATIONS, false)
        };
        if changed {
            self.status_revision = self.status_revision.wrapping_add(1);
        }
        ApMlmeAction::AuthenticationResponse { peer, status }
    }

    pub fn associate_wpa2(
        &mut self,
        peer: [u8; 6],
        rsn_ie: &[u8],
        authenticator_nonce: [u8; 32],
        initial_replay_counter: u64,
    ) -> Result<ApMlmeAction, ApServiceError> {
        let access_point = self.address;
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
        let wpa2 = Wpa2ApState::new(
            access_point,
            peer,
            authenticator_nonce,
            initial_replay_counter,
        )?;
        existing.phase = ApPeerPhase::Securing;
        existing.wpa2 = Some(wpa2);
        let association_id = existing.association_id;
        self.status_revision = self.status_revision.wrapping_add(1);
        Ok(ApMlmeAction::AssociationResponse {
            peer,
            status: AP_STATUS_SUCCESS,
            association_id: Some(association_id),
        })
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
                    Wpa2ApAction::AuthorizePeer => Ok(ApWpa2Progress::AuthorizePeer),
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
        self.checked_peer_mut(peer)?.pending_ptk = Some(ptk);
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

    pub fn authorize(&mut self, peer: [u8; 6]) -> Result<(), ApServiceError> {
        let existing = self.checked_peer_mut(peer)?;
        if existing.wpa2.as_ref().map(Wpa2ApState::phase) != Some(Wpa2ApPhase::Authorized) {
            return Err(ApServiceError::WrongPeerPhase);
        }
        existing.phase = ApPeerPhase::Authorized;
        existing.pending_ptk = None;
        self.status_revision = self.status_revision.wrapping_add(1);
        Ok(())
    }

    pub fn remove_peer(&mut self, peer: [u8; 6]) -> Result<ApMlmeAction, ApServiceError> {
        let index = self.peer_index(peer).ok_or(ApServiceError::UnknownPeer)?;
        self.storage_mut().peers[index] = None;
        self.status_revision = self.status_revision.wrapping_add(1);
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
    use open_esp_radio_ieee80211::beacon::WPA2_PERSONAL_CCMP_PSK_RSN_IE;
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

    fn service(storage: &mut AccessPointPeerStorage) -> AccessPointService<'_> {
        AccessPointService::new(
            AP,
            Pmk::derive(b"password", b"test-ap").unwrap(),
            Wpa2Gtk::new(1, true, [0x55; 16]).unwrap(),
            AccessPointClientLimit::new(2).unwrap(),
            storage,
        )
    }

    #[test]
    fn runtime_limit_is_enforced_before_hardware_moves() {
        let mut storage = AccessPointPeerStorage::new();
        let mut service = service(&mut storage);
        assert_eq!(
            service.authenticate_open(PEER),
            ApMlmeAction::AuthenticationResponse {
                peer: PEER,
                status: AP_STATUS_SUCCESS,
            }
        );
        assert_eq!(
            service.authenticate_open(OTHER),
            ApMlmeAction::AuthenticationResponse {
                peer: OTHER,
                status: AP_STATUS_SUCCESS,
            }
        );
        let third = [0x02, 0, 0, 0, 0, 4];
        assert_eq!(
            service.authenticate_open(third),
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
    fn client_limit_rejects_zero_and_values_above_the_owned_tables() {
        assert_eq!(AccessPointClientLimit::new(0).unwrap_err().value(), 0,);
        assert_eq!(AccessPointClientLimit::new(16).unwrap_err().value(), 16,);
        assert_eq!(AccessPointClientLimit::new(15).unwrap().get(), 15);
    }

    #[test]
    fn association_owns_a_bounded_wpa2_state() {
        let mut storage = AccessPointPeerStorage::new();
        let mut service = service(&mut storage);
        service.authenticate_open(PEER);
        assert_eq!(
            service.associate_wpa2(PEER, &WPA2_RSN, [7; 32], 9).unwrap(),
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
    fn complete_four_way_handshake_retains_ptk_until_hardware_authorization() {
        const ANONCE: [u8; 32] = [7; 32];
        const SNONCE: [u8; 32] = [8; 32];

        let mut storage = AccessPointPeerStorage::new();
        let mut service = service(&mut storage);
        service.authenticate_open(PEER);
        // Supplicants may add their own RSN capabilities. Message 3 must not
        // reflect those bytes back: it authenticates the AP's beacon RSN IE.
        service
            .associate_wpa2(PEER, &SUPPLICANT_RSN, ANONCE, 9)
            .unwrap();
        let message1 = service.begin_wpa2_frame::<512>(PEER).unwrap();
        assert_eq!(
            message1.key_frame().message(),
            EapolKeyMessage::PairwiseMessage1
        );

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
        assert!(service.pending_ptk(PEER).is_ok());
        service.authorize(PEER).unwrap();
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
    fn invalid_rsn_does_not_open_the_controlled_port() {
        let mut storage = AccessPointPeerStorage::new();
        let mut service = service(&mut storage);
        service.authenticate_open(PEER);
        assert_eq!(
            service.associate_wpa2(PEER, &[0x30, 0], [7; 32], 9),
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
            &mut storage,
        );
        for suffix in 1..=15_u8 {
            let peer = [0x02, 0, 0, 0, 1, suffix];
            assert_eq!(
                service.authenticate_open(peer),
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
                    .associate_wpa2(peer, &WPA2_RSN, [suffix; 32], u64::from(suffix))
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
            service.authenticate_open(overflow),
            ApMlmeAction::AuthenticationResponse {
                peer: overflow,
                status: AP_STATUS_TOO_MANY_STATIONS,
            }
        );
        let released = [0x02, 0, 0, 0, 1, 7];
        service.remove_peer(released).unwrap();
        assert_eq!(
            service.authenticate_open(overflow),
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
        assert!(core::mem::size_of::<AccessPointPeerStorage>() <= 4_096);
    }
}
