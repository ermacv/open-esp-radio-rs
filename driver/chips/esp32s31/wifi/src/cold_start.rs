//! Reusable role-neutral cold PHY/Wi-Fi composition for ESP32-S31.
//!
//! This boundary owns the production ordering shared by standalone firmware
//! and HIL: power, finite PHY registration, Wi-Fi RX enable and initial
//! channel selection. Board token construction, persistent calibration
//! storage and diagnostics remain caller policy.

use open_esp_radio_esp32s31_hal::{
    PowerClockControl, PowerUpFailure, Radio, analog_i2c::PhyPmuControl,
    phy_i2c::PhyI2cMasterControl, phy_prelude::PhyPreludePlatformControl,
    phy_temperature::PhyTemperatureSystemControl,
    power_detector_platform::PhyPowerDetectorPlatformControl, state::Powered,
    wifi_bb::PhyWifiBbControl,
};
use open_esp_radio_esp32s31_phy::{
    PhyAsyncDelay, PhyCalibrationCache, PhyCalibrationIdentity, PhyRegisterOutcome,
    PhyRegisterRunError, PhyRegisterTransition, PhyState, PhyTargetObserver, PhyTargetPortCounters,
    PhyTargetPortError, PhyTxTargetPowerProfile, TargetPhyRegisterPort, run_phy_register,
    select_phy_channel,
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
pub enum Esp32s31WifiColdStartFailure<P> {
    Power(PowerUpFailure<P>),
    Registration {
        radio: Radio<P, Powered>,
        transition: PhyRegisterTransition,
        port_counters: PhyTargetPortCounters,
        error: PhyRegisterRunError<PhyTargetPortError>,
    },
    MissingPhyOwner {
        radio: Radio<P, Powered>,
        transition: PhyRegisterTransition,
    },
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
pub async fn start_esp32s31_wifi<P, D, O>(
    radio: Radio<P>,
    config: Esp32s31WifiColdStartConfig,
    calibration_cache: Option<PhyCalibrationCache>,
    observer: O,
) -> Result<Esp32s31WifiColdStart<P>, Esp32s31WifiColdStartFailure<P>>
where
    P: PowerClockControl
        + PhyPreludePlatformControl
        + PhyPmuControl
        + PhyWifiBbControl
        + PhyPowerDetectorPlatformControl
        + PhyTemperatureSystemControl
        + PhyI2cMasterControl,
    D: PhyAsyncDelay,
    O: PhyTargetObserver + Clone,
{
    let mut powered = radio
        .power_up()
        .map_err(Esp32s31WifiColdStartFailure::Power)?;
    let mut transition = PhyRegisterTransition::with_production_config_and_calibration(
        config.calibration_identity,
        calibration_cache,
    );
    let mut port = TargetPhyRegisterPort::<_, D, _>::new(&mut powered, observer.clone());
    let registration = match run_phy_register(&mut transition, &mut port).await {
        Ok(outcome) => outcome,
        Err(error) => {
            let port_counters = port.counters();
            drop(port);
            return Err(Esp32s31WifiColdStartFailure::Registration {
                radio: powered,
                transition,
                port_counters,
                error,
            });
        }
    };
    let port_counters = port.counters();
    drop(port);
    let calibration_cache = transition.take_calibration_cache();
    let mut phy = match transition.into_state() {
        Ok(state) => state,
        Err(transition) => {
            return Err(Esp32s31WifiColdStartFailure::MissingPhyOwner {
                radio: powered,
                transition,
            });
        }
    };
    let report = Esp32s31WifiColdStartReport {
        registration,
        port_counters,
        initial_channel: config.initial_channel,
    };

    powered.enable_wifi_rx();
    let (platform, registers) = powered.parts_mut();
    let mut channel_observer = observer;
    let initial_channel = lower_wifi_channel(config.initial_channel);
    if let Err(error) = select_phy_channel::<D, _, _>(
        &mut phy,
        initial_channel.channel_or_frequency,
        initial_channel.cbw,
        platform,
        registers,
        &mut channel_observer,
    )
    .await
    {
        return Err(Esp32s31WifiColdStartFailure::InitialChannel {
            radio: powered,
            phy,
            calibration_cache,
            report,
            error,
        });
    }

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
