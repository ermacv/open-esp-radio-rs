//! Route-owned modem clock leases and the reversible cold-power baseline.
//!
//! The restricted PAC exposes straight-line gate, configuration and baseline
//! transactions. This module decides when a route enables a shared clock,
//! which state it restores on release and when the captured cold-power
//! baseline may be committed. Every value here is route-scoped: a route
//! releases all leases and restores the baseline before the neutral root is
//! reconstructed.

use oer_esp32s31_pac::{
    BLUETOOTH_APB_CLOCKS, BLUETOOTH_CLOCK_COUNT, BLUETOOTH_CONTROLLER_CLOCKS,
    BLUETOOTH_MAIN_XTAL_LOW_POWER_DIVIDER, BluetoothClockBaseline,
    BluetoothLowPowerTimerConfiguration, ModemLowPowerClockDivider, ModemLowPowerClockSource,
    ModemSysconBluetoothClock, PlatformPllSourceBaseline, RadioPhyRegisters, SharedModemClockGate,
    WifiPowerBaseline, WifiPowerRestoreReadback,
};

use crate::root::WifiPowerRestoreCheckpoint;

/// Register transactions consumed by route clock policy.
pub(crate) trait ClockPort {
    fn gate_enabled(&self, gate: SharedModemClockGate) -> bool;
    fn set_gate(&mut self, gate: SharedModemClockGate, enabled: bool);
    fn low_power_timer_configuration(&self) -> BluetoothLowPowerTimerConfiguration;
    fn configure_low_power_timer(
        &mut self,
        source: ModemLowPowerClockSource,
        divider: ModemLowPowerClockDivider,
    );
    fn restore_low_power_timer(&mut self, configuration: BluetoothLowPowerTimerConfiguration);
    fn pll_source_baseline(&self) -> PlatformPllSourceBaseline;
    fn configure_modem_source_clocks(&mut self);
    fn restore_pll_source(&mut self, baseline: PlatformPllSourceBaseline);
    fn capture_power_baseline(&self) -> WifiPowerBaseline;
    fn restore_power_baseline(
        &mut self,
        baseline: WifiPowerBaseline,
    ) -> Result<(), WifiPowerRestoreReadback>;
    fn prepare_syscon_clock_map(&mut self);
    fn bluetooth_clock_baselines(&self) -> [BluetoothClockBaseline; BLUETOOTH_CLOCK_COUNT];
    fn enable_bluetooth_clock(&mut self, clock: ModemSysconBluetoothClock);
    fn restore_bluetooth_clock(
        &mut self,
        clock: ModemSysconBluetoothClock,
        baseline: BluetoothClockBaseline,
    );
    /// Run the common modem/PHY power sequence, retaining the PHY-I2C gate
    /// in `leases` at its exact late edge.
    fn run_common_power_sequence(
        &mut self,
        leases: &mut SharedClockLeases,
    ) -> Result<(), crate::power::PowerError>;
}

