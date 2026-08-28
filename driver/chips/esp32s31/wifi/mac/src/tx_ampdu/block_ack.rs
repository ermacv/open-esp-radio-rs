//! ESP32-S31 BlockAck policy and fixed-slot completion adapter.
//!
//! Frame parsing and one generic agreement state machine live in
//! [`open_esp_radio_ieee80211::block_ack`]. This module retains only the
//! vendor STA TID policy, the S31 three-register completion snapshot and the
//! fixed hardware-slot batch. It still owns no DMA address or register access.

pub use open_esp_radio_ieee80211::block_ack::{
    ADDBA_ACTION_BODY_LEN, ADDBA_REQUEST_ACTION, ADDBA_RESPONSE_ACTION, AddbaRequest,
    BLOCK_ACK_CATEGORY, BlockAckAction, DELBA_ACTION, OperationalTxBlockAck, TxBlockAckAlarm,
    TxBlockAckConfig, TxBlockAckDialogToken, TxBlockAckError, TxBlockAckResponse,
    TxBlockAckSession, parse_block_ack_action,
};

/// Strict S31 TX window recovered from the fixed vendor queue geometry.
pub const TX_BLOCK_ACK_MAX_WINDOW: u16 = 32;
pub const TX_AMPDU_SLOT_CAPACITY: usize = TX_BLOCK_ACK_MAX_WINDOW as usize;
const SEQUENCE_NUMBER_MASK: u16 = 0x0fff;
const BLOCK_ACK_BITMAP_BITS: u16 = 64;

/// Shared vendor Dialog Token owner for all S31 STA TX agreements.
///
/// Complete `libnet80211.a[ieee80211_ht.o]::ieee80211_ampdu_request`
/// increments one archive-static token modulo 63. This policy intentionally
/// remains outside the portable IEEE agreement state.
pub struct TxBlockAckDialogTokenSequence {
    next: u8,
}

impl TxBlockAckDialogTokenSequence {
    pub const fn new() -> Self {
        Self { next: 1 }
    }

    pub fn take(&mut self) -> TxBlockAckDialogToken {
        let token = TxBlockAckDialogToken::from_value(self.next);
        self.next = next_vendor_block_ack_dialog_token(self.next);
        token
    }
}

impl Default for TxBlockAckDialogTokenSequence {
    fn default() -> Self {
        Self::new()
    }
}

const fn next_vendor_block_ack_dialog_token(current: u8) -> u8 {
    if current >= 62 { 0 } else { current + 1 }
}

/// TX BlockAck TIDs started by the vendor STA connection-complete path.
///
/// SOURCE: complete `libnet80211.a[wl_cnx.o]::cnx_auth_done`
/// invokes `ieee80211_ampdu_request` for TIDs 0, 7 and 5 in this order.
pub const STA_TX_BLOCK_ACK_TIDS: [u8; 3] = [0, 7, 5];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaTxBlockAckSessionsError {
    UnsupportedTid(u8),
    MalformedResponse,
    Session { tid: u8, error: TxBlockAckError },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaTxBlockAckResponse {
    pub tid: u8,
    pub response: TxBlockAckResponse,
}

/// Classification of a received ADDBA response against the currently owned
/// negotiations.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaTxBlockAckResponseDisposition {
    Matched(StaTxBlockAckResponse),
    /// The response belongs to an expired or otherwise unowned negotiation.
    /// It must not mutate a newer live session or terminate the station link.
    StaleDialogToken(u8),
}

/// Fixed, allocation-free owner for all TX BlockAck agreements created when
/// an S31 station enters the connected state.
///
/// The three sessions deliberately share one Dialog Token sequence while
/// retaining independent negotiation generations and alarms. This is the
/// ownership boundary recovered from the vendor connection-complete path;
/// an executor supplies timestamps and transmits the returned action body.
pub struct StaTxBlockAckSessions {
    sessions: [TxBlockAckSession; 3],
    alarms: [Option<TxBlockAckAlarm>; 3],
    dialog_tokens: TxBlockAckDialogTokenSequence,
}

