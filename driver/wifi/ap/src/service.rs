//! Bounded single-peer AP MLME and security ownership.

use open_esp_radio_ieee80211::ap::{
    ApPowerSaveObservation, observe_ap_power_save, updated_tim_bitmap_byte,
};
use open_esp_radio_wpa2::{
    OwnedEapolFrame, Pmk, Ptk, PtkContext,
    aes::{SoftwareAesKeyWrapError, software_aes128_key_wrap},
    ap::{ValidatedWpa2ApRsn, validate_wpa2_ap_rsn},
    frames::{
        WPA2_PLAIN_KEY_DATA_CAPACITY, Wpa2FrameError, Wpa2Gtk, Wpa2PlainKeyData, Wpa2TxFrame,
        build_ap_action_frame,
    },
    state::{
        PtkContext as Wpa2StatePtkContext, Wpa2ApAction, Wpa2ApPhase, Wpa2ApState, Wpa2StateError,
    },
};

pub const AP_ASSOCIATION_ID: u16 = 1;
pub const AP_STATUS_SUCCESS: u16 = 0;
pub const AP_STATUS_TOO_MANY_STATIONS: u16 = 17;
pub const AP_STATUS_INVALID_RSN: u16 = 40;

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
pub enum ApPowerSaveAction {
    None,
    PeerSleeping,
    PeerActive,
    ReleaseOneBufferedUnicast,
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
    phase: ApPeerPhase,
    sleeping: bool,
    unicast_pending: bool,
    rsn: Option<ValidatedWpa2ApRsn>,
    wpa2: Option<Wpa2ApState>,
    pending_ptk: Option<Ptk>,
}

impl ApPeer {
    const fn authenticated(address: [u8; 6]) -> Self {
        Self {
            address,
            phase: ApPeerPhase::Authenticated,
            sleeping: false,
            unicast_pending: false,
            rsn: None,
            wpa2: None,
            pending_ptk: None,
        }
    }
}

/// Complete portable owner for one AP service epoch.
///
/// Dropping this value clears the PMK and GTK through their zeroize-on-drop
/// implementations. A chip runtime still must clear its typed hardware slots
/// before it may classify the corresponding physical owner as stopped.
pub struct AccessPointService {
    address: [u8; 6],
    pmk: Pmk,
    gtk: Wpa2Gtk,
    peer: Option<ApPeer>,
    group_pending: bool,
    next_management_sequence: u16,
    next_data_sequence: u16,
}

impl AccessPointService {
    pub const fn new(address: [u8; 6], pmk: Pmk, gtk: Wpa2Gtk) -> Self {
        Self {
            address,
            pmk,
            gtk,
            peer: None,
            group_pending: false,
            next_management_sequence: 0,
            next_data_sequence: 0,
        }
    }

    pub const fn address(&self) -> [u8; 6] {
        self.address
    }

    pub fn peer(&self) -> Option<[u8; 6]> {
        self.peer.as_ref().map(|peer| peer.address)
    }

    pub fn peer_phase(&self) -> Option<ApPeerPhase> {
        self.peer.as_ref().map(|peer| peer.phase)
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
        let status = match self.peer.as_ref() {
            None => {
                self.peer = Some(ApPeer::authenticated(peer));
                AP_STATUS_SUCCESS
            }
            Some(existing) if existing.address == peer => {
                self.peer = Some(ApPeer::authenticated(peer));
                AP_STATUS_SUCCESS
            }
            Some(_) => AP_STATUS_TOO_MANY_STATIONS,
        };
        ApMlmeAction::AuthenticationResponse { peer, status }
    }

