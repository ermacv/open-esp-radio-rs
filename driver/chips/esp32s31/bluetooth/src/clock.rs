//! Owned ESP32-S31 Bluetooth controller clock lifecycle.
//!
//! The operation order comes from the exact ESP32-S31 controller source bound
//! to the investigated libraries:
//! `espressif/esp-idf@aeab6dcfbeb44aba4b1f8ed102e3086172833153` and
//! `espressif/esp32s31-bt-lib@7f20740dd66ee774ffce5db0b55507892551aa31`.

use open_esp_radio_esp32s31_hal::BluetoothColdOwner;
use open_esp_radio_esp32s31_pac::{BluetoothLowPowerClockObservation, SharedModemClockObservation};

use crate::resources::BluetoothStopped;

const MAIN_XTAL_LOW_POWER_DIVIDER: u16 = 399;

/// Platform-owned clock/reset operations required by standalone Bluetooth.
///
/// Implementations keep the official system PAC owners private. Each method
/// is one semantic operation from the ESP32-S31 modem-clock implementation;
/// raw register handles and masks do not cross into the Bluetooth driver.
pub trait BluetoothClockControl {
    /// Retain the lowest `PERIPH_BT_MODULE` dependency: the 160 MHz source.
    fn enable_bluetooth_controller_pll_source(&mut self);

    /// Retain the `PERIPH_BT_MODULE` dependencies above coexistence.
    fn enable_bluetooth_controller_dependents(&mut self);

    /// Enable the dependency set of `PERIPH_BT_APB_MODULE`.
    fn enable_bluetooth_apb_clocks(&mut self);

    /// Pulse and release the BT MAC, BT MAC APB, BLE timer and modem-security
    /// resets in the reviewed platform order.
    fn reset_bluetooth_controller_domains(&mut self);

    /// Observe the system-PAC-owned portion of the clock prerequisite.
    fn bluetooth_platform_clock_state(&mut self) -> BluetoothPlatformClockState;

    /// Disable the dependency set of `PERIPH_BT_APB_MODULE`.
    fn disable_bluetooth_apb_clocks(&mut self);

    /// Release the lowest `PERIPH_BT_MODULE` dependency: the 160 MHz source.
    fn disable_bluetooth_controller_pll_source(&mut self);

    /// Release the `PERIPH_BT_MODULE` dependencies above coexistence.
    fn disable_bluetooth_controller_dependents(&mut self);
}

/// Platform-only read-back; route-owned MODEM_LPCON state is joined by this
/// crate from the cold radio owner retained inside [`BluetoothStopped`].
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct BluetoothPlatformClockState {
    pub controller_clocks_enabled: bool,
    pub apb_clocks_enabled: bool,
    pub controller_resets_released: bool,
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
    pub main_xtal_selected: bool,
    /// Divider-minus-one programmed for the low-power timer source.
    pub low_power_divider: u16,
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
    registers: BluetoothColdOwner,
    platform: P,
}

impl<P> BluetoothClockedResources<P> {
    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) fn into_parts(self) -> (BluetoothColdOwner, P) {
        (self.registers, self.platform)
    }

    #[cfg(test)]
    pub(crate) const fn for_validation(registers: BluetoothColdOwner, platform: P) -> Self {
        Self {
            registers,
            platform,
        }
    }
}

trait BluetoothSharedClockControl {
    fn prepare_shared_modem_clock_map(&mut self);
    fn retain_coexistence_clock(&mut self) -> bool;
    fn release_coexistence_clock(&mut self);
    fn select_main_xtal_low_power_clock(&mut self, divider: u16) -> bool;
    fn release_bluetooth_low_power_timer(&mut self);
    fn bluetooth_shared_clock_observation(
        &self,
    ) -> (
        SharedModemClockObservation,
        BluetoothLowPowerClockObservation,
    );
}

impl BluetoothSharedClockControl for BluetoothColdOwner {
    fn prepare_shared_modem_clock_map(&mut self) {
        BluetoothColdOwner::prepare_shared_modem_clock_map(self);
    }

    fn retain_coexistence_clock(&mut self) -> bool {
        BluetoothColdOwner::retain_coexistence_clock(self)
    }

    fn release_coexistence_clock(&mut self) {
        BluetoothColdOwner::release_coexistence_clock(self);
    }

    fn select_main_xtal_low_power_clock(&mut self, divider: u16) -> bool {
        BluetoothColdOwner::select_main_xtal_low_power_clock(self, divider)
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
        disable_owned(&mut self.registers, &mut self.platform);
        BluetoothStopped::from_parts(self.registers, self.platform)
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
            registers,
            platform,
        })
    }
}

