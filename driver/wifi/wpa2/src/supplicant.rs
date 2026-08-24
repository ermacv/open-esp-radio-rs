//! Allocation-free WPA2-Personal supplicant orchestration.
//!
//! [`Wpa2StaState`] remains the protocol transition owner. This module joins
//! those transitions to PMK/PTK derivation, MIC verification, asynchronous
//! key-data unwrap, exact Association security IEs and typed key-install
//! requests. A target executor therefore handles only complete EAPOL RX/TX
//! frames and the platform-specific key-slot transaction.

use crate::{
    Pmk, Ptk, PtkContext, Wpa2Interface,
    aes::AsyncWpa2KeyUnwrap,
    frames::{
        OwnedAssociationSecurityIes, OwnedRsnIe, Wpa2FrameError, Wpa2TxFrame,
        build_sta_action_frame, parse_group_gtk_key_data, parse_gtk_key_data,
    },
    keys::Wpa2KeyInstall,
    state::{Wpa2StaAction, Wpa2StaPhase, Wpa2StaState, Wpa2StateError, Wpa2Transmit},
};

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
    UnsupportedDescriptorVersion,
    UnsupportedMessage,
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

/// WPA2 state which must survive for the complete connected association.
///
/// It retains only PTK-derived authentication material and replay state. PMK
/// and association retry policy remain owned by the outer station lifecycle.
pub struct Wpa2ConnectedSupplicant {
    authenticator: [u8; 6],
    ptk: Ptk,
    replay_counter: u64,
    last_group_replay: Option<u64>,
    pending: Option<(u32, u64)>,
    next_ticket: u32,
}