    pub fn associate_wpa2(
        &mut self,
        peer: [u8; 6],
        rsn_ie: &[u8],
        authenticator_nonce: [u8; 32],
        initial_replay_counter: u64,
    ) -> Result<ApMlmeAction, ApServiceError> {
        let Some(existing) = self.peer.as_mut() else {
            return Err(ApServiceError::UnknownPeer);
        };
        if existing.address != peer {
            return Ok(ApMlmeAction::AssociationResponse {
                peer,
                status: AP_STATUS_TOO_MANY_STATIONS,
                association_id: None,
            });
        }
        if existing.phase != ApPeerPhase::Authenticated {
            return Err(ApServiceError::WrongPeerPhase);
        }
        let rsn = match validate_wpa2_ap_rsn(rsn_ie) {
            Ok(rsn) => rsn,
            Err(_) => {
                return Ok(ApMlmeAction::AssociationResponse {
                    peer,
                    status: AP_STATUS_INVALID_RSN,
                    association_id: None,
                });
            }
        };
        let wpa2 = Wpa2ApState::new(
            self.address,
            peer,
            authenticator_nonce,
            initial_replay_counter,
        )?;
        existing.phase = ApPeerPhase::Securing;
        existing.rsn = Some(rsn);
        existing.wpa2 = Some(wpa2);
        Ok(ApMlmeAction::AssociationResponse {
            peer,
            status: AP_STATUS_SUCCESS,
            association_id: Some(AP_ASSOCIATION_ID),
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

        let rsn = self
            .checked_peer(peer)?
            .rsn
            .as_ref()
            .ok_or(ApServiceError::WrongPeerPhase)?
            .owned()
            .clone();
        let plain = Wpa2PlainKeyData::<WPA2_PLAIN_KEY_DATA_CAPACITY>::build(&rsn, &self.gtk)?;
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
        let rsn = existing
            .rsn
            .as_ref()
            .ok_or(ApServiceError::WrongPeerPhase)?;
        let plain =
            Wpa2PlainKeyData::<WPA2_PLAIN_KEY_DATA_CAPACITY>::build(rsn.owned(), &self.gtk)?;
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
        Ok(())
    }

    pub fn remove_peer(&mut self, peer: [u8; 6]) -> Result<ApMlmeAction, ApServiceError> {
        if self.peer.as_ref().map(|existing| existing.address) != Some(peer) {
            return Err(ApServiceError::UnknownPeer);
        }
        self.peer = None;
        Ok(ApMlmeAction::PeerRemoved { peer })
    }

    pub fn observe_power_save(&mut self, frame: &[u8]) -> ApPowerSaveAction {
        let Some(observation) = observe_ap_power_save(frame) else {
            return ApPowerSaveAction::None;
        };
        match observation {
            ApPowerSaveObservation::Sleeping { peer } => {
                let Ok(existing) = self.checked_authorized_peer_mut(peer) else {
                    return ApPowerSaveAction::None;
                };
                existing.sleeping = true;
                ApPowerSaveAction::PeerSleeping
            }
            ApPowerSaveObservation::Active { peer } => {
                let Ok(existing) = self.checked_authorized_peer_mut(peer) else {
                    return ApPowerSaveAction::None;
                };
                existing.sleeping = false;
                ApPowerSaveAction::PeerActive
            }
            ApPowerSaveObservation::PsPoll {
                peer,
                association_id,
            } => {
                let Ok(existing) = self.checked_authorized_peer_mut(peer) else {
                    return ApPowerSaveAction::None;
                };
                if association_id != AP_ASSOCIATION_ID || !existing.unicast_pending {
                    return ApPowerSaveAction::None;
                }
                existing.unicast_pending = false;
                ApPowerSaveAction::ReleaseOneBufferedUnicast
            }
        }
    }

    pub fn set_unicast_pending(
        &mut self,
        peer: [u8; 6],
        pending: bool,
    ) -> Result<(), ApServiceError> {
        let existing = self.checked_authorized_peer_mut(peer)?;
        existing.unicast_pending = pending;
        Ok(())
    }

    pub const fn set_group_pending(&mut self, pending: bool) {
        self.group_pending = pending;
    }

    pub fn group_pending(&self) -> bool {
        self.group_pending
    }

    pub fn tim_bitmap_byte(&self) -> u8 {
        match self.peer.as_ref() {
            Some(peer) if peer.unicast_pending => {
                updated_tim_bitmap_byte(0, AP_ASSOCIATION_ID, true)
            }
            _ => 0,
        }
    }

    fn checked_peer(&self, peer: [u8; 6]) -> Result<&ApPeer, ApServiceError> {
        self.peer
            .as_ref()
            .filter(|existing| existing.address == peer)
            .ok_or(ApServiceError::UnknownPeer)
    }

    fn checked_peer_mut(&mut self, peer: [u8; 6]) -> Result<&mut ApPeer, ApServiceError> {
        self.peer
            .as_mut()
            .filter(|existing| existing.address == peer)
            .ok_or(ApServiceError::UnknownPeer)
    }

    fn checked_authorized_peer_mut(
        &mut self,
        peer: [u8; 6],
    ) -> Result<&mut ApPeer, ApServiceError> {
        let existing = self.checked_peer_mut(peer)?;
        if existing.phase != ApPeerPhase::Authorized {
            return Err(ApServiceError::WrongPeerPhase);
        }
        Ok(existing)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use open_esp_radio_wpa2::{
        EapolKeyMessage, OwnedEapolFrame, PtkContext, Wpa2Interface,
        frames::{OwnedRsnIe, Wpa2Gtk, Wpa2TxFrame},
        state::{Wpa2ApAction, Wpa2Ticket},
    };

    const AP: [u8; 6] = [0x02, 0, 0, 0, 0, 1];
    const PEER: [u8; 6] = [0x02, 0, 0, 0, 0, 2];
    const OTHER: [u8; 6] = [0x02, 0, 0, 0, 0, 3];
    const WPA2_RSN: [u8; 22] = [
        0x30, 20, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 2, 0, 0,
    ];

    fn service() -> AccessPointService {
        AccessPointService::new(
            AP,
            Pmk::derive(b"password", b"test-ap").unwrap(),
            Wpa2Gtk::new(1, true, [0x55; 16]).unwrap(),
        )
    }

    #[test]
    fn one_peer_is_enforced_before_hardware_moves() {
        let mut service = service();
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
                status: AP_STATUS_TOO_MANY_STATIONS,
            }
        );
        assert_eq!(service.peer(), Some(PEER));
    }

    #[test]
    fn association_owns_a_bounded_wpa2_state() {
        let mut service = service();
        service.authenticate_open(PEER);
        assert_eq!(
            service.associate_wpa2(PEER, &WPA2_RSN, [7; 32], 9).unwrap(),
            ApMlmeAction::AssociationResponse {
                peer: PEER,
                status: AP_STATUS_SUCCESS,
                association_id: Some(AP_ASSOCIATION_ID),
            }
        );
        assert_eq!(service.peer_phase(), Some(ApPeerPhase::Securing));
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

        let mut service = service();
        service.authenticate_open(PEER);
        service.associate_wpa2(PEER, &WPA2_RSN, ANONCE, 9).unwrap();
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
        let rsn = OwnedRsnIe::<64>::try_copy(&WPA2_RSN).unwrap();
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
        assert_eq!(service.peer_phase(), Some(ApPeerPhase::Authorized));
        assert_eq!(
            service.pending_ptk(PEER).err(),
            Some(ApServiceError::WrongPeerPhase)
        );
    }

    #[test]
    fn invalid_rsn_does_not_open_the_controlled_port() {
        let mut service = service();
        service.authenticate_open(PEER);
        assert_eq!(
            service.associate_wpa2(PEER, &[0x30, 0], [7; 32], 9),
            Ok(ApMlmeAction::AssociationResponse {
                peer: PEER,
                status: AP_STATUS_INVALID_RSN,
                association_id: None,
            })
        );
        assert_eq!(service.peer_phase(), Some(ApPeerPhase::Authenticated));
    }

    #[test]
    fn management_sequence_wraps_at_twelve_bits() {
        let mut service = service();
        for expected in 0..=0x0fff {
            assert_eq!(service.next_management_sequence(), expected);
        }
        assert_eq!(service.next_management_sequence(), 0);
    }
}
