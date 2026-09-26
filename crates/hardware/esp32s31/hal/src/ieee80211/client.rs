//! Wi-Fi as a client of the shared radio arbiter.
//!
//! The concurrent split hands Wi-Fi its MAC partition and interrupt setup;
//! common radio power, the modem clocks and every shared register stay with
//! the arbiter. These owners follow ESP-IDF's bring-up: common power, then
//! `modem_clock_module_enable(PERIPH_WIFI_MODULE)`, then the Wi-Fi
//! initialized status that keeps the Wi-Fi clocks enabled while Wi-Fi is
//! initialized. Each stage proves only its own step; MAC, PHY and RF
//! readiness are later stages.
//!
//! A clocked client borrows its capabilities with the arbiter lease: the
//! cold MAC HAL and the channel HAL take the shared registers from the lease
//! for one transaction, the MAC reset pulses through it, and the runtime MAC
//! HAL needs no lease because the Wi-Fi hot path touches Wi-Fi registers
//! only.

use core::fmt;

use oer_esp32s31_pac::{MacInterruptSetup, WifiRadioRegisters};

use crate::{
    ieee80211::{
        channel::RadioChannelHal,
        mac::{WifiMacColdHal, WifiMacHal},
    },
    root::WifiPartition,
    shared_radio::{
        CommonRadioPowerError, ModemClockError, PlatformClockProvider, SharedRadioLease,
    },
};

/// The Wi-Fi MAC register set and its interrupt setup.
struct WifiClientRegisters {
    mac: WifiRadioRegisters,
    interrupts: MacInterruptSetup,
}

/// Unpowered Wi-Fi client holding its partition.
///
/// Construction performs no MMIO and proves neither power nor clocks.
#[must_use = "the Wi-Fi client retains its radio partition"]
pub struct WifiCold {
    registers: WifiClientRegisters,
}

/// Wi-Fi client inside common radio power; its module clocks are off.
#[must_use = "the powered Wi-Fi client must leave common power"]
pub struct WifiPowered {
    registers: WifiClientRegisters,
}

/// Wi-Fi client with its module clocks enabled.
#[must_use = "the clocked Wi-Fi client must disable its clocks"]
pub struct WifiClocked {
    registers: WifiClientRegisters,
    initialized: bool,
}

/// Failed Wi-Fi lifecycle transition retaining the unchanged owner.
#[must_use = "a failed Wi-Fi transition still owns the partition"]
pub struct WifiTransitionFailure<Owner, Error> {
    owner: Owner,
    error: Error,
}

impl<Owner, Error: Copy> WifiTransitionFailure<Owner, Error> {
    pub const fn error(&self) -> Error {
        self.error
    }

    /// Recover the owner. After [`ModemClockError::Poisoned`] no further
    /// modem clock change succeeds until reset.
    pub fn into_owner(self) -> Owner {
        self.owner
    }
}

impl<Owner, Error: fmt::Debug> fmt::Debug for WifiTransitionFailure<Owner, Error> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WifiTransitionFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

impl WifiCold {
    /// Take the partition of a concurrent split. This performs no MMIO.
    pub fn from_partition(partition: WifiPartition) -> Self {
        let (mac, interrupts) = partition.into_parts();
        Self {
            registers: WifiClientRegisters {
                mac: WifiRadioRegisters::new(mac),
                interrupts,
            },
        }
    }

    /// Return the partition. This performs no MMIO.
    pub fn into_partition(self) -> WifiPartition {
        let WifiClientRegisters { mac, interrupts } = self.registers;
        WifiPartition::from_parts(mac.into_partition(), interrupts)
    }

    /// Enter common radio power as the Wi-Fi client. Only the first client
    /// runs the modem/PHY power sequence.
    ///
    /// # Errors
    ///
    /// The client already holds power, or the first client's sequence failed
    /// a read-back checkpoint; the unchanged owner is returned.
    pub fn power_up<T>(
        self,
        lease: &mut SharedRadioLease<'_, T>,
    ) -> Result<WifiPowered, WifiTransitionFailure<Self, CommonRadioPowerError>> {
        match lease.enter_common_power(&self.registers.mac) {
            Ok(()) => Ok(WifiPowered {
                registers: self.registers,
            }),
            Err(error) => Err(WifiTransitionFailure { owner: self, error }),
        }
    }
}

impl WifiPowered {
    /// Leave common radio power; the last client restores the cold baseline.
    ///
    /// # Errors
    ///
    /// The baseline did not read back; the client stays entered.
    pub fn power_down<T>(
        self,
        lease: &mut SharedRadioLease<'_, T>,
    ) -> Result<WifiCold, WifiTransitionFailure<Self, CommonRadioPowerError>> {
        match lease.exit_common_power(&self.registers.mac) {
            Ok(()) => Ok(WifiCold {
                registers: self.registers,
            }),
            Err(error) => Err(WifiTransitionFailure { owner: self, error }),
        }
    }

