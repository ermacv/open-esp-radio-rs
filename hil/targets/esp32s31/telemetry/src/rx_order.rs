//! Correlation of application UDP order with public 802.11 sequence/TID.

use core::sync::atomic::{AtomicU32, Ordering};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RxOrderSnapshot {
    pub gap_events: u32,
    pub forward_missing: u32,
    pub backward: u32,
    pub adjacent_duplicates: u32,
    pub backward_mac_backward: u32,
    pub backward_mac_same: u32,
    pub backward_mac_forward: u32,
    pub backward_mac_other_tid: u32,
    pub backward_mac_unavailable: u32,
}

impl RxOrderSnapshot {
    pub fn wrapping_delta_since(self, earlier: Self) -> Self {
        Self {
            gap_events: self.gap_events.wrapping_sub(earlier.gap_events),
            forward_missing: self.forward_missing.wrapping_sub(earlier.forward_missing),
            backward: self.backward.wrapping_sub(earlier.backward),
            adjacent_duplicates: self
                .adjacent_duplicates
                .wrapping_sub(earlier.adjacent_duplicates),
            backward_mac_backward: self
                .backward_mac_backward
                .wrapping_sub(earlier.backward_mac_backward),
            backward_mac_same: self
                .backward_mac_same
                .wrapping_sub(earlier.backward_mac_same),
            backward_mac_forward: self
                .backward_mac_forward
                .wrapping_sub(earlier.backward_mac_forward),
            backward_mac_other_tid: self
                .backward_mac_other_tid
                .wrapping_sub(earlier.backward_mac_other_tid),
            backward_mac_unavailable: self
                .backward_mac_unavailable
                .wrapping_sub(earlier.backward_mac_unavailable),
        }
    }
}

pub struct RxOrderCounters {
    gap_events: AtomicU32,
    forward_missing: AtomicU32,
    backward: AtomicU32,
    adjacent_duplicates: AtomicU32,
    backward_mac_backward: AtomicU32,
    backward_mac_same: AtomicU32,
    backward_mac_forward: AtomicU32,
    backward_mac_other_tid: AtomicU32,
    backward_mac_unavailable: AtomicU32,
}

impl RxOrderCounters {
    pub const fn new() -> Self {
        Self {
            gap_events: AtomicU32::new(0),
            forward_missing: AtomicU32::new(0),
            backward: AtomicU32::new(0),
            adjacent_duplicates: AtomicU32::new(0),
            backward_mac_backward: AtomicU32::new(0),
            backward_mac_same: AtomicU32::new(0),
            backward_mac_forward: AtomicU32::new(0),
            backward_mac_other_tid: AtomicU32::new(0),
            backward_mac_unavailable: AtomicU32::new(0),
        }
    }

    pub fn snapshot(&self) -> RxOrderSnapshot {
        RxOrderSnapshot {
            gap_events: self.gap_events.load(Ordering::Relaxed),
            forward_missing: self.forward_missing.load(Ordering::Relaxed),
            backward: self.backward.load(Ordering::Relaxed),
            adjacent_duplicates: self.adjacent_duplicates.load(Ordering::Relaxed),
            backward_mac_backward: self.backward_mac_backward.load(Ordering::Relaxed),
            backward_mac_same: self.backward_mac_same.load(Ordering::Relaxed),
            backward_mac_forward: self.backward_mac_forward.load(Ordering::Relaxed),
            backward_mac_other_tid: self.backward_mac_other_tid.load(Ordering::Relaxed),
            backward_mac_unavailable: self.backward_mac_unavailable.load(Ordering::Relaxed),
        }
    }
}

impl Default for RxOrderCounters {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MacOrder {
    Backward,
    SameMpdu,
    Forward,
}

#[derive(Default)]
pub struct RxOrderTracker {
    udp_expected: Option<u32>,
    mac_expected: [Option<u16>; 16],
    last_mac: Option<(u8, u16)>,
}

impl RxOrderTracker {
    pub fn observe(
        &mut self,
        counters: &RxOrderCounters,
        udp_sequence: i32,
        mac: Option<(u8, u16)>,
    ) {
        if udp_sequence < 0 {
            self.reset();
            return;
        }
        let udp_sequence = udp_sequence as u32;
        let mac_order = mac.map(|(tid, sequence)| self.observe_mac(tid, sequence));
        let previous_mac = self.last_mac;
        self.last_mac = mac;

        let Some(expected) = self.udp_expected else {
            self.udp_expected = Some(udp_sequence.saturating_add(1));
            return;
        };
        if udp_sequence == expected {
            self.udp_expected = Some(udp_sequence.saturating_add(1));
        } else if udp_sequence > expected {
            counters.gap_events.fetch_add(1, Ordering::Relaxed);
            counters
                .forward_missing
                .fetch_add(udp_sequence - expected, Ordering::Relaxed);
            self.udp_expected = Some(udp_sequence.saturating_add(1));
        } else if udp_sequence.saturating_add(1) == expected {
            counters.adjacent_duplicates.fetch_add(1, Ordering::Relaxed);
        } else {
            counters.backward.fetch_add(1, Ordering::Relaxed);
            let counter = match (previous_mac, mac, mac_order) {
                (Some((previous_tid, _)), Some((tid, _)), _) if previous_tid != tid => {
                    &counters.backward_mac_other_tid
                }
                (_, Some(_), Some(MacOrder::Backward)) => &counters.backward_mac_backward,
                (_, Some(_), Some(MacOrder::SameMpdu)) => &counters.backward_mac_same,
                (_, Some(_), Some(MacOrder::Forward)) => &counters.backward_mac_forward,
                _ => &counters.backward_mac_unavailable,
            };
            counter.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn reset(&mut self) {
        self.udp_expected = None;
        self.mac_expected.fill(None);
        self.last_mac = None;
    }

    fn observe_mac(&mut self, tid: u8, sequence: u16) -> MacOrder {
        if self.last_mac == Some((tid, sequence)) {
            return MacOrder::SameMpdu;
        }
        let Some(expected) = self.mac_expected.get_mut(usize::from(tid)) else {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracker_correlates_udp_regression_with_mac_direction() {
        let counters = RxOrderCounters::new();
        let mut tracker = RxOrderTracker::default();
        tracker.observe(&counters, 10, Some((0, 100)));
        tracker.observe(&counters, 12, Some((0, 101)));
        tracker.observe(&counters, 9, Some((0, 99)));
        let snapshot = counters.snapshot();
        assert_eq!(snapshot.gap_events, 1);
        assert_eq!(snapshot.forward_missing, 1);
        assert_eq!(snapshot.backward, 1);
        assert_eq!(snapshot.backward_mac_backward, 1);
    }
}
