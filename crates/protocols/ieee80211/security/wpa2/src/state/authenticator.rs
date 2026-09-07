//! Authenticator four-way-handshake state and its bounded peer table.

use super::*;

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
                    // Public replay/SNonce fields do not authenticate a
                    // duplicate M2. The bounded AP M3 retry owner will
                    // retransmit on its timer; never emit a MIC-bearing M3 in
                    // direct response to an unverified peer frame.
                    Ok(Wpa2ApAction::None)
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
            // SNonce and M2 replay are peer input until the MIC authenticates
            // them. A spoofed M2 must not poison this peer's M1 transaction.
            self.supplicant_nonce = [0; WPA2_NONCE_LEN];
            self.phase = Wpa2ApPhase::AwaitingMessage2;
            return Ok(Wpa2ApAction::None);
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
            // Retain the installed-candidate PTK and M3 response window. A
            // forged M4 can then be ignored while a valid retry still
            // authorizes exactly this handshake.
            self.phase = Wpa2ApPhase::AwaitingMessage4;
            return Ok(Wpa2ApAction::None);
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
