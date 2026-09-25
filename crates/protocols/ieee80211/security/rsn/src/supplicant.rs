//! Allocation-free RSN supplicant orchestration.
//!
//! [`RsnStaState`] remains the protocol transition owner. This module joins
//! those transitions to PMK/PTK derivation, MIC verification, asynchronous
//! key-data unwrap, exact Association security IEs and typed key-install
//! requests. A target executor therefore handles only complete EAPOL RX/TX
//! frames and the platform-specific key-slot transaction.

use crate::{
    Akm, EapolKeyFrame, Pmk, Ptk, PtkContext, RsnInterface, RsnKeyConfirmationKey,
    RsnKeyEncryptionKey,
    aes::AsyncRsnKeyUnwrap,
    element::{RsnElementError, validate_rsn_element},
    frames::{
        OwnedAssociationSecurityIes, OwnedRsnIe, RsnFrameError, RsnTxFrame, build_sta_action_frame,
        parse_group_gtk_key_data, parse_gtk_key_data,
    },
    keys::RsnKeyInstall,
    state::{RsnStaAction, RsnStaPhase, RsnStaState, RsnStateError, RsnTransmit},
};
use zeroize::{Zeroize, ZeroizeOnDrop};

/// Hardware-qualified open-STA compatibility window for the first EAPOL-Key
/// message. The numeric value is retained from the pre-transfer HIL policy;
/// it is not claimed as a recovered vendor constant.
pub const RSN_STA_MESSAGE1_TIMEOUT_MS: u32 = 3_000;

/// Complete window after the original Message 2 transmission.
///
/// SOURCE: complete `libwpa_supplicant.a[wpa.c.obj]::
/// wpa_supplicant_process_1_of_4` emits one Message 2 and does not register a
/// retransmission timeout. A repeated Message 1 instead re-enters through
/// `wpa_sm_rx_eapol` and produces another Message 2. The six-second value
/// preserves the two former three-second HIL receive windows without
/// inventing a station-originated Message 2 retry.
pub const RSN_STA_MESSAGE3_TIMEOUT_MS: u32 = 6_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RsnStaResponseWait {
    Message1,
    Message3,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RsnStaDeadlineEvent {
    Pending,
    Expired {
        wait: RsnStaResponseWait,
        elapsed_ms: u32,
    },
}

/// Finite millisecond deadline owned independently from Embassy or a hardware
/// alarm. A target calls [`Self::finish_millisecond`] after servicing all RX
/// completions for that interval.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RsnStaResponseDeadline {
    wait: RsnStaResponseWait,
    elapsed_ms: u32,
}

impl RsnStaResponseDeadline {
    pub const fn new(wait: RsnStaResponseWait) -> Self {
        Self {
            wait,
            elapsed_ms: 0,
        }
    }

    pub const fn elapsed_ms(&self) -> u32 {
        self.elapsed_ms
    }

    pub fn finish_millisecond(&mut self) -> RsnStaDeadlineEvent {
        self.elapsed_ms = self.elapsed_ms.saturating_add(1);
        if self.elapsed_ms >= self.timeout_ms() {
            RsnStaDeadlineEvent::Expired {
                wait: self.wait,
                elapsed_ms: self.elapsed_ms,
            }
        } else {
            RsnStaDeadlineEvent::Pending
        }
    }

