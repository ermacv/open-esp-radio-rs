//! Owned ESP32-S31 Bluetooth controller clock lifecycle.
//!
//! The operation order comes from the exact ESP32-S31 controller source bound
//! to the investigated libraries:
//! `espressif/esp-idf@aeab6dcfbeb44aba4b1f8ed102e3086172833153` and
//! `espressif/esp32s31-bt-lib@7f20740dd66ee774ffce5db0b55507892551aa31`.

use open_esp_radio_esp32s31_hal::BluetoothColdOwner;
use open_esp_radio_esp32s31_pac::{
    BluetoothLowPowerClockObservation, ModemSysconBluetoothObservation,
    PlatformClockPowerObservation, SharedModemClockObservation,
};

use crate::resources::BluetoothStopped;

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
/// The route owner retains every reversible clock lease, while the platform
/// witness remains affine beside it. No controller/baseband MMIO is exposed by
/// this slice; the next lifecycle transaction consumes this value.
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
    fn retain_platform_pll_source(&mut self);
    fn release_platform_pll_source(&mut self);
    fn platform_clock_power_observation(&self) -> PlatformClockPowerObservation;
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
    fn retain_platform_pll_source(&mut self) {
        self.retain_platform_pll_source();
    }

    fn release_platform_pll_source(&mut self) {
        self.release_platform_pll_source();
    }

    fn platform_clock_power_observation(&self) -> PlatformClockPowerObservation {
        self.platform_clock_power_observation()
    }

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

impl<P> BluetoothClockedResources<P> {
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

impl<P> BluetoothStopped<P> {
    /// Execute the exact outer ESP32-S31 Bluetooth clock prerequisite.
    ///
    /// The current standalone profile uses the 40 MHz main crystal divided to
    /// 100 kHz, matching the source-bound S31 default. Any failed read-back is
    /// rolled back in the vendor's low-bit-first dependency order before the
    /// owners are returned.
    pub fn enable_clocks(
        self,
    ) -> Result<BluetoothClockedResources<P>, BluetoothClockEnableFailure<P>> {
        let (mut registers, platform) = self.into_parts();
        if let Err(error) = enable_owned(&mut registers) {
            return Err(BluetoothClockEnableFailure {
                stopped: BluetoothStopped::from_parts(registers, platform),
                error,
            });
        }

        Ok(BluetoothClockedResources {
            registers: Some(registers),
            platform: Some(platform),
            cleanup: |registers, _| disable_owned(registers),
            cleanup_armed: true,
        })
    }
}

fn enable_owned(
    resources: &mut impl BluetoothSharedClockControl,
) -> Result<(), BluetoothClockError> {
    resources.prepare_shared_modem_clock_map();
    resources.prepare_modem_syscon_clock_map();
    resources.retain_platform_pll_source();
    resources.retain_coexistence_clock();
    resources.enable_bluetooth_controller_dependents();
    resources.enable_bluetooth_apb_clocks();
    resources.reset_bluetooth_controller_domains();
    resources.retain_main_xtal_low_power_clock();

    let platform_state = resources.platform_clock_power_observation();
    let modem_state = resources.modem_syscon_clock_state();
    let (shared, low_power) = resources.bluetooth_shared_clock_observation();
    let state = BluetoothClockState {
        controller_clocks_enabled: platform_state.ref_160m_clock_enabled
            && platform_state.modem_source_clocks_configured
            && modem_state.controller_clocks_enabled
            && shared.coexistence_clock_enabled,
        apb_clocks_enabled: modem_state.apb_clocks_enabled,
        controller_resets_released: modem_state.controller_resets_released,
        exclusive_main_xtal_selected: low_power.exclusive_main_xtal_selected,
        low_power_divider_configured: low_power.bluetooth_divider_configured,
        low_power_timer_enabled: low_power.timer_enabled,
    };
    if let Err(error) = validate_state(state) {
        disable_owned(resources);
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

fn disable_owned(resources: &mut impl BluetoothSharedClockControl) {
    resources.release_bluetooth_low_power_timer();
    resources.disable_bluetooth_apb_clocks();
    resources.release_platform_pll_source();
    resources.release_coexistence_clock();
    resources.disable_bluetooth_controller_dependents();
}

#[cfg(test)]
mod tests;
