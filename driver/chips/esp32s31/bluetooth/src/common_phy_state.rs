//! Retained Controller ownership after common PHY initialization.

use embassy_sync::blocking_mutex::raw::RawMutex;
use open_esp_radio_esp32s31_phy::{PhyCalibrationCache, PhyRegisterOutcome, PhyState};

use crate::hci::BluetoothControllerLowPowerHardwareInitialized;

/// Observable, value-only result of the full common-PHY transition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothPhyInitializationReport {
    /// Terminal result returned by the shared `register_chipv7_phy` model.
    pub registration: PhyRegisterOutcome,
    /// Number of recovered MMIO actions completed by the target port.
    pub mmio_operations: u16,
    /// Number of recovered delay actions completed by the target port.
    pub delays: u16,
    /// Number of reset-readback samples completed by the target port.
    pub reset_samples: u16,
    /// Number of bounded RF operations completed by the target port.
    pub rf_operations: u32,
    /// Number of bounded baseband operations completed by the target port.
    pub baseband_operations: u32,
}

/// Powered Controller after complete shared PHY registration/calibration.
///
/// Construction is possible only from the state that already retains the
/// scheduler, HCI resources and completed modem low-power hardware component.
/// The unique PHY state remains coupled to that exact powered Controller
/// epoch. Later BTBB/controller transitions may borrow its private task
/// partition only inside this crate and must retain this whole state by value.
///
/// There is deliberately no conversion back to cold ownership: complete
/// last-owner common-PHY shutdown is not yet recovered. Dropping this value is
/// fail-stop and does not run an unverified implicit teardown.
#[must_use = "initialized common PHY state retains every Bluetooth hardware owner"]
pub struct BluetoothControllerPhyInitialized<
    P,
    M,
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
> where
    M: RawMutex,
{
    pub(crate) controller: BluetoothControllerLowPowerHardwareInitialized<
        P,
        M,
        MODEM_TIMER_CAPACITY,
        SCHEDULER_CAPACITY,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >,
    pub(crate) phy: PhyState,
    pub(crate) calibration_cache: Option<PhyCalibrationCache>,
    pub(crate) report: BluetoothPhyInitializationReport,
}

impl<
    P,
    M,
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
>
    BluetoothControllerPhyInitialized<
        P,
        M,
        MODEM_TIMER_CAPACITY,
        SCHEDULER_CAPACITY,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >
where
    M: RawMutex,
{
    /// Inspect the value-only result without obtaining hardware authority.
    pub const fn report(&self) -> BluetoothPhyInitializationReport {
        self.report
    }

    /// Borrow the retained calibration cache for caller-selected persistence.
    pub const fn calibration_cache(&self) -> Option<&PhyCalibrationCache> {
        self.calibration_cache.as_ref()
    }
}
