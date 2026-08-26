//! Owned ESP32-S31 Bluetooth controller clock lifecycle.
//!
//! The operation order comes from the exact ESP32-S31 controller source bound
//! to the investigated libraries:
//! `espressif/esp-idf@aeab6dcfbeb44aba4b1f8ed102e3086172833153` and
//! `espressif/esp32s31-bt-lib@7f20740dd66ee774ffce5db0b55507892551aa31`.

use open_esp_radio_esp32s31_hal::BluetoothColdOwner;
use open_esp_radio_esp32s31_pac::{
    BluetoothLowPowerClockObservation, ModemSysconBluetoothObservation, SharedModemClockObservation,
};

use crate::resources::BluetoothStopped;

/// Platform-owned clock/reset operations required by standalone Bluetooth.
///
/// Implementations keep the official system PAC owners private. Each method
/// is one semantic operation from the ESP32-S31 modem-clock implementation;
/// raw register handles and masks do not cross into the Bluetooth driver.
pub trait BluetoothClockControl {
    /// Retain the lowest `PERIPH_BT_MODULE` dependency: the 160 MHz source.
    fn enable_bluetooth_controller_pll_source(&mut self);

    /// Observe the system-PAC-owned portion of the clock prerequisite.
    fn bluetooth_platform_clock_state(&mut self) -> BluetoothPlatformClockState;

    /// Release the lowest `PERIPH_BT_MODULE` dependency: the 160 MHz source.
    fn disable_bluetooth_controller_pll_source(&mut self);
}

/// Platform-only read-back; route-owned MODEM_LPCON state is joined by this
/// crate from the cold radio owner retained inside [`BluetoothStopped`].
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct BluetoothPlatformClockState {
    /// The upstream 160 MHz source retained through the platform singleton.
    pub pll_160mhz_source_enabled: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct BluetoothModemClockState {
    controller_clocks_enabled: bool,
    apb_clocks_enabled: bool,
    controller_resets_released: bool,
}

/// Semantic read-back for the first standalone clock profile.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct BluetoothClockState {
    /// The complete `PERIPH_BT_MODULE` dependency set is enabled.
    pub controller_clocks_enabled: bool,
    /// The complete `PERIPH_BT_APB_MODULE` dependency set is enabled.
    pub apb_clocks_enabled: bool,
    /// Every controller reset pulsed by the S31 lifecycle is released.
    pub controller_resets_released: bool,
    /// The BLE low-power timer is sourced from the main crystal.
    pub exclusive_main_xtal_selected: bool,
    /// The reviewed Bluetooth divider is programmed for the low-power timer.
    pub low_power_divider_configured: bool,
    /// The BLE low-power timer clock is enabled.
    pub low_power_timer_enabled: bool,
}

/// Exact semantic checkpoint that failed after clock setup.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothClockCheckpoint {
    /// The controller dependency clock set did not read back enabled.
    ControllerClocks,
    /// The APB dependency clock set did not read back enabled.
    ApbClocks,
    /// At least one required reset remained asserted.
    ControllerReset,
    /// The low-power clock was not sourced from the main crystal.
    LowPowerClockSource,
    /// The low-power clock divider did not match the selected profile.
    LowPowerClockDivider,
    /// The BLE low-power timer clock did not read back enabled.
    LowPowerTimerClock,
}

/// Read-back failure after a finite Bluetooth clock transaction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothClockError {
    /// First semantic read-back checkpoint that failed.
    pub checkpoint: BluetoothClockCheckpoint,
}

/// Bluetooth hardware after the complete clock/reset prerequisite.
///
/// The platform owner is retained by value so no other subsystem can gate the
/// same clocks behind this typestate. No controller/baseband MMIO is exposed
/// by this slice; the next lifecycle transaction consumes this value.
#[must_use = "clocked Bluetooth resources retain the radio and platform owners"]
pub struct BluetoothClockedResources<P> {
    registers: Option<BluetoothColdOwner>,
    platform: Option<P>,
    cleanup: fn(&mut BluetoothColdOwner, &mut P),
    cleanup_armed: bool,
}

