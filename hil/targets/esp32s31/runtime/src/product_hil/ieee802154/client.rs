//! The IEEE 802.15.4 client shared by the air check and peer sessions.

use core::pin::pin;

use esp_hal::{efuse, time::Instant};
use oer_esp32s31_hal::root::RadioHardware;
use oer_esp32s31_hal::shared_radio::SharedRadio;
use oer_esp32s31_ieee80211_esp_hal::EspHalRadioPeripheral;
use oer_esp32s31_ieee802154::{engine::Ieee802154EngineBuffers, pib::Ieee802154PibDefaults};
use oer_esp32s31_ieee802154_system::{Ieee802154Parked, Ieee802154System, start};
use oer_esp32s31_phy::{
    PhyCalibrationIdentity, PhyRegisterConfig,
    concurrent::{ConcurrentPhy, MaintenancePolicy},
    phy_get_rf_cal_version,
};
use oer_esp32s31_radio_esp_hal::EspHalRadioClocks;
use oer_hil_protocol::{
    Ieee802154AirTxOutcome, Ieee802154SessionMaintenancePolicy, Ieee802154SessionResult,
};
use oer_ieee802154::TxStatus;
use static_cell::ConstStaticCell;

static BUFFERS: ConstStaticCell<Ieee802154EngineBuffers> =
    ConstStaticCell::new(Ieee802154EngineBuffers::new());

pub(super) fn now_micros() -> u64 {
    Instant::now().duration_since_epoch().as_micros()
}

fn calibration_identity() -> PhyCalibrationIdentity {
    let mut base_mac_address = [0; 6];
    base_mac_address.copy_from_slice(efuse::base_mac_address().as_bytes());
    PhyCalibrationIdentity {
        rf_cal_version: phy_get_rf_cal_version(),
        base_mac_address,
        mac_extension: efuse::read_field_le::<u16>(efuse::MAC_EXT),
    }
}

/// The concurrently split radio and the IEEE 802.15.4 client's owners. The
/// images are terminal: the radio stays split.
pub(super) struct Client {
    pub(super) radio: SharedRadio<ConcurrentPhy>,
    pub(super) platform: EspHalRadioPeripheral,
    pub(super) clocks: EspHalRadioClocks,
    pub(super) defaults: Ieee802154PibDefaults,
}

impl Client {
    /// Claim the radio once for this image.
    pub(super) fn claim(platform: EspHalRadioPeripheral) -> Option<(Self, Ieee802154Parked)> {
        let hardware = RadioHardware::take()?;
        let buffers = BUFFERS.try_take()?;
        let (radio, partitions) = hardware.into_concurrent(ConcurrentPhy::new());
        let defaults = Ieee802154PibDefaults::default();
        let parked = Ieee802154Parked::new(partitions.ieee802154, buffers, defaults);
        Some((
            Self {
                radio,
                platform,
                clocks: EspHalRadioClocks::new(),
                defaults,
            },
            parked,
        ))
    }

    /// Start the client. Bring-up holds the PHY registration future, so it
    /// is pinned in place.
    pub(super) async fn start(&mut self, parked: Ieee802154Parked) -> Option<Ieee802154System> {
        let started = pin!(start(
            &self.radio,
            parked,
            &mut self.platform,
            &mut self.clocks,
            PhyRegisterConfig::new(calibration_identity()),
            self.defaults,
        ));
        started.await.ok()
    }

    /// Set the shared domain's tracking admission for this session.
    pub(super) fn set_maintenance_policy(
        &self,
        policy: Ieee802154SessionMaintenancePolicy,
    ) -> Result<(), Ieee802154SessionResult> {
        let mut lease = self
            .radio
            .try_acquire()
            .map_err(|_| Ieee802154SessionResult::StartFailed)?;
        lease.attachment_mut().set_maintenance_policy(match policy {
            Ieee802154SessionMaintenancePolicy::Vendor => MaintenancePolicy::Vendor,
            Ieee802154SessionMaintenancePolicy::Quiesced => MaintenancePolicy::Quiesced,
        });
        Ok(())
    }

    /// Stop the client. Teardown holds the RF close future, pinned in place.
    pub(super) async fn stop(&mut self, system: Ieee802154System) -> Option<Ieee802154Parked> {
        let stopped = pin!(system.stop(&self.radio, &mut self.platform, &mut self.clocks));
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
