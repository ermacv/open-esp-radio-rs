//! Owned ESP32-S31 Bluetooth controller clock lifecycle.
//!
//! The operation order comes from the exact ESP32-S31 controller source bound
//! to the investigated libraries:
//! `espressif/esp-idf@aeab6dcfbeb44aba4b1f8ed102e3086172833153` and
//! `espressif/esp32s31-bt-lib@7f20740dd66ee774ffce5db0b55507892551aa31`.

use crate::BluetoothPhysicalResources;

const MAIN_XTAL_LOW_POWER_DIVIDER: u16 = 399;

/// Platform-owned clock/reset operations required by standalone Bluetooth.
///
/// Implementations keep the official system PAC owners private. Each method
/// is one semantic operation from the ESP32-S31 modem-clock implementation;
/// raw register handles and masks do not cross into the Bluetooth driver.
pub trait BluetoothClockControl {
    /// Enable the dependency set of `PERIPH_BT_MODULE`.
    fn enable_bluetooth_controller_clocks(&mut self);

    /// Enable the dependency set of `PERIPH_BT_APB_MODULE`.
    fn enable_bluetooth_apb_clocks(&mut self);

    /// Pulse and release the BT MAC, BT MAC APB, BLE timer and modem-security
    /// resets in the reviewed platform order.
    fn reset_bluetooth_controller_domains(&mut self);

    /// Select the 40 MHz main crystal for the BLE low-power timer.
    ///
    /// `divider` is the hardware divider-minus-one image. The first supported
    /// profile is the vendor default 100 kHz timer, hence 399.
    fn select_main_xtal_low_power_clock(&mut self, divider: u16);

    /// Observe semantic read-back after the complete enable sequence.
    fn bluetooth_clock_state(&mut self) -> BluetoothClockState;

    /// Remove every BLE low-power timer source and gate its timer clock.
    fn deselect_low_power_clock(&mut self);

    /// Disable the dependency set of `PERIPH_BT_APB_MODULE`.
    fn disable_bluetooth_apb_clocks(&mut self);

    /// Disable the dependency set of `PERIPH_BT_MODULE`.
    fn disable_bluetooth_controller_clocks(&mut self);
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
    resources: BluetoothPhysicalResources,
    platform: P,
}

impl<P> BluetoothClockedResources<P> {
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn into_parts(self) -> (BluetoothPhysicalResources, P) {
        (self.resources, self.platform)
    }
}

impl<P: BluetoothClockControl> BluetoothClockedResources<P> {
    /// Reverse the exact clock prerequisite and recover both owners.
    pub fn disable_clocks(mut self) -> (BluetoothPhysicalResources, P) {
        disable_owned(&mut self.platform);
        (self.resources, self.platform)
    }
}

/// Failed clock setup after automatic reverse-order rollback.
#[must_use = "a failed Bluetooth clock transaction still owns both resources"]
pub struct BluetoothClockEnableFailure<P> {
    resources: BluetoothPhysicalResources,
    platform: P,
    error: BluetoothClockError,
}

impl<P> BluetoothClockEnableFailure<P> {
    /// Return the semantic reason for the failed transaction.
    pub const fn error(&self) -> BluetoothClockError {
        self.error
    }

    /// Recover the cold radio and platform owners after completed rollback.
    pub fn into_parts(self) -> (BluetoothPhysicalResources, P, BluetoothClockError) {
        (self.resources, self.platform, self.error)
    }
}

impl BluetoothPhysicalResources {
    /// Execute the exact outer ESP32-S31 Bluetooth clock prerequisite.
    ///
    /// The current standalone profile uses the 40 MHz main crystal divided to
    /// 100 kHz, matching the source-bound S31 default. Any failed read-back is
    /// rolled back in reverse order before the owners are returned.
    pub fn enable_clocks<P: BluetoothClockControl>(
        self,
        mut platform: P,
    ) -> Result<BluetoothClockedResources<P>, BluetoothClockEnableFailure<P>> {
        platform.enable_bluetooth_controller_clocks();
        platform.enable_bluetooth_apb_clocks();
        platform.reset_bluetooth_controller_domains();
        platform.select_main_xtal_low_power_clock(MAIN_XTAL_LOW_POWER_DIVIDER);

        let state = platform.bluetooth_clock_state();
        if let Err(error) = validate_state(state) {
            disable_owned(&mut platform);
            return Err(BluetoothClockEnableFailure {
                resources: self,
                platform,
                error,
            });
        }

        Ok(BluetoothClockedResources {
            resources: self,
            platform,
        })
    }
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

pub(crate) fn disable_owned(platform: &mut impl BluetoothClockControl) {
    platform.deselect_low_power_clock();
    platform.disable_bluetooth_apb_clocks();
    platform.disable_bluetooth_controller_clocks();
}

#[cfg(test)]
mod tests {
    use std::vec::Vec;

