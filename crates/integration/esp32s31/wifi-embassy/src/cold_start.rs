//! Reusable cold PHY composition for ESP32-S31 station applications.
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
    PhyAsyncDelay, PhyCalibrationIdentity, PhyRegisterOutcome, PhyRegisterRunError,
    PhyRegisterTransition, PhyTargetObserver, PhyTargetPortCounters, PhyTargetPortError,
    PhyTxTargetPowerProfile, TargetPhyRegisterPort,
    phy_cold::{PhyCalibrationRecord, PhyColdState},
    run_phy_register, select_phy_channel,
};

/// Application-selected inputs for one cold radio start.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Esp32s31ColdStartConfig {
    pub calibration_identity: PhyCalibrationIdentity,
    pub initial_channel_or_frequency: u16,
    pub channel_width: u8,
    pub maximum_tx_power_quarter_dbm: i8,
}

impl Esp32s31ColdStartConfig {
    pub const fn new(
        calibration_identity: PhyCalibrationIdentity,
        initial_channel_or_frequency: u16,
    ) -> Self {
        Self {
            calibration_identity,
            initial_channel_or_frequency,
            channel_width: 0,
            maximum_tx_power_quarter_dbm: i8::MAX,
        }
    }

    pub const fn with_channel_width(mut self, channel_width: u8) -> Self {
        self.channel_width = channel_width;
        self
    }

    pub const fn with_maximum_tx_power_quarter_dbm(mut self, maximum: i8) -> Self {
        self.maximum_tx_power_quarter_dbm = maximum;
        self
    }
}

/// Observable result of the finite cold start without HIL telemetry policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Esp32s31ColdStartReport {
    pub registration: PhyRegisterOutcome,
    pub port_counters: PhyTargetPortCounters,
}

/// Complete owner set returned at the cold-MAC boundary.
pub struct Esp32s31ColdStart<P> {
    radio: Radio<P, Powered>,
    phy: PhyColdState,
    tx_power: PhyTxTargetPowerProfile,
    calibration_record: Option<PhyCalibrationRecord>,
    report: Esp32s31ColdStartReport,
}

impl<P> Esp32s31ColdStart<P> {
    pub const fn report(&self) -> Esp32s31ColdStartReport {
        self.report
    }

    pub const fn calibration_record(&self) -> Option<&PhyCalibrationRecord> {
        self.calibration_record.as_ref()
    }

    pub fn into_parts(
        self,
    ) -> (
        Radio<P, Powered>,
        PhyColdState,
        PhyTxTargetPowerProfile,
        Option<PhyCalibrationRecord>,
        Esp32s31ColdStartReport,
    ) {
        (
            self.radio,
            self.phy,
            self.tx_power,
            self.calibration_record,
            self.report,
        )
    }
}

/// Failure which always returns the unique radio owner at its exact phase.
pub enum Esp32s31ColdStartFailure<P> {
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
        phy: PhyColdState,
        calibration_record: Option<PhyCalibrationRecord>,
        report: Esp32s31ColdStartReport,
        error: PhyTargetPortError,
    },
}

/// Run the common production cold-start sequence without diagnostics or board
/// allocation policy.
pub async fn start_esp32s31_station_radio<P, D, O>(
    radio: Radio<P>,
    config: Esp32s31ColdStartConfig,
    calibration_record: Option<PhyCalibrationRecord>,
    observer: O,
) -> Result<Esp32s31ColdStart<P>, Esp32s31ColdStartFailure<P>>
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
    let mut powered = radio.power_up().map_err(Esp32s31ColdStartFailure::Power)?;
    let mut transition = PhyRegisterTransition::with_default_profile_and_calibration(
        config.calibration_identity,
        calibration_record,
    );
    let mut port = TargetPhyRegisterPort::<_, D, _>::new(&mut powered, observer.clone());
    let registration = match run_phy_register(&mut transition, &mut port).await {
        Ok(outcome) => outcome,
        Err(error) => {
            let port_counters = port.counters();
            drop(port);
            return Err(Esp32s31ColdStartFailure::Registration {
                radio: powered,
                transition,
                port_counters,
                error,
            });
        }
    };
    let port_counters = port.counters();
    drop(port);
    let calibration_record = transition.take_calibration_record();
    let mut phy = match transition.into_state() {
        Ok(state) => state,
        Err(transition) => {
            return Err(Esp32s31ColdStartFailure::MissingPhyOwner {
                radio: powered,
                transition,
            });
        }
    };
    let report = Esp32s31ColdStartReport {
        registration,
        port_counters,
    };

    powered.enable_wifi_rx();
    let (platform, registers) = powered.parts_mut();
    let mut channel_observer = observer;
    if let Err(error) = select_phy_channel::<D, _, _>(
        &mut phy,
        config.initial_channel_or_frequency,
        config.channel_width,
        platform,
        registers,
        &mut channel_observer,
    )
    .await
    {
        return Err(Esp32s31ColdStartFailure::InitialChannel {
            radio: powered,
            phy,
            calibration_record,
            report,
            error,
        });
    }

    let tx_power = phy
        .tx_target_power_profile()
        .with_maximum_quarter_dbm(config.maximum_tx_power_quarter_dbm);
    Ok(Esp32s31ColdStart {
        radio: powered,
        phy,
        tx_power,
        calibration_record,
        report,
    })
}
