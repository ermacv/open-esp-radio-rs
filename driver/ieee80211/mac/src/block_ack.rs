//! Hardware-independent IEEE 802.11 TX BlockAck protocol state.
//!
//! This module owns bounded action-frame parsing and one allocation-free TX
//! agreement state machine. It has no MMIO, DMA, interrupt, executor,
//! allocator, chip, vendor archive or ROM ABI dependency.

const SEQUENCE_NUMBER_MASK: u16 = 0x0fff;
const BLOCK_ACK_WINDOW_FIELD_MAX: u16 = 0x03ff;

pub const BLOCK_ACK_CATEGORY: u8 = 3;
pub const ADDBA_REQUEST_ACTION: u8 = 0;
pub const ADDBA_RESPONSE_ACTION: u8 = 1;
pub const DELBA_ACTION: u8 = 2;
pub const ADDBA_ACTION_BODY_LEN: usize = 9;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BlockAckAction {
    AddbaRequest {
        dialog_token: u8,
        tid: u8,
        immediate: bool,
        amsdu: bool,
        window: u16,
        timeout_tu: u16,
        starting_sequence: u16,
    },
    AddbaResponse {
        dialog_token: u8,
        status: u16,
        tid: u8,
        immediate: bool,
        amsdu: bool,
        window: u16,
        timeout_tu: u16,
    },
    Delba {
        tid: u8,
        initiator: bool,
        reason: u16,
    },
}

/// Parse the body of one IEEE 802.11 Block Ack Action frame.
///
/// This is a stateless leaf: it only reads the supplied bytes and does not
/// allocate, wait, access global state or call into the vendor library.
pub fn parse_block_ack_action(body: &[u8]) -> Option<BlockAckAction> {
    if body.len() < 2 || body[0] != BLOCK_ACK_CATEGORY {
        return None;
    }
    match body[1] {
        ADDBA_REQUEST_ACTION if body.len() >= ADDBA_ACTION_BODY_LEN => {
            let parameters = u16::from_le_bytes([body[3], body[4]]);
            let starting_sequence = u16::from_le_bytes([body[7], body[8]]) >> 4;
            Some(BlockAckAction::AddbaRequest {
                dialog_token: body[2],
                tid: ((parameters >> 2) & 0x0f) as u8,
                immediate: parameters & 0x0002 != 0,
                amsdu: parameters & 0x0001 != 0,
                window: (parameters >> 6) & 0x03ff,
                timeout_tu: u16::from_le_bytes([body[5], body[6]]),
                starting_sequence,
            })
        }
        ADDBA_RESPONSE_ACTION if body.len() >= ADDBA_ACTION_BODY_LEN => {
            let parameters = u16::from_le_bytes([body[5], body[6]]);
            Some(BlockAckAction::AddbaResponse {
                dialog_token: body[2],
                status: u16::from_le_bytes([body[3], body[4]]),
                tid: ((parameters >> 2) & 0x0f) as u8,
                immediate: parameters & 0x0002 != 0,
                amsdu: parameters & 0x0001 != 0,
                window: (parameters >> 6) & 0x03ff,
                timeout_tu: u16::from_le_bytes([body[7], body[8]]),
            })
        }
        DELBA_ACTION if body.len() >= 6 => {
            let parameters = u16::from_le_bytes([body[2], body[3]]);
            Some(BlockAckAction::Delba {
                tid: ((parameters >> 12) & 0x0f) as u8,
                initiator: parameters & 0x0800 != 0,
                reason: u16::from_le_bytes([body[4], body[5]]),
            })
        }
        _ => None,
    }
}