impl StaTxBlockAckSessions {
    pub const fn new(
        window: u16,
        negotiation_timeout_us: u32,
        tid0_amsdu: bool,
    ) -> Result<Self, TxBlockAckError> {
        if window == 0 || window > TX_BLOCK_ACK_MAX_WINDOW {
            return Err(TxBlockAckError::InvalidWindow(window));
        }
        let tid0 = match TxBlockAckSession::new(TxBlockAckConfig {
            tid: 0,
            window,
            timeout_tu: 0,
            negotiation_timeout_us,
            amsdu: tid0_amsdu,
        }) {
            Ok(session) => session,
            Err(error) => return Err(error),
        };
        let tid7 = match TxBlockAckSession::new(TxBlockAckConfig {
            tid: 7,
            window,
            timeout_tu: 0,
            negotiation_timeout_us,
            amsdu: false,
        }) {
            Ok(session) => session,
            Err(error) => return Err(error),
        };
        let tid5 = match TxBlockAckSession::new(TxBlockAckConfig {
            tid: 5,
            window,
            timeout_tu: 0,
            negotiation_timeout_us,
            amsdu: false,
        }) {
            Ok(session) => session,
            Err(error) => return Err(error),
        };
        Ok(Self {
            sessions: [tid0, tid7, tid5],
            alarms: [None; 3],
            dialog_tokens: TxBlockAckDialogTokenSequence::new(),
        })
    }

    /// Begin one of the three recovered STA negotiations.
    ///
    /// The returned request owns its encoded action body. Its alarm is stored
    /// internally so a caller cannot accidentally pair it with another TID.
    pub fn begin(
        &mut self,
        tid: u8,
        starting_sequence: u16,
        now_us: u64,
    ) -> Result<AddbaRequest, StaTxBlockAckSessionsError> {
        let index =
            sta_tx_block_ack_index(tid).ok_or(StaTxBlockAckSessionsError::UnsupportedTid(tid))?;
        let dialog_token = self.dialog_tokens.take();
        let request = self.sessions[index]
            .begin_with_dialog_token(starting_sequence, now_us, dialog_token)
            .map_err(|error| StaTxBlockAckSessionsError::Session { tid, error })?;
        self.alarms[index] = Some(request.alarm);
        Ok(request)
    }

    /// Route one ADDBA response by the shared Dialog Token and update exactly
    /// one session. A terminal response also consumes that session's alarm.
    pub fn on_response(
        &mut self,
        body: &[u8],
    ) -> Result<StaTxBlockAckResponseDisposition, StaTxBlockAckSessionsError> {
        let action =
            parse_block_ack_action(body).ok_or(StaTxBlockAckSessionsError::MalformedResponse)?;
        self.on_response_action(action)
    }

    /// Route an already parsed ADDBA response without retaining its borrowed
    /// management-frame body.
    ///
    /// This is the ownership boundary used by an async RX dispatcher: the
    /// fixed fields are copied into [`BlockAckAction`] while the staged frame
    /// is live, then protocol state can be updated after that storage is
    /// released.
    pub fn on_response_action(
        &mut self,
        action: BlockAckAction,
    ) -> Result<StaTxBlockAckResponseDisposition, StaTxBlockAckSessionsError> {
        let BlockAckAction::AddbaResponse {
            dialog_token: response_token,
            ..
        } = action
        else {
            return Err(StaTxBlockAckSessionsError::MalformedResponse);
        };
        let Some(index) = self
            .sessions
            .iter()
            .position(|session| session.awaiting_dialog_token() == Some(response_token))
        else {
            // A response may cross the finite negotiation timeout in either
            // direction. Like mac80211, classify a token which owns no live
            // negotiation as stale. The response cannot be applied to a new
            // session, but peer timing must not become a hardware failure.
            return Ok(StaTxBlockAckResponseDisposition::StaleDialogToken(
                response_token,
            ));
        };
        let tid = STA_TX_BLOCK_ACK_TIDS[index];
        let response = self.sessions[index]
            .on_response_action(action)
            .map_err(|error| StaTxBlockAckSessionsError::Session { tid, error })?;
        self.alarms[index] = None;
        Ok(StaTxBlockAckResponseDisposition::Matched(
            StaTxBlockAckResponse { tid, response },
        ))
    }

