//! Route-owned modem clock leases and the reversible cold-power baseline.
//!
//! The restricted PAC exposes straight-line gate, configuration and baseline
//! transactions. This module decides when a route enables a shared clock,
//! which state it restores on release and when the captured cold-power
//! baseline may be committed. Every value here is route-scoped: a route
//! releases all leases and restores the baseline before the neutral root is
//! reconstructed.

use oer_esp32s31_pac::{
    RadioPhyRegisters, SharedModemClockGate, WifiPowerBaseline, WifiPowerRestoreReadback,
};

use crate::root::WifiPowerRestoreCheckpoint;

/// Register transactions consumed by route clock policy.
pub(crate) trait ClockPort {
    fn gate_enabled(&self, gate: SharedModemClockGate) -> bool;
    fn set_gate(&mut self, gate: SharedModemClockGate, enabled: bool);
    fn capture_power_baseline(&self) -> WifiPowerBaseline;
    fn restore_power_baseline(
        &mut self,
        baseline: WifiPowerBaseline,
    ) -> Result<(), WifiPowerRestoreReadback>;
    /// Run the common modem/PHY power sequence, retaining the PHY-I2C gate
    /// in `leases` at its exact late edge.
    fn run_common_power_sequence(
        &mut self,
        leases: &mut SharedClockLeases,
        entry: crate::power::PowerEntry,
    ) -> Result<(), crate::power::PowerError>;
}

impl ClockPort for RadioPhyRegisters {
    fn gate_enabled(&self, gate: SharedModemClockGate) -> bool {
        self.shared_modem_clock_gate_enabled(gate)
    }
    fn set_gate(&mut self, gate: SharedModemClockGate, enabled: bool) {
        self.set_shared_modem_clock_gate(gate, enabled);
    }
    fn capture_power_baseline(&self) -> WifiPowerBaseline {
        self.capture_wifi_power_baseline()
    }
    fn restore_power_baseline(
        &mut self,
        baseline: WifiPowerBaseline,
    ) -> Result<(), WifiPowerRestoreReadback> {
        self.restore_wifi_power_baseline(baseline)
    }
    fn run_common_power_sequence(
        &mut self,
        leases: &mut SharedClockLeases,
        entry: crate::power::PowerEntry,
    ) -> Result<(), crate::power::PowerError> {
        crate::power::execute_owned(&mut crate::power::RoutePower { phy: self, leases }, entry)
    }
}

/// One retained shared gate and the state observed before retention.
struct GateLease {
    gate: SharedModemClockGate,
    baseline: bool,
}

impl GateLease {
    fn retain(port: &mut impl ClockPort, gate: SharedModemClockGate) -> Self {
        let baseline = port.gate_enabled(gate);
        if !baseline {
            port.set_gate(gate, true);
        }
        Self { gate, baseline }
    }

    fn release(self, port: &mut impl ClockPort) {
        if port.gate_enabled(self.gate) != self.baseline {
            port.set_gate(self.gate, self.baseline);
        }
    }
}

/// The shared PHY-I2C gate retained while common radio power is held.
#[derive(Default)]
pub(crate) struct SharedClockLeases {
    phy_i2c: Option<GateLease>,
}

impl SharedClockLeases {
    /// Retain the PHY-I2C gate at its exact late power-sequence edge.
    pub(crate) fn retain_phy_i2c(&mut self, port: &mut impl ClockPort) {
        if self.phy_i2c.is_none() {
            self.phy_i2c = Some(GateLease::retain(port, SharedModemClockGate::PhyI2cMaster));
        }
    }

    pub(crate) fn release_phy_i2c(&mut self, port: &mut impl ClockPort) {
        if let Some(lease) = self.phy_i2c.take() {
            lease.release(port);
        }
    }

    /// Release every retained gate.
    pub(crate) fn release_all(&mut self, port: &mut impl ClockPort) {
        self.release_phy_i2c(port);
    }
}

/// Protocol clients that can share the powered radio.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RadioClient {
    Wifi,
    Bluetooth,
    Ieee802154,
}

impl RadioClient {
    const fn bit(self) -> u8 {
        match self {
            Self::Wifi => 1,
            Self::Bluetooth => 1 << 1,
            Self::Ieee802154 => 1 << 2,
        }
    }
}

/// Why a client cannot enter or leave the common radio power.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommonRadioPowerError {
    /// The client already holds common power.
    AlreadyEntered,
    /// The client does not hold common power.
    NotEntered,
    /// The first client's power sequence failed a read-back checkpoint.
    Power(crate::power::PowerError),
    /// The last client's cold-power baseline did not read back.
    Restore(WifiPowerRestoreCheckpoint),
}

