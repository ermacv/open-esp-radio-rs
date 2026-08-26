//! Reusable role-neutral cold PHY/Wi-Fi composition for ESP32-S31.
//!
//! This boundary owns the production ordering shared by standalone firmware
//! and HIL: power, finite PHY registration, Wi-Fi RX enable and initial
//! channel selection. Board token construction, persistent calibration
//! storage and diagnostics remain caller policy.

use open_esp_radio_esp32s31_hal::{
    PowerClockControl, PowerUpFailure, Radio, analog_i2c::PhyPmuControl,
    phy_i2c::PhyI2cMasterControl, state::Powered,
};
use open_esp_radio_esp32s31_phy::{
    PhyAsyncDelay, PhyCalibrationCache, PhyCalibrationIdentity, PhyRegisterOutcome, PhyState,
    PhyTargetObserver, PhyTargetPortCounters, PhyTargetPortError, PhyTxTargetPowerProfile,
    TargetPhyRegisterAttempt, TargetPhyRegisterFailure, run_target_phy_register,
    select_phy_channel_with_hal,
};
use open_esp_radio_ieee80211::channel::WifiChannel;

use crate::channel::lower_wifi_channel;

/// Application-selected inputs for one cold radio start.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Esp32s31WifiColdStartConfig {
    pub calibration_identity: PhyCalibrationIdentity,
    pub initial_channel: WifiChannel,
    pub maximum_tx_power_quarter_dbm: i8,
}

impl Esp32s31WifiColdStartConfig {
    pub const fn new(
        calibration_identity: PhyCalibrationIdentity,
        initial_channel: WifiChannel,
    ) -> Self {
        Self {
            calibration_identity,
            initial_channel,
            maximum_tx_power_quarter_dbm: i8::MAX,
        }
    }

    pub const fn with_maximum_tx_power_quarter_dbm(mut self, maximum: i8) -> Self {
        self.maximum_tx_power_quarter_dbm = maximum;
        self
    }
}

/// Observable result of the finite cold start without HIL telemetry policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Esp32s31WifiColdStartReport {
    pub registration: PhyRegisterOutcome,
    pub port_counters: PhyTargetPortCounters,
    pub initial_channel: WifiChannel,
}

/// Complete owner set returned at the cold-MAC boundary.
pub struct Esp32s31WifiColdStart<P> {
    radio: Radio<P, Powered>,
    phy: PhyState,
    tx_power: PhyTxTargetPowerProfile,
    calibration_cache: Option<PhyCalibrationCache>,
    report: Esp32s31WifiColdStartReport,
}

impl<P> Esp32s31WifiColdStart<P> {
    pub const fn report(&self) -> Esp32s31WifiColdStartReport {
        self.report
    }

    pub const fn calibration_cache(&self) -> Option<&PhyCalibrationCache> {
        self.calibration_cache.as_ref()
    }

    pub fn into_parts(
        self,
    ) -> (
        Radio<P, Powered>,
        PhyState,
        PhyTxTargetPowerProfile,
        Option<PhyCalibrationCache>,
        Esp32s31WifiColdStartReport,
    ) {
        (
            self.radio,
            self.phy,
            self.tx_power,
            self.calibration_cache,
            self.report,
        )
    }
}

/// Failure which always returns the unique radio owner at its exact phase.
#[allow(
    clippy::large_enum_variant,
    reason = "the allocation-free failure retains the exact opaque radio/PHY owner"
)]
pub enum Esp32s31WifiColdStartFailure<P> {
    Power(PowerUpFailure<P>),
    Registration(TargetPhyRegisterFailure<P>),
    InitialChannel {
        radio: Radio<P, Powered>,
        phy: PhyState,
        calibration_cache: Option<PhyCalibrationCache>,
        report: Esp32s31WifiColdStartReport,
        error: PhyTargetPortError,
    },
}

/// Run the common production cold-start sequence without diagnostics or board
/// allocation policy.
///
/// This operation owns the sole radio value across hardware awaits and must run
/// to completion. Cancelling it after polling is fail-closed: no ready owner or
/// PHY-registration proof is returned, and the integration must reset the
/// peripheral or chip before establishing another `Radio` owner.
#[must_use = "Wi-Fi cold start must be driven to a terminal result"]
pub async fn start_esp32s31_wifi<P, D, O>(
    radio: Radio<P>,
    config: Esp32s31WifiColdStartConfig,
    calibration_cache: Option<PhyCalibrationCache>,
    observer: O,
) -> Result<Esp32s31WifiColdStart<P>, Esp32s31WifiColdStartFailure<P>>
where
    P: PowerClockControl + PhyPmuControl + PhyI2cMasterControl,
    D: PhyAsyncDelay,
    O: PhyTargetObserver + Clone,
{
    let powered = radio
        .power_up()
        .map_err(Esp32s31WifiColdStartFailure::Power)?;
    let attempt = TargetPhyRegisterAttempt::with_production_config_and_calibration(
        powered,
        config.calibration_identity,
        calibration_cache,
    );
    let target_registration = run_target_phy_register::<_, D, _>(attempt, observer.clone())
        .await
        .map_err(Esp32s31WifiColdStartFailure::Registration)?;
    // This legacy Wi-Fi owner still stores `PhyState` directly. The downgrade
    // is performed inside the PHY crate, so safe callers cannot separate a
    // target proof from its radio and splice hardware epochs. The following
    // ownership iteration will retain an opaque coupled owner instead.
    let (mut powered, mut phy, calibration_cache, registration, port_counters) =
        target_registration.into_ordinary_parts();
    let report = Esp32s31WifiColdStartReport {
        registration,
        port_counters,
        initial_channel: config.initial_channel,
    };

    powered.enable_wifi_rx();
    let mut channel_hal = powered.channel_hal();
    let mut channel_observer = observer;
    let initial_channel = lower_wifi_channel(config.initial_channel);
    if let Err(error) = select_phy_channel_with_hal::<D, _, _>(
        &mut phy,
        initial_channel.channel_or_frequency,
        initial_channel.cbw,
        &mut channel_hal,
        &mut channel_observer,
    )
    .await
    {
        drop(channel_hal);
        return Err(Esp32s31WifiColdStartFailure::InitialChannel {
            radio: powered,
            phy,
            calibration_cache,
            report,
            error,
        });
    }
    drop(channel_hal);

    let tx_power = phy
        .tx_target_power_profile()
        .with_maximum_quarter_dbm(config.maximum_tx_power_quarter_dbm);
    Ok(Esp32s31WifiColdStart {
        radio: powered,
        phy,
        tx_power,
        calibration_cache,
        report,
    })
}
