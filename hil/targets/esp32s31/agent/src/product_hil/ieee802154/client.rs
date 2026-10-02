//! The IEEE 802.15.4 client shared by the air check and peer sessions.

use core::pin::pin;

use esp_hal::time::Instant;
use oer_esp32s31_ieee802154_system::{Ieee802154Parked, Ieee802154System, start};
use oer_esp32s31_phy::concurrent::MaintenancePolicy;
use oer_esp32s31_radio_esp_hal::EspHalRadioPlatform;
use oer_espressif_ieee802154_engine::pib::Ieee802154PibDefaults;
use oer_hil_protocol::{
    ieee802154::Ieee802154AirTxOutcome, ieee802154::Ieee802154SessionMaintenancePolicy,
};
use oer_ieee802154::TxStatus;

pub(super) fn now_micros() -> u64 {
    Instant::now().duration_since_epoch().as_micros()
}

type Radio = oer_esp32s31_radio_system::SharedRadio;

/// The concurrently split radio and the IEEE 802.15.4 client's owners. The
/// images are terminal: the radio stays split. The radio is placed by the
/// PHY register-image reader, which reads it under its lease.
pub(super) struct Client {
    pub(super) radio: &'static Radio,
    pub(super) defaults: Ieee802154PibDefaults,
}

impl Client {
    /// Start the radio once for this image. The image runs the coexistence
    /// schedule and PHY tracking itself, for the length of a session.
    pub(super) fn claim(
        spawner: embassy_executor::Spawner,
        platform: EspHalRadioPlatform,
    ) -> Option<(Self, Ieee802154Parked)> {
        use oer_esp32s31_radio_system::{RadioStart, Schedule, Tracking};
        let start = RadioStart::new()
            .with_tracking(Tracking::Caller)
            .with_schedule(Schedule::Caller);
        let (radio, partitions) =
            oer_esp32s31_radio_system::start(spawner, platform, start).ok()?;
        crate::product_hil::phy_register_image::adopt(radio);
        let defaults = Ieee802154PibDefaults::default();
        let parked = Ieee802154Parked::new(partitions.ieee802154, defaults)?;
        Some((Self { radio, defaults }, parked))
    }

    /// Start the client. Bring-up holds the PHY registration future, so it
    /// is pinned in place.
    pub(super) async fn start(&mut self, parked: Ieee802154Parked) -> Option<Ieee802154System> {
        let started = pin!(start(self.radio, parked, self.defaults));
        started.await.ok()
    }

    /// Set the shared domain's tracking admission for this session.
    pub(super) async fn set_maintenance_policy(&self, policy: Ieee802154SessionMaintenancePolicy) {
        let mut guard = self.radio.lock().await;
        guard
            .lease()
            .attachment_mut()
            .set_maintenance_policy(match policy {
                Ieee802154SessionMaintenancePolicy::Vendor => MaintenancePolicy::Vendor,
                Ieee802154SessionMaintenancePolicy::Quiesced => MaintenancePolicy::Quiesced,
            });
    }

    /// Stop the client. Teardown holds the RF close future, pinned in place.
    pub(super) async fn stop(&mut self, system: Ieee802154System) -> Option<Ieee802154Parked> {
        let stopped = pin!(system.stop(self.radio));
        stopped.await.ok()
    }
}

pub(super) const fn tx_outcome(status: TxStatus) -> Ieee802154AirTxOutcome {
    match status {
        TxStatus::Success => Ieee802154AirTxOutcome::Success,
        TxStatus::ChannelBusy => Ieee802154AirTxOutcome::ChannelBusy,
        TxStatus::NoAcknowledgement => Ieee802154AirTxOutcome::NoAcknowledgement,
        TxStatus::Aborted => Ieee802154AirTxOutcome::Aborted,
        TxStatus::InvalidFrame => Ieee802154AirTxOutcome::InvalidFrame,
        TxStatus::HardwareFailure => Ieee802154AirTxOutcome::HardwareFailure,
        TxStatus::CoexistenceRejected => Ieee802154AirTxOutcome::CoexistenceRejected,
        TxStatus::SecurityFailure => Ieee802154AirTxOutcome::SecurityFailure,
        TxStatus::InvalidAcknowledgement => Ieee802154AirTxOutcome::InvalidAcknowledgement,
    }
}
