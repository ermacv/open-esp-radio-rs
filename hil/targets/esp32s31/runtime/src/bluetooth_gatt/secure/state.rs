//! Single-executor evidence and UI routing, compiled unchanged by host tests.
use bluetooth_example::security::{
    comparison::{Challenge, NumericComparison},
    gatt::Observation,
};
use core::cell::Cell;
use embassy_sync::{blocking_mutex::raw::NoopRawMutex, signal::Signal};
use open_esp_radio_hil_protocol::{
    BluetoothNumericChallenge, BluetoothSecureGattEvidence, Command, Event, RejectReason,
};

pub(crate) struct State {
    evidence: Cell<BluetoothSecureGattEvidence>,
    pub(crate) comparison: NumericComparison,
    pub(crate) restart: Signal<NoopRawMutex, ()>,
}
impl State {
    pub(crate) fn new() -> Self {
        Self {
            evidence: Cell::new(BluetoothSecureGattEvidence {
                epoch: 1,
                ..Default::default()
            }),
            comparison: NumericComparison::new(),
            restart: Signal::new(),
        }
    }
    pub(crate) fn stopped(&self) {
        let mut e = self.evidence.get();
        e.application_stopped = true;
        self.evidence.set(e);
    }
    pub(crate) fn cold(&self, old_hci_closed: bool) {
        let mut e = self.evidence.get();
        e.cold_releases = e
            .cold_releases
            .checked_add(1)
            .expect("cold sequence exhausted");
        e.old_hci_closed = old_hci_closed;
        e.traffic.advertising = false;
        e.traffic.connected = false;
        self.evidence.set(e);
    }
    pub(crate) fn restarted(&self) {
        let mut e = self.evidence.get();
        e.epoch = e.epoch.checked_add(1).expect("Host epoch exhausted");
        e.restarting = false;
        // The characteristic belongs to a fresh application, unlike the bond.
        e.traffic.value = 0;
        self.evidence.set(e);
    }
    pub(crate) fn command(&self, command: Command) -> Event {
        match command {
            Command::RestartBluetoothGatt { epoch }
                if epoch == self.evidence.get().epoch
                    && !self.evidence.get().application_stopped
                    && !self.evidence.get().restarting =>
            {
                let mut e = self.evidence.get();
                e.restarting = true;
                self.evidence.set(e);
                self.restart.signal(());
                Event::BluetoothSecureGatt(e)
            }
            Command::QueryBluetoothSecureGatt => {
                let mut value = self.evidence.get();
                value.comparison = self
                    .comparison
                    .pending()
                    .map(|c| BluetoothNumericChallenge {
                        id: c.id,
                        number: c.number,
                    });
                Event::BluetoothSecureGatt(value)
            }
            Command::ConfirmBluetoothGatt(decision)
                if !self.evidence.get().application_stopped && !self.evidence.get().restarting =>
            {
                match self.comparison.respond(
                    Challenge {
                        id: decision.challenge.id,
                        number: decision.challenge.number,
                    },
                    decision.accept,
                ) {
                    Ok(()) => Event::BluetoothGattDecisionRecorded(decision),
                    Err(_) => Event::Rejected(RejectReason::InvalidState),
                }
            }
            _ => Event::Rejected(RejectReason::InvalidState),
        }
    }
    pub(crate) fn observe(&self, event: Observation) {
        let mut e = self.evidence.get();
        let inc = |n: &mut u32| *n = n.checked_add(1).expect("secure GATT evidence exhausted");
        match event {
            Observation::Ready(address) => e.traffic.address = Some(address),
            Observation::Advertising => {
                e.traffic.advertising = true;
                inc(&mut e.traffic.advertising_starts);
            }
            Observation::Connected => {
                e.traffic.advertising = false;
                e.traffic.connected = true;
                inc(&mut e.traffic.connections);
            }
            Observation::Disconnected(reason) => {
                e.traffic.connected = false;
                e.traffic.last_disconnect_reason = Some(reason);
                inc(&mut e.traffic.disconnections);
            }
            Observation::Read(value) => {
                e.traffic.value = value;
                inc(&mut e.traffic.reads);
            }
            Observation::Written(value) => {
                e.traffic.value = value;
                inc(&mut e.traffic.writes);
            }
            Observation::Comparison(_) => inc(&mut e.comparisons),
            Observation::ComparisonAccepted => inc(&mut e.accepted),
            Observation::ComparisonRejected => inc(&mut e.declined),
            Observation::BondStored => inc(&mut e.bonds_stored),
            Observation::BondResumed => inc(&mut e.bonds_resumed),
            Observation::PairingFailed => inc(&mut e.pairing_failures),
            Observation::Rejected => inc(&mut e.rejected),
            Observation::NotificationQueued(_) => inc(&mut e.notifications_queued),
        }
        self.evidence.set(e);
    }
}
