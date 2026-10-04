//! The station's TX Block Ack originator: one agreement per TID its policy
//! names, a Dialog Token sequence the agreements share, and a bounded number
//! of negotiation attempts per TID.
//!
//! Each agreement is a [`TxBlockAckSession`] of `oer-ieee80211-mac`. Which
//! TIDs a station negotiates and how its Dialog Tokens advance is an
//! integrator's policy ([`StaTxBlockAckPolicy`]; the Espressif station's is
//! `oer-espressif-ieee80211-policy::block_ack`). The originator reads no
//! time and sends nothing: the caller hands it the time, sends the request
//! bodies it returns and reports their outcome.

use oer_ieee80211_mac::{
    block_ack::{
        AddbaRequest, BlockAckAction, OperationalTxBlockAck, TxBlockAckAlarm, TxBlockAckConfig,
        TxBlockAckDialogToken, TxBlockAckError, TxBlockAckResponse, TxBlockAckSession,
        parse_block_ack_action,
    },
    sequence::SequenceNumber,
};
use oer_time::{Duration, Instant};

/// The QoS TIDs an originator can own.
pub const STA_TX_BLOCK_ACK_MAX_TIDS: usize = 8;

/// Which agreements a station originates and how its Dialog Tokens advance.
#[derive(Clone, Copy, Debug)]
pub struct StaTxBlockAckPolicy {
    /// The TIDs negotiated once connected, in order; at most
    /// [`STA_TX_BLOCK_ACK_MAX_TIDS`], each below 8.
    pub tids: &'static [u8],
    /// The Dialog Token of the first request.
    pub first_dialog_token: u8,
    /// The Dialog Token after one.
    pub next_dialog_token: fn(u8) -> u8,
}