    /// Consume at most one due alarm. Repeated calls drain simultaneous
    /// expirations without placing an unbounded loop inside the state owner.
    pub fn expire_next(&mut self, now_us: u64) -> Option<u8> {
        for (index, tid) in STA_TX_BLOCK_ACK_TIDS.into_iter().enumerate() {
            let Some(alarm) = self.alarms[index] else {
                continue;
            };
            if now_us < alarm.deadline_us {
                continue;
            }
            self.alarms[index] = None;
            if self.sessions[index].on_alarm(alarm) {
                return Some(tid);
            }
        }
        None
    }

    /// Stop the recovered session for `tid` and invalidate its alarm.
    pub fn stop(&mut self, tid: u8) -> bool {
        let Some(index) = sta_tx_block_ack_index(tid) else {
            return false;
        };
        self.sessions[index].stop();
        self.alarms[index] = None;
        true
    }

    pub const fn operational(&self, tid: u8) -> Option<OperationalTxBlockAck> {
        let Some(index) = sta_tx_block_ack_index(tid) else {
            return None;
        };
        self.sessions[index].operational()
    }

    pub const fn alarm(&self, tid: u8) -> Option<TxBlockAckAlarm> {
        let Some(index) = sta_tx_block_ack_index(tid) else {
            return None;
        };
        self.alarms[index]
    }

    /// Earliest negotiation deadline already owned by this fixed session set.
    ///
    /// The control readiness path calls this for every serviced RX batch. Walk
    /// the three physical alarm slots directly instead of rebuilding the
    /// public TID-to-slot mapping on each query.
    #[inline(always)]
    pub const fn earliest_alarm_deadline(&self) -> Option<u64> {
        let mut earliest = None;
        let mut index = 0;
        while index < self.alarms.len() {
            if let Some(alarm) = self.alarms[index] {
                earliest = match earliest {
                    Some(deadline) if deadline <= alarm.deadline_us => Some(deadline),
                    _ => Some(alarm.deadline_us),
                };
            }
            index += 1;
        }
        earliest
    }
}

const fn sta_tx_block_ack_index(tid: u8) -> Option<usize> {
    match tid {
        0 => Some(0),
        7 => Some(1),
        5 => Some(2),
        _ => None,
    }
}

/// Opaque index of one statically owned TX frame.
///
/// The strict S31 data path has exactly 32 fixed TX slots. Keeping only their
/// indices here prevents the BlockAck state machine from owning raw pointers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TxAmpduSlot(u8);

impl TxAmpduSlot {
    pub const fn new(index: u8) -> Option<Self> {
        if (index as usize) < TX_AMPDU_SLOT_CAPACITY {
            Some(Self(index))
        } else {
            None
        }
    }

    pub const fn index(self) -> u8 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TxAmpduMpdu {
    pub slot: TxAmpduSlot,
    pub sequence: u16,
}

/// Semantic BlockAck information decoded by the PAC completion owner.
///
/// Bit zero acknowledges `starting_sequence`, bit one the following sequence,
/// and so on. The S31 completion block exposes 64 bits even though strict mode
/// deliberately negotiates a window of at most 32 frames.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TxBlockAckBitmap {
    pub starting_sequence: u16,
    pub bitmap: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HtBlockAckObservation {
    pub control: u8,
    pub block_ack: TxBlockAckBitmap,
}

impl HtBlockAckObservation {
    pub const fn new(control: u8, starting_sequence: u16, bitmap: u64) -> Self {
        Self {
            control,
            block_ack: TxBlockAckBitmap::new(starting_sequence, bitmap),
        }
    }
}

impl TxBlockAckBitmap {
    #[inline(always)]
    pub const fn new(starting_sequence: u16, bitmap: u64) -> Self {
        Self {
            starting_sequence: starting_sequence & SEQUENCE_NUMBER_MASK,
            bitmap,
        }
    }

