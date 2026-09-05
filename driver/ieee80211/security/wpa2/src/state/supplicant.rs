//! Station four-way-handshake state and its complete event/action transitions.

use super::*;

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
        self.validate_message3_fields(&frame)?;
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
            // The candidate replay counter is not authenticated until the
            // MIC succeeds. Roll back the speculative M3 edge so a forged
            // parse-valid frame cannot kill the join or reserve its replay
            // value ahead of the real authenticator frame.
            self.message3_replay = 0;
            self.phase = Wpa2StaPhase::AwaitingMessage3;
            return Ok(Wpa2StaAction::None);
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
        self.validate_message3_fields(frame)?;
        let key = frame.key_frame();
        if key.message() != EapolKeyMessage::PairwiseMessage3
            || key.replay_counter() != self.message3_replay
        {
            return Err(Wpa2StateError::RetainedFrameMismatch);
        }
        Ok(())
    }

    fn validate_message3_fields<const N: usize>(
        &self,
        frame: &OwnedEapolFrame<N>,
    ) -> Result<(), Wpa2StateError> {
        let key = frame.key_frame();
        if key.key_info().raw() != WPA2_PAIRWISE_MESSAGE3_KEY_INFO {
            return Err(Wpa2StateError::InvalidMessage3KeyInfo);
        }
        if key.key_length() != WPA2_CCMP_TEMPORAL_KEY_LEN {
            return Err(Wpa2StateError::InvalidKeyLength);
        }
        if key.nonce() != &self.authenticator_nonce {
            return Err(Wpa2StateError::AuthenticatorNonceMismatch);
        }
        if key.key_iv().iter().any(|byte| *byte != 0) {
            return Err(Wpa2StateError::NonzeroKeyIv);
        }
        if key.key_identifier().iter().any(|byte| *byte != 0) {
            return Err(Wpa2StateError::NonzeroKeyIdentifier);
        }
        if key.key_data().is_empty() {
            return Err(Wpa2StateError::MissingEncryptedKeyData);
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
