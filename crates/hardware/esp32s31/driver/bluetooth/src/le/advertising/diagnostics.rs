//! Optional observations before advertising RX storage is recycled.
//! Header counts describe SRAM observations, not validated Link Layer requests.

use core::cell::Cell;
use critical_section::Mutex;
use oer_esp32s31_bluetooth_memory::LeRxNodeObservation;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct AdvertisingRxDiagnostics {
    pub events: u32,
    pub scan_headers: u32,
    pub connect_headers: u32,
    pub unfinished_packets: u32,
    pub last_progress: Option<[LeRxNodeObservation; 2]>,
}

static RX: Mutex<Cell<AdvertisingRxDiagnostics>> =
    Mutex::new(Cell::new(AdvertisingRxDiagnostics {
        events: 0,
        scan_headers: 0,
        connect_headers: 0,
        unfinished_packets: 0,
        last_progress: None,
    }));

pub fn snapshot() -> AdvertisingRxDiagnostics {
    critical_section::with(|cs| RX.borrow(cs).get())
}

#[cfg(any(target_arch = "riscv32", test))]
impl AdvertisingRxDiagnostics {
    fn observe(&mut self, nodes: [LeRxNodeObservation; 2]) {
        self.events = self.events.saturating_add(1);
        if nodes
            .iter()
            .any(|node| node.producer_updated || node.epoch_updated)
        {
            self.last_progress = Some(nodes);
        }
        for node in nodes {
            if !node.completed && (node.producer_updated || node.epoch_updated) {
                self.unfinished_packets = self.unfinished_packets.saturating_add(1);
            }
            match node.header.map(|header| header & 0x0f) {
                Some(3) => self.scan_headers = self.scan_headers.saturating_add(1),
                Some(5) => self.connect_headers = self.connect_headers.saturating_add(1),
                _ => {}
            }
        }
    }
}

#[cfg(target_arch = "riscv32")]
pub(crate) fn record(nodes: [LeRxNodeObservation; 2]) {
    critical_section::with(|cs| {
        let mut state = RX.borrow(cs).get();
        state.observe(nodes);
        RX.borrow(cs).set(state);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unfinished_connection_header_survives_later_empty_events() {
        let empty = LeRxNodeObservation {
            completed: false,
            packet_retained: true,
            producer_updated: false,
            epoch_updated: false,
            header: None,
        };
        let pending = LeRxNodeObservation {
            producer_updated: true,
            epoch_updated: true,
            header: Some(5),
            ..empty
        };
        let mut state = AdvertisingRxDiagnostics::default();
        state.observe([pending, empty]);
        state.observe([empty; 2]);
        assert_eq!(state.events, 2);
        assert_eq!(state.connect_headers, 1);
        assert_eq!(state.unfinished_packets, 1);
        assert_eq!(state.last_progress, Some([pending, empty]));
    }
}
