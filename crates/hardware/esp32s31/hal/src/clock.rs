//! Common radio power, its platform clock references and the reversible
//! cold-power baseline.
//!
//! The restricted PAC exposes straight-line configuration and baseline
//! transactions. This module decides when common power takes and drops the
//! platform-owned 160 MHz and analog-I2C references, which state it restores
//! on release and when the captured cold-power baseline may be committed.
//! Every value here is route-scoped: a route drops its references and
//! restores the baseline before the neutral root is reconstructed.

use oer_esp32s31_pac::{RadioPhyRegisters, WifiPowerBaseline, WifiPowerRestoreReadback};

use crate::power::{PlatformClock, PlatformClockError, PlatformClockProvider};
use crate::root::WifiPowerRestoreCheckpoint;

/// Register transactions consumed by route clock policy.
pub(crate) trait ClockPort {
    fn capture_power_baseline(&self) -> WifiPowerBaseline;
    fn restore_power_baseline(
        &mut self,
        baseline: WifiPowerBaseline,
    ) -> Result<(), WifiPowerRestoreReadback>;
    /// Run the common modem/PHY power sequence, taking the platform clock
    /// references into `refs` at their exact edges.
    fn run_common_power_sequence(
        &mut self,
        refs: &mut PlatformClockRefs,
        platform: &mut impl PlatformClockProvider,
        entry: crate::power::PowerEntry,
    ) -> Result<(), crate::power::PowerError>;
}

impl ClockPort for RadioPhyRegisters {
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
        refs: &mut PlatformClockRefs,
        platform: &mut impl PlatformClockProvider,
        entry: crate::power::PowerEntry,
    ) -> Result<(), crate::power::PowerError> {
        crate::power::execute_owned(
            &mut crate::power::RoutePower {
                phy: self,
                refs,
                platform,
            },
            entry,
        )
    }
}

/// The platform clock references common radio power holds.
///
/// The 160 MHz reference, the analog-I2C master clock and the MPLL are shared
/// with other SoC users, so their platform owner (ESP-HAL) counts references
/// and alone writes their gates. Common power takes one reference to each for
/// as long as any client holds it; the flags make a retried power-up or
/// power-down take or drop each reference exactly once.
#[derive(Default)]
pub(crate) struct PlatformClockRefs {
    pll_f160m: bool,
    analog_i2c: bool,
    mpll: bool,
}

impl PlatformClockRefs {
    fn held(&mut self, clock: PlatformClock) -> &mut bool {
        match clock {
            PlatformClock::Pll160m => &mut self.pll_f160m,
            PlatformClock::AnalogI2cMaster => &mut self.analog_i2c,
            PlatformClock::Mpll => &mut self.mpll,
            PlatformClock::ModemCoexistence | PlatformClock::ModemLowPowerTimer => {
                unreachable!("common power holds no modem module gate")
            }
        }
    }

    pub(crate) fn acquire(
        &mut self,
        clock: PlatformClock,
        platform: &mut impl PlatformClockProvider,
    ) -> Result<(), PlatformClockError> {
        if !*self.held(clock) {
            platform.acquire(clock)?;
            *self.held(clock) = true;
        }
        Ok(())
    }

    fn release(
        &mut self,
        clock: PlatformClock,
        platform: &mut impl PlatformClockProvider,
    ) -> Result<(), PlatformClockError> {
        if *self.held(clock) {
            platform.release(clock)?;
            *self.held(clock) = false;
        }
        Ok(())
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
    /// The platform clock owner refused to release a reference.
    PlatformClock(PlatformClockError),
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
    refs: PlatformClockRefs,
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
    /// and any platform reference already taken, so a retry restarts from
    /// the same cold state without taking a reference twice.
    pub(crate) fn enter(
        &mut self,
        port: &mut impl ClockPort,
        platform: &mut impl PlatformClockProvider,
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
            port.run_common_power_sequence(&mut self.refs, platform, entry)
                .map_err(CommonRadioPowerError::Power)?;
            self.wifi_resets_pulsed = true;
        }
        self.clients |= client.bit();
        Ok(())
    }

    /// Leave common power; the last client drops the analog-I2C reference,
    /// restores the cold baseline and then drops the 160 MHz and MPLL
    /// references, as the vendor closes a gate before releasing its source.
    ///
    /// A failed step keeps `client` entered so the exit can be retried.
    pub(crate) fn exit(
        &mut self,
        port: &mut impl ClockPort,
        platform: &mut impl PlatformClockProvider,
        client: RadioClient,
    ) -> Result<(), CommonRadioPowerError> {
        if !self.holds(client) {
            return Err(CommonRadioPowerError::NotEntered);
        }
        if self.clients == client.bit() {
            self.refs
                .release(PlatformClock::AnalogI2cMaster, platform)
                .map_err(CommonRadioPowerError::PlatformClock)?;
            self.power
                .restore(port)
                .map_err(CommonRadioPowerError::Restore)?;
            for clock in [PlatformClock::Pll160m, PlatformClock::Mpll] {
                self.refs
                    .release(clock, platform)
                    .map_err(CommonRadioPowerError::PlatformClock)?;
            }
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
    /// The analog-I2C reference must be released first. The baseline is committed
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
