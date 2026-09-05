//! Application-visible latest state of the supervised station link.

use core::cell::RefCell;

use embassy_sync::{
    blocking_mutex::{Mutex, raw::CriticalSectionRawMutex},
    signal::Signal,
};

use open_esp_radio_esp32s31_wifi_sta::connected_control::ConnectedDisconnectReason;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31StationLinkState {
    Disconnected(Option<ConnectedDisconnectReason>),
    Connected,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Esp32s31StationStatusSnapshot {
    pub revision: u64,
    pub state: Esp32s31StationLinkState,
    /// Bit per QoS TID whose station TX BlockAck agreement is operational.
    pub tx_block_ack_operational_tids: u8,
}

impl Esp32s31StationStatusSnapshot {
    const INITIAL: Self = Self {
        revision: 0,
        state: Esp32s31StationLinkState::Disconnected(None),
        tx_block_ack_operational_tids: 0,
    };
}

struct StationStatusChannel {
    snapshot: Mutex<CriticalSectionRawMutex, RefCell<Esp32s31StationStatusSnapshot>>,
    changed: Signal<CriticalSectionRawMutex, ()>,
}

impl StationStatusChannel {
    const fn new() -> Self {
        Self {
            snapshot: Mutex::new(RefCell::new(Esp32s31StationStatusSnapshot::INITIAL)),
            changed: Signal::new(),
        }
    }

    fn snapshot(&self) -> Esp32s31StationStatusSnapshot {
        self.snapshot.lock(|snapshot| *snapshot.borrow())
    }

    fn publish_link(&self, state: Esp32s31StationLinkState) {
        self.snapshot.lock(|snapshot| {
            let revision = snapshot.borrow().revision.wrapping_add(1);
            *snapshot.borrow_mut() = Esp32s31StationStatusSnapshot {
                revision,
                state,
                tx_block_ack_operational_tids: 0,
            };
        });
        self.changed.signal(());
    }

    fn publish_tx_block_ack(&self, tid: u8, operational: bool) {
        let Some(bit) = 1_u8.checked_shl(u32::from(tid)) else {
            return;
        };
        let changed = self.snapshot.lock(|snapshot| {
            let mut snapshot = snapshot.borrow_mut();
            let updated = if operational {
                snapshot.tx_block_ack_operational_tids | bit
            } else {
                snapshot.tx_block_ack_operational_tids & !bit
            };
            if updated == snapshot.tx_block_ack_operational_tids {
                return false;
            }
            snapshot.tx_block_ack_operational_tids = updated;
            snapshot.revision = snapshot.revision.wrapping_add(1);
            true
        });
        if changed {
            self.changed.signal(());
        }
    }
}

static STATION_STATUS: StationStatusChannel = StationStatusChannel::new();

/// Single application receiver for the production station link state.
pub struct Esp32s31StationStatus {
    seen_revision: u64,
}

impl Esp32s31StationStatus {
    pub(crate) fn new() -> Self {
        Self {
            seen_revision: STATION_STATUS.snapshot().revision,
        }
    }

    pub fn snapshot(&self) -> Esp32s31StationStatusSnapshot {
        STATION_STATUS.snapshot()
    }

    pub async fn changed(&mut self) -> Esp32s31StationStatusSnapshot {
        loop {
            let snapshot = STATION_STATUS.snapshot();
            if snapshot.revision != self.seen_revision {
                self.seen_revision = snapshot.revision;
                return snapshot;
            }
            STATION_STATUS.changed.wait().await;
        }
    }
}

pub(crate) fn publish_station_connected() {
    STATION_STATUS.publish_link(Esp32s31StationLinkState::Connected);
}

pub(crate) fn publish_station_disconnected(reason: ConnectedDisconnectReason) {
    STATION_STATUS.publish_link(Esp32s31StationLinkState::Disconnected(Some(reason)));
}

pub(crate) fn publish_station_tx_block_ack(tid: u8, operational: bool) {
    STATION_STATUS.publish_tx_block_ack(tid, operational);
}

#[cfg(test)]
mod tests;