impl<P> BluetoothClockedResources<P> {
    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) fn into_parts(mut self) -> (BluetoothColdOwner, P) {
        self.cleanup_armed = false;
        (
            self.registers
                .take()
                .expect("clocked Bluetooth registers are present"),
            self.platform
                .take()
                .expect("clocked Bluetooth platform is present"),
        )
    }

    #[cfg(test)]
    pub(crate) fn for_validation(registers: BluetoothColdOwner, platform: P) -> Self {
        Self {
            registers: Some(registers),
            platform: Some(platform),
            cleanup: |_, _| {},
            cleanup_armed: false,
        }
    }

    fn restore_reversible_clocks(&mut self) {
        if !self.cleanup_armed {
            return;
        }
        self.cleanup_armed = false;
        let registers = self
            .registers
            .as_mut()
            .expect("armed Bluetooth clock state retains its register owner");
        let platform = self
            .platform
            .as_mut()
            .expect("armed Bluetooth clock state retains its platform owner");
        (self.cleanup)(registers, platform);
    }
}

impl<P> Drop for BluetoothClockedResources<P> {
    fn drop(&mut self) {
        self.restore_reversible_clocks();
    }
}

trait BluetoothSharedClockControl {
    fn prepare_modem_syscon_clock_map(&mut self);
    fn enable_bluetooth_controller_dependents(&mut self);
    fn enable_bluetooth_apb_clocks(&mut self);
    fn reset_bluetooth_controller_domains(&mut self);
    fn modem_syscon_clock_state(&self) -> BluetoothModemClockState;
    fn disable_bluetooth_apb_clocks(&mut self);
    fn disable_bluetooth_controller_dependents(&mut self);
    fn prepare_shared_modem_clock_map(&mut self);
    fn retain_coexistence_clock(&mut self);
    fn release_coexistence_clock(&mut self);
    fn retain_main_xtal_low_power_clock(&mut self);
    fn release_bluetooth_low_power_timer(&mut self);
    fn bluetooth_shared_clock_observation(
        &self,
    ) -> (
        SharedModemClockObservation,
        BluetoothLowPowerClockObservation,
    );
}

impl BluetoothSharedClockControl for BluetoothColdOwner {
    fn prepare_modem_syscon_clock_map(&mut self) {
        self.prepare_modem_syscon_clock_map();
    }

    fn enable_bluetooth_controller_dependents(&mut self) {
        self.retain_modem_syscon_controller_clocks();
    }

    fn enable_bluetooth_apb_clocks(&mut self) {
        self.retain_modem_syscon_apb_clocks();
    }

    fn reset_bluetooth_controller_domains(&mut self) {
        self.reset_modem_syscon_bluetooth_domains();
    }

    fn modem_syscon_clock_state(&self) -> BluetoothModemClockState {
        let ModemSysconBluetoothObservation {
            controller_clocks_enabled,
            apb_clocks_enabled,
            controller_resets_released,
        } = self.modem_syscon_bluetooth_observation();
        BluetoothModemClockState {
            controller_clocks_enabled,
            apb_clocks_enabled,
            controller_resets_released,
        }
    }

    fn disable_bluetooth_apb_clocks(&mut self) {
        self.release_modem_syscon_apb_clocks();
    }

    fn disable_bluetooth_controller_dependents(&mut self) {
        self.release_modem_syscon_controller_clocks();
    }
    fn prepare_shared_modem_clock_map(&mut self) {
        BluetoothColdOwner::prepare_shared_modem_clock_map(self);
    }

    fn retain_coexistence_clock(&mut self) {
        BluetoothColdOwner::retain_coexistence_clock(self);
    }

    fn release_coexistence_clock(&mut self) {
        BluetoothColdOwner::release_coexistence_clock(self);
    }

    fn retain_main_xtal_low_power_clock(&mut self) {
        BluetoothColdOwner::retain_main_xtal_bluetooth_low_power_clock(self);
    }

    fn release_bluetooth_low_power_timer(&mut self) {
        BluetoothColdOwner::release_bluetooth_low_power_timer(self);
    }

    fn bluetooth_shared_clock_observation(
        &self,
    ) -> (
        SharedModemClockObservation,
        BluetoothLowPowerClockObservation,
    ) {
        BluetoothColdOwner::bluetooth_shared_clock_observation(self)
    }
}

