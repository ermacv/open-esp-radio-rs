//! Owned Bluetooth baseband transition after common PHY initialization.
//!
//! The transition contains only the finite MMIO effects proven equivalent to
//! vendor `bt_bb_v2_init_cmplx(1)`. It does not claim that controller tasks,
//! interrupts, HCI, or the Link Layer are ready.

use open_esp_radio_esp32s31_phy::{PhyCalibrationCache, PhyState};

use crate::{
    common_phy_state::{BluetoothPhyInitializationReport, BluetoothPhyInitialized},
    resources::BluetoothTeardownPendingPlatform,
};

/// Value-only observation of the reviewed baseband initialization input.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothBasebandInitializationReport {
    /// Byte owned by the common PHY state and consumed by the BTBB sequence.
    ///
    /// This is the exact replacement for the vendor read at
    /// `phy_param + 0x120`; its undocumented inner meaning remains unassigned.
    pub gain_parameter: u8,
}

/// Bluetooth hardware after common PHY and the finite BTBB-v2 transaction.
///
/// Every unique hardware owner remains private and retained by value. This
/// state intentionally exposes neither controller readiness nor a conversion
/// back to cold ownership: the complete last-owner PHY teardown is not yet
/// recovered and verified. Dropping it is fail-stop and retains the platform
/// reservation rather than gating clocks behind initialized radio state.
#[must_use = "initialized Bluetooth baseband retains every hardware owner"]
pub struct BluetoothBasebandInitialized<P> {
    _task: crate::resources::BluetoothTaskResources,
    _interrupts: crate::resources::BluetoothInterruptBankOwner,
    _platform: BluetoothTeardownPendingPlatform<P>,
    _phy: PhyState,
    calibration_cache: Option<PhyCalibrationCache>,
    phy_report: BluetoothPhyInitializationReport,
    baseband_report: BluetoothBasebandInitializationReport,
}

impl<P> BluetoothBasebandInitialized<P> {
    /// Inspect the completed common-PHY transition without hardware access.
    pub const fn phy_report(&self) -> BluetoothPhyInitializationReport {
        self.phy_report
    }

    /// Inspect the finite BTBB transition input without hardware access.
    pub const fn baseband_report(&self) -> BluetoothBasebandInitializationReport {
        self.baseband_report
    }

    /// Borrow the retained calibration cache for caller-selected persistence.
    pub const fn calibration_cache(&self) -> Option<&PhyCalibrationCache> {
        self.calibration_cache.as_ref()
    }
}

impl<P> BluetoothPhyInitialized<P> {
    /// Execute the exact finite MMIO effects of `bt_bb_v2_init_cmplx(1)`.
    ///
    /// The gain byte is projected from the completed common-PHY owner rather
    /// than accepted as an unrelated caller argument. The diagnostic print in
    /// the vendor body is not hardware state and is deliberately absent. The
    /// returned type does not claim that any controller task or interrupt is
    /// operational.
    #[cfg(target_arch = "riscv32")]
    pub fn initialize_baseband(self) -> BluetoothBasebandInitialized<P> {
        self.initialize_baseband_with(|task, gain_parameter| {
            #[allow(
                unsafe_code,
                reason = "self owns the completed common-PHY state and its matching task partition"
            )]
            unsafe {
                task.initialize_baseband_v2(gain_parameter);
            }
        })
    }

    fn initialize_baseband_with(
        mut self,
        initialize: impl FnOnce(&mut crate::resources::BluetoothTaskResources, u8),
    ) -> BluetoothBasebandInitialized<P> {
        let gain_parameter = self.phy.register_init_parameters().parameter_120;
        initialize(&mut self.task, gain_parameter);

        BluetoothBasebandInitialized {
            _task: self.task,
            _interrupts: self.interrupts,
            _platform: self.platform,
            _phy: self.phy,
            calibration_cache: self.calibration_cache,
            phy_report: self.report,
            baseband_report: BluetoothBasebandInitializationReport { gain_parameter },
        }
    }
}

#[cfg(test)]
mod tests {
    use open_esp_radio_esp32s31_pac::RadioHardware;
    use open_esp_radio_esp32s31_phy::{PhyCalibrationPath, PhyRegisterOutcome, PhyState};

    use super::BluetoothBasebandInitializationReport;
    use crate::{
        common_phy_state::{BluetoothPhyInitializationReport, BluetoothPhyInitialized},
        resources::{BluetoothPhysicalResources, BluetoothTeardownPendingPlatform},
    };

    #[test]
    fn transition_consumes_terminal_phy_projection_and_retains_reports() {
        let resources =
            BluetoothPhysicalResources::from_radio_hardware(RadioHardware::for_validation());
        let (task, interrupts) = resources.separate_interrupt_owner();
        let mut phy = PhyState::default();
        let _parameters = phy.prepare_rx_table_init();
        let report = BluetoothPhyInitializationReport {
            registration: PhyRegisterOutcome {
                full_calibration_performed: true,
                calibration_path: PhyCalibrationPath::FullUncached,
            },
            mmio_operations: 1,
            delays: 2,
            reset_samples: 3,
            rf_operations: 4,
            baseband_operations: 5,
        };
        let initialized = BluetoothPhyInitialized {
            task,
            interrupts,
            platform: BluetoothTeardownPendingPlatform::new(()),
            phy,
            calibration_cache: None,
            report,
        };
        let mut observed = None;

        assert_eq!(initialized.report(), report);
        assert!(initialized.calibration_cache().is_none());

        let initialized = initialized.initialize_baseband_with(|_, gain_parameter| {
            observed = Some(gain_parameter);
        });

        assert_eq!(observed, Some(0x4f));
        assert_eq!(initialized.phy_report(), report);
        assert_eq!(
            initialized.baseband_report(),
            BluetoothBasebandInitializationReport {
                gain_parameter: 0x4f,
            }
        );
        assert!(initialized.calibration_cache().is_none());
    }
}
