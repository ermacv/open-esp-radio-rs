//! Fixed WPA2-Personal four-way-handshake control flow.
//!
//! The state machines never execute cryptography, transmit a frame, install a
//! key, allocate, retry, or wait. Each method consumes one completion/event
//! and returns at most one owned action for the radio executor.

use crate::{DEFAULT_EAPOL_FRAME_CAPACITY, EapolKeyMessage, OwnedEapolFrame, Wpa2Interface};

pub const WPA2_NONCE_LEN: usize = 32;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Wpa2Ticket(u32);

impl Wpa2Ticket {
    pub const fn raw(self) -> u32 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PtkContext {
    pub authenticator_address: [u8; 6],
    pub supplicant_address: [u8; 6],
    pub authenticator_nonce: [u8; WPA2_NONCE_LEN],
    pub supplicant_nonce: [u8; WPA2_NONCE_LEN],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Wpa2TxMessage {
    PairwiseMessage1,
    PairwiseMessage2,
    PairwiseMessage3,
    PairwiseMessage4,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Wpa2Transmit {
    pub message: Wpa2TxMessage,
    pub replay_counter: u64,
    pub retransmission: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Wpa2StateError {
    WrongInterface,
    WrongPeer,
    UnsupportedDescriptorVersion,
    UnsupportedMessage,
    UnexpectedMessage,
    StaleReplayCounter,
    ReplayCounterMismatch,
    ReplayCounterExhausted,
    ZeroNonce,
    MissingEncryptedKeyData,
    WrongPhase,
    StaleCompletion,
    RetainedFrameMismatch,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Wpa2StaPhase {
    AwaitingMessage1,
    DerivingPtk,
    AwaitingMessage3,
    VerifyingMessage3,
    DecryptingKeyData,
    InstallingKeys,
    Completed,
    Failed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Wpa2StaAction<const N: usize = DEFAULT_EAPOL_FRAME_CAPACITY> {
    None,
    DerivePtk {
        ticket: Wpa2Ticket,
        context: PtkContext,
    },
    VerifyMessage3Mic {
        ticket: Wpa2Ticket,
        frame: OwnedEapolFrame<N>,
    },
    DecryptMessage3KeyData {
        ticket: Wpa2Ticket,
        frame: OwnedEapolFrame<N>,
    },
    InstallKeys {
        ticket: Wpa2Ticket,
        frame: OwnedEapolFrame<N>,
    },
    Transmit(Wpa2Transmit),
    Deauthenticate,
}

pub struct Wpa2StaState {
    local: [u8; 6],
    authenticator: [u8; 6],
    supplicant_nonce: [u8; WPA2_NONCE_LEN],
    authenticator_nonce: [u8; WPA2_NONCE_LEN],
    phase: Wpa2StaPhase,
    message1_replay: u64,
    message3_replay: u64,
    active_ticket: Wpa2Ticket,
    next_ticket: u32,
}

impl Wpa2StaState {
    pub const fn new(
        local: [u8; 6],
        authenticator: [u8; 6],
        supplicant_nonce: [u8; WPA2_NONCE_LEN],
    ) -> Result<Self, Wpa2StateError> {
        if nonce_is_zero(&supplicant_nonce) {
            return Err(Wpa2StateError::ZeroNonce);
        }
        Ok(Self {
            local,
            authenticator,
            supplicant_nonce,
            authenticator_nonce: [0; WPA2_NONCE_LEN],
            phase: Wpa2StaPhase::AwaitingMessage1,
            message1_replay: 0,
            message3_replay: 0,
            active_ticket: Wpa2Ticket(0),
            next_ticket: 1,
        })
    }

    pub const fn phase(&self) -> Wpa2StaPhase {
        self.phase
    }

    pub const fn peer(&self) -> &[u8; 6] {
        &self.authenticator
    }

    pub const fn local_address(&self) -> &[u8; 6] {
        &self.local
    }

    pub const fn supplicant_nonce(&self) -> &[u8; WPA2_NONCE_LEN] {
        &self.supplicant_nonce
    }

    pub const fn authenticator_nonce(&self) -> &[u8; WPA2_NONCE_LEN] {
        &self.authenticator_nonce
    }

    /// Replay frontier authenticated by the completed four-way handshake.
    pub const fn completed_replay_counter(&self) -> Option<u64> {
        match self.phase {
            Wpa2StaPhase::Completed => Some(self.message3_replay),
            _ => None,
        }
    }

    pub fn on_frame<const N: usize>(
        &mut self,
        frame: OwnedEapolFrame<N>,
    ) -> Result<Wpa2StaAction<N>, Wpa2StateError> {
        self.validate_frame(&frame)?;
        let key = frame.key_frame();
        if key.key_info().descriptor_version() != 2 {
            return Err(Wpa2StateError::UnsupportedDescriptorVersion);
        }

        match key.message() {
            EapolKeyMessage::PairwiseMessage1 => self.on_message1(frame),
            EapolKeyMessage::PairwiseMessage3 => self.on_message3(frame),
            EapolKeyMessage::PairwiseMessage2
            | EapolKeyMessage::PairwiseMessage4
            | EapolKeyMessage::GroupMessage1
            | EapolKeyMessage::GroupMessage2
            | EapolKeyMessage::Other => Err(Wpa2StateError::UnsupportedMessage),
        }
    }

    fn on_message1<const N: usize>(
        &mut self,
        frame: OwnedEapolFrame<N>,
    ) -> Result<Wpa2StaAction<N>, Wpa2StateError> {
        let key = frame.key_frame();
        let replay = key.replay_counter();
        let nonce = *key.nonce();
        if nonce.iter().all(|byte| *byte == 0) {
            return Err(Wpa2StateError::ZeroNonce);
        }

        match self.phase {
            Wpa2StaPhase::AwaitingMessage1 | Wpa2StaPhase::Completed => {
                if self.phase == Wpa2StaPhase::Completed && replay <= self.message3_replay {
                    return Err(Wpa2StateError::StaleReplayCounter);
                }
                self.start_ptk(replay, nonce)
            }
            Wpa2StaPhase::DerivingPtk => {
                if replay == self.message1_replay && nonce == self.authenticator_nonce {
                    Ok(Wpa2StaAction::None)
                } else if replay < self.message1_replay {
                    Err(Wpa2StateError::StaleReplayCounter)
                } else {
                    self.start_ptk(replay, nonce)
                }
            }
            Wpa2StaPhase::AwaitingMessage3 => {
                if replay == self.message1_replay && nonce == self.authenticator_nonce {
                    Ok(Wpa2StaAction::Transmit(Wpa2Transmit {
                        message: Wpa2TxMessage::PairwiseMessage2,
                        replay_counter: replay,
                        retransmission: true,
                    }))
                } else if replay < self.message1_replay {
                    Err(Wpa2StateError::StaleReplayCounter)
                } else {
                    self.start_ptk(replay, nonce)
                }
            }
            Wpa2StaPhase::VerifyingMessage3
            | Wpa2StaPhase::DecryptingKeyData
            | Wpa2StaPhase::InstallingKeys => {
                if replay <= self.message1_replay {
                    Ok(Wpa2StaAction::None)
                } else {
                    Err(Wpa2StateError::UnexpectedMessage)
                }
            }
            Wpa2StaPhase::Failed => Err(Wpa2StateError::WrongPhase),
        }
    }

    fn start_ptk<const N: usize>(
        &mut self,
        replay: u64,
        nonce: [u8; WPA2_NONCE_LEN],
    ) -> Result<Wpa2StaAction<N>, Wpa2StateError> {
        self.message1_replay = replay;
        self.message3_replay = 0;
        self.authenticator_nonce = nonce;
        self.phase = Wpa2StaPhase::DerivingPtk;
        let ticket = self.issue_ticket();
        Ok(Wpa2StaAction::DerivePtk {
            ticket,
            context: self.ptk_context(),
        })
    }

    fn on_message3<const N: usize>(
        &mut self,
        frame: OwnedEapolFrame<N>,
    ) -> Result<Wpa2StaAction<N>, Wpa2StateError> {
        let replay = frame.key_frame().replay_counter();
        match self.phase {
            Wpa2StaPhase::AwaitingMessage3 => {
                if replay <= self.message1_replay {
                    return Err(Wpa2StateError::StaleReplayCounter);
                }
                self.message3_replay = replay;
                self.phase = Wpa2StaPhase::VerifyingMessage3;
                let ticket = self.issue_ticket();
                Ok(Wpa2StaAction::VerifyMessage3Mic { ticket, frame })
            }
            Wpa2StaPhase::VerifyingMessage3
            | Wpa2StaPhase::DecryptingKeyData
            | Wpa2StaPhase::InstallingKeys => {
                if replay == self.message3_replay {
                    Ok(Wpa2StaAction::None)
                } else if replay < self.message3_replay {
                    Err(Wpa2StateError::StaleReplayCounter)
                } else {
                    Err(Wpa2StateError::UnexpectedMessage)
                }
            }
            Wpa2StaPhase::Completed => {
                if replay == self.message3_replay {
                    // KRACK-safe retransmission: acknowledge again but never
                    // reinstall PTK/GTK or reset packet numbers.
                    Ok(Wpa2StaAction::Transmit(Wpa2Transmit {
                        message: Wpa2TxMessage::PairwiseMessage4,
                        replay_counter: replay,
                        retransmission: true,
                    }))
                } else if replay < self.message3_replay {
                    Err(Wpa2StateError::StaleReplayCounter)
                } else {
                    Err(Wpa2StateError::UnexpectedMessage)
                }
            }
            Wpa2StaPhase::AwaitingMessage1 | Wpa2StaPhase::DerivingPtk | Wpa2StaPhase::Failed => {
                Err(Wpa2StateError::UnexpectedMessage)
            }
        }
    }

    pub fn complete_ptk<const N: usize>(
        &mut self,
        ticket: Wpa2Ticket,
        valid: bool,
    ) -> Result<Wpa2StaAction<N>, Wpa2StateError> {
        self.check_completion(ticket, Wpa2StaPhase::DerivingPtk)?;
        if !valid {
            self.phase = Wpa2StaPhase::Failed;
            return Ok(Wpa2StaAction::Deauthenticate);
        }
        self.phase = Wpa2StaPhase::AwaitingMessage3;
        Ok(Wpa2StaAction::Transmit(Wpa2Transmit {
            message: Wpa2TxMessage::PairwiseMessage2,
            replay_counter: self.message1_replay,
            retransmission: false,
        }))
    }

    pub fn complete_message3_mic<const N: usize>(
        &mut self,
        ticket: Wpa2Ticket,
        frame: OwnedEapolFrame<N>,
        valid: bool,
    ) -> Result<Wpa2StaAction<N>, Wpa2StateError> {
        self.check_completion(ticket, Wpa2StaPhase::VerifyingMessage3)?;
        self.validate_retained_message3(&frame)?;
        if !valid {
            self.phase = Wpa2StaPhase::Failed;
            return Ok(Wpa2StaAction::Deauthenticate);
        }

        let key = frame.key_frame();
        if key.key_info().encrypted_key_data() {
            if key.key_data().is_empty() {
                return Err(Wpa2StateError::MissingEncryptedKeyData);
            }
            self.phase = Wpa2StaPhase::DecryptingKeyData;
            let ticket = self.issue_ticket();
            Ok(Wpa2StaAction::DecryptMessage3KeyData { ticket, frame })
        } else {
            self.phase = Wpa2StaPhase::InstallingKeys;
            let ticket = self.issue_ticket();
            Ok(Wpa2StaAction::InstallKeys { ticket, frame })
        }
    }

    pub fn complete_key_data<const N: usize>(
        &mut self,
        ticket: Wpa2Ticket,
        frame: OwnedEapolFrame<N>,
        valid: bool,
    ) -> Result<Wpa2StaAction<N>, Wpa2StateError> {
        self.check_completion(ticket, Wpa2StaPhase::DecryptingKeyData)?;
        self.validate_retained_message3(&frame)?;
        if !valid {
            self.phase = Wpa2StaPhase::Failed;
            return Ok(Wpa2StaAction::Deauthenticate);
        }
        self.phase = Wpa2StaPhase::InstallingKeys;
        let ticket = self.issue_ticket();
        Ok(Wpa2StaAction::InstallKeys { ticket, frame })
    }

    pub fn complete_key_install<const N: usize>(
        &mut self,
        ticket: Wpa2Ticket,
        installed: bool,
    ) -> Result<Wpa2StaAction<N>, Wpa2StateError> {
        self.check_completion(ticket, Wpa2StaPhase::InstallingKeys)?;
        if !installed {
            self.phase = Wpa2StaPhase::Failed;
            return Ok(Wpa2StaAction::Deauthenticate);
        }
        self.phase = Wpa2StaPhase::Completed;
        Ok(Wpa2StaAction::Transmit(Wpa2Transmit {
            message: Wpa2TxMessage::PairwiseMessage4,
            replay_counter: self.message3_replay,
            retransmission: false,
        }))
    }

    const fn ptk_context(&self) -> PtkContext {
        PtkContext {
            authenticator_address: self.authenticator,
            supplicant_address: self.local,
            authenticator_nonce: self.authenticator_nonce,
            supplicant_nonce: self.supplicant_nonce,
        }
    }

    fn validate_frame<const N: usize>(
        &self,
        frame: &OwnedEapolFrame<N>,
    ) -> Result<(), Wpa2StateError> {
        if frame.interface() != Wpa2Interface::Station {
            return Err(Wpa2StateError::WrongInterface);
        }
        if frame.peer() != &self.authenticator {
            return Err(Wpa2StateError::WrongPeer);
        }
        Ok(())
    }

    fn validate_retained_message3<const N: usize>(
        &self,
        frame: &OwnedEapolFrame<N>,
    ) -> Result<(), Wpa2StateError> {
        self.validate_frame(frame)?;
        let key = frame.key_frame();
        if key.message() != EapolKeyMessage::PairwiseMessage3
            || key.replay_counter() != self.message3_replay
        {
            return Err(Wpa2StateError::RetainedFrameMismatch);
        }
        Ok(())
    }

    fn check_completion(
        &self,
        ticket: Wpa2Ticket,
        phase: Wpa2StaPhase,
    ) -> Result<(), Wpa2StateError> {
        if self.phase != phase {
            return Err(Wpa2StateError::WrongPhase);
        }
        if self.active_ticket != ticket {
            return Err(Wpa2StateError::StaleCompletion);
        }
        Ok(())
    }

    fn issue_ticket(&mut self) -> Wpa2Ticket {
        let ticket = Wpa2Ticket(self.next_ticket);
        self.next_ticket = self.next_ticket.wrapping_add(1);
        if self.next_ticket == 0 {
            self.next_ticket = 1;
        }
        self.active_ticket = ticket;
        ticket
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Wpa2ApPhase {
    AwaitingMessage2,
    DerivingPtk,
    VerifyingMessage2,
    PreparingMessage3,
    AwaitingMessage4,
    VerifyingMessage4,
    Authorized,
    Failed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Wpa2ApAction<const N: usize = DEFAULT_EAPOL_FRAME_CAPACITY> {
    None,
    DerivePtk {
        ticket: Wpa2Ticket,
        context: PtkContext,
        message2: OwnedEapolFrame<N>,
    },
    VerifyMessage2Mic {
        ticket: Wpa2Ticket,
        message2: OwnedEapolFrame<N>,
    },
    PrepareMessage3 {
        ticket: Wpa2Ticket,
    },
    VerifyMessage4Mic {
        ticket: Wpa2Ticket,
        message4: OwnedEapolFrame<N>,
    },
    Transmit(Wpa2Transmit),
    AuthorizePeer,
    DeauthenticatePeer,
}

pub struct Wpa2ApState {
    authenticator: [u8; 6],
    supplicant: [u8; 6],
    authenticator_nonce: [u8; WPA2_NONCE_LEN],
    supplicant_nonce: [u8; WPA2_NONCE_LEN],
    phase: Wpa2ApPhase,
    message1_replay: u64,
    message3_replay: u64,
    active_ticket: Wpa2Ticket,
    next_ticket: u32,
}

impl Wpa2ApState {
    pub const fn new(
        authenticator: [u8; 6],
        supplicant: [u8; 6],
        authenticator_nonce: [u8; WPA2_NONCE_LEN],
        initial_replay_counter: u64,
    ) -> Result<Self, Wpa2StateError> {
        if initial_replay_counter == u64::MAX {
            return Err(Wpa2StateError::ReplayCounterExhausted);
        }
        if nonce_is_zero(&authenticator_nonce) {
            return Err(Wpa2StateError::ZeroNonce);
        }
        Ok(Self {
            authenticator,
            supplicant,
            authenticator_nonce,
            supplicant_nonce: [0; WPA2_NONCE_LEN],
            phase: Wpa2ApPhase::AwaitingMessage2,
            message1_replay: initial_replay_counter,
            message3_replay: 0,
            active_ticket: Wpa2Ticket(0),
            next_ticket: 1,
        })
    }

    pub const fn phase(&self) -> Wpa2ApPhase {
        self.phase
    }

    pub const fn peer(&self) -> &[u8; 6] {
        &self.supplicant
    }

    pub const fn local_address(&self) -> &[u8; 6] {
        &self.authenticator
    }

    pub const fn supplicant_nonce(&self) -> &[u8; WPA2_NONCE_LEN] {
        &self.supplicant_nonce
    }

    pub const fn authenticator_nonce(&self) -> &[u8; WPA2_NONCE_LEN] {
        &self.authenticator_nonce
    }

    pub const fn message1(&self, retransmission: bool) -> Result<Wpa2ApAction, Wpa2StateError> {
        if !matches!(self.phase, Wpa2ApPhase::AwaitingMessage2) {
            return Err(Wpa2StateError::WrongPhase);
        }
        Ok(Wpa2ApAction::Transmit(Wpa2Transmit {
            message: Wpa2TxMessage::PairwiseMessage1,
            replay_counter: self.message1_replay,
            retransmission,
        }))
    }

    /// Return the authenticator frame that owns the current response window.
    ///
    /// Timing and retry budgets belong to the AP service. The WPA state only
    /// exposes the protocol-correct message and replay counter for its current
    /// phase; it never reads time or schedules work itself.
    pub const fn retry_transmit(&self) -> Result<Wpa2Transmit, Wpa2StateError> {
        match self.phase {
            Wpa2ApPhase::AwaitingMessage2 => Ok(Wpa2Transmit {
                message: Wpa2TxMessage::PairwiseMessage1,
                replay_counter: self.message1_replay,
                retransmission: true,
            }),
            Wpa2ApPhase::AwaitingMessage4 => Ok(Wpa2Transmit {
                message: Wpa2TxMessage::PairwiseMessage3,
                replay_counter: self.message3_replay,
                retransmission: true,
            }),
            _ => Err(Wpa2StateError::WrongPhase),
        }
    }

    pub fn on_frame<const N: usize>(
        &mut self,
        frame: OwnedEapolFrame<N>,
    ) -> Result<Wpa2ApAction<N>, Wpa2StateError> {
        self.validate_frame(&frame)?;
        let key = frame.key_frame();
        if key.key_info().descriptor_version() != 2 {
            return Err(Wpa2StateError::UnsupportedDescriptorVersion);
        }
        match key.message() {
            EapolKeyMessage::PairwiseMessage2 => self.on_message2(frame),
            EapolKeyMessage::PairwiseMessage4 => self.on_message4(frame),
            _ => Err(Wpa2StateError::UnsupportedMessage),
        }
    }

    fn on_message2<const N: usize>(
        &mut self,
        frame: OwnedEapolFrame<N>,
    ) -> Result<Wpa2ApAction<N>, Wpa2StateError> {
        let key = frame.key_frame();
        let replay = key.replay_counter();
        let nonce = *key.nonce();
        if replay != self.message1_replay {
            return Err(Wpa2StateError::ReplayCounterMismatch);
        }
        if nonce.iter().all(|byte| *byte == 0) {
            return Err(Wpa2StateError::ZeroNonce);
        }

        match self.phase {
            Wpa2ApPhase::AwaitingMessage2 => {
                self.supplicant_nonce = nonce;
                self.phase = Wpa2ApPhase::DerivingPtk;
                let ticket = self.issue_ticket();
                Ok(Wpa2ApAction::DerivePtk {
                    ticket,
                    context: self.ptk_context(),
                    message2: frame,
                })
            }
            Wpa2ApPhase::DerivingPtk
            | Wpa2ApPhase::VerifyingMessage2
            | Wpa2ApPhase::PreparingMessage3 => Ok(Wpa2ApAction::None),
            Wpa2ApPhase::AwaitingMessage4 | Wpa2ApPhase::VerifyingMessage4 => {
                if nonce == self.supplicant_nonce {
                    Ok(Wpa2ApAction::Transmit(Wpa2Transmit {
                        message: Wpa2TxMessage::PairwiseMessage3,
                        replay_counter: self.message3_replay,
                        retransmission: true,
                    }))
                } else {
                    Err(Wpa2StateError::UnexpectedMessage)
                }
            }
            Wpa2ApPhase::Authorized => Ok(Wpa2ApAction::None),
            Wpa2ApPhase::Failed => Err(Wpa2StateError::WrongPhase),
        }
    }

    fn on_message4<const N: usize>(
        &mut self,
        frame: OwnedEapolFrame<N>,
    ) -> Result<Wpa2ApAction<N>, Wpa2StateError> {
        let replay = frame.key_frame().replay_counter();
        if replay != self.message3_replay {
            return Err(Wpa2StateError::ReplayCounterMismatch);
        }
        match self.phase {
            Wpa2ApPhase::AwaitingMessage4 => {
                self.phase = Wpa2ApPhase::VerifyingMessage4;
                let ticket = self.issue_ticket();
                Ok(Wpa2ApAction::VerifyMessage4Mic {
                    ticket,
                    message4: frame,
                })
            }
            Wpa2ApPhase::VerifyingMessage4 | Wpa2ApPhase::Authorized => Ok(Wpa2ApAction::None),
            _ => Err(Wpa2StateError::UnexpectedMessage),
        }
    }

    pub fn complete_ptk<const N: usize>(
        &mut self,
        ticket: Wpa2Ticket,
        message2: OwnedEapolFrame<N>,
        valid: bool,
    ) -> Result<Wpa2ApAction<N>, Wpa2StateError> {
        self.check_completion(ticket, Wpa2ApPhase::DerivingPtk)?;
        self.validate_retained_message2(&message2)?;
        if !valid {
            self.phase = Wpa2ApPhase::Failed;
            return Ok(Wpa2ApAction::DeauthenticatePeer);
        }
        self.phase = Wpa2ApPhase::VerifyingMessage2;
        let ticket = self.issue_ticket();
        Ok(Wpa2ApAction::VerifyMessage2Mic { ticket, message2 })
    }

    pub fn complete_message2_mic<const N: usize>(
        &mut self,
        ticket: Wpa2Ticket,
        message2: OwnedEapolFrame<N>,
        valid: bool,
    ) -> Result<Wpa2ApAction<N>, Wpa2StateError> {
        self.check_completion(ticket, Wpa2ApPhase::VerifyingMessage2)?;
        self.validate_retained_message2(&message2)?;
        if !valid {
            self.phase = Wpa2ApPhase::Failed;
            return Ok(Wpa2ApAction::DeauthenticatePeer);
        }
        self.phase = Wpa2ApPhase::PreparingMessage3;
        let ticket = self.issue_ticket();
        Ok(Wpa2ApAction::PrepareMessage3 { ticket })
    }

    pub fn complete_message3_preparation<const N: usize>(
        &mut self,
        ticket: Wpa2Ticket,
        prepared: bool,
    ) -> Result<Wpa2ApAction<N>, Wpa2StateError> {
        self.check_completion(ticket, Wpa2ApPhase::PreparingMessage3)?;
        if !prepared {
            self.phase = Wpa2ApPhase::Failed;
            return Ok(Wpa2ApAction::DeauthenticatePeer);
        }
        self.message3_replay = self
            .message1_replay
            .checked_add(1)
            .ok_or(Wpa2StateError::ReplayCounterExhausted)?;
        self.phase = Wpa2ApPhase::AwaitingMessage4;
        Ok(Wpa2ApAction::Transmit(Wpa2Transmit {
            message: Wpa2TxMessage::PairwiseMessage3,
            replay_counter: self.message3_replay,
            retransmission: false,
        }))
    }

    pub fn complete_message4_mic<const N: usize>(
        &mut self,
        ticket: Wpa2Ticket,
        message4: OwnedEapolFrame<N>,
        valid: bool,
    ) -> Result<Wpa2ApAction<N>, Wpa2StateError> {
        self.check_completion(ticket, Wpa2ApPhase::VerifyingMessage4)?;
        self.validate_retained_message4(&message4)?;
        if !valid {
            self.phase = Wpa2ApPhase::Failed;
            return Ok(Wpa2ApAction::DeauthenticatePeer);
        }
        self.phase = Wpa2ApPhase::Authorized;
        Ok(Wpa2ApAction::AuthorizePeer)
    }

    const fn ptk_context(&self) -> PtkContext {
        PtkContext {
            authenticator_address: self.authenticator,
            supplicant_address: self.supplicant,
            authenticator_nonce: self.authenticator_nonce,
            supplicant_nonce: self.supplicant_nonce,
        }
    }

    fn validate_frame<const N: usize>(
        &self,
        frame: &OwnedEapolFrame<N>,
    ) -> Result<(), Wpa2StateError> {
        if frame.interface() != Wpa2Interface::AccessPoint {
            return Err(Wpa2StateError::WrongInterface);
        }
        if frame.peer() != &self.supplicant {
            return Err(Wpa2StateError::WrongPeer);
        }
        Ok(())
    }

    fn validate_retained_message2<const N: usize>(
        &self,
        frame: &OwnedEapolFrame<N>,
    ) -> Result<(), Wpa2StateError> {
        self.validate_frame(frame)?;
        let key = frame.key_frame();
        if key.message() != EapolKeyMessage::PairwiseMessage2
            || key.replay_counter() != self.message1_replay
            || key.nonce() != &self.supplicant_nonce
        {
            return Err(Wpa2StateError::RetainedFrameMismatch);
        }
        Ok(())
    }

    fn validate_retained_message4<const N: usize>(
        &self,
        frame: &OwnedEapolFrame<N>,
    ) -> Result<(), Wpa2StateError> {
        self.validate_frame(frame)?;
        let key = frame.key_frame();
        if key.message() != EapolKeyMessage::PairwiseMessage4
            || key.replay_counter() != self.message3_replay
        {
            return Err(Wpa2StateError::RetainedFrameMismatch);
        }
        Ok(())
    }

    fn check_completion(
        &self,
        ticket: Wpa2Ticket,
        phase: Wpa2ApPhase,
    ) -> Result<(), Wpa2StateError> {
        if self.phase != phase {
            return Err(Wpa2StateError::WrongPhase);
        }
        if self.active_ticket != ticket {
            return Err(Wpa2StateError::StaleCompletion);
        }
        Ok(())
    }

    fn issue_ticket(&mut self) -> Wpa2Ticket {
        let ticket = Wpa2Ticket(self.next_ticket);
        self.next_ticket = self.next_ticket.wrapping_add(1);
        if self.next_ticket == 0 {
            self.next_ticket = 1;
        }
        self.active_ticket = ticket;
        ticket
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Wpa2ApPeerError {
    DuplicatePeer,
    Full,
}

/// Fixed AP peer table. Construction and lookup are bounded by `P`; neither
/// path allocates or waits.
pub struct Wpa2ApPeers<const P: usize> {
    peers: [Option<Wpa2ApState>; P],
}

impl<const P: usize> Wpa2ApPeers<P> {
    pub fn new() -> Self {
        assert!(P > 0);
        Self {
            peers: core::array::from_fn(|_| None),
        }
    }

    pub fn insert(&mut self, peer: Wpa2ApState) -> Result<(), Wpa2ApPeerError> {
        if self
            .peers
            .iter()
            .flatten()
            .any(|existing| existing.peer() == peer.peer())
        {
            return Err(Wpa2ApPeerError::DuplicatePeer);
        }
        let slot = self
            .peers
            .iter_mut()
            .find(|slot| slot.is_none())
            .ok_or(Wpa2ApPeerError::Full)?;
        *slot = Some(peer);
        Ok(())
    }

    pub fn get_mut(&mut self, peer: &[u8; 6]) -> Option<&mut Wpa2ApState> {
        self.peers
            .iter_mut()
            .flatten()
            .find(|state| state.peer() == peer)
    }

    pub fn remove(&mut self, peer: &[u8; 6]) -> Option<Wpa2ApState> {
        let slot = self
            .peers
            .iter_mut()
            .find(|slot| slot.as_ref().is_some_and(|state| state.peer() == peer))?;
        slot.take()
    }

    pub fn len(&self) -> usize {
        self.peers.iter().filter(|peer| peer.is_some()).count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl<const P: usize> Default for Wpa2ApPeers<P> {
    fn default() -> Self {
        Self::new()
    }
}

const fn nonce_is_zero(nonce: &[u8; WPA2_NONCE_LEN]) -> bool {
    let mut index = 0;
    while index < nonce.len() {
        if nonce[index] != 0 {
            return false;
        }
        index += 1;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        EAPOL_KEY_FIXED_LEN, EAPOL_KEY_PACKET_LEN, EAPOL_PACKET_TYPE_KEY, RSN_KEY_DESCRIPTOR_TYPE,
    };

    const PAIRWISE: u16 = 1 << 3;
    const INSTALL: u16 = 1 << 6;
    const ACK: u16 = 1 << 7;
    const MIC: u16 = 1 << 8;
    const SECURE: u16 = 1 << 9;
    const ENCRYPTED: u16 = 1 << 12;

    fn frame(
        interface: Wpa2Interface,
        peer: [u8; 6],
        info: u16,
        replay: u64,
        nonce: [u8; 32],
    ) -> OwnedEapolFrame<EAPOL_KEY_PACKET_LEN> {
        let mut bytes = [0; EAPOL_KEY_PACKET_LEN];
        bytes[0] = 2;
        bytes[1] = EAPOL_PACKET_TYPE_KEY;
        bytes[2..4].copy_from_slice(&(EAPOL_KEY_FIXED_LEN as u16).to_be_bytes());
        bytes[4] = RSN_KEY_DESCRIPTOR_TYPE;
        bytes[5..7].copy_from_slice(&(info | 2).to_be_bytes());
        bytes[9..17].copy_from_slice(&replay.to_be_bytes());
        bytes[17..49].copy_from_slice(&nonce);
        OwnedEapolFrame::try_copy(interface, peer, &bytes).unwrap()
    }

    #[test]
    fn sta_completes_once_and_does_not_reinstall_on_message3_replay() {
        let local = [1; 6];
        let ap = [2; 6];
        let snonce = [3; 32];
        let anonce = [4; 32];
        let mut state = Wpa2StaState::new(local, ap, snonce).unwrap();

        let ticket = match state
            .on_frame(frame(
                Wpa2Interface::Station,
                ap,
                PAIRWISE | ACK,
                10,
                anonce,
            ))
            .unwrap()
        {
            Wpa2StaAction::DerivePtk { ticket, context } => {
                assert_eq!(context.authenticator_nonce, anonce);
                ticket
            }
            action => panic!("unexpected action: {action:?}"),
        };
        assert_eq!(
            state
                .complete_ptk::<EAPOL_KEY_PACKET_LEN>(ticket, true)
                .unwrap(),
            Wpa2StaAction::Transmit(Wpa2Transmit {
                message: Wpa2TxMessage::PairwiseMessage2,
                replay_counter: 10,
                retransmission: false,
            })
        );

        let message3 = frame(
            Wpa2Interface::Station,
            ap,
            PAIRWISE | ACK | MIC | INSTALL | SECURE,
            11,
            anonce,
        );
        let (ticket, retained) = match state.on_frame(message3.clone()).unwrap() {
            Wpa2StaAction::VerifyMessage3Mic { ticket, frame } => (ticket, frame),
            action => panic!("unexpected action: {action:?}"),
        };
        let (ticket, retained) = match state.complete_message3_mic(ticket, retained, true).unwrap()
        {
            Wpa2StaAction::InstallKeys { ticket, frame } => (ticket, frame),
            action => panic!("unexpected action: {action:?}"),
        };
        assert_eq!(retained.key_frame().replay_counter(), 11);
        assert_eq!(
            state
                .complete_key_install::<EAPOL_KEY_PACKET_LEN>(ticket, true)
                .unwrap(),
            Wpa2StaAction::Transmit(Wpa2Transmit {
                message: Wpa2TxMessage::PairwiseMessage4,
                replay_counter: 11,
                retransmission: false,
            })
        );
        assert_eq!(state.phase(), Wpa2StaPhase::Completed);
        assert_eq!(
            state.on_frame(message3).unwrap(),
            Wpa2StaAction::Transmit(Wpa2Transmit {
                message: Wpa2TxMessage::PairwiseMessage4,
                replay_counter: 11,
                retransmission: true,
            })
        );
        assert_eq!(state.phase(), Wpa2StaPhase::Completed);
    }

    #[test]
    fn sta_rejects_stale_completion_and_stale_message3() {
        let mut state = Wpa2StaState::new([1; 6], [2; 6], [3; 32]).unwrap();
        let ticket = match state
            .on_frame(frame(
                Wpa2Interface::Station,
                [2; 6],
                PAIRWISE | ACK,
                7,
                [4; 32],
            ))
            .unwrap()
        {
            Wpa2StaAction::DerivePtk { ticket, .. } => ticket,
            _ => unreachable!(),
        };
        assert_eq!(
            state.complete_ptk::<EAPOL_KEY_PACKET_LEN>(Wpa2Ticket(ticket.raw() + 1), true),
            Err(Wpa2StateError::StaleCompletion)
        );
        state
            .complete_ptk::<EAPOL_KEY_PACKET_LEN>(ticket, true)
            .unwrap();
        assert_eq!(
            state.on_frame(frame(
                Wpa2Interface::Station,
                [2; 6],
                PAIRWISE | ACK | MIC | INSTALL | SECURE,
                7,
                [4; 32],
            )),
            Err(Wpa2StateError::StaleReplayCounter)
        );
    }

    #[test]
    fn ap_orders_crypto_install_transmit_and_authorization() {
        let ap = [1; 6];
        let station = [2; 6];
        let mut state = Wpa2ApState::new(ap, station, [3; 32], 20).unwrap();
        assert_eq!(
            state.message1(false).unwrap(),
            Wpa2ApAction::Transmit(Wpa2Transmit {
                message: Wpa2TxMessage::PairwiseMessage1,
                replay_counter: 20,
                retransmission: false,
            })
        );

        let message2 = frame(
            Wpa2Interface::AccessPoint,
            station,
            PAIRWISE | MIC,
            20,
            [4; 32],
        );
        let (ticket, retained) = match state.on_frame(message2).unwrap() {
            Wpa2ApAction::DerivePtk {
                ticket, message2, ..
            } => (ticket, message2),
            action => panic!("unexpected action: {action:?}"),
        };
        let (ticket, retained) = match state.complete_ptk(ticket, retained, true).unwrap() {
            Wpa2ApAction::VerifyMessage2Mic { ticket, message2 } => (ticket, message2),
            action => panic!("unexpected action: {action:?}"),
        };
        let ticket = match state.complete_message2_mic(ticket, retained, true).unwrap() {
            Wpa2ApAction::PrepareMessage3 { ticket } => ticket,
            action => panic!("unexpected action: {action:?}"),
        };
        assert_eq!(
            state
                .complete_message3_preparation::<EAPOL_KEY_PACKET_LEN>(ticket, true)
                .unwrap(),
            Wpa2ApAction::Transmit(Wpa2Transmit {
                message: Wpa2TxMessage::PairwiseMessage3,
                replay_counter: 21,
                retransmission: false,
            })
        );

        let message4 = frame(
            Wpa2Interface::AccessPoint,
            station,
            PAIRWISE | MIC | SECURE,
            21,
            [0; 32],
        );
        let (ticket, retained) = match state.on_frame(message4).unwrap() {
            Wpa2ApAction::VerifyMessage4Mic { ticket, message4 } => (ticket, message4),
            action => panic!("unexpected action: {action:?}"),
        };
        assert_eq!(
            state.complete_message4_mic(ticket, retained, true).unwrap(),
            Wpa2ApAction::AuthorizePeer
        );
        assert_eq!(state.phase(), Wpa2ApPhase::Authorized);
    }

    #[test]
    fn ap_peer_table_is_fixed_and_reports_full() {
        let mut peers = Wpa2ApPeers::<2>::new();
        peers
            .insert(Wpa2ApState::new([1; 6], [2; 6], [3; 32], 1).unwrap())
            .unwrap();
        assert_eq!(
            peers.insert(Wpa2ApState::new([1; 6], [2; 6], [4; 32], 2).unwrap()),
            Err(Wpa2ApPeerError::DuplicatePeer)
        );
        peers
            .insert(Wpa2ApState::new([1; 6], [5; 6], [6; 32], 3).unwrap())
            .unwrap();
        assert_eq!(
            peers.insert(Wpa2ApState::new([1; 6], [7; 6], [8; 32], 4).unwrap()),
            Err(Wpa2ApPeerError::Full)
        );
        assert_eq!(peers.len(), 2);
        assert!(peers.remove(&[2; 6]).is_some());
        assert_eq!(peers.len(), 1);
    }

    #[test]
    fn constructors_reject_zero_nonces() {
        assert_eq!(
            Wpa2StaState::new([1; 6], [2; 6], [0; 32]).err(),
            Some(Wpa2StateError::ZeroNonce)
        );
        assert_eq!(
            Wpa2ApState::new([1; 6], [2; 6], [0; 32], 1).err(),
            Some(Wpa2StateError::ZeroNonce)
        );
    }

    #[test]
    fn encrypted_message3_requires_owned_key_data() {
        let mut state = Wpa2StaState::new([1; 6], [2; 6], [3; 32]).unwrap();
        let ticket = match state
            .on_frame(frame(
                Wpa2Interface::Station,
                [2; 6],
                PAIRWISE | ACK,
                1,
                [4; 32],
            ))
            .unwrap()
        {
            Wpa2StaAction::DerivePtk { ticket, .. } => ticket,
            _ => unreachable!(),
        };
        state
            .complete_ptk::<EAPOL_KEY_PACKET_LEN>(ticket, true)
            .unwrap();
        let (ticket, message3) = match state
            .on_frame(frame(
                Wpa2Interface::Station,
                [2; 6],
                PAIRWISE | ACK | MIC | INSTALL | SECURE | ENCRYPTED,
                2,
                [4; 32],
            ))
            .unwrap()
        {
            Wpa2StaAction::VerifyMessage3Mic { ticket, frame } => (ticket, frame),
            _ => unreachable!(),
        };
        assert_eq!(
            state.complete_message3_mic(ticket, message3, true),
            Err(Wpa2StateError::MissingEncryptedKeyData)
        );
    }
}
