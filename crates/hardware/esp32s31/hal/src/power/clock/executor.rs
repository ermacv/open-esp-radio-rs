//! Target executor for the shared modem clock planner.
//!
//! The planner decides which dependency edges a module request needs; this
//! executor performs each edge and only then records it complete. Modem gates
//! go to the radio PAC. The upstream 160 MHz source and the analog-I2C master
//! clock belong to the platform clock owner and are reached through
//! [`PlatformClockProvider`], which counts its own references.
//!
//! For the modem PLL-source dependency the executor acquires the 160 MHz
//! source before opening the modem gate and releases it after closing the
//! gate, in the vendor order. A provider failure poisons the transaction: the
//! planner cannot tell a refused request from a partially applied one.

use oer_esp32s31_pac::{ModemClockDevice, RadioPhyRegisters};

use super::{
    Dependency, ModemClockLease, ModemClockPlanner, PoisonedModemClockAcquire,
    PoisonedModemClockRelease, PreparedModemClockAcquire, PreparedModemClockRelease,
};

/// A platform clock request was refused.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PlatformClockError;

/// Platform-owned clock sources that radio modules depend on.
///
/// The platform clock owner (ESP-HAL) keeps its own reference counts for
/// these sources, because other SoC users share them.
pub trait PlatformClockProvider {
    /// Acquire one reference to the 160 MHz PLL source.
    fn acquire_pll_f160m(&mut self) -> Result<(), PlatformClockError>;
    /// Release one reference to the 160 MHz PLL source.
    fn release_pll_f160m(&mut self) -> Result<(), PlatformClockError>;
    /// Acquire one reference to the analog-I2C master clock.
    fn acquire_analog_i2c_clock(&mut self) -> Result<(), PlatformClockError>;
    /// Release one reference to the analog-I2C master clock.
    fn release_analog_i2c_clock(&mut self) -> Result<(), PlatformClockError>;
}

/// Modem clock gates driven by the radio PAC.
pub(crate) trait ModemClockPort {
    fn configure_device(&mut self, device: ModemClockDevice, enable: bool);
}

impl ModemClockPort for RadioPhyRegisters {
    fn configure_device(&mut self, device: ModemClockDevice, enable: bool) {
        self.configure_modem_clock_device(device, enable);
    }
}

const fn modem_device(dependency: Dependency) -> Option<ModemClockDevice> {
    Some(match dependency {
        Dependency::ModemAdcCommonFe => ModemClockDevice::ModemAdcCommonFe,
        Dependency::ModemPrivateFe => ModemClockDevice::ModemPrivateFe,
        Dependency::Coexistence => ModemClockDevice::Coexistence,
        Dependency::WifiApb => ModemClockDevice::WifiApb,
        Dependency::WifiBb44m => ModemClockDevice::WifiBaseband44m,
        Dependency::WifiMac => ModemClockDevice::WifiMac,
        Dependency::WifiBb => ModemClockDevice::WifiBaseband,
        Dependency::WifiBb80x1 => ModemClockDevice::WifiBaseband80x1,
        Dependency::Etm => ModemClockDevice::Etm,
        Dependency::BtMac => ModemClockDevice::BluetoothMac,
        Dependency::BtPeripheral => ModemClockDevice::BluetoothPeripheral,
        Dependency::BtApbAndSecurity => ModemClockDevice::BluetoothApb,
        Dependency::BtIeee802154CommonBaseband => {
            ModemClockDevice::BluetoothIeee802154CommonBaseband
        }
        Dependency::Ieee802154ApbAndMac => ModemClockDevice::Ieee802154Mac,
        // Platform-owned halves are handled by the caller.
        Dependency::Pll160AndModemSource | Dependency::AnalogI2cMaster => return None,
    })
}

fn perform_acquire(
    dependency: Dependency,
    port: &mut impl ModemClockPort,
    platform: &mut impl PlatformClockProvider,
) -> Result<(), PlatformClockError> {
    match dependency {
        Dependency::Pll160AndModemSource => {
            platform.acquire_pll_f160m()?;
            port.configure_device(ModemClockDevice::PllSourceGate, true);
        }
        Dependency::AnalogI2cMaster => platform.acquire_analog_i2c_clock()?,
        other => {
            if let Some(device) = modem_device(other) {
                port.configure_device(device, true);
            }
        }
    }
    Ok(())
}

fn perform_release(
    dependency: Dependency,
    port: &mut impl ModemClockPort,
    platform: &mut impl PlatformClockProvider,
) -> Result<(), PlatformClockError> {
    match dependency {
        Dependency::Pll160AndModemSource => {
            port.configure_device(ModemClockDevice::PllSourceGate, false);
            platform.release_pll_f160m()?;
        }
        Dependency::AnalogI2cMaster => platform.release_analog_i2c_clock()?,
        other => {
            if let Some(device) = modem_device(other) {
                port.configure_device(device, false);
            }
        }
    }
    Ok(())
}

/// Perform and commit one prepared acquisition.
///
/// # Errors
///
/// A refused platform request poisons the transaction at that edge.
#[allow(
    clippy::result_large_err,
    reason = "the poisoned transaction retains the planner without allocation"
)]
pub(crate) fn execute_acquire<'identity>(
    prepared: PreparedModemClockAcquire<'identity>,
    port: &mut impl ModemClockPort,
    platform: &mut impl PlatformClockProvider,
) -> Result<
    (ModemClockPlanner<'identity>, ModemClockLease<'identity>),
    PoisonedModemClockAcquire<'identity>,
> {
    oer_radio_clock::execute_acquire(prepared, |dependency| {
        perform_acquire(dependency, port, platform)
    })
}

/// Perform and commit one prepared release.
///
/// # Errors
///
/// A refused platform request poisons the transaction at that edge.
#[allow(
    clippy::result_large_err,
    reason = "the poisoned transaction retains the planner and lease without allocation"
)]
pub(crate) fn execute_release<'planner, 'lease>(
    prepared: PreparedModemClockRelease<'planner, 'lease>,
    port: &mut impl ModemClockPort,
    platform: &mut impl PlatformClockProvider,
) -> Result<ModemClockPlanner<'planner>, PoisonedModemClockRelease<'planner, 'lease>> {
    oer_radio_clock::execute_release(prepared, |dependency| {
        perform_release(dependency, port, platform)
    })
}

#[cfg(test)]
mod tests;