fn enable_owned(
    resources: &mut impl BluetoothSharedClockControl,
    platform: &mut impl BluetoothClockControl,
) -> Result<(), BluetoothClockError> {
    resources.prepare_shared_modem_clock_map();
    platform.enable_bluetooth_controller_pll_source();
    assert!(
        resources.retain_coexistence_clock(),
        "exclusive Bluetooth route retains its coexistence clock"
    );
    platform.enable_bluetooth_controller_dependents();
    platform.enable_bluetooth_apb_clocks();
    platform.reset_bluetooth_controller_domains();
    assert!(
        resources.select_main_xtal_low_power_clock(MAIN_XTAL_LOW_POWER_DIVIDER),
        "reviewed Bluetooth low-power divider fits the hardware field"
    );

    let platform_state = platform.bluetooth_platform_clock_state();
    let (shared, low_power) = resources.bluetooth_shared_clock_observation();
    let state = BluetoothClockState {
        controller_clocks_enabled: platform_state.controller_clocks_enabled
            && shared.coexistence_clock_enabled,
        apb_clocks_enabled: platform_state.apb_clocks_enabled,
        controller_resets_released: platform_state.controller_resets_released,
        main_xtal_selected: low_power.main_xtal_selected
            && !low_power.slow_oscillator_selected
            && !low_power.fast_oscillator_selected
            && !low_power.xtal32k_selected,
        low_power_divider: low_power.divider,
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
    } else if !state.main_xtal_selected {
        Some(BluetoothClockCheckpoint::LowPowerClockSource)
    } else if state.low_power_divider != MAIN_XTAL_LOW_POWER_DIVIDER {
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

fn disable_owned(
    resources: &mut impl BluetoothSharedClockControl,
    platform: &mut impl BluetoothClockControl,
) {
    resources.release_bluetooth_low_power_timer();
    platform.disable_bluetooth_apb_clocks();
    platform.disable_bluetooth_controller_pll_source();
    resources.release_coexistence_clock();
    platform.disable_bluetooth_controller_dependents();
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, rc::Rc, vec::Vec};

    use open_esp_radio_esp32s31_pac::{
        BluetoothLowPowerClockObservation, SharedModemClockObservation,
    };

    use super::{
        BluetoothClockCheckpoint, BluetoothClockControl, BluetoothPlatformClockState,
        BluetoothSharedClockControl, disable_owned, enable_owned,
    };

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum Operation {
        PrepareMap,
        EnablePllSource,
        RetainCoexistence,
        EnableControllerDependents,
        EnableApb,
        ResetDomains,
        SelectMainXtal(u16),
        ObservePlatform,
        ObserveShared,
        ReleaseLowPower,
        DisableApb,
        DisablePllSource,
        ReleaseCoexistence,
        DisableControllerDependents,
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
                    slow_oscillator_selected: false,
                    fast_oscillator_selected: false,
                    main_xtal_selected: true,
                    xtal32k_selected: false,
                    divider: 399,
                    timer_enabled: true,
                },
            }
        }
    }

    impl BluetoothSharedClockControl for FakeShared {
        fn prepare_shared_modem_clock_map(&mut self) {
            self.operations.borrow_mut().push(Operation::PrepareMap);
        }

        fn retain_coexistence_clock(&mut self) -> bool {
            self.operations
                .borrow_mut()
                .push(Operation::RetainCoexistence);
            true
        }

        fn release_coexistence_clock(&mut self) {
            self.operations
                .borrow_mut()
                .push(Operation::ReleaseCoexistence);
        }

        fn select_main_xtal_low_power_clock(&mut self, divider: u16) -> bool {
            self.operations
                .borrow_mut()
                .push(Operation::SelectMainXtal(divider));
            true
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
                    controller_clocks_enabled: true,
                    apb_clocks_enabled: true,
                    controller_resets_released: true,
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

        fn bluetooth_platform_clock_state(&mut self) -> BluetoothPlatformClockState {
            self.operations
                .borrow_mut()
                .push(Operation::ObservePlatform);
            self.state
        }

        fn disable_bluetooth_apb_clocks(&mut self) {
            self.operations.borrow_mut().push(Operation::DisableApb);
        }

        fn disable_bluetooth_controller_pll_source(&mut self) {
            self.operations
                .borrow_mut()
                .push(Operation::DisablePllSource);
        }

        fn disable_bluetooth_controller_dependents(&mut self) {
            self.operations
                .borrow_mut()
                .push(Operation::DisableControllerDependents);
        }
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
                Operation::PrepareMap,
                Operation::EnablePllSource,
                Operation::RetainCoexistence,
                Operation::EnableControllerDependents,
                Operation::EnableApb,
                Operation::ResetDomains,
                Operation::SelectMainXtal(399),
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
        platform.state.apb_clocks_enabled = false;
        let mut shared = FakeShared::ready(operations.clone());

        let failure = enable_owned(&mut shared, &mut platform).unwrap_err();
        assert_eq!(failure.checkpoint, BluetoothClockCheckpoint::ApbClocks);
        assert_eq!(
            &operations.borrow()[9..],
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