    const fn timeout_ms(&self) -> u32 {
        match self.wait {
            RsnStaResponseWait::Message1 => RSN_STA_MESSAGE1_TIMEOUT_MS,
            RsnStaResponseWait::Message3 => RSN_STA_MESSAGE3_TIMEOUT_MS,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RsnStaSupplicantError {
    State(RsnStateError),
    Frame(RsnFrameError),
    /// The station's own Association RSN element names no supported suite.
    Element(RsnElementError),
    MissingPtk,
    InvalidMessage3Mic,
    UnexpectedAction,
}

impl RsnStaSupplicantError {
    /// Rejects caused by the current unauthenticated peer frame are
    /// recoverable within the existing response window. Local state,
    /// cryptographic ownership and frame-construction failures are not.
    pub const fn is_peer_input_rejection(self) -> bool {
        match self {
            Self::State(error) => error.is_peer_input_rejection(),
            Self::InvalidMessage3Mic => true,
            Self::Frame(_) | Self::Element(_) | Self::MissingPtk | Self::UnexpectedAction => false,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RsnStaProcessError<E> {
    Supplicant(RsnStaSupplicantError),
    KeyUnwrap(E),
}

impl<E> RsnStaProcessError<E> {
    pub const fn is_peer_input_rejection(&self) -> bool {
        match self {
            Self::Supplicant(error) => error.is_peer_input_rejection(),
            Self::KeyUnwrap(_) => false,
        }
    }
}

impl<E> From<RsnStaSupplicantError> for RsnStaProcessError<E> {
    fn from(error: RsnStaSupplicantError) -> Self {
        Self::Supplicant(error)
    }
}

/// Complete pairwise/group installation transaction prepared from Message 3.
///
/// Both keys are owned and zeroized on drop. The private state ticket makes a
/// completion valid only for the exact Message 3 that produced this request.
pub struct RsnStaKeyInstallRequest {
    ticket: crate::state::RsnTicket,
    replay_counter: u64,
    encrypted_key_data: bool,
    plain_key_data_len: usize,
    pairwise: RsnKeyInstall,
    group: RsnKeyInstall,
}

impl RsnStaKeyInstallRequest {
    pub const fn replay_counter(&self) -> u64 {
        self.replay_counter
    }

    pub const fn encrypted_key_data(&self) -> bool {
        self.encrypted_key_data
    }

    pub const fn plain_key_data_len(&self) -> usize {
        self.plain_key_data_len
    }

    pub const fn pairwise(&self) -> &RsnKeyInstall {
        &self.pairwise
    }

    pub const fn group(&self) -> &RsnKeyInstall {
        &self.group
    }
}

pub enum RsnStaSupplicantAction<const N: usize> {
    None,
    Transmit(RsnTxFrame<N>),
    InstallKeys(RsnStaKeyInstallRequest),
    Deauthenticate,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RsnConnectedSupplicantError {
    WrongInterface,
    WrongPeer,
    ProtocolVersionMismatch,
    UnsupportedDescriptorVersion,
    UnsupportedMessage,
    ReplayCounterMismatch,
    AuthenticatorNonceMismatch,
    RetainedMessage3Mismatch,
    RetainedGroupMessage1Mismatch,
    MissingEncryptedKeyData,
    InvalidMic,
    StaleReplayCounter,
    Busy,
    StaleCompletion,
    InstallFailed,
    Frame(RsnFrameError),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RsnConnectedProcessError<E> {
    Supplicant(RsnConnectedSupplicantError),
    KeyUnwrap(E),
}

/// Exact connected-state GTK publication ticket.
pub struct RsnGroupKeyInstallRequest<const N: usize> {
    ticket: u32,
    replay_counter: u64,
    commitment: RsnCompletedGroupMessage1,
    group: RsnKeyInstall,
    response: RsnTxFrame<N>,
}

impl<const N: usize> RsnGroupKeyInstallRequest<N> {
    pub const fn replay_counter(&self) -> u64 {
        self.replay_counter
    }

    pub const fn group(&self) -> &RsnKeyInstall {
        &self.group
    }
}

pub enum RsnConnectedAction<const N: usize> {
    InstallGroupKey(RsnGroupKeyInstallRequest<N>),
    Retransmit(RsnTxFrame<N>),
}

/// Bounded binding to the exact authenticated Message 3 which installed the
/// connected association's keys.
///
/// The retained MIC is an HMAC-SHA1-128 commitment to the complete EAPOL-Key
/// packet with its MIC field cleared. The small explicit protocol prefix lets
/// descriptor, flag and replay mismatches fail before authentication; after a
/// candidate MIC verifies under the retained KCK, equality with the completed
/// MIC binds every remaining field, including ANonce, IV, RSC and encrypted
/// Key Data, without retaining those handshake bytes after connection.
#[derive(Zeroize, ZeroizeOnDrop)]
struct RsnCompletedMessage3 {
    protocol_version: u8,
    key_info: u16,
    replay_counter: u64,
    authenticator_nonce_tag: u32,
    mic: [u8; 16],
}

impl RsnCompletedMessage3 {
    fn capture(frame: EapolKeyFrame<'_>) -> Self {
        Self {
            protocol_version: frame.protocol_version(),
            key_info: frame.key_info().raw(),
            replay_counter: frame.replay_counter(),
            authenticator_nonce_tag: message3_nonce_tag(frame.nonce()),
            mic: *frame.mic(),
        }
    }

    fn validate_fields(
        &self,
        akm: Akm,
        frame: EapolKeyFrame<'_>,
    ) -> Result<(), RsnConnectedSupplicantError> {
        if frame.protocol_version() != self.protocol_version {
            return Err(RsnConnectedSupplicantError::ProtocolVersionMismatch);
        }
        if frame.key_info().descriptor_version() != akm.key_descriptor_version() {
            return Err(RsnConnectedSupplicantError::UnsupportedDescriptorVersion);
        }
        if frame.message() != crate::EapolKeyMessage::PairwiseMessage3
            || frame.key_info().raw() != self.key_info
        {
            return Err(RsnConnectedSupplicantError::UnsupportedMessage);
        }
        if frame.replay_counter() != self.replay_counter {
            return Err(RsnConnectedSupplicantError::ReplayCounterMismatch);
        }
        // This compact tag is only a diagnostic prefilter over public ANonce;
        // the verified retained MIC below remains the cryptographic authority
        // and catches tag collisions as a completed-frame mismatch.
        if message3_nonce_tag(frame.nonce()) != self.authenticator_nonce_tag {
            return Err(RsnConnectedSupplicantError::AuthenticatorNonceMismatch);
        }
        Ok(())
    }
}

/// Authenticated commitment to the exact Group Message 1 whose GTK was
/// successfully published.
///
/// The retained MIC commits to the complete EAPOL-Key packet with the MIC
/// field cleared. Equality after candidate authentication therefore binds the
/// RSC, encrypted GTK and every other packet byte without retaining key data.
#[derive(Zeroize, ZeroizeOnDrop)]
struct RsnCompletedGroupMessage1 {
    mic: [u8; 16],
}

impl RsnCompletedGroupMessage1 {
    fn capture(frame: EapolKeyFrame<'_>) -> Self {
        Self { mic: *frame.mic() }
    }

    fn matches(&self, frame: EapolKeyFrame<'_>) -> bool {
        self.mic == *frame.mic()
    }
}

fn message3_nonce_tag(nonce: &[u8; 32]) -> u32 {
    nonce.iter().fold(0x811c_9dc5, |tag, byte| {
        (tag ^ u32::from(*byte)).wrapping_mul(0x0100_0193)
    })
}

/// WPA2 state which must survive for the complete connected association.
///
/// It retains only PTK-derived authentication material and replay state. PMK
/// and association retry policy remain owned by the outer station lifecycle.
pub struct RsnConnectedSupplicant {
    authenticator: [u8; 6],
    key_confirmation: RsnKeyConfirmationKey,
    key_encryption: RsnKeyEncryptionKey,
    completed_message3: RsnCompletedMessage3,
    replay_counter: u64,
    completed_group_message1: Option<RsnCompletedGroupMessage1>,
    pending: Option<(u32, u64)>,
    next_ticket: u32,
}

impl RsnConnectedSupplicant {
    pub const fn replay_counter(&self) -> u64 {
        self.replay_counter
    }

    /// Authenticate an exact retransmission of the Message 3 which completed
    /// this association and produce a retransmission-marked Message 4.
    ///
    /// This path cannot publish key-install work. A frame with a valid MIC but
    /// any changed completed-handshake field is still rejected, preventing a
    /// peer from replacing key material or receive sequence state after the
    /// hardware keys have been installed.
    pub fn on_duplicate_message3<const N: usize>(
        &mut self,
        frame: crate::OwnedEapolFrame<N>,
    ) -> Result<RsnTxFrame<N>, RsnConnectedSupplicantError> {
        if frame.interface() != RsnInterface::Station {
            return Err(RsnConnectedSupplicantError::WrongInterface);
        }
        if frame.peer() != &self.authenticator {
            return Err(RsnConnectedSupplicantError::WrongPeer);
        }
        let key = frame.key_frame();
        self.completed_message3
            .validate_fields(self.key_confirmation.akm(), key)?;
        if !key.verify_mic_with_confirmation_key(&self.key_confirmation) {
            return Err(RsnConnectedSupplicantError::InvalidMic);
        }
        if key.mic() != &self.completed_message3.mic {
            return Err(RsnConnectedSupplicantError::RetainedMessage3Mismatch);
        }
        RsnTxFrame::message4(
            self.key_confirmation.akm(),
            self.authenticator,
            self.completed_message3.replay_counter,
        )
        .map_err(RsnConnectedSupplicantError::Frame)
        .map(|frame| {
            frame
                .authenticate_with_confirmation_key(&self.key_confirmation)
                .mark_retransmission()
        })
    }

    pub async fn on_group_message1<const N: usize, U: AsyncRsnKeyUnwrap>(
        &mut self,
        frame: crate::OwnedEapolFrame<N>,
        unwrap: &mut U,
    ) -> Result<RsnConnectedAction<N>, RsnConnectedProcessError<U::Error>> {
        if self.pending.is_some() {
            return Err(RsnConnectedProcessError::Supplicant(
                RsnConnectedSupplicantError::Busy,
            ));
        }
        if frame.interface() != RsnInterface::Station {
            return Err(RsnConnectedProcessError::Supplicant(
                RsnConnectedSupplicantError::WrongInterface,
            ));
        }
        if frame.peer() != &self.authenticator {
            return Err(RsnConnectedProcessError::Supplicant(
                RsnConnectedSupplicantError::WrongPeer,
            ));
        }
        let key = frame.key_frame();
        if key.key_info().descriptor_version()
            != self.key_confirmation.akm().key_descriptor_version()
        {
            return Err(RsnConnectedProcessError::Supplicant(
                RsnConnectedSupplicantError::UnsupportedDescriptorVersion,
            ));
        }
        if key.message() != crate::EapolKeyMessage::GroupMessage1 {
            return Err(RsnConnectedProcessError::Supplicant(
                RsnConnectedSupplicantError::UnsupportedMessage,
            ));
        }
        let replay_counter = key.replay_counter();
        if replay_counter < self.replay_counter
            || (replay_counter == self.replay_counter && self.completed_group_message1.is_none())
        {
            return Err(RsnConnectedProcessError::Supplicant(
                RsnConnectedSupplicantError::StaleReplayCounter,
            ));
        }
        // A cached Group Message 2 is still an authenticated response. Verify
        // every repeated Group Message 1 before admitting the idempotent path,
        // otherwise a forged frame can elicit a valid MIC-bearing response.
        if !key.verify_mic_with_confirmation_key(&self.key_confirmation) {
            return Err(RsnConnectedProcessError::Supplicant(
                RsnConnectedSupplicantError::InvalidMic,
            ));
        }
        if let Some(completed) = self
            .completed_group_message1
            .as_ref()
            .filter(|_| replay_counter == self.replay_counter)
        {
            if !completed.matches(key) {
                return Err(RsnConnectedProcessError::Supplicant(
                    RsnConnectedSupplicantError::RetainedGroupMessage1Mismatch,
                ));
            }
            let response = RsnTxFrame::group_message2(
                self.key_confirmation.akm(),
                self.authenticator,
                replay_counter,
            )
            .map_err(|error| {
                RsnConnectedProcessError::Supplicant(RsnConnectedSupplicantError::Frame(error))
            })?
            .authenticate_with_confirmation_key(&self.key_confirmation);
            return Ok(RsnConnectedAction::Retransmit(response));
        }
        if !key.key_info().encrypted_key_data() || key.key_data().is_empty() {
            return Err(RsnConnectedProcessError::Supplicant(
                RsnConnectedSupplicantError::MissingEncryptedKeyData,
            ));
        }
        let plain = unwrap
            .unwrap_key_data(self.key_encryption.as_bytes(), key.key_data())
            .await
            .map_err(RsnConnectedProcessError::KeyUnwrap)?;
        let gtk = parse_group_gtk_key_data(plain.as_bytes()).map_err(|error| {
            RsnConnectedProcessError::Supplicant(RsnConnectedSupplicantError::Frame(error))
        })?;
        let group = RsnKeyInstall::group(RsnInterface::Station, &gtk, *key.key_receive_sequence());
        let response = RsnTxFrame::group_message2(
            self.key_confirmation.akm(),
            self.authenticator,
            replay_counter,
        )
        .map_err(|error| {
            RsnConnectedProcessError::Supplicant(RsnConnectedSupplicantError::Frame(error))
        })?
        .authenticate_with_confirmation_key(&self.key_confirmation);
        let ticket = self.next_ticket;
        self.next_ticket = self.next_ticket.wrapping_add(1).max(1);
        self.pending = Some((ticket, replay_counter));
        Ok(RsnConnectedAction::InstallGroupKey(
            RsnGroupKeyInstallRequest {
                ticket,
                replay_counter,
                commitment: RsnCompletedGroupMessage1::capture(key),
                group,
                response,
            },
        ))
    }

    pub fn complete_group_key_install<const N: usize>(
        &mut self,
        request: RsnGroupKeyInstallRequest<N>,
        installed: bool,
    ) -> Result<RsnTxFrame<N>, RsnConnectedSupplicantError> {
        if self.pending != Some((request.ticket, request.replay_counter)) {
            return Err(RsnConnectedSupplicantError::StaleCompletion);
        }
        self.pending = None;
        if !installed {
            return Err(RsnConnectedSupplicantError::InstallFailed);
        }
        self.replay_counter = request.replay_counter;
        self.completed_group_message1 = Some(request.commitment);
        Ok(request.response)
    }
}

/// One WPA2-Personal station handshake with owned cryptographic state.
pub struct RsnStaSupplicant {
    state: RsnStaState,
    ptk: Option<Ptk>,
    completed_message3: Option<RsnCompletedMessage3>,
    association_security_ies: OwnedAssociationSecurityIes,
    authenticator_security_ies: OwnedAssociationSecurityIes,
}

impl RsnStaSupplicant {
    pub fn try_new(
        local: [u8; 6],
        authenticator: [u8; 6],
        supplicant_nonce: [u8; 32],
        association_security_ies: &[u8],
        authenticator_rsn_ie: &[u8],
        authenticator_rsnxe: &[u8],
    ) -> Result<Self, RsnStaSupplicantError> {
        let association_security_ies =
            OwnedAssociationSecurityIes::try_copy_bytes(association_security_ies)
                .map_err(RsnStaSupplicantError::Frame)?;
        // The station's own Association RSN element names the negotiated suite.
        let akm = validate_rsn_element(association_security_ies.rsn_ie())
            .map_err(RsnStaSupplicantError::Element)?
            .akm();
        let state = RsnStaState::new(akm, local, authenticator, supplicant_nonce)
            .map_err(RsnStaSupplicantError::State)?;
        let authenticator_rsn_ie =
            OwnedRsnIe::<{ crate::frames::RSN_IE_CAPACITY }>::try_copy(authenticator_rsn_ie)
                .map_err(RsnStaSupplicantError::Frame)?;
        let authenticator_security_ies =
            OwnedAssociationSecurityIes::try_copy(&authenticator_rsn_ie, authenticator_rsnxe)
                .map_err(RsnStaSupplicantError::Frame)?;
        Ok(Self {
            state,
            ptk: None,
            completed_message3: None,
            association_security_ies,
            authenticator_security_ies,
        })
    }

    pub const fn phase(&self) -> RsnStaPhase {
        self.state.phase()
    }

    pub fn into_connected(self) -> Result<RsnConnectedSupplicant, RsnStaSupplicantError> {
        let replay_counter = self
            .state
            .completed_replay_counter()
            .ok_or(RsnStaSupplicantError::UnexpectedAction)?;
        let ptk = self.ptk.ok_or(RsnStaSupplicantError::MissingPtk)?;
        let completed_message3 = self
            .completed_message3
            .ok_or(RsnStaSupplicantError::UnexpectedAction)?;
        let (key_confirmation, key_encryption) = ptk.into_connected_keys();
        Ok(RsnConnectedSupplicant {
            authenticator: *self.state.peer(),
            key_confirmation,
            key_encryption,
            completed_message3,
            replay_counter,
            completed_group_message1: None,
            pending: None,
            next_ticket: 1,
        })
    }

    /// Consume one validated peer EAPOL-Key frame and resolve every
    /// hardware-independent action. AES unwrap remains an async capability so
    /// a future hardware backend can suspend on an interrupt without changing
    /// this state owner.
    pub async fn on_frame<const N: usize, U: AsyncRsnKeyUnwrap>(
        &mut self,
        frame: crate::OwnedEapolFrame<N>,
        pmk: &Pmk,
        unwrap: &mut U,
    ) -> Result<RsnStaSupplicantAction<N>, RsnStaProcessError<U::Error>> {
        let action = self
            .state
            .on_frame(frame)
            .map_err(RsnStaSupplicantError::State)?;
        match action {
            RsnStaAction::None => Ok(RsnStaSupplicantAction::None),
            RsnStaAction::DerivePtk { ticket, context } => {
                self.completed_message3 = None;
                self.ptk = Some(pmk.derive_ptk(
                    self.state.akm(),
                    PtkContext {
                        authenticator_address: context.authenticator_address,
                        supplicant_address: context.supplicant_address,
                        authenticator_nonce: context.authenticator_nonce,
                        supplicant_nonce: context.supplicant_nonce,
                    },
                ));
                let action = self
                    .state
                    .complete_ptk::<N>(ticket, true)
                    .map_err(RsnStaSupplicantError::State)?;
                let RsnStaAction::Transmit(transmit) = action else {
                    return Err(RsnStaSupplicantError::UnexpectedAction.into());
                };
                self.build_transmit(transmit).map_err(Into::into)
            }
            RsnStaAction::Transmit(transmit) => self.build_transmit(transmit).map_err(Into::into),
            RsnStaAction::VerifyMessage3Mic { ticket, frame } => {
                if !frame.key_frame().verify_mic(self.ptk()?) {
                    self.state
                        .complete_message3_mic::<N>(ticket, frame, false)
                        .map_err(RsnStaSupplicantError::State)?;
                    return Err(RsnStaSupplicantError::InvalidMessage3Mic.into());
                }
                let action = self
                    .state
                    .complete_message3_mic(ticket, frame, true)
                    .map_err(RsnStaSupplicantError::State)?;
                self.resolve_message3(action, unwrap).await
            }
            RsnStaAction::Deauthenticate => Ok(RsnStaSupplicantAction::Deauthenticate),
            RsnStaAction::DecryptMessage3KeyData { .. } | RsnStaAction::InstallKeys { .. } => {
                Err(RsnStaSupplicantError::UnexpectedAction.into())
            }
        }
    }

    /// Complete the exact platform key-slot transaction represented by
    /// `request`. A successful completion produces an already authenticated
    /// Message 4; a failed completion moves the protocol state to Failed.
    pub fn complete_key_install<const N: usize>(
        &mut self,
        request: RsnStaKeyInstallRequest,
        installed: bool,
    ) -> Result<RsnStaSupplicantAction<N>, RsnStaSupplicantError> {
        let action = self
            .state
            .complete_key_install::<N>(request.ticket, installed)
            .map_err(RsnStaSupplicantError::State)?;
        match action {
            RsnStaAction::Transmit(transmit) => self.build_transmit(transmit),
            RsnStaAction::Deauthenticate => Ok(RsnStaSupplicantAction::Deauthenticate),
            _ => Err(RsnStaSupplicantError::UnexpectedAction),
        }
    }

    async fn resolve_message3<const N: usize, U: AsyncRsnKeyUnwrap>(
        &mut self,
        action: RsnStaAction<N>,
        unwrap: &mut U,
    ) -> Result<RsnStaSupplicantAction<N>, RsnStaProcessError<U::Error>> {
        match action {
            RsnStaAction::DecryptMessage3KeyData { ticket, frame } => {
                let unwrapped = match unwrap
                    .unwrap_key_data(self.ptk()?.kek(), frame.key_frame().key_data())
                    .await
                {
                    Ok(unwrapped) => unwrapped,
                    Err(error) => {
                        self.state
                            .complete_key_data::<N>(ticket, frame, false)
                            .map_err(RsnStaSupplicantError::State)?;
                        return Err(RsnStaProcessError::KeyUnwrap(error));
                    }
                };
                let action = self
                    .state
                    .complete_key_data(ticket, frame, true)
                    .map_err(RsnStaSupplicantError::State)?;
                let RsnStaAction::InstallKeys { ticket, frame } = action else {
                    return Err(RsnStaSupplicantError::UnexpectedAction.into());
                };
                self.prepare_key_install(ticket, frame, Some(unwrapped.as_bytes()))
                    .map_err(Into::into)
            }
            RsnStaAction::InstallKeys { ticket, frame } => self
                .prepare_key_install(ticket, frame, None)
                .map_err(Into::into),
            _ => Err(RsnStaSupplicantError::UnexpectedAction.into()),
        }
    }

    fn prepare_key_install<const N: usize>(
        &mut self,
        ticket: crate::state::RsnTicket,
        frame: crate::OwnedEapolFrame<N>,
        plain_key_data: Option<&[u8]>,
    ) -> Result<RsnStaSupplicantAction<N>, RsnStaSupplicantError> {
        let key = frame.key_frame();
        let encrypted_key_data = key.key_info().encrypted_key_data();
        let replay_counter = key.replay_counter();
        let key_receive_sequence = *key.key_receive_sequence();
        let plain_key_data = plain_key_data.unwrap_or_else(|| key.key_data());
        let plain_key_data_len = plain_key_data.len();
        let gtk = match parse_gtk_key_data(
            plain_key_data,
            self.authenticator_security_ies.rsn_ie(),
            self.authenticator_security_ies.rsnxe(),
        ) {
            Ok(gtk) => gtk,
            Err(error) => {
                self.state
                    .complete_key_install::<N>(ticket, false)
                    .map_err(RsnStaSupplicantError::State)?;
                return Err(RsnStaSupplicantError::Frame(error));
            }
        };
        let pairwise = RsnKeyInstall::pairwise(
            RsnInterface::Station,
            *self.state.peer(),
            [0; 8],
            self.ptk()?,
        );
        let group = RsnKeyInstall::group(RsnInterface::Station, &gtk, key_receive_sequence);
        self.completed_message3 = Some(RsnCompletedMessage3::capture(key));
        Ok(RsnStaSupplicantAction::InstallKeys(
            RsnStaKeyInstallRequest {
                ticket,
                replay_counter,
                encrypted_key_data,
                plain_key_data_len,
                pairwise,
                group,
            },
        ))
    }

    fn build_transmit<const N: usize>(
        &self,
        transmit: RsnTransmit,
    ) -> Result<RsnStaSupplicantAction<N>, RsnStaSupplicantError> {
        let frame =
            build_sta_action_frame::<N, _>(&self.state, transmit, &self.association_security_ies)
                .map_err(RsnStaSupplicantError::Frame)?
                .authenticate(self.ptk()?);
        Ok(RsnStaSupplicantAction::Transmit(frame))
    }

    fn ptk(&self) -> Result<&Ptk, RsnStaSupplicantError> {
        self.ptk.as_ref().ok_or(RsnStaSupplicantError::MissingPtk)
    }
}

#[cfg(test)]
mod tests;