impl<P: BluetoothClockControl> BluetoothClockedResources<P> {
    /// Reverse the exact clock prerequisite and recover the cold owner.
    pub fn disable_clocks(mut self) -> BluetoothStopped<P> {
        self.restore_reversible_clocks();
        BluetoothStopped::from_parts(
            self.registers
                .take()
                .expect("disabled Bluetooth registers are present"),
            self.platform
                .take()
                .expect("disabled Bluetooth platform is present"),
        )
    }
}

/// Failed clock setup after automatic low-bit-first dependency rollback.
#[must_use = "a failed Bluetooth clock transaction still owns both resources"]
pub struct BluetoothClockEnableFailure<P> {
    stopped: BluetoothStopped<P>,
    error: BluetoothClockError,
}

impl<P> BluetoothClockEnableFailure<P> {
    /// Return the semantic reason for the failed transaction.
    pub const fn error(&self) -> BluetoothClockError {
        self.error
    }

    /// Recover the intact cold owner after completed rollback.
    pub fn into_stopped(self) -> BluetoothStopped<P> {
        self.stopped
    }
}

impl<P: BluetoothClockControl> BluetoothStopped<P> {
    /// Execute the exact outer ESP32-S31 Bluetooth clock prerequisite.
    ///
    /// The current standalone profile uses the 40 MHz main crystal divided to
    /// 100 kHz, matching the source-bound S31 default. Any failed read-back is
    /// rolled back in the vendor's low-bit-first dependency order before the
    /// owners are returned.
    pub fn enable_clocks(
        self,
    ) -> Result<BluetoothClockedResources<P>, BluetoothClockEnableFailure<P>> {
        let (mut registers, mut platform) = self.into_parts();
        if let Err(error) = enable_owned(&mut registers, &mut platform) {
            return Err(BluetoothClockEnableFailure {
                stopped: BluetoothStopped::from_parts(registers, platform),
                error,
            });
        }

        Ok(BluetoothClockedResources {
            registers: Some(registers),
            platform: Some(platform),
            cleanup: disable_owned::<BluetoothColdOwner, P>,
            cleanup_armed: true,
        })
    }
}

fn enable_owned(
    resources: &mut impl BluetoothSharedClockControl,
    platform: &mut impl BluetoothClockControl,
) -> Result<(), BluetoothClockError> {
    resources.prepare_shared_modem_clock_map();
    resources.prepare_modem_syscon_clock_map();
    platform.enable_bluetooth_controller_pll_source();
    resources.retain_coexistence_clock();
    resources.enable_bluetooth_controller_dependents();
    resources.enable_bluetooth_apb_clocks();
    resources.reset_bluetooth_controller_domains();
    resources.retain_main_xtal_low_power_clock();

    let platform_state = platform.bluetooth_platform_clock_state();
    let modem_state = resources.modem_syscon_clock_state();
    let (shared, low_power) = resources.bluetooth_shared_clock_observation();
    let state = BluetoothClockState {
        controller_clocks_enabled: platform_state.pll_160mhz_source_enabled
            && modem_state.controller_clocks_enabled
            && shared.coexistence_clock_enabled,
        apb_clocks_enabled: modem_state.apb_clocks_enabled,
        controller_resets_released: modem_state.controller_resets_released,
        exclusive_main_xtal_selected: low_power.exclusive_main_xtal_selected,
        low_power_divider_configured: low_power.bluetooth_divider_configured,
        low_power_timer_enabled: low_power.timer_enabled,
    };
    if let Err(error) = validate_state(state) {
        disable_owned(resources, platform);
        return Err(error);
    }
    Ok(())
}

fn validate_state(state: BluetoothClockState) -> Result<(), BluetoothClockError> {
    let failed = if !state.controller_clocks_enabled {
        Some(BluetoothClockCheckpoint::ControllerClocks)
    } else if !state.apb_clocks_enabled {
        Some(BluetoothClockCheckpoint::ApbClocks)
    } else if !state.controller_resets_released {
        Some(BluetoothClockCheckpoint::ControllerReset)
    } else if !state.exclusive_main_xtal_selected {
        Some(BluetoothClockCheckpoint::LowPowerClockSource)
    } else if !state.low_power_divider_configured {
        Some(BluetoothClockCheckpoint::LowPowerClockDivider)
    } else if !state.low_power_timer_enabled {
        Some(BluetoothClockCheckpoint::LowPowerTimerClock)
    } else {
        None
    };

    match failed {
        Some(checkpoint) => Err(BluetoothClockError { checkpoint }),
        None => Ok(()),
    }
}