/// The agreement parameters every TID of an originator requests.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaTxBlockAckConfig {
    pub window: u16,
    /// How long a negotiation waits for the ADDBA Response.
    pub negotiation_timeout: Duration,
    /// The TIDs whose agreements may carry A-MSDUs, one bit per TID.
    pub amsdu_tids: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaTxBlockAckError {
    /// The policy names more TIDs than an originator owns.
    TooManyTids,
    /// The policy names this TID twice, or the TID is outside the originator.
    UnsupportedTid(u8),
    MalformedResponse,
    Session {
        tid: u8,
        error: TxBlockAckError,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaTxBlockAckResponse {
    pub tid: u8,
    pub response: TxBlockAckResponse,
}

/// A received ADDBA Response against the negotiations the originator owns.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaTxBlockAckResponseDisposition {
    Matched(StaTxBlockAckResponse),
    /// The response names no live negotiation, such as one that already
    /// timed out: it changes no agreement and does not end the link.
    StaleDialogToken(u8),
}

struct Agreement {
    tid: u8,
    session: TxBlockAckSession,
    alarm: Option<TxBlockAckAlarm>,
    /// A negotiation waits to be sent.
    pending: bool,
    /// Negotiations left; a peer's answer ends them.
    attempts: u8,
}

/// The station's TX Block Ack agreements.
// CAPABILITY: wifi-legacy-and-ht-mac-behavior-immediate-block-ack
pub struct StaTxBlockAckOriginator {
    agreements: [Option<Agreement>; STA_TX_BLOCK_ACK_MAX_TIDS],
    next_dialog_token: u8,
    advance_dialog_token: fn(u8) -> u8,
}

impl StaTxBlockAckOriginator {
    /// The idle agreements of `policy`'s TIDs, each requesting `config`.
    pub fn new(
        policy: StaTxBlockAckPolicy,
        config: StaTxBlockAckConfig,
    ) -> Result<Self, StaTxBlockAckError> {
        if policy.tids.len() > STA_TX_BLOCK_ACK_MAX_TIDS {
            return Err(StaTxBlockAckError::TooManyTids);
        }
        let mut agreements = [const { None }; STA_TX_BLOCK_ACK_MAX_TIDS];
        for (index, &tid) in policy.tids.iter().enumerate() {
            if usize::from(tid) >= STA_TX_BLOCK_ACK_MAX_TIDS || policy.tids[..index].contains(&tid)
            {
                return Err(StaTxBlockAckError::UnsupportedTid(tid));
            }
            let session = TxBlockAckSession::new(TxBlockAckConfig {
                tid,
                window: config.window,
                timeout_tu: 0,
                negotiation_timeout: config.negotiation_timeout,
                amsdu: config.amsdu_tids & (1 << tid) != 0,
            })
            .map_err(|error| StaTxBlockAckError::Session { tid, error })?;
            agreements[index] = Some(Agreement {
                tid,
                session,
                alarm: None,
                pending: false,
                attempts: 0,
            });
        }
        Ok(Self {
            agreements,
            next_dialog_token: policy.first_dialog_token,
            advance_dialog_token: policy.next_dialog_token,
        })
    }

    fn agreement(&self, tid: u8) -> Option<&Agreement> {
        self.agreements
            .iter()
            .flatten()
            .find(|agreement| agreement.tid == tid)
    }

    fn agreement_mut(&mut self, tid: u8) -> Option<&mut Agreement> {
        self.agreements
            .iter_mut()
            .flatten()
            .find(|agreement| agreement.tid == tid)
    }

    /// The TIDs in the policy's order.
    pub fn tids(&self) -> impl Iterator<Item = u8> + '_ {
        self.agreements
            .iter()
            .flatten()
            .map(|agreement| agreement.tid)
    }

    /// Queue a negotiation of every TID, each with `attempt_limit`
    /// attempts: a missing response or a request that did not leave uses
    /// one and queues the next; the peer's answer ends them.
    pub fn queue_initial(&mut self, attempt_limit: u8) {
        for agreement in self.agreements.iter_mut().flatten() {
            agreement.pending = attempt_limit != 0;
            agreement.attempts = attempt_limit;
        }
    }

    /// Whether a negotiation waits to be sent.
    pub fn has_pending(&self) -> bool {
        self.agreements
            .iter()
            .flatten()
            .any(|agreement| agreement.pending)
    }

    /// The next TID whose negotiation waits, in the policy's order; taking
    /// it uses one of its attempts. The caller then [`Self::begin`]s it.
    pub fn take_pending(&mut self) -> Option<u8> {
        let agreement = self
            .agreements
            .iter_mut()
            .flatten()
            .find(|agreement| agreement.pending)?;
        agreement.pending = false;
        agreement.attempts = agreement.attempts.saturating_sub(1);
        Some(agreement.tid)
    }

    /// Begin the negotiation of `tid` from `starting_sequence`; the request
    /// owns its encoded action body.
    pub fn begin(
        &mut self,
        tid: u8,
        starting_sequence: SequenceNumber,
        now: Instant,
    ) -> Result<AddbaRequest, StaTxBlockAckError> {
        let token = self.next_dialog_token;
        let agreement = self
            .agreement_mut(tid)
            .ok_or(StaTxBlockAckError::UnsupportedTid(tid))?;
        let request = agreement
            .session
            .begin_with_dialog_token(
                starting_sequence,
                now,
                TxBlockAckDialogToken::from_value(token),
            )
            .map_err(|error| StaTxBlockAckError::Session { tid, error })?;
        agreement.alarm = Some(request.alarm);
        self.next_dialog_token = (self.advance_dialog_token)(token);
        Ok(request)
    }

    /// The request of `tid` did not leave: its negotiation ends, and the
    /// next one is queued while attempts remain.
    pub fn transmit_failed(&mut self, tid: u8) {
        if let Some(agreement) = self.agreement_mut(tid) {
            agreement.session.stop();
            agreement.alarm = None;
            agreement.pending = agreement.attempts != 0;
        }
    }

    /// Route one ADDBA Response body by its Dialog Token.
    pub fn on_response(
        &mut self,
        body: &[u8],
    ) -> Result<StaTxBlockAckResponseDisposition, StaTxBlockAckError> {
        let action = parse_block_ack_action(body).ok_or(StaTxBlockAckError::MalformedResponse)?;
        self.on_response_action(action)
    }

    /// Route one parsed ADDBA Response by its Dialog Token. The peer's
    /// answer, an agreement or a refusal, ends the TID's attempts.
    pub fn on_response_action(
        &mut self,
        action: BlockAckAction,
    ) -> Result<StaTxBlockAckResponseDisposition, StaTxBlockAckError> {
        let BlockAckAction::AddbaResponse { dialog_token, .. } = action else {
            return Err(StaTxBlockAckError::MalformedResponse);
        };
        let Some(agreement) = self
            .agreements
            .iter_mut()
            .flatten()
            .find(|agreement| agreement.session.awaiting_dialog_token() == Some(dialog_token))
        else {
            return Ok(StaTxBlockAckResponseDisposition::StaleDialogToken(
                dialog_token,
            ));
        };
        let tid = agreement.tid;
        let response = agreement
            .session
            .on_response_action(action)
            .map_err(|error| StaTxBlockAckError::Session { tid, error })?;
        agreement.alarm = None;
        agreement.pending = false;
        agreement.attempts = 0;
        Ok(StaTxBlockAckResponseDisposition::Matched(
            StaTxBlockAckResponse { tid, response },
        ))
    }

    /// Consume at most one negotiation whose response is overdue at `now`
    /// and queue its next attempt while attempts remain; its TID.
    pub fn expire_next(&mut self, now: Instant) -> Option<u8> {
        for agreement in self.agreements.iter_mut().flatten() {
            let Some(alarm) = agreement.alarm.filter(|alarm| now >= alarm.deadline) else {
                continue;
            };
            agreement.alarm = None;
            if agreement.session.on_alarm(alarm) {
                agreement.pending = agreement.attempts != 0;
                return Some(agreement.tid);
            }
        }
        None
    }

    /// End the agreement or negotiation of `tid`, queueing nothing; `false`
    /// for a TID the originator does not own.
    pub fn stop(&mut self, tid: u8) -> bool {
        match self.agreement_mut(tid) {
            Some(agreement) => {
                agreement.session.stop();
                agreement.alarm = None;
                true
            }
            None => false,
        }
    }

    /// End every agreement and negotiation and drop every queued attempt;
    /// how many were operational or negotiating.
    pub fn stop_all(&mut self) -> u8 {
        let mut active = 0_u8;
        for agreement in self.agreements.iter_mut().flatten() {
            if agreement.session.operational().is_some() || agreement.alarm.is_some() {
                active = active.saturating_add(1);
            }
            agreement.session.stop();
            agreement.alarm = None;
            agreement.pending = false;
            agreement.attempts = 0;
        }
        active
    }

    pub fn operational(&self, tid: u8) -> Option<OperationalTxBlockAck> {
        self.agreement(tid)?.session.operational()
    }

    pub fn alarm(&self, tid: u8) -> Option<TxBlockAckAlarm> {
        self.agreement(tid)?.alarm
    }

    /// The earliest negotiation deadline.
    pub fn earliest_alarm_deadline(&self) -> Option<Instant> {
        self.agreements
            .iter()
            .flatten()
            .filter_map(|agreement| agreement.alarm)
            .map(|alarm| alarm.deadline)
            .min()
    }
}

#[cfg(test)]
mod tests;
