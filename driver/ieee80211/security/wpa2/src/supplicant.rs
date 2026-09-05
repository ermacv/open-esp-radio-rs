//! Allocation-free WPA2-Personal supplicant orchestration.
//!
//! [`Wpa2StaState`] remains the protocol transition owner. This module joins
//! those transitions to PMK/PTK derivation, MIC verification, asynchronous
//! key-data unwrap, exact Association security IEs and typed key-install
//! requests. A target executor therefore handles only complete EAPOL RX/TX
//! frames and the platform-specific key-slot transaction.

use crate::{
    EapolKeyFrame, Pmk, Ptk, PtkContext, Wpa2Interface, Wpa2KeyConfirmationKey,
    Wpa2KeyEncryptionKey,
    aes::AsyncWpa2KeyUnwrap,
    frames::{
        OwnedAssociationSecurityIes, OwnedRsnIe, Wpa2FrameError, Wpa2TxFrame,
        build_sta_action_frame, parse_group_gtk_key_data, parse_gtk_key_data,
    },
    keys::Wpa2KeyInstall,
    state::{Wpa2StaAction, Wpa2StaPhase, Wpa2StaState, Wpa2StateError, Wpa2Transmit},
};
use zeroize::{Zeroize, ZeroizeOnDrop};

/// Hardware-qualified open-STA compatibility window for the first EAPOL-Key
/// message. The numeric value is retained from the pre-transfer HIL policy;
/// it is not claimed as a recovered vendor constant.
pub const WPA2_STA_MESSAGE1_TIMEOUT_MS: u32 = 3_000;

/// Complete window after the original Message 2 transmission.
///
/// SOURCE: complete `libwpa_supplicant.a[wpa.c.obj]::
/// wpa_supplicant_process_1_of_4` emits one Message 2 and does not register a
/// retransmission timeout. A repeated Message 1 instead re-enters through
/// `wpa_sm_rx_eapol` and produces another Message 2. The six-second value
/// preserves the two former three-second HIL receive windows without
/// inventing a station-originated Message 2 retry.
pub const WPA2_STA_MESSAGE3_TIMEOUT_MS: u32 = 6_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Wpa2StaResponseWait {
    Message1,
    Message3,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Wpa2StaDeadlineEvent {
    Pending,
    Expired {
        wait: Wpa2StaResponseWait,
        elapsed_ms: u32,
    },
}

/// Finite millisecond deadline owned independently from Embassy or a hardware
/// alarm. A target calls [`Self::finish_millisecond`] after servicing all RX
/// completions for that interval.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Wpa2StaResponseDeadline {
    wait: Wpa2StaResponseWait,
    elapsed_ms: u32,
}

impl Wpa2StaResponseDeadline {
    pub const fn new(wait: Wpa2StaResponseWait) -> Self {
        Self {
            wait,
            elapsed_ms: 0,
        }
    }

    pub const fn elapsed_ms(&self) -> u32 {
        self.elapsed_ms
    }

    pub fn finish_millisecond(&mut self) -> Wpa2StaDeadlineEvent {
        self.elapsed_ms = self.elapsed_ms.saturating_add(1);
        if self.elapsed_ms >= self.timeout_ms() {
            Wpa2StaDeadlineEvent::Expired {
                wait: self.wait,
                elapsed_ms: self.elapsed_ms,
            }
        } else {
            Wpa2StaDeadlineEvent::Pending
        }
    }