impl Wpa2ConnectedSupplicant {
    pub const fn replay_counter(&self) -> u64 {
        self.replay_counter
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
            || (replay_counter == self.replay_counter
                && self.last_group_replay != Some(replay_counter))
        {
            return Err(Wpa2ConnectedProcessError::Supplicant(
                Wpa2ConnectedSupplicantError::StaleReplayCounter,
            ));
        }
        // A cached Group Message 2 is still an authenticated response. Verify
        // every repeated Group Message 1 before admitting the idempotent path,
        // otherwise a forged frame can elicit a valid MIC-bearing response.
        if !key.verify_mic(&self.ptk) {
            return Err(Wpa2ConnectedProcessError::Supplicant(
                Wpa2ConnectedSupplicantError::InvalidMic,
            ));
        }
        if self.last_group_replay == Some(replay_counter) {
            let response = Wpa2TxFrame::group_message2(self.authenticator, replay_counter)
                .map_err(|error| {
                    Wpa2ConnectedProcessError::Supplicant(Wpa2ConnectedSupplicantError::Frame(
                        error,
                    ))
                })?
                .authenticate(&self.ptk);
            return Ok(Wpa2ConnectedAction::Retransmit(response));
        }
        if !key.key_info().encrypted_key_data() || key.key_data().is_empty() {
            return Err(Wpa2ConnectedProcessError::Supplicant(
                Wpa2ConnectedSupplicantError::MissingEncryptedKeyData,
            ));
        }
        let plain = unwrap
            .unwrap_key_data(self.ptk.kek(), key.key_data())
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
            .authenticate(&self.ptk);
        let ticket = self.next_ticket;
        self.next_ticket = self.next_ticket.wrapping_add(1).max(1);
        self.pending = Some((ticket, replay_counter));
        Ok(Wpa2ConnectedAction::InstallGroupKey(
            Wpa2GroupKeyInstallRequest {
                ticket,
                replay_counter,
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
        self.last_group_replay = Some(request.replay_counter);
        Ok(request.response)
    }
}

/// One WPA2-Personal station handshake with owned cryptographic state.
pub struct Wpa2StaSupplicant {
    state: Wpa2StaState,
    ptk: Option<Ptk>,
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
        Ok(Wpa2ConnectedSupplicant {
            authenticator: *self.state.peer(),
            ptk,
            replay_counter,
            last_group_replay: None,
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
mod tests {
    use std::{
        boxed::Box,
        future::Future,
        task::{Context, Poll, Waker},
    };

    use super::*;
    use crate::{
        aes::{Wpa2SoftwareAes, Wpa2UnwrappedKeyData, software_aes128_key_wrap},
        frames::{Wpa2Gtk, Wpa2PlainKeyData},
        keys::Wpa2KeyKind,
    };

    const LOCAL: [u8; 6] = [1; 6];
    const AP: [u8; 6] = [2; 6];
    const SNONCE: [u8; 32] = [3; 32];
    const ANONCE: [u8; 32] = [4; 32];
    const RSN: [u8; 22] = [
        0x30, 20, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 2, 0, 0,
    ];

    fn block_on<F: Future>(future: F) -> F::Output {
        let mut context = Context::from_waker(Waker::noop());
        let mut future = Box::pin(future);
        loop {
            match future.as_mut().poll(&mut context) {
                Poll::Ready(output) => return output,
                Poll::Pending => std::thread::yield_now(),
            }
        }
    }

    fn owned(frame: &Wpa2TxFrame<512>) -> crate::OwnedEapolFrame<512> {
        crate::OwnedEapolFrame::try_copy(Wpa2Interface::Station, AP, frame.as_bytes()).unwrap()
    }

    fn context() -> PtkContext {
        PtkContext {
            authenticator_address: AP,
            supplicant_address: LOCAL,
            authenticator_nonce: ANONCE,
            supplicant_nonce: SNONCE,
        }
    }

    fn encrypted_message3(
        ptk: &Ptk,
        key_rsc: [u8; 8],
        plain_key_data: &[u8],
    ) -> crate::OwnedEapolFrame<512> {
        let wrapped = software_aes128_key_wrap(ptk.kek(), plain_key_data).unwrap();
        let frame = Wpa2TxFrame::<512>::message3(LOCAL, 2, ANONCE, key_rsc, wrapped.as_bytes())
            .unwrap()
            .authenticate(ptk);
        owned(&frame)
    }

    struct GroupKeyUnwrap {
        plain: [u8; 24],
    }

    impl AsyncWpa2KeyUnwrap for GroupKeyUnwrap {
        type Error = ();

        async fn unwrap_key_data(
            &mut self,
            _kek: &[u8; 16],
            _encrypted: &[u8],
        ) -> Result<Wpa2UnwrappedKeyData, Self::Error> {
            Ok(Wpa2UnwrappedKeyData::try_copy(&self.plain).unwrap())
        }
    }

    fn connected(ptk: Ptk) -> Wpa2ConnectedSupplicant {
        Wpa2ConnectedSupplicant {
            authenticator: AP,
            ptk,
            replay_counter: 2,
            last_group_replay: None,
            pending: None,
            next_ticket: 1,
        }
    }

    fn group_kde(key_id: u8, key: u8) -> [u8; 24] {
        let mut kde = [0; 24];
        kde[..8].copy_from_slice(&[0xdd, 22, 0, 0x0f, 0xac, 1, key_id, 0]);
        kde[8..].fill(key);
        kde
    }

    #[test]
    fn resolves_m1_through_typed_install_and_authenticated_m4() {
        let pmk = Pmk::derive(b"password", b"ssid").unwrap();
        let expected_ptk = pmk.derive_ptk(context());
        let mut supplicant =
            Wpa2StaSupplicant::try_new(LOCAL, AP, SNONCE, &RSN, &RSN, &[]).unwrap();
        let mut aes = Wpa2SoftwareAes::new();

        let message1 = Wpa2TxFrame::<512>::message1(LOCAL, 1, ANONCE).unwrap();
        let Wpa2StaSupplicantAction::Transmit(message2) =
            block_on(supplicant.on_frame(owned(&message1), &pmk, &mut aes)).unwrap()
        else {
            panic!("Message 1 must produce Message 2")
        };
        assert_eq!(message2.key_frame().replay_counter(), 1);
        assert!(message2.key_frame().verify_mic(&expected_ptk));

        let rsn = OwnedRsnIe::<64>::try_copy(&RSN).unwrap();
        let gtk = Wpa2Gtk::new(2, false, [0x5a; 16]).unwrap();
        let plain = Wpa2PlainKeyData::<64>::build(&rsn, &gtk).unwrap();
        let rsc = [7, 6, 5, 4, 3, 2, 1, 0];
        let message3 = encrypted_message3(&expected_ptk, rsc, plain.as_bytes());
        let Wpa2StaSupplicantAction::InstallKeys(request) =
            block_on(supplicant.on_frame(message3, &pmk, &mut aes)).unwrap()
        else {
            panic!("Message 3 must produce one key transaction")
        };
        assert_eq!(request.replay_counter(), 2);
        assert!(request.encrypted_key_data());
        assert_eq!(request.plain_key_data_len(), plain.as_bytes().len());
        assert_eq!(request.pairwise().peer(), &AP);
        assert_eq!(
            request.pairwise().key().as_bytes(),
            expected_ptk.temporal_key()
        );
        assert_eq!(
            request.group().kind(),
            Wpa2KeyKind::Group {
                key_id: 2,
                transmit: false,
            }
        );
        assert_eq!(request.group().receive_sequence(), &rsc);
        assert_eq!(request.group().key().as_bytes(), &[0x5a; 16]);

        let Wpa2StaSupplicantAction::Transmit(message4) = supplicant
            .complete_key_install::<512>(request, true)
            .unwrap()
        else {
            panic!("installed keys must produce Message 4")
        };
        assert_eq!(supplicant.phase(), Wpa2StaPhase::Completed);
        assert_eq!(message4.key_frame().replay_counter(), 2);
        assert!(message4.key_frame().verify_mic(&expected_ptk));
    }

    #[test]
    fn invalid_message3_mic_is_ignored_and_valid_retry_can_install() {
        let pmk = Pmk::derive(b"password", b"ssid").unwrap();
        let expected_ptk = pmk.derive_ptk(context());
        let mut supplicant =
            Wpa2StaSupplicant::try_new(LOCAL, AP, SNONCE, &RSN, &RSN, &[]).unwrap();
        let mut aes = Wpa2SoftwareAes::new();
        let message1 = Wpa2TxFrame::<512>::message1(LOCAL, 1, ANONCE).unwrap();
        block_on(supplicant.on_frame(owned(&message1), &pmk, &mut aes)).unwrap();
        let rsn = OwnedRsnIe::<64>::try_copy(&RSN).unwrap();
        let gtk = Wpa2Gtk::new(1, false, [9; 16]).unwrap();
        let plain = Wpa2PlainKeyData::<64>::build(&rsn, &gtk).unwrap();
        let message3 = encrypted_message3(&expected_ptk, [0; 8], plain.as_bytes());
        let mut bytes = [0; 512];
        let len = message3.as_bytes().len();
        bytes[..len].copy_from_slice(message3.as_bytes());
        bytes[81] ^= 1;
        let changed =
            crate::OwnedEapolFrame::<512>::try_copy(Wpa2Interface::Station, AP, &bytes[..len])
                .unwrap();
        assert!(matches!(
            block_on(supplicant.on_frame(changed, &pmk, &mut aes)),
            Err(Wpa2StaProcessError::Supplicant(
                Wpa2StaSupplicantError::InvalidMessage3Mic
            ))
        ));
        assert_eq!(supplicant.phase(), Wpa2StaPhase::AwaitingMessage3);

        let valid_retry = encrypted_message3(&expected_ptk, [0; 8], plain.as_bytes());
        let Wpa2StaSupplicantAction::InstallKeys(request) =
            block_on(supplicant.on_frame(valid_retry, &pmk, &mut aes)).unwrap()
        else {
            panic!("valid M3 retry must retain the original join")
        };
        assert_eq!(request.replay_counter(), 2);
        assert_eq!(supplicant.phase(), Wpa2StaPhase::InstallingKeys);
    }

    #[test]
    fn connected_group_rekey_installs_once_and_retransmits_idempotently() {
        let pmk = Pmk::derive(b"password", b"ssid").unwrap();
        let ptk = pmk.derive_ptk(context());
        let frame = Wpa2TxFrame::<512>::group_message1(LOCAL, 3, [9; 8], &[0x55; 24])
            .unwrap()
            .authenticate(&ptk);
        let mut connected = connected(pmk.derive_ptk(context()));
        let mut unwrap = GroupKeyUnwrap {
            plain: group_kde(1, 0x6a),
        };
        let Wpa2ConnectedAction::InstallGroupKey(request) =
            block_on(connected.on_group_message1(owned(&frame), &mut unwrap)).unwrap()
        else {
            panic!("new Group Message 1 must request one GTK replacement")
        };
        assert_eq!(request.replay_counter(), 3);
        assert_eq!(
            request.group().kind(),
            Wpa2KeyKind::Group {
                key_id: 1,
                transmit: false,
            }
        );
        assert_eq!(request.group().key().as_bytes(), &[0x6a; 16]);
        let response = connected.complete_group_key_install(request, true).unwrap();
        assert_eq!(
            response.key_frame().message(),
            crate::EapolKeyMessage::GroupMessage2
        );
        assert!(response.key_frame().verify_mic(&ptk));
        assert_eq!(connected.replay_counter(), 3);

        let Wpa2ConnectedAction::Retransmit(repeated) =
            block_on(connected.on_group_message1(owned(&frame), &mut unwrap)).unwrap()
        else {
            panic!("repeated Group Message 1 must not reinstall GTK")
        };
        assert!(repeated.key_frame().verify_mic(&ptk));
    }

    #[test]
    fn connected_group_rekey_authenticates_duplicate_before_cached_response() {
        let pmk = Pmk::derive(b"password", b"ssid").unwrap();
        let ptk = pmk.derive_ptk(context());
        let frame = Wpa2TxFrame::<512>::group_message1(LOCAL, 3, [9; 8], &[0x55; 24])
            .unwrap()
            .authenticate(&ptk);
        let mut connected = connected(pmk.derive_ptk(context()));
        let mut unwrap = GroupKeyUnwrap {
            plain: group_kde(1, 0x6a),
        };
        let Wpa2ConnectedAction::InstallGroupKey(request) =
            block_on(connected.on_group_message1(owned(&frame), &mut unwrap)).unwrap()
        else {
            panic!("new Group Message 1 must request one GTK replacement")
        };
        connected.complete_group_key_install(request, true).unwrap();

        let mut bytes = [0; 512];
        let length = frame.as_bytes().len();
        bytes[..length].copy_from_slice(frame.as_bytes());
        bytes[81] ^= 1;
        let forged_duplicate =
            crate::OwnedEapolFrame::<512>::try_copy(Wpa2Interface::Station, AP, &bytes[..length])
                .unwrap();
        assert!(matches!(
            block_on(connected.on_group_message1(forged_duplicate, &mut unwrap)),
            Err(Wpa2ConnectedProcessError::Supplicant(
                Wpa2ConnectedSupplicantError::InvalidMic
            ))
        ));
    }

    #[test]
    fn connected_group_rekey_rejects_bad_mic_and_stale_replay() {
        let pmk = Pmk::derive(b"password", b"ssid").unwrap();
        let ptk = pmk.derive_ptk(context());
        let valid = Wpa2TxFrame::<512>::group_message1(LOCAL, 3, [0; 8], &[0x55; 24])
            .unwrap()
            .authenticate(&ptk);
        let mut bytes = [0; 512];
        bytes[..valid.as_bytes().len()].copy_from_slice(valid.as_bytes());
        bytes[81] ^= 1;
        let invalid = crate::OwnedEapolFrame::<512>::try_copy(
            Wpa2Interface::Station,
            AP,
            &bytes[..valid.as_bytes().len()],
        )
        .unwrap();
        let mut connected = connected(pmk.derive_ptk(context()));
        let mut unwrap = GroupKeyUnwrap {
            plain: group_kde(1, 0x6a),
        };
        assert!(matches!(
            block_on(connected.on_group_message1(invalid, &mut unwrap)),
            Err(Wpa2ConnectedProcessError::Supplicant(
                Wpa2ConnectedSupplicantError::InvalidMic
            ))
        ));

        let stale = Wpa2TxFrame::<512>::group_message1(LOCAL, 1, [0; 8], &[0x55; 24])
            .unwrap()
            .authenticate(&ptk);
        assert!(matches!(
            block_on(connected.on_group_message1(owned(&stale), &mut unwrap)),
            Err(Wpa2ConnectedProcessError::Supplicant(
                Wpa2ConnectedSupplicantError::StaleReplayCounter
            ))
        ));
    }

    #[test]
    fn response_deadlines_preserve_total_wait_without_spontaneous_m2_retry() {
        let mut message1 = Wpa2StaResponseDeadline::new(Wpa2StaResponseWait::Message1);
        for elapsed in 1..WPA2_STA_MESSAGE1_TIMEOUT_MS {
            assert_eq!(message1.finish_millisecond(), Wpa2StaDeadlineEvent::Pending);
            assert_eq!(message1.elapsed_ms(), elapsed);
        }
        assert_eq!(
            message1.finish_millisecond(),
            Wpa2StaDeadlineEvent::Expired {
                wait: Wpa2StaResponseWait::Message1,
                elapsed_ms: WPA2_STA_MESSAGE1_TIMEOUT_MS,
            }
        );

        let mut message3 = Wpa2StaResponseDeadline::new(Wpa2StaResponseWait::Message3);
        for _ in 1..WPA2_STA_MESSAGE3_TIMEOUT_MS {
            assert_eq!(message3.finish_millisecond(), Wpa2StaDeadlineEvent::Pending);
        }
        assert_eq!(
            message3.finish_millisecond(),
            Wpa2StaDeadlineEvent::Expired {
                wait: Wpa2StaResponseWait::Message3,
                elapsed_ms: WPA2_STA_MESSAGE3_TIMEOUT_MS,
            }
        );
    }
}