/// Common modem/PHY power shared by concurrently running clients.
///
/// The power sequence runs once, for the first client; a later client must
/// not repeat it while another protocol runs. Only the first power-up since
/// boot pulses the Wi-Fi baseband and MAC resets: after every client left,
/// a new first client runs the sequence without that pulse, so the retained
/// PHY configuration in the Wi-Fi MAC register window survives. The
/// cold-power clock baseline is captured before the first edge and restored
/// after the last client leaves; the reset lines are not part of it. Refcounted modem
/// clock dependencies such as coexistence belong to the shared modem clock
/// planner, not to this membership.
#[derive(Default)]
pub(crate) struct CommonRadioPower {
    clients: u8,
    leases: SharedClockLeases,
    power: PowerEpoch,
    /// The Wi-Fi baseband and MAC resets were pulsed by a successful
    /// power-up since boot.
    wifi_resets_pulsed: bool,
}

impl CommonRadioPower {
    /// Whether `client` currently holds common power.
    pub(crate) const fn holds(&self, client: RadioClient) -> bool {
        self.clients & client.bit() != 0
    }

    /// Whether any client holds common power.
    pub(crate) const fn any_client(&self) -> bool {
        self.clients != 0
    }

    #[cfg(test)]
    pub(crate) fn hold_for_test(&mut self, client: RadioClient) {
        self.clients |= client.bit();
    }

    /// Enter common power; only the first client runs the power sequence.
    ///
    /// A failed sequence admits no client. It keeps the original baseline
    /// and any retained gate, so a retry restarts from the same cold state.
    pub(crate) fn enter(
        &mut self,
        port: &mut impl ClockPort,
        client: RadioClient,
    ) -> Result<(), CommonRadioPowerError> {
        if self.holds(client) {
            return Err(CommonRadioPowerError::AlreadyEntered);
        }
        if self.clients == 0 {
            self.power.prepare(port);
            let entry = if self.wifi_resets_pulsed {
                crate::power::PowerEntry::Repeated
            } else {
                crate::power::PowerEntry::FirstSinceBoot
            };
            port.run_common_power_sequence(&mut self.leases, entry)
                .map_err(CommonRadioPowerError::Power)?;
            self.wifi_resets_pulsed = true;
        }
        self.clients |= client.bit();
        Ok(())
    }

    /// Leave common power; the last client restores the cold baseline.
    ///
    /// A failed restore keeps `client` entered so the exit can be retried.
    pub(crate) fn exit(
        &mut self,
        port: &mut impl ClockPort,
        client: RadioClient,
    ) -> Result<(), CommonRadioPowerError> {
        if !self.holds(client) {
            return Err(CommonRadioPowerError::NotEntered);
        }
        if self.clients == client.bit() {
            self.leases.release_all(port);
            self.power
                .restore(port)
                .map_err(CommonRadioPowerError::Restore)?;
        }
        self.clients &= !client.bit();
        Ok(())
    }
}

/// Cold-power baseline captured before the first route power edge.
#[derive(Default)]
pub(crate) struct PowerEpoch {
    baseline: Option<WifiPowerBaseline>,
}

impl PowerEpoch {
    /// Capture the reversible baseline before the first mutation.
    ///
    /// A retry of the same partially executed power-up keeps the original
    /// baseline instead of sampling its own mutations as a new cold state.
    pub(crate) fn prepare(&mut self, port: &impl ClockPort) {
        if self.baseline.is_none() {
            self.baseline = Some(port.capture_power_baseline());
        }
    }

    /// Restore every non-monotonic field changed by the cold-power path.
    ///
    /// Shared gate leases must be released first. The baseline is committed
    /// only after every readback matches.
    pub(crate) fn restore(
        &mut self,
        port: &mut impl ClockPort,
    ) -> Result<(), WifiPowerRestoreCheckpoint> {
        let Some(baseline) = self.baseline else {
            return Ok(());
        };
        port.restore_power_baseline(baseline)
            .map_err(|readback| match readback {
                WifiPowerRestoreReadback::ModemSyscon => WifiPowerRestoreCheckpoint::ModemSyscon,
                WifiPowerRestoreReadback::ModemSourceClocks => {
                    WifiPowerRestoreCheckpoint::ModemSourceClocks
                }
                WifiPowerRestoreReadback::ModemRegisterBusClock => {
                    WifiPowerRestoreCheckpoint::ModemRegisterBusClock
                }
            })?;
        self.baseline = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
