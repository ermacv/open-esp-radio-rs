//! Owned Bluetooth baseband transition after common PHY initialization.
//!
//! The transition contains only the finite MMIO effects proven equivalent to
//! vendor `bt_bb_v2_init_cmplx(1)`. It does not claim that controller tasks,
//! interrupts, HCI, or the Link Layer are ready.

#[cfg(target_arch = "riscv32")]
use open_esp_radio_esp32s31_phy::PhyCalibrationCache;

#[cfg(target_arch = "riscv32")]
use crate::common_phy_state::{
    BluetoothControllerPhyInitialized, BluetoothPhyInitializationReport,
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

/// Powered Controller after common PHY and the finite BTBB-v2 transaction.
///
/// The completed PHY owner and every earlier Controller prerequisite remain
/// nested by value. This state intentionally exposes neither operational
/// Link-Layer readiness nor a conversion back to cold ownership: complete
/// last-owner PHY teardown is not yet recovered and verified. Dropping it is
/// fail-stop and does not gate clocks behind initialized radio state.
#[must_use = "initialized Bluetooth baseband retains every hardware owner"]
#[cfg(target_arch = "riscv32")]
pub struct BluetoothControllerBasebandInitialized<
    P,
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize,
> {
    pub(crate) initialized:
        BluetoothControllerPhyInitialized<P, MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY>,
    pub(crate) baseband_report: BluetoothBasebandInitializationReport,
}

#[cfg(target_arch = "riscv32")]
impl<P, const MODEM_TIMER_CAPACITY: usize, const SCHEDULER_CAPACITY: usize>
    BluetoothControllerBasebandInitialized<P, MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY>
{
    /// Inspect the completed common-PHY transition without hardware access.
    pub const fn phy_report(&self) -> BluetoothPhyInitializationReport {
        self.initialized.report()
    }

    /// Inspect the finite BTBB transition input without hardware access.
    pub const fn baseband_report(&self) -> BluetoothBasebandInitializationReport {
        self.baseband_report
    }

    /// Borrow the retained calibration cache for caller-selected persistence.
    pub const fn calibration_cache(&self) -> Option<&PhyCalibrationCache> {
        self.initialized.calibration_cache()
    }
}

#[cfg(target_arch = "riscv32")]
impl<P, const MODEM_TIMER_CAPACITY: usize, const SCHEDULER_CAPACITY: usize>
    BluetoothControllerPhyInitialized<P, MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY>
{
    /// Execute the exact finite MMIO effects of `bt_bb_v2_init_cmplx(1)`.
    ///
    /// The gain byte is projected from this completed common-PHY owner rather
    /// than accepted as an unrelated caller argument. The diagnostic print in
    /// the vendor body is not hardware state and is deliberately absent. The
    /// returned type does not claim that the BLE engine, interrupts, or HCI
    /// dataplane are operational.
    #[cfg(target_arch = "riscv32")]
    #[allow(
        unsafe_code,
        reason = "the consuming Controller-PHY state proves every prerequisite of the narrow HAL bridge"
    )]
    pub fn initialize_baseband(
        self,
    ) -> BluetoothControllerBasebandInitialized<P, MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY> {
        let BluetoothControllerPhyInitialized {
            mut controller,
            phy,
            calibration_cache,
            report,
        } = self;
        let baseband_report = apply_baseband_input(phy.phy_state(), |gain_parameter| {
            // SAFETY: this consuming state retains the powered Controller, the
            // matching task partition and the terminal common-PHY owner from
            // which the only input was projected.
            unsafe {
                controller.task_mut().initialize_baseband_v2(gain_parameter);
            }
        });

        BluetoothControllerBasebandInitialized {
            initialized: BluetoothControllerPhyInitialized {
                controller,
                phy,
                calibration_cache,
                report,
            },
            baseband_report,
        }
    }
}

fn apply_baseband_input(
    phy: &open_esp_radio_esp32s31_phy::PhyState,
    initialize: impl FnOnce(u8),
) -> BluetoothBasebandInitializationReport {
    let report = BluetoothBasebandInitializationReport {
        gain_parameter: phy.register_init_parameters().parameter_120,
    };
    initialize(report.gain_parameter);
    report
}

#[cfg(test)]
mod tests;
