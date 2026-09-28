//! Target executor for the shared modem clock planner.
//!
//! The planner decides which dependency edges a module request needs; this
//! executor performs each edge and only then records it complete. Modem gates
//! go to the radio PAC. The upstream 160 MHz source and the analog-I2C master
//! clock belong to the platform clock owner and are reached through
//! [`PlatformClockProvider`], which counts its own references.
//!
//! The coexistence gate shares its register word with the analog-I2C master,
//! so it is a platform-owned half too.
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

/// A clock whose gate the platform clock owner writes and counts.
///
/// Each lives in a register word that the platform also writes for other
/// SoC users, so the radio requests it instead of writing the gate.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlatformClock {
    /// The 160 MHz PLL output that feeds the modem PLL source.
    Pll160m,
    /// The analog-I2C master clock gate in `MODEM_LPCON.CLK_CONF`.
    AnalogI2cMaster,
    /// The MPLL, whose PMU power bits share a word with the front-end
    /// baseband power the PHY writes.
    Mpll,
    /// The modem coexistence clock gate in `MODEM_LPCON.CLK_CONF`.
    ModemCoexistence,
    /// The modem low-power timer clock gate in `MODEM_LPCON.CLK_CONF`.
    ModemLowPowerTimer,
}

/// Platform-owned clocks that radio modules depend on.
///
/// The platform clock owner (ESP-HAL) keeps its own reference count of each,
/// because other SoC users share them.
pub trait PlatformClockProvider {
    /// Acquire one reference to `clock`.
    fn acquire(&mut self, clock: PlatformClock) -> Result<(), PlatformClockError>;
    /// Release one reference to `clock`.
    fn release(&mut self, clock: PlatformClock) -> Result<(), PlatformClockError>;
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
        Dependency::Pll160AndModemSource
        | Dependency::AnalogI2cMaster
        | Dependency::Coexistence => return None,
    })
}

fn perform_acquire(
    dependency: Dependency,
    port: &mut impl ModemClockPort,
    platform: &mut impl PlatformClockProvider,
) -> Result<(), PlatformClockError> {
    match dependency {
        Dependency::Pll160AndModemSource => {
            platform.acquire(PlatformClock::Pll160m)?;
            port.configure_device(ModemClockDevice::PllSourceGate, true);
        }
        Dependency::AnalogI2cMaster => platform.acquire(PlatformClock::AnalogI2cMaster)?,
        Dependency::Coexistence => platform.acquire(PlatformClock::ModemCoexistence)?,
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
            platform.release(PlatformClock::Pll160m)?;
        }
        Dependency::AnalogI2cMaster => platform.release(PlatformClock::AnalogI2cMaster)?,
        Dependency::Coexistence => platform.release(PlatformClock::ModemCoexistence)?,
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
