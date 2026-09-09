//! Reusable role-neutral cold PHY/Wi-Fi composition for ESP32-S31.
//!
//! This boundary owns the production ordering shared by standalone firmware
//! and HIL: power, finite PHY registration, Wi-Fi client acquisition and
//! initial tracking, Wi-Fi RX enable and initial
//! channel selection. Board token construction, persistent calibration
//! storage and diagnostics remain caller policy.

use crate::channel::lower_wifi_channel;

use oer_esp32s31_hal::owner::{PowerUpFailure, Radio};

use oer_esp32s31_phy::{
    PhyAsyncDelay, PhyCalibrationCache, PhyCalibrationIdentity, PhyRegisterOutcome,
    PhyTargetObserver, PhyTargetPortCounters, PhyTargetPortError, PhyTxTargetPowerProfile,
    RegisteredPhyClientAcquireFailure, RegisteredPhyRadio, TargetPhyParamTrackingFailure,
    TargetPhyRegisterAttempt, TargetPhyRegisterFailure, run_target_phy_param_tracking,
    run_target_phy_register,
};

use oer_esp32s31_phy::{
    state::client::{PhyModemClient, PhyPllTrackClock},
    tracking::parameters::PhyParamTrackingOutcome,
};
use oer_ieee80211::channel::WifiChannel;

/// Application-selected inputs for one cold radio start.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WifiColdStartConfig {
    pub calibration_identity: PhyCalibrationIdentity,
    pub initial_channel: WifiChannel,
    pub maximum_tx_power_quarter_dbm: i8,
}

impl WifiColdStartConfig {
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
pub struct WifiColdStartReport {
    pub registration: PhyRegisterOutcome,
    pub port_counters: PhyTargetPortCounters,
    pub initial_channel: WifiChannel,
    /// None when the acquisition timestamp did not require tracking.
    pub initial_tracking: Option<PhyParamTrackingOutcome>,
}

/// Complete owner set returned at the cold-MAC boundary.
pub struct WifiColdStart<P> {
    radio: RegisteredPhyRadio<P>,
    tx_power: PhyTxTargetPowerProfile,
    calibration_cache: Option<PhyCalibrationCache>,
    report: WifiColdStartReport,
}

impl<P> WifiColdStart<P> {
    pub const fn report(&self) -> WifiColdStartReport {
        self.report
    }

    pub const fn calibration_cache(&self) -> Option<&PhyCalibrationCache> {
        self.calibration_cache.as_ref()
    }

    pub fn into_parts(
        self,
    ) -> (
        RegisteredPhyRadio<P>,
        PhyTxTargetPowerProfile,
        Option<PhyCalibrationCache>,
        WifiColdStartReport,
    ) {
        (
            self.radio,
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
pub enum WifiColdStartFailure<P> {
    Power(PowerUpFailure<P>),
    Registration(TargetPhyRegisterFailure<P>),
    ClientAcquire(RegisteredPhyClientAcquireFailure<P>),
    InitialTracking(TargetPhyParamTrackingFailure<P>),
    InitialChannel {
        radio: RegisteredPhyRadio<P>,
        calibration_cache: Option<PhyCalibrationCache>,
        report: WifiColdStartReport,
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
    config: WifiColdStartConfig,
    calibration_cache: Option<PhyCalibrationCache>,
    observer: O,
    clock: &mut impl PhyPllTrackClock,
) -> Result<WifiColdStart<P>, WifiColdStartFailure<P>>
where
    D: PhyAsyncDelay,
    O: PhyTargetObserver + Clone,
{
    let powered = radio.power_up().map_err(WifiColdStartFailure::Power)?;
    let attempt = TargetPhyRegisterAttempt::with_production_config_and_calibration(
        powered,
        config.calibration_identity,
        calibration_cache,
    );
    let target_registration = run_target_phy_register::<_, D, _>(attempt, observer.clone())
        .await
        .map_err(WifiColdStartFailure::Registration)?;
    let (powered, calibration_cache, registration, port_counters) =
        target_registration.into_registered_parts();
    let acquired = powered
        .acquire_client(PhyModemClient::Wifi, clock)
        .map_err(WifiColdStartFailure::ClientAcquire)?;
    let (mut powered, initial_tracking) = match acquired.into_owner() {
        Ok(powered) => (powered, None),
        Err(pending) => {
            let success = run_target_phy_param_tracking::<_, D, _>(
                pending.begin_tracking(),
                observer.clone(),
            )
            .await
            .map_err(WifiColdStartFailure::InitialTracking)?;
            let (powered, outcome) = success.into_parts();
            (powered, Some(outcome))
        }
    };
    let report = WifiColdStartReport {
        registration,
        port_counters,
        initial_tracking,
        initial_channel: config.initial_channel,
    };

    let mut channel_observer = observer;
    let initial_channel = lower_wifi_channel(config.initial_channel);
    if let Err(error) = powered
        .initialize_wifi_channel::<D, _>(
            initial_channel.channel_or_frequency,
            initial_channel.cbw,
            &mut channel_observer,
        )
        .await
    {
        return Err(WifiColdStartFailure::InitialChannel {
            radio: powered,
            calibration_cache,
            report,
            error,
        });
    }

    let tx_power = powered
        .state()
        .tx_target_power_profile()
        .with_maximum_quarter_dbm(config.maximum_tx_power_quarter_dbm);
    Ok(WifiColdStart {
        radio: powered,
        tx_power,
        calibration_cache,
        report,
    })
}
