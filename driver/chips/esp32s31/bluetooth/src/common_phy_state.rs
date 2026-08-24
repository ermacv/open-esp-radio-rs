//! Retained ownership after common PHY initialization begins.

use open_esp_radio_esp32s31_phy::{PhyCalibrationCache, PhyRegisterOutcome, PhyState};

use crate::resources::{
    BluetoothInterruptBankOwner, BluetoothTaskResources, BluetoothTeardownPendingPlatform,
};

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

/// Bluetooth hardware after the complete shared PHY registration/calibration.
///
/// Construction is possible only through the target common-PHY transition.
/// The unique PHY software state and both Bluetooth PAC partitions remain
/// private. Later BTBB/controller transitions may borrow the task partition
/// only inside this crate and must retain this whole state by value. This
/// state deliberately has no conversion back to cold radio ownership: the
/// complete last-owner common-PHY shutdown is not yet recovered. Dropping the
/// value is fail-stop: it intentionally retains the platform reservation and
/// clocks instead of running an unverified implicit teardown.
#[must_use = "initialized common PHY state retains every Bluetooth hardware owner"]
pub struct BluetoothPhyInitialized<P> {
    pub(crate) task: BluetoothTaskResources,
    pub(crate) interrupts: BluetoothInterruptBankOwner,
    pub(crate) platform: BluetoothTeardownPendingPlatform<P>,
    pub(crate) phy: PhyState,
    pub(crate) calibration_cache: Option<PhyCalibrationCache>,
    pub(crate) report: BluetoothPhyInitializationReport,
}

impl<P> BluetoothPhyInitialized<P> {
    /// Inspect the value-only result without obtaining hardware authority.
    pub const fn report(&self) -> BluetoothPhyInitializationReport {
        self.report
    }

    /// Borrow the retained calibration cache for caller-selected persistence.
    pub const fn calibration_cache(&self) -> Option<&PhyCalibrationCache> {
        self.calibration_cache.as_ref()
    }
}