    /// Return whether the transmitter must consider `sequence` complete.
    ///
    /// Peers use both standard-compliant SSN conventions: some retain the
    /// oldest possible sequence and describe it with the bitmap, while others
    /// advance SSN to the first not-yet-acknowledged sequence. In the latter
    /// form an MPDU immediately left of the new window is already complete
    /// even though it no longer has a bitmap bit. Only a bounded predecessor
    /// is admitted; a sequence beyond either side of the 64-entry BA window
    /// remains unacknowledged so a stale result cannot release new traffic.
    pub const fn acknowledges(self, sequence: u16) -> bool {
        let distance = sequence.wrapping_sub(self.starting_sequence) & SEQUENCE_NUMBER_MASK;
        if distance < BLOCK_ACK_BITMAP_BITS {
            self.bitmap & (1_u64 << distance) != 0
        } else {
            let predecessor_distance =
                self.starting_sequence.wrapping_sub(sequence) & SEQUENCE_NUMBER_MASK;
            predecessor_distance <= BLOCK_ACK_BITMAP_BITS
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TxAmpduDisposition {
    Acknowledged,
    Retry,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TxAmpduCompletion {
    pub mpdu: TxAmpduMpdu,
    pub disposition: TxAmpduDisposition,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TxAmpduBatchError {
    Busy,
    NotBuilding,
    Empty,
    InvalidWindow(u8),
    InvalidSlot(u8),
    DuplicateSlot(u8),
    DuplicateSequence(u16),
    Full,
}

#[derive(Clone, Copy)]
enum TxAmpduBatchPhase {
    Idle,
    Building,
    Completing(Option<TxBlockAckBitmap>),
}

/// One fixed TX A-MPDU batch owned by the Rust radio task.
///
/// `next_completion` returns at most one frame on every call. The executor can
/// therefore recycle or retry one MPDU and yield, instead of running the
/// vendor linked-list drains inside one PP event. There is no allocation,
/// clock read, retry loop, lock, or raw-pointer ownership in this type.
pub struct TxAmpduBatch {
    entries: [Option<TxAmpduMpdu>; TX_AMPDU_SLOT_CAPACITY],
    phase: TxAmpduBatchPhase,
    starting_sequence: u16,
    window: u8,
    count: u8,
    completion_index: u8,
    slot_mask: u32,
}

impl TxAmpduBatch {
    pub const fn new() -> Self {
        Self {
            entries: [None; TX_AMPDU_SLOT_CAPACITY],
            phase: TxAmpduBatchPhase::Idle,
            starting_sequence: 0,
            window: 0,
            count: 0,
            completion_index: 0,
            slot_mask: 0,
        }
    }

    pub fn begin(&mut self, starting_sequence: u16, window: u8) -> Result<(), TxAmpduBatchError> {
        if !matches!(self.phase, TxAmpduBatchPhase::Idle) {
            return Err(TxAmpduBatchError::Busy);
        }
        if window == 0 || usize::from(window) > TX_AMPDU_SLOT_CAPACITY {
            return Err(TxAmpduBatchError::InvalidWindow(window));
        }
        self.starting_sequence = starting_sequence & SEQUENCE_NUMBER_MASK;
        self.window = window;
        self.count = 0;
        self.completion_index = 0;
        self.slot_mask = 0;
        self.phase = TxAmpduBatchPhase::Building;
        Ok(())
    }

    /// Append one statically owned frame and assign its consecutive QoS
    /// sequence number. Duplicate slot ownership is rejected in O(1).
    pub fn push(&mut self, slot: u8) -> Result<TxAmpduMpdu, TxAmpduBatchError> {
        let sequence =
            self.starting_sequence.wrapping_add(u16::from(self.count)) & SEQUENCE_NUMBER_MASK;
        self.push_sequence(slot, sequence)
    }

    /// Append a statically owned frame whose sequence was already assigned by
    /// the finite PP framing leaf.
    ///
    /// This is the path used for a prepared hardware A-MPDU. It preserves the
    /// exact per-MPDU sequence numbers, including retry aggregates with holes,
    /// so BlockAck completion never depends on an inferred order.
    pub fn push_sequence(
        &mut self,
        slot: u8,
        sequence: u16,
    ) -> Result<TxAmpduMpdu, TxAmpduBatchError> {
        if !matches!(self.phase, TxAmpduBatchPhase::Building) {
            return Err(TxAmpduBatchError::NotBuilding);
        }
        let slot = TxAmpduSlot::new(slot).ok_or(TxAmpduBatchError::InvalidSlot(slot))?;
        let slot_bit = 1_u32 << slot.index();
        if self.slot_mask & slot_bit != 0 {
            return Err(TxAmpduBatchError::DuplicateSlot(slot.index()));
        }
        if self.count >= self.window {
            return Err(TxAmpduBatchError::Full);
        }

        let sequence = sequence & SEQUENCE_NUMBER_MASK;
        let mut index = 0_usize;
        while index < usize::from(self.count) {
            if self.entries[index].is_some_and(|entry| entry.sequence == sequence) {
                return Err(TxAmpduBatchError::DuplicateSequence(sequence));
            }
            index += 1;
        }
        let mpdu = TxAmpduMpdu { slot, sequence };
        self.entries[usize::from(self.count)] = Some(mpdu);
        self.count += 1;
        self.slot_mask |= slot_bit;
        Ok(mpdu)
    }

    pub fn complete_with_block_ack(
        &mut self,
        block_ack: TxBlockAckBitmap,
    ) -> Result<(), TxAmpduBatchError> {
        self.begin_completion(Some(block_ack))
    }

    /// Complete a hardware timeout/error edge. Every submitted MPDU is
    /// returned as `Retry`, one per `next_completion` call.
    pub fn complete_without_block_ack(&mut self) -> Result<(), TxAmpduBatchError> {
        self.begin_completion(None)
    }

    fn begin_completion(
        &mut self,
        block_ack: Option<TxBlockAckBitmap>,
    ) -> Result<(), TxAmpduBatchError> {
        if !matches!(self.phase, TxAmpduBatchPhase::Building) {
            return Err(TxAmpduBatchError::NotBuilding);
        }
        if self.count == 0 {
            return Err(TxAmpduBatchError::Empty);
        }
        self.completion_index = 0;
        self.phase = TxAmpduBatchPhase::Completing(block_ack);
        Ok(())
    }

    /// Consume exactly one completion result. Returning the last result also
    /// returns the batch to idle; no separate drain or cleanup loop exists.
    pub fn next_completion(&mut self) -> Option<TxAmpduCompletion> {
        let TxAmpduBatchPhase::Completing(block_ack) = self.phase else {
            return None;
        };
        if self.completion_index >= self.count {
            self.reset();
            return None;
        }

        let index = usize::from(self.completion_index);
        let mpdu = self.entries[index].take()?;
        self.completion_index += 1;
        self.slot_mask &= !(1_u32 << mpdu.slot.index());
        let disposition = if block_ack.is_some_and(|ack| ack.acknowledges(mpdu.sequence)) {
            TxAmpduDisposition::Acknowledged
        } else {
            TxAmpduDisposition::Retry
        };
        let completion = TxAmpduCompletion { mpdu, disposition };
        if self.completion_index == self.count {
            self.reset();
        }
        Some(completion)
    }

    pub const fn len(&self) -> usize {
        self.count as usize
    }

    pub const fn is_empty(&self) -> bool {
        self.count == 0
    }

    pub const fn is_idle(&self) -> bool {
        matches!(self.phase, TxAmpduBatchPhase::Idle)
    }

    fn reset(&mut self) {
        self.phase = TxAmpduBatchPhase::Idle;
        self.window = 0;
        self.count = 0;
        self.completion_index = 0;
        self.slot_mask = 0;
    }
}

impl Default for TxAmpduBatch {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::{StaTxBlockAckSessions, TxBlockAckDialogTokenSequence};

    #[test]
    fn shared_dialog_tokens_reproduce_the_qualified_vendor_modulus() {
        let mut tokens = TxBlockAckDialogTokenSequence::new();
        for expected in 1..=62 {
            assert_eq!(tokens.take().value(), expected);
        }
        assert_eq!(tokens.take().value(), 0);
        assert_eq!(tokens.take().value(), 1);
    }

    #[test]
    fn earliest_alarm_deadline_walks_owned_slots_without_tid_remapping() {
        let mut sessions = StaTxBlockAckSessions::new(16, 100_000, false).unwrap();
        assert_eq!(sessions.earliest_alarm_deadline(), None);

        sessions.begin(0, 0, 75).unwrap();
        sessions.begin(7, 0, 25).unwrap();
        sessions.begin(5, 0, 50).unwrap();

        assert_eq!(sessions.earliest_alarm_deadline(), Some(100_025));
        assert!(sessions.stop(7));
        assert_eq!(sessions.earliest_alarm_deadline(), Some(100_050));
    }
}