    /// Enable the Wi-Fi module clocks through the shared planner:
    /// `modem_clock_module_enable(PERIPH_WIFI_MODULE)`. Only dependencies no
    /// other client holds are switched on.
    ///
    /// # Errors
    ///
    /// The planner rejected the request before any access, or a platform
    /// request failed and poisoned the modem clocks.
    pub fn enable_clocks<T>(
        self,
        lease: &mut SharedRadioLease<'_, T>,
        platform: &mut impl PlatformClockProvider,
    ) -> Result<WifiClocked, WifiTransitionFailure<Self, ModemClockError>> {
        match lease.enable_modem_clocks(&self.registers.mac, platform) {
            Ok(()) => Ok(WifiClocked {
                registers: self.registers,
                initialized: false,
            }),
            Err(error) => Err(WifiTransitionFailure { owner: self, error }),
        }
    }
}

impl WifiClocked {
    /// Mark Wi-Fi initialized (`esp_wifi_init` success) or no longer
    /// initialized (`esp_wifi_deinit`), as `modem_clock_configure_wifi_status`
    /// does.
    ///
    /// # Errors
    ///
    /// The modem clocks are poisoned.
    pub fn set_initialized<T>(
        &mut self,
        lease: &mut SharedRadioLease<'_, T>,
        initialized: bool,
    ) -> Result<(), ModemClockError> {
        lease.set_wifi_initialized(&self.registers.mac, initialized)?;
        self.initialized = initialized;
        Ok(())
    }

    /// A clocked owner over an isolated validation partition, without MMIO
    /// and without claiming any clock state.
    #[cfg(any(test, feature = "validation-probes"))]
    #[doc(hidden)]
    pub fn for_validation(cold: WifiCold) -> Self {
        Self {
            registers: cold.registers,
            initialized: false,
        }
    }

    /// Whether Wi-Fi is marked initialized.
    pub const fn initialized(&self) -> bool {
        self.initialized
    }

    /// Pulse the Wi-Fi MAC reset, as
    /// `modem_clock_module_mac_reset(PERIPH_WIFI_MODULE)` does through
    /// `modem_syscon_ll_reset_wifimac`.
    pub fn reset_mac<T>(&mut self, lease: &mut SharedRadioLease<'_, T>) {
        let phy = lease.registers_mut().radio_phy_mut();
        phy.set_wifi_mac_reset(true);
        phy.set_wifi_mac_reset(false);
    }

    /// Borrow the cold MAC HAL for one transaction, with the shared radio
    /// owner borrowed from the arbiter lease.
    ///
    /// Clock and modem-source configuration of the exclusive route are
    /// no-ops here: the arbiter enabled the Wi-Fi module clocks already.
    pub fn cold_mac_hal<'hal, T>(
        &'hal mut self,
        lease: &'hal mut SharedRadioLease<'_, T>,
    ) -> WifiMacColdHal<'hal> {
        let WifiClientRegisters { mac, interrupts } = &mut self.registers;
        WifiMacColdHal::from_leased(mac, interrupts, lease.registers_mut())
    }

    /// Borrow the runtime MAC HAL over the MAC alone.
    pub fn wifi_mac_hal(&mut self) -> WifiMacHal<'_> {
        WifiMacHal::from_mac(&mut self.registers.mac)
    }

    /// Borrow the channel HAL for one channel transaction, with the shared
    /// PHY borrowed from the arbiter lease.
    pub fn channel_hal<'hal, P, T>(
        &'hal mut self,
        platform: &'hal mut P,
        lease: &'hal mut SharedRadioLease<'_, T>,
    ) -> RadioChannelHal<'hal, P> {
        let (phy, restore) = lease.phy_parts_mut();
        RadioChannelHal::from_leased(platform, &mut self.registers.mac, phy, restore)
    }

    /// Borrow the channel HAL together with the arbiter's attachment, for a
    /// PHY channel operation that updates the shared domain's state.
    pub fn channel_hal_with_attachment<'hal, P, T>(
        &'hal mut self,
        platform: &'hal mut P,
        lease: &'hal mut SharedRadioLease<'_, T>,
    ) -> (RadioChannelHal<'hal, P>, &'hal mut T) {
        let (phy, restore, attachment) = lease.phy_parts_with_attachment();
        (
            RadioChannelHal::from_leased(platform, &mut self.registers.mac, phy, restore),
            attachment,
        )
    }

    /// Disable the Wi-Fi module clocks. While Wi-Fi is marked initialized
    /// the planner keeps the Wi-Fi dependencies' hardware enabled, as the
    /// vendor does.
    ///
    /// # Errors
    ///
    /// The planner rejected the release before any access, or a platform
    /// request failed and poisoned the modem clocks.
    pub fn disable_clocks<T>(
        self,
        lease: &mut SharedRadioLease<'_, T>,
        platform: &mut impl PlatformClockProvider,
    ) -> Result<WifiPowered, WifiTransitionFailure<Self, ModemClockError>> {
        match lease.disable_modem_clocks(&self.registers.mac, platform) {
            Ok(()) => Ok(WifiPowered {
                registers: self.registers,
            }),
            Err(error) => Err(WifiTransitionFailure { owner: self, error }),
        }
    }
}

#[cfg(test)]
mod tests;
