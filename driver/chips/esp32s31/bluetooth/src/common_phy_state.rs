//! Retained Controller ownership after common PHY initialization.

use open_esp_radio_esp32s31_phy::{
    PhyCalibrationCache, PhyRegisterOutcome, RegisteredBluetoothPhy, RegisteredBluetoothPhyClient,
};

use crate::hci::BluetoothControllerLowPowerHardwareInitialized;

/// Observable, value-only result of the full common-PHY transition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothPhyInitializationReport {
    /// Terminal registration result returned by the concrete target runner.
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

/// Powered Controller after complete target shared-PHY registration.
///
/// Construction is possible only from the state that already retains the
/// scheduler and completed modem low-power hardware component.
/// The target-issued registration proof remains coupled to that exact powered
/// Controller epoch, but the Bluetooth client has not yet been acquired. This
/// state therefore cannot enter BTBB initialization.
///
/// There is deliberately no conversion back to cold ownership: complete
/// last-owner common-PHY shutdown is not yet recovered. Dropping this value is
/// fail-stop and does not run an unverified implicit teardown.
#[must_use = "registered common PHY state retains every Bluetooth hardware owner"]
pub struct BluetoothControllerPhyRegistered<
    P,
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize,
> {
    pub(crate) controller:
        BluetoothControllerLowPowerHardwareInitialized<P, MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY>,
    pub(crate) phy: RegisteredBluetoothPhy,
    pub(crate) calibration_cache: Option<PhyCalibrationCache>,
    pub(crate) report: BluetoothPhyInitializationReport,
}

impl<P, const MODEM_TIMER_CAPACITY: usize, const SCHEDULER_CAPACITY: usize>
    BluetoothControllerPhyRegistered<P, MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY>
{
    /// Inspect the value-only registration result without obtaining hardware authority.
    pub const fn report(&self) -> BluetoothPhyInitializationReport {
        self.report
    }

    /// Borrow the retained calibration cache for caller-selected persistence.
    pub const fn calibration_cache(&self) -> Option<&PhyCalibrationCache> {
        self.calibration_cache.as_ref()
    }
}

/// Powered Controller after target registration and settled Bluetooth-client
/// acquisition.
///
/// The lower owner retains both [`open_esp_radio_esp32s31_phy::RegisteredPhyState`]
/// and the source-owned Bluetooth client bit. A due immediate tracking request
/// must complete through the concrete target runner before this state can be
/// constructed. It is the sole common-PHY predecessor accepted by BTBB
/// initialization.
///
/// There is deliberately no conversion back to registered-only or cold
/// ownership. Complete Bluetooth-client, BTBB and common-PHY teardown is not
/// yet recovered, so dropping this value remains fail-stop.
#[must_use = "settled Bluetooth PHY client retains every powered Controller owner"]
pub struct BluetoothControllerPhyInitialized<
    P,
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize,
> {
    pub(crate) controller:
        BluetoothControllerLowPowerHardwareInitialized<P, MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY>,
    pub(crate) phy: RegisteredBluetoothPhyClient,
    pub(crate) calibration_cache: Option<PhyCalibrationCache>,
    pub(crate) report: BluetoothPhyInitializationReport,
}

impl<P, const MODEM_TIMER_CAPACITY: usize, const SCHEDULER_CAPACITY: usize>
    BluetoothControllerPhyInitialized<P, MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY>
{
    /// Inspect the value-only target registration result.
    pub const fn report(&self) -> BluetoothPhyInitializationReport {
        self.report
    }

    /// Borrow the retained calibration cache for caller-selected persistence.
    pub const fn calibration_cache(&self) -> Option<&PhyCalibrationCache> {
        self.calibration_cache.as_ref()
    }
}
