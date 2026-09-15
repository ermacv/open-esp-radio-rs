//! Application-visible latest state of the supervised station link.

use core::cell::RefCell;

use embassy_sync::{
    blocking_mutex::{Mutex, raw::CriticalSectionRawMutex},
    signal::Signal,
};

use oer_esp32s31_wifi_sta::connected_control::ConnectedDisconnectReason;
use oer_ieee80211::security::WifiSecurityMode;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StationLinkState {
    Disconnected(Option<ConnectedDisconnectReason>),
    Connected,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StationLinkSecurity {
    Open,
    Wpa2Personal,
}

impl From<WifiSecurityMode> for StationLinkSecurity {
    fn from(mode: WifiSecurityMode) -> Self {
        match mode {
            WifiSecurityMode::Open => Self::Open,
            WifiSecurityMode::Wpa2Personal => Self::Wpa2Personal,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StationStatusSnapshot {
    pub revision: u64,
    pub state: StationLinkState,
    /// Bit per QoS TID whose station TX BlockAck agreement is operational.
    pub tx_block_ack_operational_tids: u8,
    /// Negotiated association width from the production connected report.
    pub association_bandwidth_mhz: Option<u16>,
    /// Security owner actually installed before publishing Connected.
    pub link_security: Option<StationLinkSecurity>,
}

impl StationStatusSnapshot {
    const INITIAL: Self = Self {
        revision: 0,
        state: StationLinkState::Disconnected(None),
        tx_block_ack_operational_tids: 0,
        association_bandwidth_mhz: None,
        link_security: None,
    };
}

struct StationStatusChannel {
    snapshot: Mutex<CriticalSectionRawMutex, RefCell<StationStatusSnapshot>>,
    changed: Signal<CriticalSectionRawMutex, ()>,
}

impl StationStatusChannel {
    const fn new() -> Self {
        Self {
            snapshot: Mutex::new(RefCell::new(StationStatusSnapshot::INITIAL)),
            changed: Signal::new(),
        }
    }

    fn snapshot(&self) -> StationStatusSnapshot {
        self.snapshot.lock(|snapshot| *snapshot.borrow())
    }

    fn publish_link(
        &self,
        state: StationLinkState,
        negotiated: Option<(u16, StationLinkSecurity)>,
    ) {
        self.snapshot.lock(|snapshot| {
            let revision = snapshot.borrow().revision.wrapping_add(1);
            let negotiated = if matches!(state, StationLinkState::Connected) {
                negotiated
            } else {
                None
            };
            *snapshot.borrow_mut() = StationStatusSnapshot {
                revision,
                state,
                tx_block_ack_operational_tids: 0,
                association_bandwidth_mhz: negotiated.map(|(bandwidth, _)| bandwidth),
                link_security: negotiated.map(|(_, security)| security),
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
pub struct StationStatus {
    seen_revision: u64,
}

impl StationStatus {
    pub(crate) fn new() -> Self {
        Self {
            seen_revision: STATION_STATUS.snapshot().revision,
        }
    }

    pub fn snapshot(&self) -> StationStatusSnapshot {
        STATION_STATUS.snapshot()
    }

    pub async fn changed(&mut self) -> StationStatusSnapshot {
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

pub(crate) fn publish_station_connected(
    association_bandwidth_mhz: u16,
    security: WifiSecurityMode,
) {
    STATION_STATUS.publish_link(
        StationLinkState::Connected,
        Some((association_bandwidth_mhz, security.into())),
    );
}

pub(crate) fn publish_station_disconnected(reason: ConnectedDisconnectReason) {
    STATION_STATUS.publish_link(StationLinkState::Disconnected(Some(reason)), None);
}

pub(crate) fn publish_station_tx_block_ack(tid: u8, operational: bool) {
    STATION_STATUS.publish_tx_block_ack(tid, operational);
}

#[cfg(test)]
mod tests;
