//! Station TBTT and modem-wakeup rollback obligations.
//!
//! The restricted PAC captures, programs and restores the reviewed station
//! wake fields. This module owns the single outstanding obligation of each
//! kind for the Wi-Fi route. A forgotten rollback token leaves the route
//! quarantined: no `Drop` implementation may touch MMIO through a lost borrow,
//! so the next prepare fails closed until an explicit restore or the
//! enclosing hardware-reset lifecycle replaces the route.

use core::cell::RefMut;

use oer_esp32s31_pac::{
    StaModemWakeConfig, StaModemWakeRestore, StaTbttWakeGateBaselineUnsupported,
    StaTbttWakeRestore, WifiRadioRegisters,
};

/// A second modem-wakeup transaction cannot overlap the first one.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaModemWakePrepareError {
    AlreadyConfigured,
}

/// Failure to consume one modem-wakeup rollback obligation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaModemWakeRestoreError {
    NotConfigured,
}

/// Failed rollback retaining the unique obligation token.
pub struct StaModemWakeRestoreFailure {
    pub error: StaModemWakeRestoreError,
    pub restore: StaModemWakeRestore,
}

/// Failure to start the reviewed station-TBTT wake prefix.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaTbttWakePrepareError {
    AlreadyPrepared,
    /// The complete vendor disable leaf leaves RTC CONTROL bit 21 asserted.
    /// Entry therefore requires that exact idle image: synthesizing a clear
    /// during rollback would not be evidence-backed.
    WakeGateBaselineUnsupported,
}

/// Failure to consume one station-TBTT rollback obligation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaTbttWakeRestoreError {
    NotPrepared,
}

/// Failed rollback retaining the unique obligation token.
pub struct StaTbttWakeRestoreFailure {
    pub error: StaTbttWakeRestoreError,
    pub restore: StaTbttWakeRestore,
}

/// Outstanding station wake obligations of one Wi-Fi route.
#[derive(Debug, Default)]
pub(crate) struct StationWakeState {
    tbtt_prepared: bool,
    modem_wakeup_configured: bool,
}

impl StationWakeState {
    fn acquire_modem_wakeup(&mut self) -> Result<(), StaModemWakePrepareError> {
        if self.modem_wakeup_configured {
            return Err(StaModemWakePrepareError::AlreadyConfigured);
        }
        self.modem_wakeup_configured = true;
        Ok(())
    }

    fn require_modem_wakeup(&self) -> Result<(), StaModemWakeRestoreError> {
        if self.modem_wakeup_configured {
            Ok(())
        } else {
            Err(StaModemWakeRestoreError::NotConfigured)
        }
    }

    fn require_tbtt_idle(&self) -> Result<(), StaTbttWakePrepareError> {
        if self.tbtt_prepared {
            Err(StaTbttWakePrepareError::AlreadyPrepared)
        } else {
            Ok(())
        }
    }

    fn require_tbtt_prepared(&self) -> Result<(), StaTbttWakeRestoreError> {
        if self.tbtt_prepared {
            Ok(())
        } else {
            Err(StaTbttWakeRestoreError::NotPrepared)
        }
    }
}

enum StationWakeRegisters<'registers> {
    Owned(
        &'registers mut WifiRadioRegisters,
        &'registers mut StationWakeState,
    ),
    Published(
        RefMut<'registers, WifiRadioRegisters>,
        RefMut<'registers, StationWakeState>,
    ),
}

impl StationWakeRegisters<'_> {
    fn parts_mut(&mut self) -> (&mut WifiRadioRegisters, &mut StationWakeState) {
        match self {
            Self::Owned(registers, state) => (registers, state),
            Self::Published(registers, state) => (registers, state),
        }
    }

    fn state(&self) -> &StationWakeState {
        match self {
            Self::Owned(_, state) => state,
            Self::Published(_, state) => state,
        }
    }
}

/// Closed HAL authority for the station TBTT and modem-wakeup transactions.
///
/// This type exposes neither the Wi-Fi register set nor `Deref`. Each
/// prepare returns the only rollback token for its obligation.
pub struct StationWakeHal<'registers> {
    registers: StationWakeRegisters<'registers>,
}

impl<'registers> StationWakeHal<'registers> {
    pub(crate) fn from_owned(
        registers: &'registers mut WifiRadioRegisters,
        state: &'registers mut StationWakeState,
    ) -> Self {
        Self {
            registers: StationWakeRegisters::Owned(registers, state),
        }
    }

    pub(crate) fn from_published(
        registers: RefMut<'registers, WifiRadioRegisters>,
        state: RefMut<'registers, StationWakeState>,
    ) -> Self {
        Self {
            registers: StationWakeRegisters::Published(registers, state),
        }
    }

    /// Apply only the reviewed raw modem-wakeup field transaction and retain
    /// its exact rollback obligation. This does not derive counter units or
    /// authorize RF/PHY sleep.
    pub fn configure_station_modem_wakeup(
        &mut self,
        config: StaModemWakeConfig,
    ) -> Result<StaModemWakeRestore, StaModemWakePrepareError> {
        let (registers, state) = self.registers.parts_mut();
        state.acquire_modem_wakeup()?;
        Ok(registers.configure_station_modem_wakeup(config))
    }

    /// Consume the exact field rollback returned by
    /// [`Self::configure_station_modem_wakeup`].
    pub fn restore_station_modem_wakeup(
        &mut self,
        restore: StaModemWakeRestore,
    ) -> Result<(), StaModemWakeRestoreFailure> {
        let (registers, state) = self.registers.parts_mut();
        if let Err(error) = state.require_modem_wakeup() {
            return Err(StaModemWakeRestoreFailure { error, restore });
        }
        registers.restore_station_modem_wakeup(restore);
        state.modem_wakeup_configured = false;
        Ok(())
    }

    /// Value-only quarantine diagnostic for a missing rollback token.
    ///
    /// It grants no recovery authority; `true` after a forgotten token means
    /// the route is intentionally quarantined.
    pub fn station_modem_wakeup_restore_pending(&self) -> bool {
        self.registers.state().modem_wakeup_configured
    }

    /// Program only the reviewed station-TBTT wake prefix. The returned token
    /// owns rollback; this operation does not claim RF/PHY sleep entry.
    pub fn prepare_station_tbtt_wake(
        &mut self,
        wake_tsf: u64,
    ) -> Result<StaTbttWakeRestore, StaTbttWakePrepareError> {
        let (registers, state) = self.registers.parts_mut();
        state.require_tbtt_idle()?;
        let restore = registers.prepare_station_tbtt_wake(wake_tsf).map_err(
            |StaTbttWakeGateBaselineUnsupported| {
                StaTbttWakePrepareError::WakeGateBaselineUnsupported
            },
        )?;
        state.tbtt_prepared = true;
        Ok(restore)
    }

    /// Consume the exact rollback obligation created by
    /// [`Self::prepare_station_tbtt_wake`].
    pub fn restore_station_tbtt_wake(
        &mut self,
        restore: StaTbttWakeRestore,
    ) -> Result<(), StaTbttWakeRestoreFailure> {
        let (registers, state) = self.registers.parts_mut();
        if let Err(error) = state.require_tbtt_prepared() {
            return Err(StaTbttWakeRestoreFailure { error, restore });
        }
        registers.restore_station_tbtt_wake(restore);
        state.tbtt_prepared = false;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