impl ClockPort for RadioPhyRegisters {
    fn gate_enabled(&self, gate: SharedModemClockGate) -> bool {
        self.shared_modem_clock_gate_enabled(gate)
    }
    fn set_gate(&mut self, gate: SharedModemClockGate, enabled: bool) {
        self.set_shared_modem_clock_gate(gate, enabled);
    }
    fn low_power_timer_configuration(&self) -> BluetoothLowPowerTimerConfiguration {
        self.bluetooth_low_power_timer_configuration()
    }
    fn configure_low_power_timer(
        &mut self,
        source: ModemLowPowerClockSource,
        divider: ModemLowPowerClockDivider,
    ) {
        self.configure_bluetooth_low_power_timer(source, divider);
    }
    fn restore_low_power_timer(&mut self, configuration: BluetoothLowPowerTimerConfiguration) {
        self.restore_bluetooth_low_power_timer_configuration(configuration);
    }
    fn pll_source_baseline(&self) -> PlatformPllSourceBaseline {
        self.platform_pll_source_baseline()
    }
    fn configure_modem_source_clocks(&mut self) {
        RadioPhyRegisters::configure_modem_source_clocks(self);
    }
    fn restore_pll_source(&mut self, baseline: PlatformPllSourceBaseline) {
        self.restore_platform_pll_source_baseline(baseline);
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
    fn prepare_syscon_clock_map(&mut self) {
        self.prepare_modem_syscon_clock_map();
    }
    fn bluetooth_clock_baselines(&self) -> [BluetoothClockBaseline; BLUETOOTH_CLOCK_COUNT] {
        RadioPhyRegisters::bluetooth_clock_baselines(self)
    }
    fn enable_bluetooth_clock(&mut self, clock: ModemSysconBluetoothClock) {
        RadioPhyRegisters::enable_bluetooth_clock(self, clock);
    }
    fn restore_bluetooth_clock(
        &mut self,
        clock: ModemSysconBluetoothClock,
        baseline: BluetoothClockBaseline,
    ) {
        RadioPhyRegisters::restore_bluetooth_clock(self, clock, baseline);
    }
    fn run_common_power_sequence(
        &mut self,
        leases: &mut SharedClockLeases,
    ) -> Result<(), crate::power::PowerError> {
        crate::power::execute_owned(&mut crate::power::RoutePower { phy: self, leases })
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

/// Shared PHY-I2C and coexistence gates retained by one protocol route.
#[derive(Default)]
pub(crate) struct SharedClockLeases {
    phy_i2c: Option<GateLease>,
    coexistence: Option<GateLease>,
}

impl SharedClockLeases {
    /// Retain the PHY-I2C gate at its exact late power-sequence edge.
    pub(crate) fn retain_phy_i2c(&mut self, port: &mut impl ClockPort) {
        if self.phy_i2c.is_none() {
            self.phy_i2c = Some(GateLease::retain(port, SharedModemClockGate::PhyI2cMaster));
        }
    }

    /// Retain the coexistence gate once for the route epoch.
    pub(crate) fn retain_coexistence(&mut self, port: &mut impl ClockPort) {
        if self.coexistence.is_none() {
            self.coexistence = Some(GateLease::retain(port, SharedModemClockGate::Coexistence));
        }
    }

    pub(crate) fn release_phy_i2c(&mut self, port: &mut impl ClockPort) {
        if let Some(lease) = self.phy_i2c.take() {
            lease.release(port);
        }
    }

    pub(crate) fn release_coexistence(&mut self, port: &mut impl ClockPort) {
        if let Some(lease) = self.coexistence.take() {
            lease.release(port);
        }
    }

    /// Release the coexistence and then the PHY-I2C gate.
    pub(crate) fn release_all(&mut self, port: &mut impl ClockPort) {
        self.release_coexistence(port);
        self.release_phy_i2c(port);
    }
}

/// Common PHY power held between two protocol routes.
///
/// The first route's power sequence installs the modem clocks, resets and the
/// PHY-I2C gate that every route needs, and captures the cold-power baseline.
/// A route that hands its registered PHY to another route keeps these instead
/// of restoring them, and releases only its protocol-specific leases. The next
/// route inherits them without repeating the power sequence, which would reset
/// the baseband. The baseline is restored once, by the route that finally
/// returns the neutral root.
pub(crate) struct CommonPhyPower {
    phy_i2c: GateLease,
    power: PowerEpoch,
}

#[cfg(test)]
impl CommonPhyPower {
    /// Established common power for ownership tests that must not touch MMIO.
    pub(crate) fn for_test() -> Self {
        Self {
            phy_i2c: GateLease {
                gate: SharedModemClockGate::PhyI2cMaster,
                baseline: true,
            },
            power: PowerEpoch {
                baseline: Some(WifiPowerBaseline::for_validation(false)),
            },
        }
    }
}

/// Why a route cannot hand its common PHY power to another route.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommonPhyPowerError {
    /// The route never completed the common PHY power sequence.
    NotPowered,
}

impl SharedClockLeases {
    /// Release this route's coexistence lease and keep the PHY-I2C lease for
    /// the common PHY power.
    fn into_common(
        self,
        port: &mut impl ClockPort,
        power: PowerEpoch,
    ) -> Result<CommonPhyPower, (Self, PowerEpoch, CommonPhyPowerError)> {
        if !Self::powered(&self, &power) {
            return Err((self, power, CommonPhyPowerError::NotPowered));
        }
        Ok(self.into_powered_common(port, power))
    }

    fn powered(&self, power: &PowerEpoch) -> bool {
        self.phy_i2c.is_some() && power.baseline.is_some()
    }

    /// Caller has checked [`Self::powered`].
    fn into_powered_common(
        mut self,
        port: &mut impl ClockPort,
        power: PowerEpoch,
    ) -> CommonPhyPower {
        self.release_coexistence(port);
        let phy_i2c = self
            .phy_i2c
            .take()
            .expect("a powered route retains its PHY-I2C lease");
        CommonPhyPower { phy_i2c, power }
    }

    fn from_common(common: CommonPhyPower) -> (Self, PowerEpoch) {
        (
            Self {
                phy_i2c: Some(common.phy_i2c),
                coexistence: None,
            },
            common.power,
        )
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
/// The power sequence pulses the Wi-Fi baseband and MAC resets, so it runs
/// once, for the first client; a later client must not repeat it while
/// another protocol runs. The cold-power baseline is captured before that
/// first edge and restored after the last client leaves. Refcounted modem
/// clock dependencies such as coexistence belong to the shared modem clock
/// planner, not to this membership.
#[derive(Default)]
pub(crate) struct CommonRadioPower {
    clients: u8,
    leases: SharedClockLeases,
    power: PowerEpoch,
}

impl CommonRadioPower {
    /// Whether `client` currently holds common power.
    pub(crate) const fn holds(&self, client: RadioClient) -> bool {
        self.clients & client.bit() != 0
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
            port.run_common_power_sequence(&mut self.leases)
                .map_err(CommonRadioPowerError::Power)?;
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
                .restore(port, false)
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
        pll_retained: bool,
    ) -> Result<(), WifiPowerRestoreCheckpoint> {
        let Some(baseline) = self.baseline else {
            return Ok(());
        };
        if pll_retained {
            return Err(WifiPowerRestoreCheckpoint::PlatformPllLease);
        }
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

/// Bluetooth low-power timer configuration and gate retained by one route.
struct LowPowerTimerLease {
    configuration: BluetoothLowPowerTimerConfiguration,
    gate: GateLease,
}

/// Logical MODEM_SYSCON clock groups retained by the Bluetooth route.
///
/// The controller and APB sets overlap. A group is enabled on its first
/// retain and restored to its sampled baseline on its last release.
#[derive(Default)]
pub(crate) struct BluetoothSysconClocks {
    counts: [u8; BLUETOOTH_CLOCK_COUNT],
    baselines: [Option<BluetoothClockBaseline>; BLUETOOTH_CLOCK_COUNT],
}

impl BluetoothSysconClocks {
    fn retain_set(&mut self, port: &mut impl ClockPort, clocks: &[ModemSysconBluetoothClock]) {
        port.prepare_syscon_clock_map();
        let samples = clocks
            .iter()
            .any(|clock| self.counts[clock.index()] == 0)
            .then(|| port.bluetooth_clock_baselines());
        for &clock in clocks {
            let index = clock.index();
            if self.counts[index] == 0 {
                let baseline =
                    samples.expect("first group retain sampled the clock registers")[index];
                self.baselines[index] = Some(baseline);
                self.counts[index] = 1;
                if !baseline.all_enabled(clock) {
                    port.enable_bluetooth_clock(clock);
                }
            } else {
                self.counts[index] = self.counts[index]
                    .checked_add(1)
                    .expect("Bluetooth route MODEM_SYSCON reference count cannot overflow");
            }
        }
    }

    fn release_set(&mut self, port: &mut impl ClockPort, clocks: &[ModemSysconBluetoothClock]) {
        for &clock in clocks {
            let index = clock.index();
            assert!(
                self.counts[index] != 0,
                "unbalanced Bluetooth MODEM_SYSCON release"
            );
            self.counts[index] -= 1;
            if self.counts[index] == 0 {
                let baseline = self.baselines[index]
                    .take()
                    .expect("a retained group keeps its first-retain baseline");
                if !baseline.all_enabled(clock) {
                    port.restore_bluetooth_clock(clock, baseline);
                }
            }
        }
    }
}

/// Every clock lease and baseline retained by the Bluetooth route.
#[derive(Default)]
pub(crate) struct BluetoothClocks {
    shared: SharedClockLeases,
    power: PowerEpoch,
    low_power_timer: Option<LowPowerTimerLease>,
    platform_pll: Option<PlatformPllSourceBaseline>,
    syscon: BluetoothSysconClocks,
    syscon_controller_retained: bool,
    syscon_apb_retained: bool,
    /// The common PHY power sequence was inherited from another route.
    common_inherited: bool,
}

impl BluetoothClocks {
    /// Keep the common PHY power for another route and release every lease
    /// Bluetooth retained for itself.
    ///
    /// The platform PLL-source lease configures the same modem source clocks
    /// as the common power sequence and was taken after the cold-power
    /// baseline was captured. It is therefore superseded rather than restored:
    /// the final cold-power restoration returns those fields.
    pub(crate) fn into_common(
        mut self,
        port: &mut impl ClockPort,
    ) -> Result<CommonPhyPower, (Self, CommonPhyPowerError)> {
        if !self.shared.powered(&self.power) {
            return Err((self, CommonPhyPowerError::NotPowered));
        }
        self.release_low_power_timer(port);
        self.release_syscon_apb_clocks(port);
        self.release_syscon_controller_clocks(port);
        self.platform_pll = None;
        let Self { shared, power, .. } = self;
        Ok(shared.into_powered_common(port, power))
    }

    /// Enter the Bluetooth route with the common PHY power already in effect.
    pub(crate) fn from_common(common: CommonPhyPower) -> Self {
        let (shared, power) = SharedClockLeases::from_common(common);
        Self {
            shared,
            power,
            common_inherited: true,
            ..Self::default()
        }
    }

    /// Whether the common PHY power sequence was inherited from another route.
    pub(crate) const fn common_inherited(&self) -> bool {
        self.common_inherited
    }

    pub(crate) fn shared_mut(&mut self) -> &mut SharedClockLeases {
        &mut self.shared
    }

    pub(crate) fn prepare_power_epoch(&mut self, port: &impl ClockPort) {
        self.power.prepare(port);
    }
    pub(crate) fn retain_coexistence(&mut self, port: &mut impl ClockPort) {
        self.shared.retain_coexistence(port);
    }

    pub(crate) fn release_coexistence(&mut self, port: &mut impl ClockPort) {
        self.shared.release_coexistence(port);
    }

    /// Select the main crystal for the Bluetooth low-power timer and retain
    /// its gate, restoring the captured configuration on release.
    pub(crate) fn retain_main_xtal_low_power_timer(&mut self, port: &mut impl ClockPort) {
        if self.low_power_timer.is_none() {
            let configuration = port.low_power_timer_configuration();
            port.configure_low_power_timer(
                ModemLowPowerClockSource::Crystal,
                BLUETOOTH_MAIN_XTAL_LOW_POWER_DIVIDER,
            );
            let gate = GateLease::retain(port, SharedModemClockGate::LowPowerTimer);
            self.low_power_timer = Some(LowPowerTimerLease {
                configuration,
                gate,
            });
        }
    }

    pub(crate) fn release_low_power_timer(&mut self, port: &mut impl ClockPort) {
        if let Some(LowPowerTimerLease {
            configuration,
            gate,
        }) = self.low_power_timer.take()
        {
            port.restore_low_power_timer(configuration);
            gate.release(port);
        }
    }

    pub(crate) fn retain_platform_pll_source(&mut self, port: &mut impl ClockPort) {
        if self.platform_pll.is_none() {
            let baseline = port.pll_source_baseline();
            port.configure_modem_source_clocks();
            self.platform_pll = Some(baseline);
        }
    }

    pub(crate) fn release_platform_pll_source(&mut self, port: &mut impl ClockPort) {
        if let Some(baseline) = self.platform_pll.take() {
            port.restore_pll_source(baseline);
        }
    }

    pub(crate) fn retain_syscon_controller_clocks(&mut self, port: &mut impl ClockPort) {
        if !self.syscon_controller_retained {
            self.syscon.retain_set(port, &BLUETOOTH_CONTROLLER_CLOCKS);
            self.syscon_controller_retained = true;
        }
    }

    pub(crate) fn retain_syscon_apb_clocks(&mut self, port: &mut impl ClockPort) {
        if !self.syscon_apb_retained {
            self.syscon.retain_set(port, &BLUETOOTH_APB_CLOCKS);
            self.syscon_apb_retained = true;
        }
    }

    pub(crate) fn release_syscon_apb_clocks(&mut self, port: &mut impl ClockPort) {
        if self.syscon_apb_retained {
            self.syscon.release_set(port, &BLUETOOTH_APB_CLOCKS);
            self.syscon_apb_retained = false;
        }
    }

    pub(crate) fn release_syscon_controller_clocks(&mut self, port: &mut impl ClockPort) {
        if self.syscon_controller_retained {
            self.syscon.release_set(port, &BLUETOOTH_CONTROLLER_CLOCKS);
            self.syscon_controller_retained = false;
        }
    }

    /// Release every lease in the reviewed Bluetooth teardown order.
    pub(crate) fn release_all(&mut self, port: &mut impl ClockPort) {
        self.shared.release_phy_i2c(port);
        self.release_low_power_timer(port);
        self.shared.release_coexistence(port);
        self.release_syscon_apb_clocks(port);
        self.release_syscon_controller_clocks(port);
        self.release_platform_pll_source(port);
    }

    /// Restore the cold-power baseline after [`Self::release_all`].
    pub(crate) fn restore_power_epoch(
        &mut self,
        port: &mut impl ClockPort,
    ) -> Result<(), WifiPowerRestoreCheckpoint> {
        let pll_retained = self.platform_pll.is_some();
        self.power.restore(port, pll_retained)
    }
}

/// Clock leases and the cold-power baseline retained by the Wi-Fi route.
#[derive(Default)]
pub(crate) struct WifiClocks {
    pub(crate) shared: SharedClockLeases,
    pub(crate) power: PowerEpoch,
}

impl WifiClocks {
    /// Keep the common PHY power for another route and release Wi-Fi's own
    /// coexistence lease.
    pub(crate) fn into_common(
        self,
        port: &mut impl ClockPort,
    ) -> Result<CommonPhyPower, (Self, CommonPhyPowerError)> {
        self.shared
            .into_common(port, self.power)
            .map_err(|(shared, power, error)| (Self { shared, power }, error))
    }

    /// Whether [`Self::into_common`] would accept this route; no MMIO.
    pub(crate) fn common_powered(&self) -> bool {
        self.shared.powered(&self.power)
    }

    /// Enter the Wi-Fi route with the common PHY power already in effect.
    pub(crate) fn from_common(common: CommonPhyPower) -> Self {
        let (shared, power) = SharedClockLeases::from_common(common);
        Self { shared, power }
    }
}

#[cfg(test)]
mod tests;