const BA_PARAMETER_AMSDU: u16 = 1;
const BA_PARAMETER_IMMEDIATE: u16 = 1 << 1;
const BA_PARAMETER_TID_SHIFT: u32 = 2;
const BA_PARAMETER_WINDOW_SHIFT: u32 = 6;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TxBlockAckError {
    InvalidTid(u8),
    InvalidWindow(u16),
    ZeroTimeout,
    DeadlineOverflow,
    MalformedResponse,
    UnexpectedResponse,
    DelayedPolicyUnsupported,
    WindowExceedsCapacity(u16),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TxBlockAckConfig {
    pub tid: u8,
    pub window: u16,
    pub timeout_tu: u16,
    pub negotiation_timeout_us: u32,
    pub amsdu: bool,
}

impl TxBlockAckConfig {
    pub const fn validate(self) -> Result<Self, TxBlockAckError> {
        if self.tid > 15 {
            return Err(TxBlockAckError::InvalidTid(self.tid));
        }
        if self.window == 0 || self.window > BLOCK_ACK_WINDOW_FIELD_MAX {
            return Err(TxBlockAckError::InvalidWindow(self.window));
        }
        if self.negotiation_timeout_us == 0 {
            return Err(TxBlockAckError::ZeroTimeout);
        }
        Ok(self)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TxBlockAckAlarm {
    pub generation: u32,
    pub deadline_us: u64,
}

/// One BlockAck action Dialog Token supplied by a protocol owner.
///
/// Keeping construction private prevents independent per-TID sessions from
/// accidentally reusing a token while their negotiations overlap.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TxBlockAckDialogToken(u8);

impl TxBlockAckDialogToken {
    /// Wrap a token selected by a multi-session allocation policy.
    ///
    /// All bit patterns fit the action-frame field. Whether zero is used and
    /// when a sequence wraps are policy decisions outside this protocol leaf.
    pub const fn from_value(value: u8) -> Self {
        Self(value)
    }

    pub const fn value(self) -> u8 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AddbaRequest {
    pub generation: u32,
    pub dialog_token: u8,
    pub starting_sequence: u16,
    pub body: [u8; ADDBA_ACTION_BODY_LEN],
    pub alarm: TxBlockAckAlarm,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OperationalTxBlockAck {
    pub tid: u8,
    pub window: u16,
    pub timeout_tu: u16,
    pub starting_sequence: u16,
    pub amsdu: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TxBlockAckResponse {
    Operational(OperationalTxBlockAck),
    Rejected(u16),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TxBlockAckPhase {
    Idle,
    Awaiting {
        dialog_token: u8,
        starting_sequence: u16,
    },
    Operational(OperationalTxBlockAck),
}

/// One statically owned TX BlockAck agreement for one QoS TID.
///
/// Every method performs a fixed number of loads/stores. Timer expiry is an
/// externally delivered edge; this type never reads time, sleeps, or retries.
pub struct TxBlockAckSession {
    config: TxBlockAckConfig,
    generation: u32,
    next_dialog_token: u8,
    phase: TxBlockAckPhase,
}

impl TxBlockAckSession {
    pub const fn new(config: TxBlockAckConfig) -> Result<Self, TxBlockAckError> {
        let config = match config.validate() {
            Ok(config) => config,
            Err(error) => return Err(error),
        };
        Ok(Self {
            config,
            generation: 0,
            next_dialog_token: 1,
            phase: TxBlockAckPhase::Idle,
        })
    }

    pub fn begin(
        &mut self,
        starting_sequence: u16,
        now_us: u64,
    ) -> Result<AddbaRequest, TxBlockAckError> {
        let dialog_token = TxBlockAckDialogToken(self.next_dialog_token);
        self.next_dialog_token = next_dialog_token(dialog_token.value());
        self.begin_with_dialog_token(starting_sequence, now_us, dialog_token)
    }

    /// Start negotiation with a token supplied by a shared multi-TID owner.
    ///
    /// A caller that has more than one live TID must obtain this value from
    /// its shared token-allocation policy. The session still exclusively owns
    /// its timeout generation, starting sequence and operational agreement.
    pub fn begin_with_dialog_token(
        &mut self,
        starting_sequence: u16,
        now_us: u64,
        dialog_token: TxBlockAckDialogToken,
    ) -> Result<AddbaRequest, TxBlockAckError> {
        let deadline_us = now_us
            .checked_add(u64::from(self.config.negotiation_timeout_us))
            .ok_or(TxBlockAckError::DeadlineOverflow)?;
        self.generation = next_generation(self.generation);
        let dialog_token = dialog_token.value();
        let starting_sequence = starting_sequence & SEQUENCE_NUMBER_MASK;
        self.phase = TxBlockAckPhase::Awaiting {
            dialog_token,
            starting_sequence,
        };

        let parameters =
            encode_ba_parameters(self.config.tid, self.config.window, self.config.amsdu);
        let sequence_control = starting_sequence << 4;
        let mut body = [0_u8; ADDBA_ACTION_BODY_LEN];
        body[0] = BLOCK_ACK_CATEGORY;
        body[1] = ADDBA_REQUEST_ACTION;
        body[2] = dialog_token;
        body[3..5].copy_from_slice(&parameters.to_le_bytes());
        body[5..7].copy_from_slice(&self.config.timeout_tu.to_le_bytes());
        body[7..9].copy_from_slice(&sequence_control.to_le_bytes());

        let alarm = TxBlockAckAlarm {
            generation: self.generation,
            deadline_us,
        };
        Ok(AddbaRequest {
            generation: self.generation,
            dialog_token,
            starting_sequence,
            body,
            alarm,
        })
    }

    pub fn on_response(&mut self, body: &[u8]) -> Result<TxBlockAckResponse, TxBlockAckError> {
        // The nine-byte ADDBA response is a fixed prefix, not the complete
        // action-body length. An HE peer may append an ADDBA Extension IE
        // (element 159). Linux `net/mac80211/agg-rx.c`::
        // `ieee80211_send_addba_resp` does exactly that after the fixed
        // response fields. The controlled AX211 HE20 HIL reached
        // `parse_block_ack_action(AddbaResponse)` and then failed only this
        // former exact-length check. We deliberately consume only the fixed
        // prefix here: the negotiated low ten-bit window remains bounded by
        // `self.config.window`; a future owner of extended (>1024) windows
        // must parse the IE separately.
        let action = parse_block_ack_action(body).ok_or(TxBlockAckError::MalformedResponse)?;
        self.on_response_action(action)
    }

    /// Apply the fixed fields of an already parsed ADDBA response.
    pub fn on_response_action(
        &mut self,
        action: BlockAckAction,
    ) -> Result<TxBlockAckResponse, TxBlockAckError> {
        let BlockAckAction::AddbaResponse {
            dialog_token: response_dialog_token,
            status,
            tid,
            immediate,
            amsdu,
            window,
            timeout_tu,
        } = action
        else {
            return Err(TxBlockAckError::MalformedResponse);
        };
        let TxBlockAckPhase::Awaiting {
            dialog_token,
            starting_sequence,
        } = self.phase
        else {
            return Err(TxBlockAckError::UnexpectedResponse);
        };
        if response_dialog_token != dialog_token {
            return Err(TxBlockAckError::UnexpectedResponse);
        }

        if status != 0 {
            self.phase = TxBlockAckPhase::Idle;
            self.generation = next_generation(self.generation);
            return Ok(TxBlockAckResponse::Rejected(status));
        }

        if !immediate {
            return Err(TxBlockAckError::DelayedPolicyUnsupported);
        }
        if tid != self.config.tid {
            return Err(TxBlockAckError::UnexpectedResponse);
        }
        if window == 0 || window > self.config.window {
            return Err(TxBlockAckError::WindowExceedsCapacity(window));
        }
        let agreement = OperationalTxBlockAck {
            tid,
            window,
            timeout_tu,
            starting_sequence,
            amsdu: self.config.amsdu && amsdu,
        };
        self.phase = TxBlockAckPhase::Operational(agreement);
        self.generation = next_generation(self.generation);
        Ok(TxBlockAckResponse::Operational(agreement))
    }

    /// Consume one exact async timer edge. Returns true only when it cancelled
    /// the currently outstanding negotiation.
    pub fn on_alarm(&mut self, alarm: TxBlockAckAlarm) -> bool {
        if alarm.generation != self.generation
            || !matches!(self.phase, TxBlockAckPhase::Awaiting { .. })
        {
            return false;
        }
        self.phase = TxBlockAckPhase::Idle;
        self.generation = next_generation(self.generation);
        true
    }

    pub fn stop(&mut self) {
        self.phase = TxBlockAckPhase::Idle;
        self.generation = next_generation(self.generation);
    }

    pub const fn operational(&self) -> Option<OperationalTxBlockAck> {
        match self.phase {
            TxBlockAckPhase::Operational(agreement) => Some(agreement),
            _ => None,
        }
    }

    pub const fn is_awaiting(&self) -> bool {
        matches!(self.phase, TxBlockAckPhase::Awaiting { .. })
    }

    /// Dialog Token currently owned by an outstanding negotiation.
    ///
    /// A multi-TID dispatcher can use this before calling [`Self::on_response`]
    /// so a response is delivered to exactly one session.
    pub const fn awaiting_dialog_token(&self) -> Option<u8> {
        match self.phase {
            TxBlockAckPhase::Awaiting { dialog_token, .. } => Some(dialog_token),
            _ => None,
        }
    }
}

const fn encode_ba_parameters(tid: u8, window: u16, amsdu: bool) -> u16 {
    ((amsdu as u16) * BA_PARAMETER_AMSDU)
        | BA_PARAMETER_IMMEDIATE
        | ((tid as u16) << BA_PARAMETER_TID_SHIFT)
        | (window << BA_PARAMETER_WINDOW_SHIFT)
}

const fn next_generation(current: u32) -> u32 {
    let next = current.wrapping_add(1);
    if next == 0 { 1 } else { next }
}

const fn next_dialog_token(current: u8) -> u8 {
    let next = current.wrapping_add(1);
    if next == 0 { 1 } else { next }
}

#[cfg(test)]
mod tests;
