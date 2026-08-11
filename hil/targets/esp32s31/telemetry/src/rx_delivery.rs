//! Exact, session-scoped UDP RX delivery evidence for qualification images.

use open_esp_radio_hil_protocol::{
    RxConsumerLedgerEvidence, RxDeliveryEvidence, RxMacOrderEvidence, RxReorderDeliveryEvidence,
    RxSequenceStageEvidence,
};

const SEEN_WINDOW: usize = 256;
const SEEN_WORDS: usize = SEEN_WINDOW / 64;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NetworkDropReason {
    QueueFull,
    InvalidLength,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SequenceDisposition {
    Control,
    First,
    InOrder,
    Gap,
    LateRecovered,
    Duplicate,
    BackwardUnclassified,
}

#[derive(Default)]
struct SequenceTracker {
    evidence: RxSequenceStageEvidence,
    seen: [u64; SEEN_WORDS],
    terminal_seen: bool,
}

impl SequenceTracker {
    fn observe(&mut self, sequence: i32) -> SequenceDisposition {
        if sequence < 0 {
            self.evidence.control_markers = self.evidence.control_markers.saturating_add(1);
            if self.evidence.data_units != 0 {
                self.terminal_seen = true;
            }
            return SequenceDisposition::Control;
        }
        let sequence = sequence as u32;
        if self.terminal_seen {
            self.evidence.data_after_terminal = self.evidence.data_after_terminal.saturating_add(1);
            self.record_anomaly(sequence);
        }
        self.evidence.data_units = self.evidence.data_units.saturating_add(1);
        let Some(highest) = self.evidence.highest else {
            self.evidence.first = Some(sequence);
            self.evidence.highest = Some(sequence);
            self.seen[0] = 1;
            return SequenceDisposition::First;
        };
        if sequence > highest {
            let advance = sequence - highest;
            self.shift_seen(advance);
            self.seen[0] |= 1;
            self.evidence.highest = Some(sequence);
            if advance == 1 {
                SequenceDisposition::InOrder
            } else {
                self.evidence.gap_events = self.evidence.gap_events.saturating_add(1);
                self.evidence.forward_missing = self
                    .evidence
                    .forward_missing
                    .saturating_add(advance.saturating_sub(1));
                self.record_anomaly(sequence);
                SequenceDisposition::Gap
            }
        } else {
            let distance = highest - sequence;
            if distance >= SEEN_WINDOW as u32 {
                self.evidence.backward_unclassified =
                    self.evidence.backward_unclassified.saturating_add(1);
                self.record_anomaly(sequence);
                SequenceDisposition::BackwardUnclassified
            } else if self.seen(distance as usize) {
                self.evidence.duplicates = self.evidence.duplicates.saturating_add(1);
                self.record_anomaly(sequence);
                SequenceDisposition::Duplicate
            } else {
                self.mark_seen(distance as usize);
                self.evidence.late_recovered = self.evidence.late_recovered.saturating_add(1);
                self.record_anomaly(sequence);
                SequenceDisposition::LateRecovered
            }
        }
    }

    fn record_anomaly(&mut self, sequence: u32) {
        self.evidence.first_anomaly.get_or_insert(sequence);
    }

    fn seen(&self, distance: usize) -> bool {
        self.seen[distance / 64] & (1_u64 << (distance % 64)) != 0
    }

    fn mark_seen(&mut self, distance: usize) {
        self.seen[distance / 64] |= 1_u64 << (distance % 64);
    }

    fn shift_seen(&mut self, advance: u32) {
        if advance >= SEEN_WINDOW as u32 {
            self.seen = [0; SEEN_WORDS];
            return;
        }
        let advance = advance as usize;
        let words = advance / 64;
        let bits = advance % 64;
        let old = self.seen;
        self.seen = [0; SEEN_WORDS];
        for destination in words..SEEN_WORDS {
            let source = destination - words;
            self.seen[destination] |= old[source] << bits;
            if bits != 0 && source > 0 {
                self.seen[destination] |= old[source - 1] >> (64 - bits);
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MacOrder {
    Backward,
    Same,
    Forward,
}

#[derive(Default)]
struct MacOrderTracker {
    expected: [Option<u16>; 16],
    last: Option<(u8, u16)>,
    evidence: RxMacOrderEvidence,
}

impl MacOrderTracker {
    fn observe(&mut self, disposition: SequenceDisposition, mac: Option<(u8, u16)>) {
        if disposition == SequenceDisposition::Control {
            return;
        }
        let previous = self.last;
        let order = mac.map(|(tid, sequence)| self.observe_mac(tid, sequence));
        self.last = mac;
        if !matches!(
            disposition,
            SequenceDisposition::LateRecovered | SequenceDisposition::BackwardUnclassified
        ) {
            return;
        }
        match (previous, mac, order) {
            (Some((previous_tid, _)), Some((tid, _)), _) if previous_tid != tid => {
                self.evidence.backward_mac_other_tid =
                    self.evidence.backward_mac_other_tid.saturating_add(1);
            }
            (_, Some(_), Some(MacOrder::Backward)) => {
                self.evidence.backward_mac_backward =
                    self.evidence.backward_mac_backward.saturating_add(1);
            }
            (_, Some(_), Some(MacOrder::Same)) => {
                self.evidence.backward_mac_same = self.evidence.backward_mac_same.saturating_add(1);
            }
            (_, Some(_), Some(MacOrder::Forward)) => {
                self.evidence.backward_mac_forward =
                    self.evidence.backward_mac_forward.saturating_add(1);
            }
            _ => {
                self.evidence.backward_mac_unavailable =
                    self.evidence.backward_mac_unavailable.saturating_add(1);
            }
        }
    }

    fn observe_mac(&mut self, tid: u8, sequence: u16) -> MacOrder {
        if self.last == Some((tid, sequence)) {
            return MacOrder::Same;
        }
        let Some(expected) = self.expected.get_mut(usize::from(tid)) else {
            return MacOrder::Forward;
        };
        let Some(frontier) = *expected else {
            *expected = Some(sequence.wrapping_add(1) & 0x0fff);
            return MacOrder::Forward;
        };
        let distance = sequence.wrapping_sub(frontier) & 0x0fff;
        if distance < 0x0800 {
            *expected = Some(sequence.wrapping_add(1) & 0x0fff);
            MacOrder::Forward
        } else {
            MacOrder::Backward
        }
    }
}

struct SequenceLedger<const CAPACITY: usize> {
    entries: [Option<u32>; CAPACITY],
    head: usize,
    length: usize,
    evidence: RxConsumerLedgerEvidence,
}

impl<const CAPACITY: usize> Default for SequenceLedger<CAPACITY> {
    fn default() -> Self {
        Self {
            entries: [None; CAPACITY],
            head: 0,
            length: 0,
            evidence: RxConsumerLedgerEvidence::default(),
        }
    }
}

impl<const CAPACITY: usize> SequenceLedger<CAPACITY> {
    fn push(&mut self, sequence: u32) {
        if self.length == CAPACITY {
            self.evidence.overflow = self.evidence.overflow.saturating_add(1);
            return;
        }
        let index = (self.head + self.length) % CAPACITY;
        self.entries[index] = Some(sequence);
        self.length += 1;
    }

    fn consume(&mut self, sequence: u32) {
        let position = (0..self.length).find(|offset| self.get(*offset) == Some(sequence));
        let Some(position) = position else {
            self.record_divergence(self.front(), sequence);
            self.evidence.unexpected_consumer = self.evidence.unexpected_consumer.saturating_add(1);
            return;
        };
        if position != 0 {
            self.record_divergence(self.front(), sequence);
            self.evidence.skipped_before_observed = self
                .evidence
                .skipped_before_observed
                .saturating_add(position as u32);
            for _ in 0..position {
                self.pop_front();
            }
        }
        self.pop_front();
        self.evidence.matched = self.evidence.matched.saturating_add(1);
    }

    fn finish(mut self) -> RxConsumerLedgerEvidence {
        self.evidence.enqueued_not_consumed = self.length as u32;
        self.evidence
    }

    fn get(&self, offset: usize) -> Option<u32> {
        self.entries[(self.head + offset) % CAPACITY]
    }

    fn front(&self) -> Option<u32> {
        (self.length != 0)
            .then(|| self.entries[self.head])
            .flatten()
    }

    fn pop_front(&mut self) {
        if self.length == 0 {
            return;
        }
        self.entries[self.head] = None;
        self.head = (self.head + 1) % CAPACITY;
        self.length -= 1;
    }

    fn record_divergence(&mut self, expected: Option<u32>, observed: u32) {
        if self.evidence.first_observed.is_none() {
            self.evidence.first_expected = expected;
            self.evidence.first_observed = Some(observed);
        }
    }
}

/// Mutable state shared by the post-reorder publisher and UDP consumer only
/// in the explicit delivery diagnostic profile.
pub struct RxDeliveryTracker<const LEDGER_CAPACITY: usize> {
    session_id: Option<u64>,
    post_reorder: SequenceTracker,
    network_enqueued: SequenceTracker,
    udp_consumer: SequenceTracker,
    ledger: SequenceLedger<LEDGER_CAPACITY>,
    mac: MacOrderTracker,
    network_queue_full: u32,
    network_invalid_length: u32,
}

impl<const LEDGER_CAPACITY: usize> RxDeliveryTracker<LEDGER_CAPACITY> {
    pub fn new() -> Self {
        Self {
            session_id: None,
            post_reorder: SequenceTracker::default(),
            network_enqueued: SequenceTracker::default(),
            udp_consumer: SequenceTracker::default(),
            ledger: SequenceLedger::default(),
            mac: MacOrderTracker::default(),
            network_queue_full: 0,
            network_invalid_length: 0,
        }
    }

    pub fn begin(&mut self, session_id: u64) {
        *self = Self::new();
        self.session_id = Some(session_id);
    }

    pub fn admitted(&mut self, sequence: i32, mac: Option<(u8, u16)>) {
        if self.session_id.is_none() {
            return;
        }
        let disposition = self.post_reorder.observe(sequence);
        self.mac.observe(disposition, mac);
        self.network_enqueued.observe(sequence);
        if sequence >= 0 {
            self.ledger.push(sequence as u32);
        }
    }

    pub fn dropped(&mut self, sequence: i32, mac: Option<(u8, u16)>, reason: NetworkDropReason) {
        if self.session_id.is_none() {
            return;
        }
        let disposition = self.post_reorder.observe(sequence);
        self.mac.observe(disposition, mac);
        match reason {
            NetworkDropReason::QueueFull => {
                self.network_queue_full = self.network_queue_full.saturating_add(1)
            }
            NetworkDropReason::InvalidLength => {
                self.network_invalid_length = self.network_invalid_length.saturating_add(1)
            }
        }
    }

    pub fn consumed(&mut self, session_id: u64, sequence: i32) {
        if self.session_id != Some(session_id) {
            return;
        }
        self.udp_consumer.observe(sequence);
        if sequence >= 0 {
            self.ledger.consume(sequence as u32);
        }
    }

    pub fn finish(
        &mut self,
        session_id: u64,
        reorder: RxReorderDeliveryEvidence,
    ) -> Option<RxDeliveryEvidence> {
        if self.session_id != Some(session_id) {
            return None;
        }
        self.session_id = None;
        let ledger = core::mem::take(&mut self.ledger).finish();
        Some(RxDeliveryEvidence {
            post_reorder: self.post_reorder.evidence,
            network_enqueued: self.network_enqueued.evidence,
            udp_consumer: self.udp_consumer.evidence,
            consumer_ledger: ledger,
            mac_order: self.mac.evidence,
            reorder,
            network_queue_full: self.network_queue_full,
            network_invalid_length: self.network_invalid_length,
        })
    }
}

impl<const LEDGER_CAPACITY: usize> Default for RxDeliveryTracker<LEDGER_CAPACITY> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sequence_tracker_separates_gap_recovery_duplicate_and_terminal_tail() {
        let mut tracker = SequenceTracker::default();
        for sequence in [0, 2, 1, 1, -1, 3] {
            tracker.observe(sequence);
        }
        assert_eq!(tracker.evidence.data_units, 5);
        assert_eq!(tracker.evidence.gap_events, 1);
        assert_eq!(tracker.evidence.forward_missing, 1);
        assert_eq!(tracker.evidence.late_recovered, 1);
        assert_eq!(tracker.evidence.duplicates, 1);
        assert_eq!(tracker.evidence.control_markers, 1);
        assert_eq!(tracker.evidence.data_after_terminal, 1);
    }

    #[test]
    fn ledger_reports_the_first_exact_enqueue_consumer_divergence() {
        let mut ledger = SequenceLedger::<8>::default();
        for sequence in [10, 11, 12] {
            ledger.push(sequence);
        }
        ledger.consume(10);
        ledger.consume(12);
        ledger.consume(11);
        let evidence = ledger.finish();
        assert_eq!(evidence.matched, 2);
        assert_eq!(evidence.skipped_before_observed, 1);
        assert_eq!(evidence.unexpected_consumer, 1);
        assert_eq!(evidence.first_expected, Some(11));
        assert_eq!(evidence.first_observed, Some(12));
    }

    #[test]
    fn session_tracker_accounts_network_drop_without_entering_ledger() {
        let mut tracker = RxDeliveryTracker::<8>::new();
        tracker.begin(7);
        tracker.admitted(0, Some((0, 100)));
        tracker.dropped(1, Some((0, 101)), NetworkDropReason::QueueFull);
        tracker.consumed(7, 0);
        tracker.consumed(8, 1);
        let evidence = tracker
            .finish(7, RxReorderDeliveryEvidence::default())
            .unwrap();
        assert_eq!(evidence.post_reorder.data_units, 2);
        assert_eq!(evidence.network_enqueued.data_units, 1);
        assert_eq!(evidence.udp_consumer.data_units, 1);
        assert_eq!(evidence.network_queue_full, 1);
        assert_eq!(evidence.consumer_ledger.matched, 1);
    }
}