fn disable_owned<R: BluetoothSharedClockControl, P: BluetoothClockControl>(
    resources: &mut R,
    platform: &mut P,
) {
    resources.release_bluetooth_low_power_timer();
    resources.disable_bluetooth_apb_clocks();
    platform.disable_bluetooth_controller_pll_source();
    resources.release_coexistence_clock();
    resources.disable_bluetooth_controller_dependents();
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, rc::Rc, vec::Vec};

    use open_esp_radio_esp32s31_pac::{
        BluetoothLowPowerClockObservation, RadioHardware, SharedModemClockObservation,
    };

    use super::{
        BluetoothClockCheckpoint, BluetoothClockControl, BluetoothClockedResources,
        BluetoothColdOwner, BluetoothModemClockState, BluetoothPlatformClockState,
        BluetoothSharedClockControl, disable_owned, enable_owned,
    };

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum Operation {
        PrepareSharedMap,
        PrepareModemMap,
        EnablePllSource,
        RetainCoexistence,
        EnableControllerDependents,
        EnableApb,
        ResetDomains,
        SelectMainXtal,
        ObservePlatform,
        ObserveShared,
        ReleaseLowPower,
        DisableApb,
        DisablePllSource,
        ReleaseCoexistence,
        DisableControllerDependents,
        Cleanup,
    }

    struct FakePlatform {
        operations: Rc<RefCell<Vec<Operation>>>,
        state: BluetoothPlatformClockState,
    }

    struct FakeShared {
        operations: Rc<RefCell<Vec<Operation>>>,
        shared: SharedModemClockObservation,
        low_power: BluetoothLowPowerClockObservation,
    }

    impl FakeShared {
        fn ready(operations: Rc<RefCell<Vec<Operation>>>) -> Self {
            Self {
                operations,
                shared: SharedModemClockObservation {
                    power_state_map_configured: true,
                    coexistence_clock_enabled: true,
                    phy_i2c_master_clock_enabled: false,
                    low_power_timer_clock_enabled: true,
                },
                low_power: BluetoothLowPowerClockObservation {
                    exclusive_main_xtal_selected: true,
                    bluetooth_divider_configured: true,
                    timer_enabled: true,
                },
            }
        }
    }

    impl BluetoothSharedClockControl for FakeShared {
        fn prepare_modem_syscon_clock_map(&mut self) {
            self.operations
                .borrow_mut()
                .push(Operation::PrepareModemMap);
        }

        fn enable_bluetooth_controller_dependents(&mut self) {
            self.operations
                .borrow_mut()
                .push(Operation::EnableControllerDependents);
        }

        fn enable_bluetooth_apb_clocks(&mut self) {
            self.operations.borrow_mut().push(Operation::EnableApb);
        }

        fn reset_bluetooth_controller_domains(&mut self) {
            self.operations.borrow_mut().push(Operation::ResetDomains);
        }

        fn modem_syscon_clock_state(&self) -> BluetoothModemClockState {
            BluetoothModemClockState {
                controller_clocks_enabled: true,
                apb_clocks_enabled: true,
                controller_resets_released: true,
            }
        }

        fn disable_bluetooth_apb_clocks(&mut self) {
            self.operations.borrow_mut().push(Operation::DisableApb);
        }

        fn disable_bluetooth_controller_dependents(&mut self) {
            self.operations
                .borrow_mut()
                .push(Operation::DisableControllerDependents);
        }

        fn prepare_shared_modem_clock_map(&mut self) {
            self.operations
                .borrow_mut()
                .push(Operation::PrepareSharedMap);
        }

        fn retain_coexistence_clock(&mut self) {
            self.operations
                .borrow_mut()
                .push(Operation::RetainCoexistence);
        }

        fn release_coexistence_clock(&mut self) {
            self.operations
                .borrow_mut()
                .push(Operation::ReleaseCoexistence);
        }

        fn retain_main_xtal_low_power_clock(&mut self) {
            self.operations.borrow_mut().push(Operation::SelectMainXtal);
        }

        fn release_bluetooth_low_power_timer(&mut self) {
            self.operations
                .borrow_mut()
                .push(Operation::ReleaseLowPower);
        }

        fn bluetooth_shared_clock_observation(
            &self,
        ) -> (
            SharedModemClockObservation,
            BluetoothLowPowerClockObservation,
        ) {
            self.operations.borrow_mut().push(Operation::ObserveShared);
            (self.shared, self.low_power)
        }
    }

    impl FakePlatform {
        fn ready(operations: Rc<RefCell<Vec<Operation>>>) -> Self {
            Self {
                operations,
                state: BluetoothPlatformClockState {
                    pll_160mhz_source_enabled: true,
                },
            }
        }
    }

    impl BluetoothClockControl for FakePlatform {
        fn enable_bluetooth_controller_pll_source(&mut self) {
            self.operations
                .borrow_mut()
                .push(Operation::EnablePllSource);
        }

        fn bluetooth_platform_clock_state(&mut self) -> BluetoothPlatformClockState {
            self.operations
                .borrow_mut()
                .push(Operation::ObservePlatform);
            self.state
        }

        fn disable_bluetooth_controller_pll_source(&mut self) {
            self.operations
                .borrow_mut()
                .push(Operation::DisablePllSource);
        }
    }

    fn record_cleanup(_: &mut BluetoothColdOwner, platform: &mut FakePlatform) {
        platform.operations.borrow_mut().push(Operation::Cleanup);
    }

    fn validation_clocked(
        operations: Rc<RefCell<Vec<Operation>>>,
    ) -> BluetoothClockedResources<FakePlatform> {
        BluetoothClockedResources {
            registers: Some(BluetoothColdOwner::from_radio_hardware(
                RadioHardware::for_validation(),
            )),
            platform: Some(FakePlatform::ready(operations)),
            cleanup: record_cleanup,
            cleanup_armed: true,
        }
    }

    #[test]
    fn dropping_clocked_resources_runs_reversible_cleanup_once() {
        let operations = Rc::new(RefCell::new(Vec::new()));
        drop(validation_clocked(operations.clone()));
        assert_eq!(*operations.borrow(), [Operation::Cleanup]);
    }

    #[test]
    fn consuming_clocked_resources_disarms_drop_cleanup() {
        let operations = Rc::new(RefCell::new(Vec::new()));
        let resources = validation_clocked(operations.clone());
        let (_registers, _platform) = resources.into_parts();
        assert!(operations.borrow().is_empty());
    }

    #[test]
    fn exact_clock_order_joins_platform_and_shared_observations() {
        let operations = Rc::new(RefCell::new(Vec::new()));
        let mut shared = FakeShared::ready(operations.clone());
        let mut platform = FakePlatform::ready(operations.clone());
        assert_eq!(enable_owned(&mut shared, &mut platform), Ok(()));
        disable_owned(&mut shared, &mut platform);

        assert_eq!(
            *operations.borrow(),
            [
                Operation::PrepareSharedMap,
                Operation::PrepareModemMap,
                Operation::EnablePllSource,
                Operation::RetainCoexistence,
                Operation::EnableControllerDependents,
                Operation::EnableApb,
                Operation::ResetDomains,
                Operation::SelectMainXtal,
                Operation::ObservePlatform,
                Operation::ObserveShared,
                Operation::ReleaseLowPower,
                Operation::DisableApb,
                Operation::DisablePllSource,
                Operation::ReleaseCoexistence,
                Operation::DisableControllerDependents,
            ]
        );
    }

    #[test]
    fn failed_readback_rolls_back_before_returning_owners() {
        let operations = Rc::new(RefCell::new(Vec::new()));
        let mut platform = FakePlatform::ready(operations.clone());
        let mut shared = FakeShared::ready(operations.clone());
        shared.low_power.timer_enabled = false;

        let failure = enable_owned(&mut shared, &mut platform).unwrap_err();
        assert_eq!(
            failure.checkpoint,
            BluetoothClockCheckpoint::LowPowerTimerClock
        );
        assert_eq!(
            &operations.borrow()[10..],
            [
                Operation::ReleaseLowPower,
                Operation::DisableApb,
                Operation::DisablePllSource,
                Operation::ReleaseCoexistence,
                Operation::DisableControllerDependents,
            ]
        );
    }
}
