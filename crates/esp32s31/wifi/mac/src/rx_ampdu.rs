//! Allocation-free receive BlockAck reorder state in the live MAC crate.
//!
//! The vendor implementation allocates one 40-byte agreement object and a
//! variable pointer array for every receive TID. This module keeps the same
//! protocol ownership in a fixed array of slot indices. Raw packet pointers
//! stay outside the state machine.

use crate::rx_ampdu_hw::{S31_RX_BLOCK_ACK_MAX_TID, S31RxBlockAckAgreement};

// SOURCE: complete `_oracles/libnet80211.a[ieee80211_ht.o]::
// ampdu_rx_start.constprop.0`. The vendor agreement owner selects the smaller
// of the peer request and `g_wifi_menuconfig+0x30`, whose normal configured
// value is 64. Its later ordinary activation path passes the constant 64 to
// `ic_add_rx_ba`; complete `_oracles/libpp.a[hal_ampdu.o]::
// hal_agreement_add_rx_ba` publishes that value in the seven-bit hardware
// window. Keep the protocol/reorder window independent of the number of RX DMA
// descriptors: descriptors are recycled while this sequence window remains
// active. A reduced-memory build can still select eight explicitly.
#[cfg(feature = "rx-ba-window-8")]
pub const RX_BLOCK_ACK_MAX_WINDOW: u16 = 8;
#[cfg(not(feature = "rx-ba-window-8"))]
pub const RX_BLOCK_ACK_MAX_WINDOW: u16 = 64;
pub const RX_AMPDU_SLOT_CAPACITY: usize = RX_BLOCK_ACK_MAX_WINDOW as usize;
#[cfg(feature = "large-rx-pool-48")]
pub(crate) const RX_ESF_SLOT_ID_CAPACITY: usize = 48;
#[cfg(not(feature = "large-rx-pool-48"))]
pub(crate) const RX_ESF_SLOT_ID_CAPACITY: usize = 32;
// Rare multi-descriptor MPDUs use a separate split SRAM/PSRAM pool. Its
// IDs participate in reorder ownership, but must not inflate the ordinary
// zero-copy channel budget computed from `RX_ESF_SLOT_ID_CAPACITY`.
pub(crate) const RX_REORDER_SLOT_ID_CAPACITY: usize = RX_ESF_SLOT_ID_CAPACITY + 2;
const SEQUENCE_MASK: u16 = 0x0fff;
const SEQUENCE_HALF_RANGE: u16 = 0x0800;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RxAmpduMpdu {
    pub sequence: u16,
    pub slot: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RxAmpduError {
    InvalidWindow(u16),
    InvalidSequence(u16),
    InvalidSlot(u8),
    DuplicateSequence(u16),
    SlotAlreadyOwned(u8),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RxAddbaResponseError {
    InvalidBodyLength(usize),
    InvalidTid(u8),
    InvalidWindow(u16),
}

pub const STA_RX_BLOCK_ACK_BANK_COUNT: usize = 8;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaRxBlockAckSessionsError {
    DelayedPolicyUnsupported,
    InvalidTid(u8),
    InvalidWindow(u16),
    NonzeroTimeout(u16),
    InvalidStartingSequence(u16),
    ActivationBusy,
    StaleActivation,
    NoFreeHardwareBank,
    Reorder(RxAmpduError),
    Response(RxAddbaResponseError),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PendingStaRxBlockAck {
    dialog_token: u8,
    tid: u8,
    requested_window: u16,
    starting_sequence: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaRxBlockAckSnapshot {
    pub hardware_index: u8,
    pub tid: u8,
    pub window: u16,
    pub starting_sequence: u16,
}

struct ActiveStaRxBlockAck {
    snapshot: StaRxBlockAckSnapshot,
    // The agreement owns this software reorder state for exactly as long as
    // the corresponding hardware bank remains published.
    _reorder: RxBlockAckReorder,
}

/// One private activation transaction spanning software and hardware state.
///
/// A caller cannot construct or clone this value. It must either return it to
/// [`StaRxBlockAckSessions::commit`] after programming the hardware and
/// transmitting the response, or to [`StaRxBlockAckSessions::cancel`] after
/// undoing any hardware publication.
pub struct StaRxBlockAckActivation {
    generation: u32,
    hardware: S31RxBlockAckAgreement,
    response_body: [u8; 9],
    negotiated: StaRxBlockAckSnapshot,
    replaced: Option<StaRxBlockAckSnapshot>,
    reorder: RxBlockAckReorder,
}

impl StaRxBlockAckActivation {
    pub const fn hardware(&self) -> S31RxBlockAckAgreement {
        self.hardware
    }

    pub const fn response_body(&self) -> &[u8; 9] {
        &self.response_body
    }

    pub const fn negotiated(&self) -> StaRxBlockAckSnapshot {
        self.negotiated
    }

    pub const fn replaced(&self) -> Option<StaRxBlockAckSnapshot> {
        self.replaced
    }
}

/// Fixed owner of the eight ordinary S31 receive BlockAck banks.
///
/// SOURCE: complete `_oracles/libnet80211.a[ieee80211_ht.o]::
/// ampdu_rx_start.constprop.0` stores one independent agreement per TID and
/// bounds the negotiated software window by the configured maximum. Complete
/// `_oracles/libpp.a[hal_ampdu.o]::hal_agreement_add_rx_ba` owns eight
/// hardware banks and receives the literal hardware window 64 on the ordinary
/// STA path. The protocol window and hardware window therefore remain
/// deliberately distinct here.
pub struct StaRxBlockAckSessions {
    pending: [Option<PendingStaRxBlockAck>; STA_RX_BLOCK_ACK_BANK_COUNT],
    active: [Option<ActiveStaRxBlockAck>; STA_RX_BLOCK_ACK_BANK_COUNT],
    generation: u32,
    in_flight: Option<u32>,
}

impl StaRxBlockAckSessions {
    pub fn new() -> Self {
        Self {
            pending: [None; STA_RX_BLOCK_ACK_BANK_COUNT],
            active: core::array::from_fn(|_| None),
            generation: 0,
            in_flight: None,
        }
    }

    /// Admit one parsed immediate ADDBA request into its per-TID pending slot.
    /// A newer request for the same TID replaces the older unexecuted request,
    /// matching the former dispatcher behavior without exposing its arrays.
    pub fn offer(
        &mut self,
        dialog_token: u8,
        tid: u8,
        immediate: bool,
        requested_window: u16,
        timeout_tu: u16,
        starting_sequence: u16,
    ) -> Result<(), StaRxBlockAckSessionsError> {
        if !immediate {
            return Err(StaRxBlockAckSessionsError::DelayedPolicyUnsupported);
        }
        if tid > S31_RX_BLOCK_ACK_MAX_TID {
            return Err(StaRxBlockAckSessionsError::InvalidTid(tid));
        }
        if requested_window == 0 || requested_window > 0x03ff {
            return Err(StaRxBlockAckSessionsError::InvalidWindow(requested_window));
        }
        if timeout_tu != 0 {
            return Err(StaRxBlockAckSessionsError::NonzeroTimeout(timeout_tu));
        }
        if starting_sequence > 0x0fff {
            return Err(StaRxBlockAckSessionsError::InvalidStartingSequence(
                starting_sequence,
            ));
        }
        self.pending[usize::from(tid)] = Some(PendingStaRxBlockAck {
            dialog_token,
            tid,
            requested_window,
            starting_sequence,
        });
        Ok(())
    }

    /// Begin the oldest pending TID by numeric index and reserve its hardware
    /// bank until the caller commits or cancels the returned transaction.
    pub fn begin_pending(
        &mut self,
        peer: [u8; 6],
    ) -> Result<Option<StaRxBlockAckActivation>, StaRxBlockAckSessionsError> {
        if self.in_flight.is_some() {
            return Err(StaRxBlockAckSessionsError::ActivationBusy);
        }
        let Some((pending_index, request)) = self
            .pending
            .iter()
            .enumerate()
            .find_map(|(index, request)| request.map(|request| (index, request)))
        else {
            return Ok(None);
        };
        self.pending[pending_index] = None;

        let hardware_index = self
            .active
            .iter()
            .position(|agreement| {
                agreement
                    .as_ref()
                    .is_some_and(|agreement| agreement.snapshot.tid == request.tid)
            })
            .or_else(|| self.active.iter().position(Option::is_none))
            .ok_or(StaRxBlockAckSessionsError::NoFreeHardwareBank)?;
        let replaced = self.active[hardware_index]
            .take()
            .map(|agreement| agreement.snapshot);
        let window = request.requested_window.min(RX_BLOCK_ACK_MAX_WINDOW);
        let reorder = RxBlockAckReorder::new(request.starting_sequence, window)
            .map_err(StaRxBlockAckSessionsError::Reorder)?;
        let mut response_body = [0_u8; 9];
        write_successful_addba_response(
            &mut response_body,
            request.dialog_token,
            request.tid,
            window,
        )
        .map_err(StaRxBlockAckSessionsError::Response)?;

        self.generation = next_sta_rx_block_ack_generation(self.generation);
        self.in_flight = Some(self.generation);
        let negotiated = StaRxBlockAckSnapshot {
            hardware_index: hardware_index as u8,
            tid: request.tid,
            window,
            starting_sequence: request.starting_sequence,
        };
        Ok(Some(StaRxBlockAckActivation {
            generation: self.generation,
            hardware: S31RxBlockAckAgreement {
                hardware_index: hardware_index as u8,
                interface: 0,
                peer,
                tid: request.tid,
                starting_sequence: request.starting_sequence,
                // The vendor ordinary STA hardware leaf receives 64 even
                // when the negotiated/reorder window is smaller.
                window: RX_BLOCK_ACK_MAX_WINDOW,
            },
            response_body,
            negotiated,
            replaced,
            reorder,
        }))
    }

    pub fn commit(
        &mut self,
        activation: StaRxBlockAckActivation,
    ) -> Result<StaRxBlockAckSnapshot, StaRxBlockAckSessionsError> {
        if self.in_flight != Some(activation.generation) {
            return Err(StaRxBlockAckSessionsError::StaleActivation);
        }
        self.in_flight = None;
        let snapshot = activation.negotiated;
        self.active[usize::from(snapshot.hardware_index)] = Some(ActiveStaRxBlockAck {
            snapshot,
            _reorder: activation.reorder,
        });
        Ok(snapshot)
    }

    pub fn cancel(
        &mut self,
        activation: StaRxBlockAckActivation,
    ) -> Result<(), StaRxBlockAckSessionsError> {
        if self.in_flight != Some(activation.generation) {
            return Err(StaRxBlockAckSessionsError::StaleActivation);
        }
        self.in_flight = None;
        Ok(())
    }

    /// Remove the software owner before the caller clears the returned bank.
    pub fn stop(&mut self, tid: u8) -> Option<StaRxBlockAckSnapshot> {
        let index = self.active.iter().position(|agreement| {
            agreement
                .as_ref()
                .is_some_and(|agreement| agreement.snapshot.tid == tid)
        })?;
        self.active[index]
            .take()
            .map(|agreement| agreement.snapshot)
    }

    pub fn snapshots(&self) -> [Option<StaRxBlockAckSnapshot>; STA_RX_BLOCK_ACK_BANK_COUNT] {
        core::array::from_fn(|index| {
            self.active[index]
                .as_ref()
                .map(|agreement| agreement.snapshot)
        })
    }
}

impl Default for StaRxBlockAckSessions {
    fn default() -> Self {
        Self::new()
    }
}

const fn next_sta_rx_block_ack_generation(current: u32) -> u32 {
    let next = current.wrapping_add(1);
    if next == 0 { 1 } else { next }
}

/// Write one successful immediate BlockAck response into an owned action body.
///
/// A-MSDU is deliberately not advertised by the initial strict RX path.
pub fn write_successful_addba_response(
    body: &mut [u8],
    dialog_token: u8,
    tid: u8,
    window: u16,
) -> Result<(), RxAddbaResponseError> {
    if body.len() != 9 {
        return Err(RxAddbaResponseError::InvalidBodyLength(body.len()));
    }
    if tid > 15 {
        return Err(RxAddbaResponseError::InvalidTid(tid));
    }
    if window == 0 || window > 0x03ff {
        return Err(RxAddbaResponseError::InvalidWindow(window));
    }
    body.fill(0);
    body[0] = crate::tx_ampdu::BLOCK_ACK_CATEGORY;
    body[1] = crate::tx_ampdu::ADDBA_RESPONSE_ACTION;
    body[2] = dialog_token;
    let parameters = 1_u16 << 1 | u16::from(tid) << 2 | window << 6;
    body[5..7].copy_from_slice(&parameters.to_le_bytes());
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RxAmpduRelease {
    pub frames: [Option<RxAmpduMpdu>; RX_AMPDU_SLOT_CAPACITY],
    pub count: u8,
    pub missing: u16,
    pub rejected: Option<RxAmpduMpdu>,
    pub buffered: bool,
}

impl RxAmpduRelease {
    const fn empty() -> Self {
        Self {
            frames: [None; RX_AMPDU_SLOT_CAPACITY],
            count: 0,
            missing: 0,
            rejected: None,
            buffered: false,
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = RxAmpduMpdu> + '_ {
        self.frames[..usize::from(self.count)]
            .iter()
            .copied()
            .flatten()
    }

    fn push(&mut self, frame: RxAmpduMpdu) {
        let index = usize::from(self.count);
        debug_assert!(index < self.frames.len());
        self.frames[index] = Some(frame);
        self.count += 1;
    }
}

/// One receive BlockAck agreement with a fixed, build-selected maximum window.
///
/// Every retained packet is represented only by a checked slot index. The
/// owner maps that index to its SRAM ESF storage and recycles every frame
/// returned by `ingest`, `expire_gap`, or `stop`.
pub struct RxBlockAckReorder {
    next_sequence: u16,
    window: u16,
    occupied: u64,
    frames: [Option<RxAmpduMpdu>; RX_AMPDU_SLOT_CAPACITY],
}

impl RxBlockAckReorder {
    pub const fn new(starting_sequence: u16, window: u16) -> Result<Self, RxAmpduError> {
        if window == 0 || window > RX_BLOCK_ACK_MAX_WINDOW {
            return Err(RxAmpduError::InvalidWindow(window));
        }
        if starting_sequence > SEQUENCE_MASK {
            return Err(RxAmpduError::InvalidSequence(starting_sequence));
        }
        Ok(Self {
            next_sequence: starting_sequence,
            window,
            occupied: 0,
            frames: [None; RX_AMPDU_SLOT_CAPACITY],
        })
    }

    pub const fn next_sequence(&self) -> u16 {
        self.next_sequence
    }

    pub const fn window(&self) -> u16 {
        self.window
    }

    pub const fn occupied(&self) -> u32 {
        self.occupied.count_ones()
    }

    pub fn ingest(&mut self, frame: RxAmpduMpdu) -> Result<RxAmpduRelease, RxAmpduError> {
        if frame.sequence > SEQUENCE_MASK {
            return Err(RxAmpduError::InvalidSequence(frame.sequence));
        }
        if usize::from(frame.slot) >= RX_REORDER_SLOT_ID_CAPACITY {
            return Err(RxAmpduError::InvalidSlot(frame.slot));
        }
        if self
            .frames
            .iter()
            .flatten()
            .any(|owned| owned.slot == frame.slot)
        {
            return Err(RxAmpduError::SlotAlreadyOwned(frame.slot));
        }

        let mut release = RxAmpduRelease::empty();
        let distance = forward_distance(self.next_sequence, frame.sequence);
        if distance >= SEQUENCE_HALF_RANGE {
            release.rejected = Some(frame);
            return Ok(release);
        }
        if distance >= self.window {
            let advance = distance - self.window + 1;
            self.advance(advance, &mut release);
        }

        let index = slot_index(frame.sequence);
        if self.occupied & (1_u64 << index) != 0 {
            return Err(RxAmpduError::DuplicateSequence(frame.sequence));
        }
        self.frames[index] = Some(frame);
        self.occupied |= 1_u64 << index;
        self.release_contiguous(&mut release);
        release.buffered = self.occupied & (1_u64 << index) != 0;
        Ok(release)
    }

    /// Release the first buffered run after an async reorder-age timer edge.
    ///
    /// The scan is bounded by the negotiated window. It never reads time or
    /// waits; the async owner decides when this edge is due.
    pub fn expire_gap(&mut self) -> RxAmpduRelease {
        let mut release = RxAmpduRelease::empty();
        let mut distance = 0_u16;
        while distance < self.window {
            let sequence = wrapping_sequence(self.next_sequence, distance);
            if self.has_sequence(sequence) {
                self.advance(distance, &mut release);
                self.release_contiguous(&mut release);
                return release;
            }
            distance += 1;
        }
        release
    }

    pub fn stop(&mut self) -> RxAmpduRelease {
        let mut release = RxAmpduRelease::empty();
        let mut distance = 0_u16;
        while distance < self.window {
            let sequence = wrapping_sequence(self.next_sequence, distance);
            if let Some(frame) = self.take_sequence(sequence) {
                release.push(frame);
            }
            distance += 1;
        }
        self.occupied = 0;
        release
    }

    fn advance(&mut self, count: u16, release: &mut RxAmpduRelease) {
        let retained_span = count.min(self.window);
        let released_before = release.count;
        let mut offset = 0_u16;
        while offset < retained_span {
            let sequence = wrapping_sequence(self.next_sequence, offset);
            if let Some(frame) = self.take_sequence(sequence) {
                release.push(frame);
            }
            offset += 1;
        }
        let released_while_advancing = release.count - released_before;
        release.missing = release
            .missing
            .saturating_add(count.saturating_sub(u16::from(released_while_advancing)));
        self.next_sequence = wrapping_sequence(self.next_sequence, count);
    }

    fn release_contiguous(&mut self, release: &mut RxAmpduRelease) {
        let mut count = 0_u16;
        while count < self.window {
            let Some(frame) = self.take_sequence(self.next_sequence) else {
                break;
            };
            release.push(frame);
            self.next_sequence = wrapping_sequence(self.next_sequence, 1);
            count += 1;
        }
    }

    fn has_sequence(&self, sequence: u16) -> bool {
        let index = slot_index(sequence);
        self.occupied & (1_u64 << index) != 0
            && self.frames[index].is_some_and(|frame| frame.sequence == sequence)
    }

    fn take_sequence(&mut self, sequence: u16) -> Option<RxAmpduMpdu> {
        let index = slot_index(sequence);
        if !self.has_sequence(sequence) {
            return None;
        }
        self.occupied &= !(1_u64 << index);
        self.frames[index].take()
    }
}

const fn forward_distance(from: u16, to: u16) -> u16 {
    to.wrapping_sub(from) & SEQUENCE_MASK
}

const fn wrapping_sequence(sequence: u16, increment: u16) -> u16 {
    sequence.wrapping_add(increment) & SEQUENCE_MASK
}

const fn slot_index(sequence: u16) -> usize {
    sequence as usize % RX_AMPDU_SLOT_CAPACITY
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(sequence: u16, slot: u8) -> RxAmpduMpdu {
        RxAmpduMpdu { sequence, slot }
    }

    #[test]
    fn in_order_frames_are_released_immediately() {
        let mut reorder = RxBlockAckReorder::new(10, RX_BLOCK_ACK_MAX_WINDOW).unwrap();
        let release = reorder.ingest(frame(10, 0)).unwrap();
        assert_eq!(release.iter().collect::<std::vec::Vec<_>>(), [frame(10, 0)]);
        assert_eq!(reorder.next_sequence(), 11);
        assert_eq!(reorder.occupied(), 0);
    }

    #[test]
    fn gap_is_buffered_and_then_released_in_sequence_order() {
        let mut reorder = RxBlockAckReorder::new(100, RX_BLOCK_ACK_MAX_WINDOW).unwrap();
        assert!(reorder.ingest(frame(102, 2)).unwrap().buffered);
        assert!(reorder.ingest(frame(101, 1)).unwrap().buffered);
        let release = reorder.ingest(frame(100, 0)).unwrap();
        assert_eq!(
            release.iter().collect::<std::vec::Vec<_>>(),
            [frame(100, 0), frame(101, 1), frame(102, 2)]
        );
        assert_eq!(reorder.next_sequence(), 103);
    }

    #[test]
    #[cfg(not(feature = "rx-ba-window-8"))]
    fn window_advance_releases_owned_frames_and_counts_missing_without_long_loop() {
        let mut reorder = RxBlockAckReorder::new(0, RX_BLOCK_ACK_MAX_WINDOW).unwrap();
        reorder.ingest(frame(2, 2)).unwrap();
        reorder.ingest(frame(31, 31)).unwrap();
        let release = reorder.ingest(frame(1000, 1)).unwrap();
        assert_eq!(
            release.iter().collect::<std::vec::Vec<_>>(),
            [frame(2, 2), frame(31, 31)]
        );
        let expected_advance = 1000 - RX_BLOCK_ACK_MAX_WINDOW + 1;
        assert_eq!(release.missing, expected_advance - 2);
        assert!(release.buffered);
        assert_eq!(reorder.next_sequence(), expected_advance);
    }

    #[test]
    fn async_expiry_skips_only_the_current_gap() {
        let mut reorder = RxBlockAckReorder::new(20, 8).unwrap();
        reorder.ingest(frame(22, 2)).unwrap();
        reorder.ingest(frame(23, 3)).unwrap();
        let release = reorder.expire_gap();
        assert_eq!(release.missing, 2);
        assert_eq!(
            release.iter().collect::<std::vec::Vec<_>>(),
            [frame(22, 2), frame(23, 3)]
        );
        assert_eq!(reorder.next_sequence(), 24);
    }

    #[test]
    fn sequence_wrap_and_stale_rejection_are_unambiguous() {
        let mut reorder = RxBlockAckReorder::new(0x0fff, 8).unwrap();
        reorder.ingest(frame(0, 1)).unwrap();
        let release = reorder.ingest(frame(0x0fff, 0)).unwrap();
        assert_eq!(
            release.iter().collect::<std::vec::Vec<_>>(),
            [frame(0x0fff, 0), frame(0, 1)]
        );
        let stale = reorder.ingest(frame(0x0fff, 2)).unwrap();
        assert_eq!(stale.rejected, Some(frame(0x0fff, 2)));
    }

    #[test]
    fn a_slot_index_cannot_be_owned_twice() {
        let mut reorder = RxBlockAckReorder::new(1, 8).unwrap();
        reorder.ingest(frame(2, 4)).unwrap();
        assert_eq!(
            reorder.ingest(frame(3, 4)),
            Err(RxAmpduError::SlotAlreadyOwned(4))
        );
    }

    #[test]
    fn esf_slot_id_is_independent_of_reorder_window_index() {
        let mut reorder = RxBlockAckReorder::new(1, RX_BLOCK_ACK_MAX_WINDOW).unwrap();
        let highest_valid = (RX_REORDER_SLOT_ID_CAPACITY - 1) as u8;
        let first_invalid = RX_REORDER_SLOT_ID_CAPACITY as u8;
        assert_eq!(
            reorder
                .ingest(frame(1, highest_valid))
                .unwrap()
                .iter()
                .collect::<std::vec::Vec<_>>(),
            [frame(1, highest_valid)]
        );
        assert_eq!(
            reorder.ingest(frame(2, first_invalid)),
            Err(RxAmpduError::InvalidSlot(first_invalid))
        );
    }

    #[test]
    fn window_cannot_exceed_the_owned_reorder_slot_pool() {
        assert!(RxBlockAckReorder::new(0, RX_BLOCK_ACK_MAX_WINDOW).is_ok());
        assert!(matches!(
            RxBlockAckReorder::new(0, RX_BLOCK_ACK_MAX_WINDOW + 1),
            Err(RxAmpduError::InvalidWindow(_))
        ));
    }

    #[test]
    fn stop_releases_every_owned_slot_in_sequence_order() {
        let mut reorder = RxBlockAckReorder::new(4094, 8).unwrap();
        reorder.ingest(frame(1, 3)).unwrap();
        reorder.ingest(frame(4095, 1)).unwrap();
        let release = reorder.stop();
        assert_eq!(
            release.iter().collect::<std::vec::Vec<_>>(),
            [frame(4095, 1), frame(1, 3)]
        );
        assert_eq!(reorder.occupied(), 0);
    }

    #[test]
    fn successful_response_narrows_the_window_and_disables_amsdu() {
        let mut body = [0xff; 9];
        write_successful_addba_response(&mut body, 137, 0, 16).unwrap();
        assert_eq!(body, [3, 1, 137, 0, 0, 0x02, 0x04, 0, 0]);
        assert_eq!(
            crate::tx_ampdu::parse_block_ack_action(&body),
            Some(crate::tx_ampdu::BlockAckAction::AddbaResponse {
                dialog_token: 137,
                status: 0,
                tid: 0,
                immediate: true,
                amsdu: false,
                window: 16,
                timeout_tu: 0,
            })
        );
    }

    #[test]
    fn station_rx_sessions_bind_protocol_window_hardware_bank_and_response() {
        let peer = [0x30, 0xed, 0xa0, 0xf3, 0xf6, 0xd0];
        let mut sessions = StaRxBlockAckSessions::new();
        sessions.offer(17, 7, true, 1023, 0, 0x0abc).unwrap();
        let activation = sessions.begin_pending(peer).unwrap().unwrap();
        assert_eq!(
            activation.negotiated(),
            StaRxBlockAckSnapshot {
                hardware_index: 0,
                tid: 7,
                window: RX_BLOCK_ACK_MAX_WINDOW,
                starting_sequence: 0x0abc,
            }
        );
        assert_eq!(
            activation.hardware(),
            S31RxBlockAckAgreement {
                hardware_index: 0,
                interface: 0,
                peer,
                tid: 7,
                starting_sequence: 0x0abc,
                window: RX_BLOCK_ACK_MAX_WINDOW,
            }
        );
        assert_eq!(
            crate::tx_ampdu::parse_block_ack_action(activation.response_body()),
            Some(crate::tx_ampdu::BlockAckAction::AddbaResponse {
                dialog_token: 17,
                status: 0,
                tid: 7,
                immediate: true,
                amsdu: false,
                window: RX_BLOCK_ACK_MAX_WINDOW,
                timeout_tu: 0,
            })
        );
        assert!(matches!(
            sessions.begin_pending(peer),
            Err(StaRxBlockAckSessionsError::ActivationBusy)
        ));
        let snapshot = sessions.commit(activation).unwrap();
        assert_eq!(sessions.snapshots()[0], Some(snapshot));
        assert_eq!(sessions.stop(7), Some(snapshot));
        assert_eq!(sessions.snapshots(), [None; STA_RX_BLOCK_ACK_BANK_COUNT]);
    }

    #[test]
    fn replacement_and_cancel_remove_the_previous_hardware_owner() {
        let peer = [0x30, 0xed, 0xa0, 0xf3, 0xf6, 0xd0];
        let mut sessions = StaRxBlockAckSessions::new();
        sessions.offer(1, 0, true, 32, 0, 10).unwrap();
        let first = sessions.begin_pending(peer).unwrap().unwrap();
        let first_snapshot = sessions.commit(first).unwrap();

        sessions.offer(2, 0, true, 16, 0, 20).unwrap();
        let replacement = sessions.begin_pending(peer).unwrap().unwrap();
        assert_eq!(replacement.replaced(), Some(first_snapshot));
        assert_eq!(replacement.hardware().hardware_index, 0);
        sessions.cancel(replacement).unwrap();
        assert_eq!(sessions.snapshots(), [None; STA_RX_BLOCK_ACK_BANK_COUNT]);
    }

    #[test]
    fn station_rx_sessions_reject_every_unsupported_request_class() {
        let mut sessions = StaRxBlockAckSessions::new();
        assert_eq!(
            sessions.offer(1, 0, false, 32, 0, 0),
            Err(StaRxBlockAckSessionsError::DelayedPolicyUnsupported)
        );
        assert_eq!(
            sessions.offer(1, 8, true, 32, 0, 0),
            Err(StaRxBlockAckSessionsError::InvalidTid(8))
        );
        assert_eq!(
            sessions.offer(1, 0, true, 0, 0, 0),
            Err(StaRxBlockAckSessionsError::InvalidWindow(0))
        );
        assert_eq!(
            sessions.offer(1, 0, true, 32, 1, 0),
            Err(StaRxBlockAckSessionsError::NonzeroTimeout(1))
        );
        assert_eq!(
            sessions.offer(1, 0, true, 32, 0, 0x1000),
            Err(StaRxBlockAckSessionsError::InvalidStartingSequence(0x1000))
        );
    }
}