    const fn timeout_ms(&self) -> u32 {
        match self.wait {
            Wpa2StaResponseWait::Message1 => WPA2_STA_MESSAGE1_TIMEOUT_MS,
            Wpa2StaResponseWait::Message3 => WPA2_STA_MESSAGE3_TIMEOUT_MS,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Wpa2StaSupplicantError {
    State(Wpa2StateError),
    Frame(Wpa2FrameError),
    MissingPtk,
    InvalidMessage3Mic,
    UnexpectedAction,
}

impl Wpa2StaSupplicantError {
    /// Rejects caused by the current unauthenticated peer frame are
    /// recoverable within the existing response window. Local state,
    /// cryptographic ownership and frame-construction failures are not.
    pub const fn is_peer_input_rejection(self) -> bool {
        match self {
            Self::State(error) => error.is_peer_input_rejection(),
            Self::InvalidMessage3Mic => true,
            Self::Frame(_) | Self::MissingPtk | Self::UnexpectedAction => false,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Wpa2StaProcessError<E> {
    Supplicant(Wpa2StaSupplicantError),
    KeyUnwrap(E),
}

impl<E> Wpa2StaProcessError<E> {
    pub const fn is_peer_input_rejection(&self) -> bool {
        match self {
            Self::Supplicant(error) => error.is_peer_input_rejection(),
            Self::KeyUnwrap(_) => false,
        }
    }
}

impl<E> From<Wpa2StaSupplicantError> for Wpa2StaProcessError<E> {
    fn from(error: Wpa2StaSupplicantError) -> Self {
        Self::Supplicant(error)
    }
}

/// Complete pairwise/group installation transaction prepared from Message 3.
///
/// Both keys are owned and zeroized on drop. The private state ticket makes a
/// completion valid only for the exact Message 3 that produced this request.
pub struct Wpa2StaKeyInstallRequest {
    ticket: crate::state::Wpa2Ticket,
    replay_counter: u64,
    encrypted_key_data: bool,
    plain_key_data_len: usize,
    pairwise: Wpa2KeyInstall,
    group: Wpa2KeyInstall,
}

impl Wpa2StaKeyInstallRequest {
    pub const fn replay_counter(&self) -> u64 {
        self.replay_counter
    }

    pub const fn encrypted_key_data(&self) -> bool {
        self.encrypted_key_data
    }

    pub const fn plain_key_data_len(&self) -> usize {
        self.plain_key_data_len
    }

    pub const fn pairwise(&self) -> &Wpa2KeyInstall {
        &self.pairwise
    }

    pub const fn group(&self) -> &Wpa2KeyInstall {
        &self.group
    }
}

pub enum Wpa2StaSupplicantAction<const N: usize> {
    None,
    Transmit(Wpa2TxFrame<N>),
    InstallKeys(Wpa2StaKeyInstallRequest),
    Deauthenticate,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Wpa2ConnectedSupplicantError {
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
    Frame(Wpa2FrameError),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Wpa2ConnectedProcessError<E> {
    Supplicant(Wpa2ConnectedSupplicantError),
    KeyUnwrap(E),
}

/// Exact connected-state GTK publication ticket.
pub struct Wpa2GroupKeyInstallRequest<const N: usize> {
    ticket: u32,
    replay_counter: u64,
    commitment: Wpa2CompletedGroupMessage1,
    group: Wpa2KeyInstall,
    response: Wpa2TxFrame<N>,
}

impl<const N: usize> Wpa2GroupKeyInstallRequest<N> {
    pub const fn replay_counter(&self) -> u64 {
        self.replay_counter
    }

    pub const fn group(&self) -> &Wpa2KeyInstall {
        &self.group
    }
}

pub enum Wpa2ConnectedAction<const N: usize> {
    InstallGroupKey(Wpa2GroupKeyInstallRequest<N>),
    Retransmit(Wpa2TxFrame<N>),
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
struct Wpa2CompletedMessage3 {
    protocol_version: u8,
    key_info: u16,
    replay_counter: u64,
    authenticator_nonce_tag: u32,
    mic: [u8; 16],
}

impl Wpa2CompletedMessage3 {
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
        frame: EapolKeyFrame<'_>,
    ) -> Result<(), Wpa2ConnectedSupplicantError> {
        if frame.protocol_version() != self.protocol_version {
            return Err(Wpa2ConnectedSupplicantError::ProtocolVersionMismatch);
        }
        if frame.key_info().descriptor_version() != 2 {
            return Err(Wpa2ConnectedSupplicantError::UnsupportedDescriptorVersion);
        }
        if frame.message() != crate::EapolKeyMessage::PairwiseMessage3
            || frame.key_info().raw() != self.key_info
        {
            return Err(Wpa2ConnectedSupplicantError::UnsupportedMessage);
        }
        if frame.replay_counter() != self.replay_counter {
            return Err(Wpa2ConnectedSupplicantError::ReplayCounterMismatch);
        }
        // This compact tag is only a diagnostic prefilter over public ANonce;
        // the verified retained MIC below remains the cryptographic authority
        // and catches tag collisions as a completed-frame mismatch.
        if message3_nonce_tag(frame.nonce()) != self.authenticator_nonce_tag {
            return Err(Wpa2ConnectedSupplicantError::AuthenticatorNonceMismatch);
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
struct Wpa2CompletedGroupMessage1 {
    mic: [u8; 16],
}

impl Wpa2CompletedGroupMessage1 {
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
pub struct Wpa2ConnectedSupplicant {
    authenticator: [u8; 6],
    key_confirmation: Wpa2KeyConfirmationKey,
    key_encryption: Wpa2KeyEncryptionKey,
    completed_message3: Wpa2CompletedMessage3,
    replay_counter: u64,
    completed_group_message1: Option<Wpa2CompletedGroupMessage1>,
    pending: Option<(u32, u64)>,
    next_ticket: u32,
}

impl Wpa2ConnectedSupplicant {
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
    ) -> Result<Wpa2TxFrame<N>, Wpa2ConnectedSupplicantError> {
        if frame.interface() != Wpa2Interface::Station {
            return Err(Wpa2ConnectedSupplicantError::WrongInterface);
        }
        if frame.peer() != &self.authenticator {
            return Err(Wpa2ConnectedSupplicantError::WrongPeer);
        }
        let key = frame.key_frame();
        self.completed_message3.validate_fields(key)?;
        if !key.verify_mic_with_confirmation_key(&self.key_confirmation) {
            return Err(Wpa2ConnectedSupplicantError::InvalidMic);
        }
        if key.mic() != &self.completed_message3.mic {
            return Err(Wpa2ConnectedSupplicantError::RetainedMessage3Mismatch);
        }
        Wpa2TxFrame::message4(self.authenticator, self.completed_message3.replay_counter)
            .map_err(Wpa2ConnectedSupplicantError::Frame)
            .map(|frame| {
                frame
                    .authenticate_with_confirmation_key(&self.key_confirmation)
                    .mark_retransmission()
            })
    }

    pub async fn on_group_message1<const N: usize, U: AsyncWpa2KeyUnwrap>(
        &mut self,
        frame: crate::OwnedEapolFrame<N>,
        unwrap: &mut U,
    ) -> Result<Wpa2ConnectedAction<N>, Wpa2ConnectedProcessError<U::Error>> {
        if self.pending.is_some() {
            return Err(Wpa2ConnectedProcessError::Supplicant(
                Wpa2ConnectedSupplicantError::Busy,
            ));
        }
        if frame.interface() != Wpa2Interface::Station {
            return Err(Wpa2ConnectedProcessError::Supplicant(
                Wpa2ConnectedSupplicantError::WrongInterface,
            ));
        }
        if frame.peer() != &self.authenticator {
            return Err(Wpa2ConnectedProcessError::Supplicant(
                Wpa2ConnectedSupplicantError::WrongPeer,
            ));
        }
        let key = frame.key_frame();
        if key.key_info().descriptor_version() != 2 {
            return Err(Wpa2ConnectedProcessError::Supplicant(
                Wpa2ConnectedSupplicantError::UnsupportedDescriptorVersion,
            ));
        }
        if key.message() != crate::EapolKeyMessage::GroupMessage1 {
            return Err(Wpa2ConnectedProcessError::Supplicant(
                Wpa2ConnectedSupplicantError::UnsupportedMessage,
            ));
        }
        let replay_counter = key.replay_counter();
        if replay_counter < self.replay_counter
            || (replay_counter == self.replay_counter && self.completed_group_message1.is_none())
        {
            return Err(Wpa2ConnectedProcessError::Supplicant(
                Wpa2ConnectedSupplicantError::StaleReplayCounter,
            ));
        }
        // A cached Group Message 2 is still an authenticated response. Verify
        // every repeated Group Message 1 before admitting the idempotent path,
        // otherwise a forged frame can elicit a valid MIC-bearing response.
        if !key.verify_mic_with_confirmation_key(&self.key_confirmation) {
            return Err(Wpa2ConnectedProcessError::Supplicant(
                Wpa2ConnectedSupplicantError::InvalidMic,
            ));
        }
        if let Some(completed) = self
            .completed_group_message1
            .as_ref()
            .filter(|_| replay_counter == self.replay_counter)
        {
            if !completed.matches(key) {
                return Err(Wpa2ConnectedProcessError::Supplicant(
                    Wpa2ConnectedSupplicantError::RetainedGroupMessage1Mismatch,
                ));
            }
            let response = Wpa2TxFrame::group_message2(self.authenticator, replay_counter)
                .map_err(|error| {
                    Wpa2ConnectedProcessError::Supplicant(Wpa2ConnectedSupplicantError::Frame(
                        error,
                    ))
                })?
                .authenticate_with_confirmation_key(&self.key_confirmation);
            return Ok(Wpa2ConnectedAction::Retransmit(response));
        }
        if !key.key_info().encrypted_key_data() || key.key_data().is_empty() {
            return Err(Wpa2ConnectedProcessError::Supplicant(
                Wpa2ConnectedSupplicantError::MissingEncryptedKeyData,
            ));
        }
        let plain = unwrap
            .unwrap_key_data(self.key_encryption.as_bytes(), key.key_data())
            .await
            .map_err(Wpa2ConnectedProcessError::KeyUnwrap)?;
        let gtk = parse_group_gtk_key_data(plain.as_bytes()).map_err(|error| {
            Wpa2ConnectedProcessError::Supplicant(Wpa2ConnectedSupplicantError::Frame(error))
        })?;
        let group =
            Wpa2KeyInstall::group(Wpa2Interface::Station, &gtk, *key.key_receive_sequence());
        let response = Wpa2TxFrame::group_message2(self.authenticator, replay_counter)
            .map_err(|error| {
                Wpa2ConnectedProcessError::Supplicant(Wpa2ConnectedSupplicantError::Frame(error))
            })?
            .authenticate_with_confirmation_key(&self.key_confirmation);
        let ticket = self.next_ticket;
        self.next_ticket = self.next_ticket.wrapping_add(1).max(1);
        self.pending = Some((ticket, replay_counter));
        Ok(Wpa2ConnectedAction::InstallGroupKey(
            Wpa2GroupKeyInstallRequest {
                ticket,
                replay_counter,
                commitment: Wpa2CompletedGroupMessage1::capture(key),
                group,
                response,
            },
        ))
    }

    pub fn complete_group_key_install<const N: usize>(
        &mut self,
        request: Wpa2GroupKeyInstallRequest<N>,
        installed: bool,
    ) -> Result<Wpa2TxFrame<N>, Wpa2ConnectedSupplicantError> {
        if self.pending != Some((request.ticket, request.replay_counter)) {
            return Err(Wpa2ConnectedSupplicantError::StaleCompletion);
        }
        self.pending = None;
        if !installed {
            return Err(Wpa2ConnectedSupplicantError::InstallFailed);
        }
        self.replay_counter = request.replay_counter;
        self.completed_group_message1 = Some(request.commitment);
        Ok(request.response)
    }
}

/// One WPA2-Personal station handshake with owned cryptographic state.
pub struct Wpa2StaSupplicant {
    state: Wpa2StaState,
    ptk: Option<Ptk>,
    completed_message3: Option<Wpa2CompletedMessage3>,
    association_security_ies: OwnedAssociationSecurityIes,
    authenticator_security_ies: OwnedAssociationSecurityIes,
}

impl Wpa2StaSupplicant {
    pub fn try_new(
        local: [u8; 6],
        authenticator: [u8; 6],
        supplicant_nonce: [u8; 32],
        association_security_ies: &[u8],
        authenticator_rsn_ie: &[u8],
        authenticator_rsnxe: &[u8],
    ) -> Result<Self, Wpa2StaSupplicantError> {
        let state = Wpa2StaState::new(local, authenticator, supplicant_nonce)
            .map_err(Wpa2StaSupplicantError::State)?;
        let association_security_ies =
            OwnedAssociationSecurityIes::try_copy_bytes(association_security_ies)
                .map_err(Wpa2StaSupplicantError::Frame)?;
        let authenticator_rsn_ie =
            OwnedRsnIe::<{ crate::frames::WPA2_RSN_IE_CAPACITY }>::try_copy(authenticator_rsn_ie)
                .map_err(Wpa2StaSupplicantError::Frame)?;
        let authenticator_security_ies =
            OwnedAssociationSecurityIes::try_copy(&authenticator_rsn_ie, authenticator_rsnxe)
                .map_err(Wpa2StaSupplicantError::Frame)?;
        Ok(Self {
            state,
            ptk: None,
            completed_message3: None,
            association_security_ies,
            authenticator_security_ies,
        })
    }

    pub const fn phase(&self) -> Wpa2StaPhase {
        self.state.phase()
    }

    pub fn into_connected(self) -> Result<Wpa2ConnectedSupplicant, Wpa2StaSupplicantError> {
        let replay_counter = self
            .state
            .completed_replay_counter()
            .ok_or(Wpa2StaSupplicantError::UnexpectedAction)?;
        let ptk = self.ptk.ok_or(Wpa2StaSupplicantError::MissingPtk)?;
        let completed_message3 = self
            .completed_message3
            .ok_or(Wpa2StaSupplicantError::UnexpectedAction)?;
        let (key_confirmation, key_encryption) = ptk.into_connected_keys();
        Ok(Wpa2ConnectedSupplicant {
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
    pub async fn on_frame<const N: usize, U: AsyncWpa2KeyUnwrap>(
        &mut self,
        frame: crate::OwnedEapolFrame<N>,
        pmk: &Pmk,
        unwrap: &mut U,
    ) -> Result<Wpa2StaSupplicantAction<N>, Wpa2StaProcessError<U::Error>> {
        let action = self
            .state
            .on_frame(frame)
            .map_err(Wpa2StaSupplicantError::State)?;
        match action {
            Wpa2StaAction::None => Ok(Wpa2StaSupplicantAction::None),
            Wpa2StaAction::DerivePtk { ticket, context } => {
                self.completed_message3 = None;
                self.ptk = Some(pmk.derive_ptk(PtkContext {
                    authenticator_address: context.authenticator_address,
                    supplicant_address: context.supplicant_address,
                    authenticator_nonce: context.authenticator_nonce,
                    supplicant_nonce: context.supplicant_nonce,
                }));
                let action = self
                    .state
                    .complete_ptk::<N>(ticket, true)
                    .map_err(Wpa2StaSupplicantError::State)?;
                let Wpa2StaAction::Transmit(transmit) = action else {
                    return Err(Wpa2StaSupplicantError::UnexpectedAction.into());
                };
                self.build_transmit(transmit).map_err(Into::into)
            }
            Wpa2StaAction::Transmit(transmit) => self.build_transmit(transmit).map_err(Into::into),
            Wpa2StaAction::VerifyMessage3Mic { ticket, frame } => {
                if !frame.key_frame().verify_mic(self.ptk()?) {
                    self.state
                        .complete_message3_mic::<N>(ticket, frame, false)
                        .map_err(Wpa2StaSupplicantError::State)?;
                    return Err(Wpa2StaSupplicantError::InvalidMessage3Mic.into());
                }
                let action = self
                    .state
                    .complete_message3_mic(ticket, frame, true)
                    .map_err(Wpa2StaSupplicantError::State)?;
                self.resolve_message3(action, unwrap).await
            }
            Wpa2StaAction::Deauthenticate => Ok(Wpa2StaSupplicantAction::Deauthenticate),
            Wpa2StaAction::DecryptMessage3KeyData { .. } | Wpa2StaAction::InstallKeys { .. } => {
                Err(Wpa2StaSupplicantError::UnexpectedAction.into())
            }
        }
    }

    /// Complete the exact platform key-slot transaction represented by
    /// `request`. A successful completion produces an already authenticated
    /// Message 4; a failed completion moves the protocol state to Failed.
    pub fn complete_key_install<const N: usize>(
        &mut self,
        request: Wpa2StaKeyInstallRequest,
        installed: bool,
    ) -> Result<Wpa2StaSupplicantAction<N>, Wpa2StaSupplicantError> {
        let action = self
            .state
            .complete_key_install::<N>(request.ticket, installed)
            .map_err(Wpa2StaSupplicantError::State)?;
        match action {
            Wpa2StaAction::Transmit(transmit) => self.build_transmit(transmit),
            Wpa2StaAction::Deauthenticate => Ok(Wpa2StaSupplicantAction::Deauthenticate),
            _ => Err(Wpa2StaSupplicantError::UnexpectedAction),
        }
    }

    async fn resolve_message3<const N: usize, U: AsyncWpa2KeyUnwrap>(
        &mut self,
        action: Wpa2StaAction<N>,
        unwrap: &mut U,
    ) -> Result<Wpa2StaSupplicantAction<N>, Wpa2StaProcessError<U::Error>> {
        match action {
            Wpa2StaAction::DecryptMessage3KeyData { ticket, frame } => {
                let unwrapped = match unwrap
                    .unwrap_key_data(self.ptk()?.kek(), frame.key_frame().key_data())
                    .await
                {
                    Ok(unwrapped) => unwrapped,
                    Err(error) => {
                        self.state
                            .complete_key_data::<N>(ticket, frame, false)
                            .map_err(Wpa2StaSupplicantError::State)?;
                        return Err(Wpa2StaProcessError::KeyUnwrap(error));
                    }
                };
                let action = self
                    .state
                    .complete_key_data(ticket, frame, true)
                    .map_err(Wpa2StaSupplicantError::State)?;
                let Wpa2StaAction::InstallKeys { ticket, frame } = action else {
                    return Err(Wpa2StaSupplicantError::UnexpectedAction.into());
                };
                self.prepare_key_install(ticket, frame, Some(unwrapped.as_bytes()))
                    .map_err(Into::into)
            }
            Wpa2StaAction::InstallKeys { ticket, frame } => self
                .prepare_key_install(ticket, frame, None)
                .map_err(Into::into),
            _ => Err(Wpa2StaSupplicantError::UnexpectedAction.into()),
        }
    }

    fn prepare_key_install<const N: usize>(
        &mut self,
        ticket: crate::state::Wpa2Ticket,
        frame: crate::OwnedEapolFrame<N>,
        plain_key_data: Option<&[u8]>,
    ) -> Result<Wpa2StaSupplicantAction<N>, Wpa2StaSupplicantError> {
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
                    .map_err(Wpa2StaSupplicantError::State)?;
                return Err(Wpa2StaSupplicantError::Frame(error));
            }
        };
        let pairwise = Wpa2KeyInstall::pairwise(
            Wpa2Interface::Station,
            *self.state.peer(),
            [0; 8],
            self.ptk()?,
        );
        let group = Wpa2KeyInstall::group(Wpa2Interface::Station, &gtk, key_receive_sequence);
        self.completed_message3 = Some(Wpa2CompletedMessage3::capture(key));
        Ok(Wpa2StaSupplicantAction::InstallKeys(
            Wpa2StaKeyInstallRequest {
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
        transmit: Wpa2Transmit,
    ) -> Result<Wpa2StaSupplicantAction<N>, Wpa2StaSupplicantError> {
        let frame =
            build_sta_action_frame::<N, _>(&self.state, transmit, &self.association_security_ies)
                .map_err(Wpa2StaSupplicantError::Frame)?
                .authenticate(self.ptk()?);
        Ok(Wpa2StaSupplicantAction::Transmit(frame))
    }

    fn ptk(&self) -> Result<&Ptk, Wpa2StaSupplicantError> {
        self.ptk.as_ref().ok_or(Wpa2StaSupplicantError::MissingPtk)
    }
}

#[cfg(test)]
mod tests;