    use open_esp_radio_esp32s31_pac::RadioHardware;

    use super::{
        BluetoothClockCheckpoint, BluetoothClockControl, BluetoothClockState,
        BluetoothPhysicalResources,
    };

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum Operation {
        EnableController,
        EnableApb,
        ResetDomains,
        SelectMainXtal(u16),
        Observe,
        DeselectLowPower,
        DisableApb,
        DisableController,
    }

    struct FakePlatform {
        operations: Vec<Operation>,
        state: BluetoothClockState,
    }

    impl FakePlatform {
        fn ready() -> Self {
            Self {
                operations: Vec::new(),
                state: BluetoothClockState {
                    controller_clocks_enabled: true,
                    apb_clocks_enabled: true,
                    controller_resets_released: true,
                    main_xtal_selected: true,
                    low_power_divider: 399,
                    low_power_timer_enabled: true,
                },
            }
        }
    }

    impl BluetoothClockControl for FakePlatform {
        fn enable_bluetooth_controller_clocks(&mut self) {
            self.operations.push(Operation::EnableController);
        }

        fn enable_bluetooth_apb_clocks(&mut self) {
            self.operations.push(Operation::EnableApb);
        }

        fn reset_bluetooth_controller_domains(&mut self) {
            self.operations.push(Operation::ResetDomains);
        }

        fn select_main_xtal_low_power_clock(&mut self, divider: u16) {
            self.operations.push(Operation::SelectMainXtal(divider));
        }

        fn bluetooth_clock_state(&mut self) -> BluetoothClockState {
            self.operations.push(Operation::Observe);
            self.state
        }

        fn deselect_low_power_clock(&mut self) {
            self.operations.push(Operation::DeselectLowPower);
        }

        fn disable_bluetooth_apb_clocks(&mut self) {
            self.operations.push(Operation::DisableApb);
        }

        fn disable_bluetooth_controller_clocks(&mut self) {
            self.operations.push(Operation::DisableController);
        }
    }

    fn resources() -> BluetoothPhysicalResources {
        BluetoothPhysicalResources::from_radio_hardware(RadioHardware::for_validation())
    }

    #[test]
    fn exact_clock_order_retains_both_owners_until_reverse_shutdown() {
        let clocked = match resources().enable_clocks(FakePlatform::ready()) {
            Ok(clocked) => clocked,
            Err(_) => panic!("valid clock state was rejected"),
        };
        let (resources, platform) = clocked.disable_clocks();

        assert_eq!(
            platform.operations,
            [
                Operation::EnableController,
                Operation::EnableApb,
                Operation::ResetDomains,
                Operation::SelectMainXtal(399),
                Operation::Observe,
                Operation::DeselectLowPower,
                Operation::DisableApb,
                Operation::DisableController,
            ]
        );
        let _hardware = resources.release();
    }

    #[test]
    fn failed_readback_rolls_back_before_returning_owners() {
        let mut platform = FakePlatform::ready();
        platform.state.low_power_timer_enabled = false;

        let failure = match resources().enable_clocks(platform) {
            Ok(_) => panic!("invalid clock state was accepted"),
            Err(failure) => failure,
        };
        assert_eq!(
            failure.error().checkpoint,
            BluetoothClockCheckpoint::LowPowerTimerClock
        );
        let (resources, platform, _) = failure.into_parts();
        assert_eq!(
            &platform.operations[5..],
            [
                Operation::DeselectLowPower,
                Operation::DisableApb,
                Operation::DisableController,
            ]
        );
        let _hardware = resources.release();
    }
}
