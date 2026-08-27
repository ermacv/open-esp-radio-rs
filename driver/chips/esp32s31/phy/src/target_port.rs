//! Complete ESP32-S31 target port for the recovered PHY transitions.
//!
//! This module owns the target-side composition of the individual PHY state
//! machines. Applications inject only an asynchronous delay and an optional
//! observer; they must not reconstruct the recovered hardware contract.

use core::marker::PhantomData;

use open_esp_radio_esp32s31_hal::{
    Ieee802154Clocked, PhyInitializationAccess, Radio, SharedPhyAccess, SharedPhyContext,
    state::Powered,
};

use crate::{
    HARDWARE_EDGE_LIMIT, PhyCalibrationTrackingPort, PhyParamTrackingPort,
    PhyParamTrackingRunError, PhyRegisterPort, PhyRegisterRunError,
    phy_bb::{PhyBbExternalBinding, PhyBbInitCompletion},
    phy_bluetooth::{
        PhyBluetoothTxGainInitCompletion, PhyBluetoothTxGainInitExternalBinding,
        PhyBluetoothTxPowerCompletion, PhyBluetoothTxPowerExternalBinding,
    },
    phy_cal_tracking::{
        PhyCalibrationChannelTransition, PhyCalibrationDcodeTransition,
        PhyCalibrationForceTxRxTransition, PhyCalibrationPbusClearTransition,
        PhyCalibrationRxGainTransition, PhyCalibrationTrackingAction,
        PhyCalibrationTrackingCompletion, PhyCalibrationTrackingExternalBinding,
        PhyCalibrationTxDcPwdetTransition,
    },
    phy_channel::{
        PhyChipChannelAction, PhyChipChannelCompletion, PhyChipChannelExternalBinding,
        PhyChipChannelFailure, PhyChipChannelOutcome, PhyChipChannelRequest,
        PhyChipChannelTransition, PhyWifiTxGainImage, PhyWifiTxGainRequest,
    },
    phy_client::PhyPendingTracking,
    phy_cold::{
        PhyColdExternalBinding, PhyColdI2cAction, PhyColdI2cError, PhyColdI2cObservation,
        PhyColdObservationRequest, PhyColdPbusObservation,
    },
    phy_dc_iq::{PhyDcIqCompletion, PhyDcIqExternalBinding},
    phy_dcode::{PhyDcodeCompletion, PhyDcodeExternalBinding},
    phy_i2c::{PhyRfInitPrefixAction, PhyRfInitPrefixCompletion},
    phy_i2c_tracking::PhyWifiI2cTrackingCompletion,
    phy_param_tracking::{
        PhyParamTrackingAction, PhyParamTrackingCalibrationTransition, PhyParamTrackingCompletion,
        PhyParamTrackingOutcome, PhyParamTrackingRfpllTransition,
        PhyParamTrackingTemperatureTransition, PhyParamTrackingTxPowerTransition,
        PhyParamTrackingWifiI2cTransition,
    },
    phy_pbus::{PhyForceTxRxExternalBinding, PhyPbusHardwareObservation},
    phy_pwdet::{PhyPwdetCompletion, PhyPwdetExternalBinding, PhyPwdetPbusObservation},
    phy_register::{
        PhyCalibrationIdentity, PhyRegisterCompletion, PhyRegisterExternalBinding,
        PhyRegisterOutcome, PhyRegisterTransition,
    },
    phy_rfpll::{
        RfpllCapCorrectionCompletion, RfpllCapCorrectionExternalBinding,
        RfpllCapTrackingCompletion, RfpllCapTrackingExternalBinding, RfpllFrequencyAction,
        RfpllFrequencyCompletion, RfpllFrequencyExternalBinding,
    },
    phy_rx_dco::{
        PhyRxDcMinimumCompletion, PhyRxDcMinimumExternalBinding, PhyRxDcoCompletion,
        PhyRxDcoExternalBinding,
    },
    phy_rx_gain::{
        PhyRxGainInitCompletion, PhyRxGainInitExternalBinding, PhyRxGainPublishCompletion,
        PhyRxGainPublishExternalBinding,
    },
    phy_rx_gain_cal::{
        PhyRxDcCalibrationCompletion, PhyRxDcCalibrationExternalBinding, PhyRxGainDcCompletion,
        PhyRxGainDcExternalBinding,
    },
    phy_rx_saturation::{PhyRxSaturationCompletion, PhyRxSaturationExternalBinding},
    phy_rxiq::{
        PhyRxIqCoverCompletion, PhyRxIqCoverExternalBinding, PhyRxIqDataCompletion,
        PhyRxIqDataExternalBinding, PhyRxIqEstimatorCompletion, PhyRxIqEstimatorExternalBinding,
        PhyRxIqGainCompletion, PhyRxIqGainExternalBinding, PhyRxIqInitCompletion,
        PhyRxIqInitExternalBinding, PhyRxIqRfCalibrationCompletion,
        PhyRxIqRfCalibrationExternalBinding,
    },
    phy_state::{PhyCalibrationCache, PhyState},
    phy_temperature::{PhyTemperatureCompletion, PhyTemperatureExternalBinding},
    phy_tx_cal::{
        PhyPowerAttenuationCompletion, PhyPowerAttenuationExternalBinding, PhyToneSarCompletion,
        PhyToneSarExternalBinding, PhyTxCalibrationEnvironmentCompletion,
        PhyTxCalibrationEnvironmentExternalBinding, PhyTxCapCompletion, PhyTxCapExternalBinding,
        PhyTxCapSearchCompletion, PhyTxCapSearchExternalBinding,
    },
    phy_tx_power::{
        PhyPowerControlPointCompletion, PhyPowerControlPointExternalBinding, PhyTxPowerCompletion,
        PhyTxPowerExternalBinding,
    },
    phy_txdc::{PhyTxDcAction, PhyTxDcCompletion, PhyTxDcExternalBinding},
    phy_txdc_pwdet::{
        PhyTxDcPwdetCompletion, PhyTxDcPwdetExternalBinding, PhyTxDcPwdetSearchCompletion,
        PhyTxDcPwdetSearchExternalBinding,
    },
    phy_txiq::{
        PhyTxIqCalibrationCompletion, PhyTxIqCalibrationExternalBinding, PhyTxIqCoverCompletion,
        PhyTxIqCoverExternalBinding, PhyTxIqInitCompletion, PhyTxIqInitExternalBinding,
        PhyTxIqLinearPowerCompletion, PhyTxIqLinearPowerExternalBinding, PhyTxIqLoopbackCompletion,
        PhyTxIqLoopbackExternalBinding, PhyTxIqMisPowerCompletion, PhyTxIqMisPowerExternalBinding,
    },
    registered_radio::{
        RegisteredIeee802154Client, RegisteredIeee802154Clocked,
        RegisteredIeee802154PendingTracking, RegisteredIeee802154TrackPoisoned,
        RegisteredPhyPendingTracking, RegisteredPhyRadio, RegisteredPhyTrackPoisoned,
        TargetRegisteredPhyEpoch,
    },
    run_phy_calibration_tracking, run_phy_param_tracking, run_phy_register,
    target_executor::{
        PhyAsyncDelay, PhyTargetPortError, complete_bluetooth_i2c, complete_bluetooth_pbus,
        complete_channel_i2c, complete_dcode_i2c, complete_final_i2c, complete_masked_i2c,
        complete_rfpll_i2c, complete_rx_dc_calibration_pbus, complete_rx_dco_pbus,
        complete_rx_gain_dc_pbus, complete_rx_gain_publish_pbus, complete_rx_saturation_pbus,
        complete_rxiq_adjusted_tx_i2c, complete_rxiq_gain_i2c, complete_rxiq_gain_pbus,
        complete_rxiq_init_i2c, complete_rxiq_init_pbus, complete_temperature_i2c,
        complete_tx_calibration_environment_pbus, complete_tx_dc_pwdet_pbus,
        complete_tx_dc_pwdet_search_pbus, complete_tx_power_i2c, complete_txiq_init_i2c,
        complete_txiq_pbus,
    },
};

const CHANNEL_READY_SAMPLE_LIMIT: u32 = 10_000;
const RF_OPERATION_LIMIT: u32 = 100_000;
const MAC_CHANNEL_SETTLE_US: u64 = 20;
const MAC_CHANNEL_IDLE_SETTLE_US: u64 = 5;

/// A semantic checkpoint exposed to target diagnostics without exporting raw
/// MMIO or application logging into the driver.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRfBoundary {
    BeforeRfInit,
    AfterPbusClear,
    BeforeI2cMasterRegisterInit,
    BeforePowerDetectorRegisterInit,
    BeforeFrontEndRegisterInit,
    BeforeTemperatureSensorReadInit,
    BeforeTxPowerControlBackgroundInit,
    BeforeChannelFrequencyInit,
}

/// Optional, synchronous target observations which cannot affect PHY state.
///
/// Production integrations normally use [`NoopPhyTargetObserver`]. HIL code
/// can implement this trait to compare a completed Rust result with ROM or to
/// capture diagnostic MMIO without placing either dependency in this crate.
pub trait PhyTargetObserver {
    fn operation_started(&mut self) {}
    fn operation_completed(&mut self) {}
    fn channel_frequency_ready_timed_out(&mut self, _samples: u32) {}
    fn channel_tx_gain(&mut self, _request: PhyWifiTxGainRequest, _image: PhyWifiTxGainImage) {}
    fn channel_completed(&mut self, _outcome: PhyChipChannelOutcome, _operations: u32) {}
    fn channel_failed(&mut self, _failure: PhyChipChannelFailure, _operations: u32) {}
    fn mac_channel_restarted(&mut self, _channel_or_frequency: u16, _cbw: u8, _link: u8) {}
    fn tx_dc_entry(&mut self) {}
    fn tx_dc_comparator(&mut self, _gain_index: u8, _iteration: u8, _comparator_high: [bool; 2]) {}
    fn power_detector_sample(
        &mut self,
        _measurement_index: u8,
        _sample_index: u8,
        _sample_value: u16,
    ) {
    }
    fn rf_boundary(&mut self, _boundary: PhyRfBoundary) {}
}

/// Observer used by production integrations which need no diagnostic side
/// channel.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoopPhyTargetObserver;

impl PhyTargetObserver for NoopPhyTargetObserver {}

/// Operation counts produced by one PHY registration run.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PhyTargetPortCounters {
    pub mmio: u16,
    pub delays: u16,
    pub reset_samples: u16,
    pub rf_operations: u32,
    pub baseband_operations: u32,
}

/// Opaque fresh target registration attempt.
///
/// The powered radio and inner model transition are deliberately inseparable.
/// Callers can construct only a fresh production attempt and pass it once to
/// [`run_target_phy_register`]. There is no conversion from a caller-driven
/// [`PhyRegisterTransition`], because such a transition may already contain
/// synthetic completions.
#[must_use = "a target PHY attempt uniquely owns the powered radio"]
pub struct TargetPhyRegisterAttempt<P> {
    radio: Radio<P, Powered>,
    transition: PhyRegisterTransition,
}

impl<P> TargetPhyRegisterAttempt<P> {
    /// Start one fresh production registration without calibration persistence.
    pub const fn with_production_config(radio: Radio<P, Powered>) -> Self {
        Self {
            radio,
            transition: PhyRegisterTransition::with_production_config(),
        }
    }

    /// Start one fresh production registration with caller-owned persistence.
    pub const fn with_production_config_and_calibration(
        radio: Radio<P, Powered>,
        identity: PhyCalibrationIdentity,
        cache: Option<PhyCalibrationCache>,
    ) -> Self {
        Self {
            radio,
            transition: PhyRegisterTransition::with_production_config_and_calibration(
                identity, cache,
            ),
        }
    }

    /// Inspect the currently retained ordinary model state.
    pub fn state(&self) -> Option<&PhyState> {
        self.transition.state()
    }
}

/// Result of one complete source-owned target registration path.
///
/// This owner records execution through the concrete ESP32-S31 target port. It
/// does not claim RF qualification or operational link readiness.
pub struct TargetPhyRegisterSuccess<P> {
    registered_epoch: TargetRegisteredPhyEpoch<P>,
    calibration_cache: Option<PhyCalibrationCache>,
    outcome: PhyRegisterOutcome,
    counters: PhyTargetPortCounters,
}

/// Caller-owned persistence inputs for one dedicated IEEE 802.15.4 common-PHY
/// registration.
///
/// The retained cache is validation input until complete replay is recovered.
/// The target transition still performs full calibration and publishes only a
/// fresh cache after terminal success.
pub struct TargetIeee802154PhyRegisterConfig {
    calibration_identity: PhyCalibrationIdentity,
    calibration_cache: Option<PhyCalibrationCache>,
}

impl TargetIeee802154PhyRegisterConfig {
    /// Request a fresh production registration and retain its resulting cache.
    pub const fn new(calibration_identity: PhyCalibrationIdentity) -> Self {
        Self {
            calibration_identity,
            calibration_cache: None,
        }
    }

    /// Supply a caller-owned cache as validation input to the same transition.
    pub fn with_calibration_cache(mut self, calibration_cache: PhyCalibrationCache) -> Self {
        self.calibration_cache = Some(calibration_cache);
        self
    }
}

/// Result of one terminally successful dedicated IEEE 802.15.4 common-PHY
/// target registration.
///
/// The registered state stays coupled to the exact clocked IEEE owner. This
/// records concrete target execution, not RF qualification, BTBB timing,
/// interrupt routing, DMA readiness, or an operational MAC.
#[must_use = "successful IEEE 802.15.4 PHY registration retains the unique radio owner"]
pub struct TargetIeee802154PhyRegisterSuccess<P> {
    registered_owner: RegisteredIeee802154Clocked<P>,
    calibration_cache: Option<PhyCalibrationCache>,
    outcome: PhyRegisterOutcome,
    counters: PhyTargetPortCounters,
}

impl<P> TargetIeee802154PhyRegisterSuccess<P> {
    /// Borrow the indivisible registered IEEE owner.
    pub const fn registered_owner(&self) -> &RegisteredIeee802154Clocked<P> {
        &self.registered_owner
    }

    /// Borrow the fresh cache published by this terminal run, if requested.
    pub const fn calibration_cache(&self) -> Option<&PhyCalibrationCache> {
        self.calibration_cache.as_ref()
    }

    /// Inspect the terminal model outcome accepted by the target runner.
    pub const fn outcome(&self) -> PhyRegisterOutcome {
        self.outcome
    }

    /// Inspect the concrete target operations performed by this run.
    pub const fn counters(&self) -> PhyTargetPortCounters {
        self.counters
    }

    /// Move all successful outputs without separating PHY proof from hardware.
    pub fn into_registered_parts(
        self,
    ) -> (
        RegisteredIeee802154Clocked<P>,
        Option<PhyCalibrationCache>,
        PhyRegisterOutcome,
        PhyTargetPortCounters,
    ) {
        (
            self.registered_owner,
            self.calibration_cache,
            self.outcome,
            self.counters,
        )
    }
}

/// Exact reason a dedicated IEEE 802.15.4 common-PHY run did not publish its
/// registered owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TargetIeee802154PhyRegisterError {
    /// A recovered external edge or the shared registration machine failed.
    Run(PhyRegisterRunError<PhyTargetPortError>),
    /// The executor returned success without the terminal unique model owner.
    MissingCompletedModelOwner,
}

/// Failed dedicated IEEE 802.15.4 registration retaining every unique owner.
///
/// There is deliberately no recovery or retry method. A target error can
/// follow a partially applied hardware edge, including an error whose model
/// cleanup is terminal, so neither the exact clocked radio nor transition is
/// released through safe code. Reset the chip before constructing another
/// radio owner.
#[must_use = "failed IEEE 802.15.4 PHY registration still owns mutated hardware"]
pub struct TargetIeee802154PhyRegisterFailure<P> {
    _owner: Ieee802154Clocked<P>,
    transition: PhyRegisterTransition,
    counters: PhyTargetPortCounters,
    error: TargetIeee802154PhyRegisterError,
}

impl<P> TargetIeee802154PhyRegisterFailure<P> {
    /// Inspect the typed failure without releasing any owner.
    pub const fn error(&self) -> TargetIeee802154PhyRegisterError {
        self.error
    }

    /// Inspect target operation counts completed before failure.
    pub const fn counters(&self) -> PhyTargetPortCounters {
        self.counters
    }

    /// Inspect the retained model state for fail-stop diagnostics.
    pub fn state(&self) -> Option<&PhyState> {
        self.transition.state()
    }
}

impl<P> TargetPhyRegisterSuccess<P> {
    /// Inspect the target-registered state without releasing its radio epoch.
    pub const fn state(&self) -> &PhyState {
        self.registered_epoch.state()
    }

    pub const fn calibration_cache(&self) -> Option<&PhyCalibrationCache> {
        self.calibration_cache.as_ref()
    }

    pub const fn outcome(&self) -> PhyRegisterOutcome {
        self.outcome
    }

    pub const fn counters(&self) -> PhyTargetPortCounters {
        self.counters
    }

    /// Preserve target-registration authority for a proof-aware role owner.
    ///
    /// The returned [`RegisteredPhyRadio`] keeps the powered radio and proof
    /// inseparable. The calibration cache is persistable output from the same
    /// terminally successful registration run.
    pub fn into_registered_parts(
        self,
    ) -> (
        RegisteredPhyRadio<P>,
        Option<PhyCalibrationCache>,
        PhyRegisterOutcome,
        PhyTargetPortCounters,
    ) {
        (
            self.registered_epoch.into_registered_radio(),
            self.calibration_cache,
            self.outcome,
            self.counters,
        )
    }

    /// Explicitly discard target-registration authority for a legacy consumer.
    ///
    /// The radio and [`crate::RegisteredPhyState`] are never exposed as independent
    /// values. This prevents safe callers from pairing proof issued for one
    /// hardware epoch with a different powered radio. Role owners which retain
    /// the proof must use [`Self::into_registered_parts`] instead.
    pub fn into_ordinary_parts(
        self,
    ) -> (
        Radio<P, Powered>,
        PhyState,
        Option<PhyCalibrationCache>,
        PhyRegisterOutcome,
        PhyTargetPortCounters,
    ) {
        let (radio, state) = self.registered_epoch.into_ordinary_parts();
        (
            radio,
            state,
            self.calibration_cache,
            self.outcome,
            self.counters,
        )
    }
}

/// Failure from the concrete target registration runner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TargetPhyRegisterError {
    Run(PhyRegisterRunError<PhyTargetPortError>),
    MissingCompletedModelOwner,
}

/// Ordinary owners released after a terminal target-registration failure.
///
/// This payload contains no registration proof. It is available only through
/// [`TargetPhyRegisterFailure::into_terminal_parts`], after the transition has
/// demonstrated its terminal failed phase and completed cleanup.
pub type TargetPhyRegisterTerminalParts<P> = (
    Radio<P, Powered>,
    PhyState,
    Option<PhyCalibrationCache>,
    PhyTargetPortCounters,
    TargetPhyRegisterError,
);

/// Exact opaque radio/transition owner returned after a target failure.
///
/// A port error may follow a partially completed hardware edge, so this owner
/// deliberately has no public path back to [`TargetPhyRegisterAttempt`]. Safe
/// code cannot reissue an ambiguous operation or reset per-run safety counters.
/// Only a transition which completed its failure cleanup can release ordinary
/// owners through [`Self::into_terminal_parts`].
pub struct TargetPhyRegisterFailure<P> {
    attempt: TargetPhyRegisterAttempt<P>,
    counters: PhyTargetPortCounters,
    error: TargetPhyRegisterError,
}

impl<P> TargetPhyRegisterFailure<P> {
    pub const fn error(&self) -> TargetPhyRegisterError {
        self.error
    }

    pub const fn counters(&self) -> PhyTargetPortCounters {
        self.counters
    }

    pub fn state(&self) -> Option<&PhyState> {
        self.attempt.state()
    }

    /// Recover ordinary owners only after the model reached terminal failure.
    ///
    /// Port, transition and target-success invariant errors may still describe
    /// a partially active hardware epoch. Those paths return this exact opaque
    /// failure unchanged, so safe code cannot separate or rerun its powered
    /// radio and transition. A genuine terminal radio failure has completed
    /// cleanup and may release the ordinary PHY state and caller retry cache.
    #[allow(
        clippy::result_large_err,
        reason = "nonterminal failure must retain the exact allocation-free radio/PHY owner"
    )]
    pub fn into_terminal_parts(self) -> Result<TargetPhyRegisterTerminalParts<P>, Self> {
        let Self {
            attempt,
            counters,
            error,
        } = self;
        let TargetPhyRegisterAttempt { radio, transition } = attempt;
        match transition.into_failed_parts() {
            Ok((state, calibration_cache)) => {
                Ok((radio, state, calibration_cache, counters, error))
            }
            Err(transition) => Err(Self {
                attempt: TargetPhyRegisterAttempt { radio, transition },
                counters,
                error,
            }),
        }
    }
}

/// Private witness accepted by the sole [`crate::RegisteredPhyState`] constructor.
///
/// The field is private to this module, so no sibling module can mint it even
/// though the proof constructor can name the type through crate visibility.
pub(crate) struct TargetRegistrationWitness {
    _private: (),
}

impl TargetRegistrationWitness {
    const fn new() -> Self {
        Self { _private: () }
    }
}

/// Complete target-side implementation of [`PhyRegisterPort`].
pub struct TargetPhyRegisterPort<'a, P, R, D, O = NoopPhyTargetObserver> {
    platform: &'a mut P,
    registers: &'a mut R,
    observer: O,
    counters: PhyTargetPortCounters,
    delay: PhantomData<D>,
}

/// Complete target port for the bounded periodic-calibration parent.
///
/// It reuses the same leaf completer as cold registration, so periodic DCODE,
/// RX gain, channel, TXDC and gain publication cannot drift into a second
/// hardware implementation.
pub struct TargetPhyCalibrationTrackingPort<'a, P, R, D, O = NoopPhyTargetObserver> {
    platform: &'a mut P,
    registers: &'a mut R,
    observer: &'a mut O,
    delay: PhantomData<D>,
}

/// Complete target port for one affine outer periodic-tracking request.
pub struct TargetPhyParamTrackingPort<'a, P, R, D, O = NoopPhyTargetObserver> {
    platform: &'a mut P,
    registers: &'a mut R,
    observer: O,
    delay: PhantomData<D>,
}

/// Terminal successful outer periodic-tracking request.
///
/// The refreshed semantic state remains inseparable from the exact powered
/// radio epoch on which the target bindings executed.
pub struct TargetPhyParamTrackingSuccess<P> {
    registered_radio: RegisteredPhyRadio<P>,
    outcome: PhyParamTrackingOutcome,
}

impl<P> TargetPhyParamTrackingSuccess<P> {
    pub const fn registered_radio(&self) -> &RegisteredPhyRadio<P> {
        &self.registered_radio
    }

    pub const fn outcome(&self) -> PhyParamTrackingOutcome {
        self.outcome
    }

    pub fn into_parts(self) -> (RegisteredPhyRadio<P>, PhyParamTrackingOutcome) {
        (self.registered_radio, self.outcome)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TargetPhyParamTrackingError {
    Run(PhyParamTrackingRunError<PhyTargetPortError>),
    MissingCompletedOwner,
}

/// Fail-stop result retaining the poisoned PHY-client epoch.
#[must_use = "failed periodic PHY tracking poisons its unique client owner"]
pub struct TargetPhyParamTrackingFailure<P> {
    poisoned: RegisteredPhyTrackPoisoned<P>,
    error: TargetPhyParamTrackingError,
}

/// Terminal successful periodic tracking on the dedicated IEEE route.
pub struct TargetIeee802154PhyParamTrackingSuccess<P> {
    owner: RegisteredIeee802154Client<P>,
    outcome: PhyParamTrackingOutcome,
}

impl<P> TargetIeee802154PhyParamTrackingSuccess<P> {
    pub const fn owner(&self) -> &RegisteredIeee802154Client<P> {
        &self.owner
    }

    pub const fn outcome(&self) -> PhyParamTrackingOutcome {
        self.outcome
    }

    pub fn into_parts(self) -> (RegisteredIeee802154Client<P>, PhyParamTrackingOutcome) {
        (self.owner, self.outcome)
    }
}

/// Fail-stop periodic tracking result on the dedicated IEEE route.
#[must_use = "failed IEEE PHY tracking poisons its registered role epoch"]
pub struct TargetIeee802154PhyParamTrackingFailure<P> {
    poisoned: RegisteredIeee802154TrackPoisoned<P>,
    error: TargetPhyParamTrackingError,
}

impl<P> TargetIeee802154PhyParamTrackingFailure<P> {
    pub const fn error(&self) -> TargetPhyParamTrackingError {
        self.error
    }

    pub const fn poisoned(&self) -> &RegisteredIeee802154TrackPoisoned<P> {
        &self.poisoned
    }
}

impl<P> TargetPhyParamTrackingFailure<P> {
    pub const fn error(&self) -> TargetPhyParamTrackingError {
        self.error
    }

    pub const fn poisoned(&self) -> &RegisteredPhyTrackPoisoned<P> {
        &self.poisoned
    }

    /// Inspect the last committed semantic state for fail-stop diagnostics.
    ///
    /// No extractor is provided: target hardware may have advanced beyond
    /// this state, so the registered epoch cannot be retried or resumed.
    pub const fn state(&self) -> &PhyState {
        self.poisoned.state()
    }
}

struct TargetCompleter<D>(PhantomData<D>);

impl<D: PhyAsyncDelay> TargetCompleter<D> {
    async fn complete_rfpll<P>(
        binding: RfpllFrequencyExternalBinding,
        _platform: &mut P,
        registers: &mut impl SharedPhyAccess,
    ) -> Result<RfpllFrequencyCompletion, PhyTargetPortError> {
        match binding {
            RfpllFrequencyExternalBinding::Mmio(binding) => match binding.action() {
                RfpllFrequencyAction::ReadChannelReady { samples }
                    if samples >= CHANNEL_READY_SAMPLE_LIMIT =>
                {
                    Ok(RfpllFrequencyCompletion::ChannelReadyTimedOut)
                }
                _ => Ok(binding.execute_target(registers)),
            },
            RfpllFrequencyExternalBinding::I2c(binding) => {
                complete_rfpll_i2c::<D>(binding, registers).await
            }
            RfpllFrequencyExternalBinding::Timer(binding) => {
                D::after_micros(u64::from(binding.micros())).await;
                Ok(binding.into_completion())
            }
        }
    }

    async fn complete_rfpll_cap_correction<P>(
        binding: RfpllCapCorrectionExternalBinding,
        _platform: &mut P,
        registers: &mut impl SharedPhyAccess,
    ) -> Result<RfpllCapCorrectionCompletion, PhyTargetPortError> {
        match binding {
            RfpllCapCorrectionExternalBinding::Memory(binding) => Ok(
                RfpllCapCorrectionCompletion::Memory(binding.execute_target(registers)),
            ),
            RfpllCapCorrectionExternalBinding::I2c(mut binding) => {
                for _ in 0..HARDWARE_EDGE_LIMIT {
                    match binding.action() {
                        PhyColdI2cAction::StartRead { .. }
                        | PhyColdI2cAction::StartWrite { .. } => {
                            match binding.start_target(registers) {
                                Ok(()) => {}
                                Err(PhyColdI2cError::BusyAtStart) => D::after_micros(1).await,
                                Err(_) => return Err(PhyTargetPortError::UnexpectedBinding),
                            }
                        }
                        PhyColdI2cAction::AwaitReadCompletionEdge { .. }
                        | PhyColdI2cAction::AwaitWriteCompletionEdge { .. } => {
                            D::after_micros(1).await;
                            match binding
                                .observe_target_edge(registers)
                                .map_err(|_| PhyTargetPortError::UnexpectedBinding)?
                            {
                                PhyColdI2cObservation::EdgeConsumed
                                | PhyColdI2cObservation::StillPending => {}
                            }
                        }
                        PhyColdI2cAction::Complete(_) => {
                            return binding
                                .into_completion()
                                .map_err(|_| PhyTargetPortError::UnexpectedBinding);
                        }
                    }
                }
                Err(PhyTargetPortError::HardwareEdgeTimedOut)
            }
        }
    }

    async fn complete_rfpll_cap<P>(
        binding: RfpllCapTrackingExternalBinding,
        platform: &mut P,
        registers: &mut impl SharedPhyAccess,
    ) -> Result<RfpllCapTrackingCompletion, PhyTargetPortError> {
        match binding {
            RfpllCapTrackingExternalBinding::Mmio(binding) => Ok(binding.execute_target(registers)),
            RfpllCapTrackingExternalBinding::Correction(binding) => {
                Ok(RfpllCapTrackingCompletion::Correction(
                    Self::complete_rfpll_cap_correction(binding, platform, registers).await?,
                ))
            }
        }
    }

    async fn complete_tx_calibration_environment(
        binding: PhyTxCalibrationEnvironmentExternalBinding,
        registers: &mut impl SharedPhyContext,
    ) -> Result<PhyTxCalibrationEnvironmentCompletion, PhyTargetPortError> {
        match binding {
            PhyTxCalibrationEnvironmentExternalBinding::Mmio(binding) => {
                Ok(binding.execute_target(registers))
            }
            PhyTxCalibrationEnvironmentExternalBinding::Pbus(binding) => {
                complete_tx_calibration_environment_pbus::<D>(binding, registers).await
            }
            PhyTxCalibrationEnvironmentExternalBinding::Timer(binding) => {
                D::after_micros(u64::from(binding.micros())).await;
                Ok(binding.into_completion())
            }
        }
    }

    async fn complete_tone_sar(
        binding: PhyToneSarExternalBinding,
        registers: &mut impl SharedPhyAccess,
    ) -> Result<PhyToneSarCompletion, PhyTargetPortError> {
        match binding {
            PhyToneSarExternalBinding::Mmio(binding) => Ok(binding.execute_target(registers)),
            PhyToneSarExternalBinding::Timer(binding) => {
                D::after_micros(u64::from(binding.micros())).await;
                Ok(binding.into_completion())
            }
        }
    }

    async fn complete_temperature<P>(
        binding: PhyTemperatureExternalBinding,
        _platform: &mut P,
        registers: &mut impl SharedPhyAccess,
    ) -> Result<PhyTemperatureCompletion, PhyTargetPortError> {
        match binding {
            PhyTemperatureExternalBinding::I2c(binding) => {
                complete_temperature_i2c::<D>(binding, registers).await
            }
            PhyTemperatureExternalBinding::Sample(binding) => Ok(binding.execute_target(registers)),
        }
    }

    async fn complete_power_control_point(
        binding: PhyPowerControlPointExternalBinding,
        registers: &mut impl SharedPhyAccess,
    ) -> Result<PhyPowerControlPointCompletion, PhyTargetPortError> {
        match binding {
            PhyPowerControlPointExternalBinding::Mmio(binding) => {
                Ok(binding.execute_target(registers))
            }
            PhyPowerControlPointExternalBinding::ToneSar(binding) => {
                let completion = Self::complete_tone_sar(binding, registers).await?;
                Ok(PhyPowerControlPointCompletion::ToneSar(completion))
            }
        }
    }

    async fn complete_tx_power<P>(
        binding: PhyTxPowerExternalBinding,
        platform: &mut P,
        registers: &mut impl SharedPhyContext,
    ) -> Result<PhyTxPowerCompletion, PhyTargetPortError> {
        match binding {
            PhyTxPowerExternalBinding::Environment(binding) => {
                Ok(PhyTxPowerCompletion::Environment(
                    Self::complete_tx_calibration_environment(binding, registers).await?,
                ))
            }
            PhyTxPowerExternalBinding::Rfpll(binding) => Ok(PhyTxPowerCompletion::Rfpll(
                Self::complete_rfpll(binding, platform, registers).await?,
            )),
            PhyTxPowerExternalBinding::I2c(binding) => {
                complete_tx_power_i2c::<D>(binding, registers).await
            }
            PhyTxPowerExternalBinding::Mmio(binding) => Ok(binding.execute_target(registers)),
            PhyTxPowerExternalBinding::ToneSar(binding) => {
                let completion = Self::complete_tone_sar(binding, registers).await?;
                Ok(PhyTxPowerCompletion::ToneSar(completion))
            }
            PhyTxPowerExternalBinding::Point(binding) => Ok(PhyTxPowerCompletion::Point(
                Self::complete_power_control_point(binding, registers).await?,
            )),
        }
    }

    async fn complete_bluetooth_tx_power<P>(
        binding: PhyBluetoothTxPowerExternalBinding,
        platform: &mut P,
        registers: &mut impl SharedPhyContext,
    ) -> Result<PhyBluetoothTxPowerCompletion, PhyTargetPortError> {
        match binding {
            PhyBluetoothTxPowerExternalBinding::I2c(binding) => {
                complete_bluetooth_i2c::<D>(binding, registers).await
            }
            PhyBluetoothTxPowerExternalBinding::Prepare(binding) => {
                Ok(PhyBluetoothTxPowerCompletion::Prepare(
                    Self::complete_tx_calibration_environment(binding, registers).await?,
                ))
            }
            PhyBluetoothTxPowerExternalBinding::Cleanup(binding) => {
                Ok(PhyBluetoothTxPowerCompletion::Cleanup(
                    Self::complete_tx_calibration_environment(binding, registers).await?,
                ))
            }
            PhyBluetoothTxPowerExternalBinding::Pbus(binding) => {
                complete_bluetooth_pbus::<D>(binding, registers).await
            }
            PhyBluetoothTxPowerExternalBinding::ReadPbus(binding) => {
                Ok(binding.execute_target(registers))
            }
            PhyBluetoothTxPowerExternalBinding::Calibration(binding) => {
                Ok(PhyBluetoothTxPowerCompletion::Calibration(
                    Self::complete_tx_power(binding, platform, registers).await?,
                ))
            }
        }
    }

    async fn complete_bluetooth_tx_gain<P, O: PhyTargetObserver>(
        binding: PhyBluetoothTxGainInitExternalBinding,
        platform: &mut P,
        registers: &mut impl SharedPhyContext,
        observer: &mut O,
    ) -> Result<PhyBluetoothTxGainInitCompletion, PhyTargetPortError> {
        match binding {
            PhyBluetoothTxGainInitExternalBinding::Rfpll(binding) => {
                Ok(PhyBluetoothTxGainInitCompletion::Rfpll(
                    Self::complete_rfpll(binding, platform, registers).await?,
                ))
            }
            PhyBluetoothTxGainInitExternalBinding::TxCap(binding) => {
                Ok(PhyBluetoothTxGainInitCompletion::TxCap(
                    Self::complete_tx_power(binding, platform, registers).await?,
                ))
            }
            PhyBluetoothTxGainInitExternalBinding::TxDc(binding) => {
                Ok(PhyBluetoothTxGainInitCompletion::TxDc(
                    Self::complete_tx_dc(binding, registers, observer).await?,
                ))
            }
            PhyBluetoothTxGainInitExternalBinding::TxPower(binding) => {
                Ok(PhyBluetoothTxGainInitCompletion::TxPower(
                    Self::complete_bluetooth_tx_power(binding, platform, registers).await?,
                ))
            }
            PhyBluetoothTxGainInitExternalBinding::TxDcPwdet(binding) => {
                Ok(PhyBluetoothTxGainInitCompletion::TxDcPwdet(
                    Self::complete_tx_dc_pwdet(binding, registers).await?,
                ))
            }
            PhyBluetoothTxGainInitExternalBinding::Publish(binding) => {
                Ok(binding.execute_target(registers))
            }
        }
    }

    async fn complete_power_attenuation(
        binding: PhyPowerAttenuationExternalBinding,
        registers: &mut impl SharedPhyContext,
    ) -> Result<PhyPowerAttenuationCompletion, PhyTargetPortError> {
        match binding {
            PhyPowerAttenuationExternalBinding::Mmio(binding) => {
                Ok(binding.execute_target(registers))
            }
            PhyPowerAttenuationExternalBinding::ToneSar(binding) => {
                let completion = Self::complete_tone_sar(binding, registers).await?;
                Ok(PhyPowerAttenuationCompletion::ToneSar(completion))
            }
        }
    }

    async fn complete_tx_cap_search<P>(
        binding: PhyTxCapSearchExternalBinding,
        _platform: &mut P,
        registers: &mut impl SharedPhyAccess,
    ) -> Result<PhyTxCapSearchCompletion, PhyTargetPortError> {
        match binding {
            PhyTxCapSearchExternalBinding::Mmio(binding) => Ok(binding.execute_target(registers)),
            PhyTxCapSearchExternalBinding::I2c(binding) => Ok(PhyTxCapSearchCompletion::I2c(
                complete_masked_i2c::<D>(binding, registers).await?,
            )),
            PhyTxCapSearchExternalBinding::ToneSar(binding) => {
                let completion = Self::complete_tone_sar(binding, registers).await?;
                Ok(PhyTxCapSearchCompletion::ToneSar(completion))
            }
        }
    }

    async fn complete_tx_cap<P>(
        binding: PhyTxCapExternalBinding,
        platform: &mut P,
        registers: &mut impl SharedPhyContext,
    ) -> Result<PhyTxCapCompletion, PhyTargetPortError> {
        match binding {
            PhyTxCapExternalBinding::Environment(binding) => Ok(PhyTxCapCompletion::Environment(
                Self::complete_tx_calibration_environment(binding, registers).await?,
            )),
            PhyTxCapExternalBinding::Rfpll(binding) => Ok(PhyTxCapCompletion::Rfpll(
                Self::complete_rfpll(binding, platform, registers).await?,
            )),
            PhyTxCapExternalBinding::I2c(binding) => Ok(PhyTxCapCompletion::I2c(
                complete_masked_i2c::<D>(binding, registers).await?,
            )),
            PhyTxCapExternalBinding::Attenuation(binding) => Ok(PhyTxCapCompletion::Attenuation(
                Self::complete_power_attenuation(binding, registers).await?,
            )),
            PhyTxCapExternalBinding::Search(binding) => Ok(PhyTxCapCompletion::Search(
                Self::complete_tx_cap_search(binding, platform, registers).await?,
            )),
        }
    }

    async fn complete_tx_dc_pwdet_search(
        binding: PhyTxDcPwdetSearchExternalBinding,
        registers: &mut impl SharedPhyAccess,
    ) -> Result<PhyTxDcPwdetSearchCompletion, PhyTargetPortError> {
        match binding {
            PhyTxDcPwdetSearchExternalBinding::Pbus(binding) => {
                complete_tx_dc_pwdet_search_pbus::<D>(binding, registers).await
            }
            PhyTxDcPwdetSearchExternalBinding::Timer(binding) => {
                D::after_micros(u64::from(binding.micros())).await;
                Ok(binding.into_completion())
            }
            PhyTxDcPwdetSearchExternalBinding::ToneSar(binding) => {
                Ok(PhyTxDcPwdetSearchCompletion::ToneSar(
                    Self::complete_tone_sar(binding, registers).await?,
                ))
            }
        }
    }

    async fn complete_tx_dc_pwdet(
        binding: PhyTxDcPwdetExternalBinding,
        registers: &mut impl SharedPhyContext,
    ) -> Result<PhyTxDcPwdetCompletion, PhyTargetPortError> {
        match binding {
            PhyTxDcPwdetExternalBinding::Mmio(binding) => binding
                .execute_target(registers)
                .map_err(|_| PhyTargetPortError::HardwareInvariant),
            PhyTxDcPwdetExternalBinding::Pbus(binding) => {
                complete_tx_dc_pwdet_pbus::<D>(binding, registers).await
            }
            PhyTxDcPwdetExternalBinding::Timer(binding) => {
                D::after_micros(u64::from(binding.micros())).await;
                Ok(binding.into_completion())
            }
            PhyTxDcPwdetExternalBinding::Search(binding) => Ok(PhyTxDcPwdetCompletion::Search(
                Self::complete_tx_dc_pwdet_search(binding, registers).await?,
            )),
        }
    }

    async fn complete_dcode<P>(
        binding: PhyDcodeExternalBinding,
        platform: &mut P,
        registers: &mut impl SharedPhyAccess,
    ) -> Result<PhyDcodeCompletion, PhyTargetPortError> {
        match binding {
            PhyDcodeExternalBinding::Rfpll(binding) => Ok(PhyDcodeCompletion::Rfpll(
                Self::complete_rfpll(binding, platform, registers).await?,
            )),
            PhyDcodeExternalBinding::Mmio(binding) => Ok(binding.execute_target(registers)),
            PhyDcodeExternalBinding::I2c(binding) => {
                complete_dcode_i2c::<D>(binding, registers).await
            }
        }
    }

    async fn complete_txiq_linear_power(
        binding: PhyTxIqLinearPowerExternalBinding,
        registers: &mut impl SharedPhyAccess,
    ) -> Result<PhyTxIqLinearPowerCompletion, PhyTargetPortError> {
        match binding {
            PhyTxIqLinearPowerExternalBinding::ToneSar(binding) => {
                Ok(PhyTxIqLinearPowerCompletion::ToneSar(
                    Self::complete_tone_sar(binding, registers).await?,
                ))
            }
        }
    }

    async fn complete_txiq_mis_power(
        binding: PhyTxIqMisPowerExternalBinding,
        registers: &mut impl SharedPhyAccess,
    ) -> Result<PhyTxIqMisPowerCompletion, PhyTargetPortError> {
        match binding {
            PhyTxIqMisPowerExternalBinding::Mmio(binding) => Ok(binding.execute_target(registers)),
            PhyTxIqMisPowerExternalBinding::Timer(binding) => {
                D::after_micros(u64::from(binding.micros())).await;
                Ok(binding.into_completion())
            }
            PhyTxIqMisPowerExternalBinding::LinearPower(binding) => {
                Ok(PhyTxIqMisPowerCompletion::LinearPower(
                    Self::complete_txiq_linear_power(binding, registers).await?,
                ))
            }
        }
    }

    async fn complete_txiq_cover(
        binding: PhyTxIqCoverExternalBinding,
        registers: &mut impl SharedPhyAccess,
    ) -> Result<PhyTxIqCoverCompletion, PhyTargetPortError> {
        match binding {
            PhyTxIqCoverExternalBinding::Mmio(binding) => Ok(binding.execute_target(registers)),
            PhyTxIqCoverExternalBinding::MisPower(binding) => Ok(PhyTxIqCoverCompletion::MisPower(
                Self::complete_txiq_mis_power(binding, registers).await?,
            )),
        }
    }

    async fn complete_txiq_loopback<P>(
        binding: PhyTxIqLoopbackExternalBinding,
        _platform: &mut P,
        registers: &mut impl SharedPhyAccess,
    ) -> Result<PhyTxIqLoopbackCompletion, PhyTargetPortError> {
        match binding {
            PhyTxIqLoopbackExternalBinding::I2c(binding) => Ok(PhyTxIqLoopbackCompletion::I2c(
                complete_masked_i2c::<D>(binding, registers).await?,
            )),
            PhyTxIqLoopbackExternalBinding::Mmio(binding) => Ok(binding.execute_target(registers)),
        }
    }

    async fn complete_txiq_calibration<P>(
        binding: PhyTxIqCalibrationExternalBinding,
        platform: &mut P,
        registers: &mut impl SharedPhyContext,
    ) -> Result<PhyTxIqCalibrationCompletion, PhyTargetPortError> {
        match binding {
            PhyTxIqCalibrationExternalBinding::Mmio(binding) => binding
                .execute_target(registers)
                .map_err(|_| PhyTargetPortError::HardwareInvariant),
            PhyTxIqCalibrationExternalBinding::Loopback(binding) => {
                Ok(PhyTxIqCalibrationCompletion::Loopback(
                    Self::complete_txiq_loopback(binding, platform, registers).await?,
                ))
            }
            PhyTxIqCalibrationExternalBinding::Pbus(binding) => {
                complete_txiq_pbus::<D>(binding, registers).await
            }
            PhyTxIqCalibrationExternalBinding::Environment(binding) => {
                Ok(PhyTxIqCalibrationCompletion::Environment(
                    Self::complete_tx_calibration_environment(binding, registers).await?,
                ))
            }
            PhyTxIqCalibrationExternalBinding::PowerAttenuation(binding) => {
                Ok(PhyTxIqCalibrationCompletion::PowerAttenuation(
                    Self::complete_power_attenuation(binding, registers).await?,
                ))
            }
            PhyTxIqCalibrationExternalBinding::Cover(binding) => {
                Ok(PhyTxIqCalibrationCompletion::Cover(
                    Self::complete_txiq_cover(binding, registers).await?,
                ))
            }
        }
    }

    async fn complete_txiq<P>(
        binding: PhyTxIqInitExternalBinding,
        platform: &mut P,
        registers: &mut impl SharedPhyContext,
    ) -> Result<PhyTxIqInitCompletion, PhyTargetPortError> {
        match binding {
            PhyTxIqInitExternalBinding::Rfpll(binding) => Ok(PhyTxIqInitCompletion::Rfpll(
                Self::complete_rfpll(binding, platform, registers).await?,
            )),
            PhyTxIqInitExternalBinding::I2c(binding) => {
                complete_txiq_init_i2c::<D>(binding, registers).await
            }
            PhyTxIqInitExternalBinding::Calibration(binding) => {
                Ok(PhyTxIqInitCompletion::Calibration(
                    Self::complete_txiq_calibration(binding, platform, registers).await?,
                ))
            }
            PhyTxIqInitExternalBinding::Temperature(binding) => {
                Ok(PhyTxIqInitCompletion::Temperature(
                    Self::complete_temperature(binding, platform, registers).await?,
                ))
            }
        }
    }

    async fn complete_dc_iq(
        binding: PhyDcIqExternalBinding,
        registers: &mut impl SharedPhyAccess,
    ) -> Result<PhyDcIqCompletion, PhyTargetPortError> {
        match binding {
            PhyDcIqExternalBinding::Mmio(binding) => Ok(binding.execute_target(registers)),
            PhyDcIqExternalBinding::Timer(binding) => {
                D::after_micros(u64::from(binding.micros())).await;
                Ok(binding.into_completion())
            }
            PhyDcIqExternalBinding::Readiness(binding) => {
                if binding.samples() >= HARDWARE_EDGE_LIMIT {
                    Ok(binding.into_timeout_completion())
                } else {
                    D::after_micros(1).await;
                    Ok(binding.execute_target(registers))
                }
            }
        }
    }

    async fn complete_rx_dco(
        binding: PhyRxDcoExternalBinding,
        registers: &mut impl SharedPhyAccess,
    ) -> Result<PhyRxDcoCompletion, PhyTargetPortError> {
        match binding {
            PhyRxDcoExternalBinding::Mmio(binding) => binding
                .execute_target(registers)
                .map_err(|_| PhyTargetPortError::HardwareInvariant),
            PhyRxDcoExternalBinding::Pbus(binding) => {
                complete_rx_dco_pbus::<D>(binding, registers).await
            }
            PhyRxDcoExternalBinding::Timer(binding) => {
                D::after_micros(u64::from(binding.micros())).await;
                Ok(binding.into_completion())
            }
            PhyRxDcoExternalBinding::DcIq(binding) => Ok(PhyRxDcoCompletion::DcIq(
                Self::complete_dc_iq(binding, registers).await?,
            )),
        }
    }

    async fn complete_rxiq_estimator(
        binding: PhyRxIqEstimatorExternalBinding,
        registers: &mut impl SharedPhyContext,
    ) -> Result<PhyRxIqEstimatorCompletion, PhyTargetPortError> {
        match binding {
            PhyRxIqEstimatorExternalBinding::Mmio(binding) => Ok(binding.execute_target(registers)),
            PhyRxIqEstimatorExternalBinding::Timer(binding) => {
                D::after_micros(u64::from(binding.micros())).await;
                Ok(binding.into_completion())
            }
            PhyRxIqEstimatorExternalBinding::Readiness(binding) => {
                if binding.samples() >= HARDWARE_EDGE_LIMIT {
                    Ok(binding.into_timeout_completion())
                } else {
                    D::after_micros(1).await;
                    Ok(binding.execute_target(registers))
                }
            }
        }
    }

    async fn complete_rxiq_cover(
        binding: PhyRxIqCoverExternalBinding,
        registers: &mut impl SharedPhyContext,
    ) -> Result<PhyRxIqCoverCompletion, PhyTargetPortError> {
        match binding {
            PhyRxIqCoverExternalBinding::Mmio(binding) => Ok(binding.execute_target(registers)),
            PhyRxIqCoverExternalBinding::Estimator(binding) => {
                Ok(PhyRxIqCoverCompletion::Estimator(
                    Self::complete_rxiq_estimator(binding, registers).await?,
                ))
            }
        }
    }

    async fn complete_rxiq_rf_calibration(
        binding: PhyRxIqRfCalibrationExternalBinding,
        registers: &mut impl SharedPhyContext,
    ) -> Result<PhyRxIqRfCalibrationCompletion, PhyTargetPortError> {
        match binding {
            PhyRxIqRfCalibrationExternalBinding::Mmio(binding) => {
                Ok(binding.execute_target(registers))
            }
            PhyRxIqRfCalibrationExternalBinding::Cover(binding) => {
                Ok(PhyRxIqRfCalibrationCompletion::Cover(
                    Self::complete_rxiq_cover(binding, registers).await?,
                ))
            }
        }
    }

    async fn complete_rxiq_data(
        binding: PhyRxIqDataExternalBinding,
        registers: &mut impl SharedPhyContext,
    ) -> Result<PhyRxIqDataCompletion, PhyTargetPortError> {
        match binding {
            PhyRxIqDataExternalBinding::Calibration(binding) => {
                Ok(PhyRxIqDataCompletion::Calibration(
                    Self::complete_rxiq_rf_calibration(binding, registers).await?,
                ))
            }
        }
    }

    async fn complete_rxiq_gain<P>(
        binding: PhyRxIqGainExternalBinding,
        _platform: &mut P,
        registers: &mut impl SharedPhyContext,
    ) -> Result<PhyRxIqGainCompletion, PhyTargetPortError> {
        match binding {
            PhyRxIqGainExternalBinding::Pbus(binding) => {
                complete_rxiq_gain_pbus::<D>(binding, registers).await
            }
            PhyRxIqGainExternalBinding::I2c(binding) => {
                complete_rxiq_gain_i2c::<D>(binding, registers).await
            }
            PhyRxIqGainExternalBinding::AdjustTx(binding) => Ok(PhyRxIqGainCompletion::AdjustTx(
                complete_rxiq_adjusted_tx_i2c::<D>(binding, registers).await?,
            )),
            PhyRxIqGainExternalBinding::Mmio(binding) => Ok(binding.execute_target(registers)),
            PhyRxIqGainExternalBinding::Dco(binding) => Ok(PhyRxIqGainCompletion::Dco(
                Self::complete_rx_dco(binding, registers).await?,
            )),
            PhyRxIqGainExternalBinding::Estimator(binding) => Ok(PhyRxIqGainCompletion::Estimator(
                Self::complete_rxiq_estimator(binding, registers).await?,
            )),
            PhyRxIqGainExternalBinding::Data(binding) => Ok(PhyRxIqGainCompletion::Data(
                Self::complete_rxiq_data(binding, registers).await?,
            )),
        }
    }

    async fn complete_rxiq<P>(
        binding: PhyRxIqInitExternalBinding,
        platform: &mut P,
        registers: &mut impl SharedPhyContext,
    ) -> Result<PhyRxIqInitCompletion, PhyTargetPortError> {
        match binding {
            PhyRxIqInitExternalBinding::Rfpll(binding) => Ok(PhyRxIqInitCompletion::Rfpll(
                Self::complete_rfpll(binding, platform, registers).await?,
            )),
            PhyRxIqInitExternalBinding::I2c(binding) => {
                complete_rxiq_init_i2c::<D>(binding, registers).await
            }
            PhyRxIqInitExternalBinding::Mmio(binding) => Ok(binding.execute_target(registers)),
            PhyRxIqInitExternalBinding::Pbus(binding) => {
                complete_rxiq_init_pbus::<D>(binding, registers).await
            }
            PhyRxIqInitExternalBinding::Loopback(binding) => Ok(PhyRxIqInitCompletion::Loopback(
                Self::complete_txiq_loopback(binding, platform, registers).await?,
            )),
            PhyRxIqInitExternalBinding::Gain(binding) => Ok(PhyRxIqInitCompletion::Gain(
                Self::complete_rxiq_gain(binding, platform, registers).await?,
            )),
            PhyRxIqInitExternalBinding::Timer(binding) => {
                D::after_micros(u64::from(binding.micros())).await;
                Ok(binding.into_completion())
            }
        }
    }

    async fn complete_rx_saturation(
        binding: PhyRxSaturationExternalBinding,
        registers: &mut impl SharedPhyContext,
    ) -> Result<PhyRxSaturationCompletion, PhyTargetPortError> {
        match binding {
            PhyRxSaturationExternalBinding::Mmio(binding) => Ok(binding.execute_target(registers)),
            PhyRxSaturationExternalBinding::Pbus(binding) => {
                complete_rx_saturation_pbus::<D>(binding, registers).await
            }
            PhyRxSaturationExternalBinding::Timer(binding) => {
                D::after_micros(u64::from(binding.micros())).await;
                Ok(binding.into_completion())
            }
            PhyRxSaturationExternalBinding::Sample(binding) => {
                Ok(binding.execute_target(registers))
            }
        }
    }

    async fn complete_rx_dc_minimum(
        binding: PhyRxDcMinimumExternalBinding,
        registers: &mut impl SharedPhyAccess,
    ) -> Result<PhyRxDcMinimumCompletion, PhyTargetPortError> {
        match binding {
            PhyRxDcMinimumExternalBinding::DcIq(binding) => Ok(PhyRxDcMinimumCompletion::DcIq(
                Self::complete_dc_iq(binding, registers).await?,
            )),
        }
    }

    async fn complete_rx_dc_calibration(
        binding: PhyRxDcCalibrationExternalBinding,
        registers: &mut impl SharedPhyContext,
    ) -> Result<PhyRxDcCalibrationCompletion, PhyTargetPortError> {
        match binding {
            PhyRxDcCalibrationExternalBinding::Mmio(binding) => binding
                .execute_target(registers)
                .map_err(|_| PhyTargetPortError::HardwareInvariant),
            PhyRxDcCalibrationExternalBinding::Pbus(binding) => {
                complete_rx_dc_calibration_pbus::<D>(binding, registers).await
            }
            PhyRxDcCalibrationExternalBinding::Timer(binding) => {
                D::after_micros(u64::from(binding.micros())).await;
                Ok(binding.into_completion())
            }
            PhyRxDcCalibrationExternalBinding::Minimum(binding) => {
                Ok(PhyRxDcCalibrationCompletion::Minimum(
                    Self::complete_rx_dc_minimum(binding, registers).await?,
                ))
            }
        }
    }

    async fn complete_rx_gain_dc<P>(
        binding: PhyRxGainDcExternalBinding,
        platform: &mut P,
        registers: &mut impl SharedPhyContext,
    ) -> Result<PhyRxGainDcCompletion, PhyTargetPortError> {
        match binding {
            PhyRxGainDcExternalBinding::Mmio(binding) => Ok(binding.execute_target(registers)),
            PhyRxGainDcExternalBinding::Rfpll(binding) => Ok(PhyRxGainDcCompletion::Rfpll(
                Self::complete_rfpll(binding, platform, registers).await?,
            )),
            PhyRxGainDcExternalBinding::Pbus(binding) => {
                complete_rx_gain_dc_pbus::<D>(binding, registers).await
            }
            PhyRxGainDcExternalBinding::I2c(binding) => Ok(PhyRxGainDcCompletion::I2c(
                complete_masked_i2c::<D>(binding, registers).await?,
            )),
            PhyRxGainDcExternalBinding::Calibration(binding) => {
                Ok(PhyRxGainDcCompletion::Calibration(
                    Self::complete_rx_dc_calibration(binding, registers).await?,
                ))
            }
            PhyRxGainDcExternalBinding::Minimum(binding) => Ok(PhyRxGainDcCompletion::Minimum(
                Self::complete_rx_dc_minimum(binding, registers).await?,
            )),
            PhyRxGainDcExternalBinding::Timer(binding) => {
                D::after_micros(u64::from(binding.micros())).await;
                Ok(binding.into_completion())
            }
        }
    }

    async fn complete_rx_gain_publish(
        binding: PhyRxGainPublishExternalBinding,
        registers: &mut impl SharedPhyContext,
    ) -> Result<PhyRxGainPublishCompletion, PhyTargetPortError> {
        match binding {
            PhyRxGainPublishExternalBinding::Mmio(binding) => Ok(binding.execute_target(registers)),
            PhyRxGainPublishExternalBinding::Pbus(binding) => {
                complete_rx_gain_publish_pbus::<D>(binding, registers).await
            }
            PhyRxGainPublishExternalBinding::Timer(binding) => {
                D::after_micros(u64::from(binding.micros())).await;
                Ok(binding.into_completion())
            }
        }
    }

    async fn complete_rx_gain<P>(
        binding: PhyRxGainInitExternalBinding,
        platform: &mut P,
        registers: &mut impl SharedPhyContext,
    ) -> Result<PhyRxGainInitCompletion, PhyTargetPortError> {
        match binding {
            PhyRxGainInitExternalBinding::Mmio(binding) => binding
                .execute_target(registers)
                .map_err(|_| PhyTargetPortError::HardwareInvariant),
            PhyRxGainInitExternalBinding::Dc(binding) => Ok(PhyRxGainInitCompletion::Dc(
                Self::complete_rx_gain_dc(binding, platform, registers).await?,
            )),
            PhyRxGainInitExternalBinding::Publish(binding) => Ok(PhyRxGainInitCompletion::Publish(
                Self::complete_rx_gain_publish(binding, registers).await?,
            )),
        }
    }

    async fn complete_channel<P, O: PhyTargetObserver>(
        binding: PhyChipChannelExternalBinding,
        platform: &mut P,
        registers: &mut impl SharedPhyAccess,
        observer: &mut O,
    ) -> Result<PhyChipChannelCompletion, PhyTargetPortError> {
        match binding {
            PhyChipChannelExternalBinding::Mmio(binding) => match binding.action() {
                PhyChipChannelAction::AwaitFrequencyReadyEdge { samples, .. }
                    if samples >= CHANNEL_READY_SAMPLE_LIMIT =>
                {
                    observer.channel_frequency_ready_timed_out(samples);
                    Ok(PhyChipChannelCompletion::FrequencyReadyTimedOut)
                }
                _ => Ok(binding.execute_target(platform, registers)),
            },
            PhyChipChannelExternalBinding::Temperature(binding) => {
                Ok(PhyChipChannelCompletion::Temperature(
                    Self::complete_temperature(binding, platform, registers).await?,
                ))
            }
            PhyChipChannelExternalBinding::Timer(binding) => {
                D::after_micros(u64::from(binding.micros())).await;
                Ok(binding.into_completion())
            }
            PhyChipChannelExternalBinding::I2c(binding) => {
                complete_channel_i2c::<D>(binding, registers).await
            }
            PhyChipChannelExternalBinding::TxGain(binding) => {
                let request = binding.request();
                let completion = binding.execute();
                if let PhyChipChannelCompletion::TxGainCalculated { image, .. } = completion {
                    observer.channel_tx_gain(request, image);
                }
                Ok(completion)
            }
        }
    }

    async fn complete_channel_hal<P, O: PhyTargetObserver>(
        binding: PhyChipChannelExternalBinding,
        channel: &mut open_esp_radio_esp32s31_hal::channel::RadioChannelHal<'_, P>,
        observer: &mut O,
    ) -> Result<PhyChipChannelCompletion, PhyTargetPortError> {
        match binding {
            PhyChipChannelExternalBinding::Mmio(binding) => match binding.action() {
                PhyChipChannelAction::AwaitFrequencyReadyEdge { samples, .. }
                    if samples >= CHANNEL_READY_SAMPLE_LIMIT =>
                {
                    observer.channel_frequency_ready_timed_out(samples);
                    Ok(PhyChipChannelCompletion::FrequencyReadyTimedOut)
                }
                _ => Ok(binding.execute_channel_hal(channel)),
            },
            PhyChipChannelExternalBinding::Temperature(binding) => {
                Ok(PhyChipChannelCompletion::Temperature(match binding {
                    PhyTemperatureExternalBinding::I2c(binding) => {
                        complete_temperature_i2c::<D>(binding, channel).await?
                    }
                    PhyTemperatureExternalBinding::Sample(binding) => {
                        binding.execute_target(channel)
                    }
                }))
            }
            PhyChipChannelExternalBinding::Timer(binding) => {
                D::after_micros(u64::from(binding.micros())).await;
                Ok(binding.into_completion())
            }
            PhyChipChannelExternalBinding::I2c(binding) => {
                complete_channel_i2c::<D>(binding, channel).await
            }
            PhyChipChannelExternalBinding::TxGain(binding) => {
                let request = binding.request();
                let completion = binding.execute();
                if let PhyChipChannelCompletion::TxGainCalculated { image, .. } = completion {
                    observer.channel_tx_gain(request, image);
                }
                Ok(completion)
            }
        }
    }

    async fn select_channel_hal<P, O: PhyTargetObserver>(
        state: &mut PhyState,
        channel_or_frequency: u16,
        cbw: u8,
        channel: &mut open_esp_radio_esp32s31_hal::channel::RadioChannelHal<'_, P>,
        observer: &mut O,
    ) -> Result<(), PhyTargetPortError> {
        let mut transition = PhyChipChannelTransition::new(PhyChipChannelRequest {
            channel_or_frequency,
            cbw,
            parameters: state.channel_parameters(),
        });

        for operation in 0..RF_OPERATION_LIMIT {
            match transition.action() {
                PhyChipChannelAction::Complete(outcome) => {
                    state.apply_channel_outcome(outcome);
                    observer.channel_completed(outcome, operation);
                    return Ok(());
                }
                PhyChipChannelAction::Failed(failure) => {
                    observer.channel_failed(failure, operation);
                    return Err(PhyTargetPortError::UnexpectedBinding);
                }
                action => {
                    let binding = PhyChipChannelExternalBinding::lower(action)
                        .map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
                    let completion = Self::complete_channel_hal(binding, channel, observer).await?;
                    transition
                        .advance(completion)
                        .map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
                }
            }
        }

        Err(PhyTargetPortError::RfOperationLimit)
    }

    async fn switch_channel_hal_with_mac_restart<P, O: PhyTargetObserver>(
        state: &mut PhyState,
        channel_or_frequency: u16,
        cbw: u8,
        channel: &mut open_esp_radio_esp32s31_hal::channel::RadioChannelHal<'_, P>,
        observer: &mut O,
    ) -> Result<(), PhyTargetPortError> {
        channel.request_mac_stop();
        D::after_micros(MAC_CHANNEL_SETTLE_US).await;
        for _ in 0..RF_OPERATION_LIMIT {
            if channel.mac_active_state() == 0 {
                D::after_micros(MAC_CHANNEL_IDLE_SETTLE_US).await;
                Self::select_channel_hal(state, channel_or_frequency, cbw, channel, observer)
                    .await?;
                let regdma_link = channel.restart_mac();
                observer.mac_channel_restarted(channel_or_frequency, cbw, regdma_link);
                return Ok(());
            }
            D::after_micros(1).await;
        }
        Err(PhyTargetPortError::HardwareEdgeTimedOut)
    }

    async fn complete_tx_dc<O: PhyTargetObserver>(
        binding: PhyTxDcExternalBinding,
        registers: &mut impl SharedPhyContext,
        observer: &mut O,
    ) -> Result<PhyTxDcCompletion, PhyTargetPortError> {
        match binding {
            PhyTxDcExternalBinding::Mmio(binding) => {
                if binding.action() == PhyTxDcAction::ConfigurePbusDebugMode {
                    observer.tx_dc_entry();
                }
                let completion = binding.execute_target(registers);
                if let PhyTxDcCompletion::ComparatorsRead {
                    gain_index,
                    iteration,
                    comparator_high,
                } = completion
                {
                    observer.tx_dc_comparator(gain_index, iteration, comparator_high);
                }
                Ok(completion)
            }
            PhyTxDcExternalBinding::Ready(binding) => Ok(binding.execute_target(registers)),
            PhyTxDcExternalBinding::Timer(binding) => {
                D::after_micros(u64::from(binding.micros())).await;
                Ok(binding.into_completion())
            }
            PhyTxDcExternalBinding::Pbus(mut binding) => {
                let mut started = false;
                for _ in 0..HARDWARE_EDGE_LIMIT {
                    if binding.start_target(registers).is_ok() {
                        started = true;
                        break;
                    }
                    D::after_micros(1).await;
                }
                if !started {
                    return Err(PhyTargetPortError::HardwareEdgeTimedOut);
                }
                for _ in 0..HARDWARE_EDGE_LIMIT {
                    D::after_micros(1).await;
                    match binding
                        .observe_target_edge(registers)
                        .map_err(|_| PhyTargetPortError::UnexpectedBinding)?
                    {
                        PhyPbusHardwareObservation::EdgeConsumed => {
                            return binding
                                .into_completion()
                                .map_err(|_| PhyTargetPortError::UnexpectedBinding);
                        }
                        PhyPbusHardwareObservation::StillPending => {}
                    }
                }
                Err(PhyTargetPortError::HardwareEdgeTimedOut)
            }
        }
    }

    async fn complete_pwdet<O: PhyTargetObserver>(
        binding: PhyPwdetExternalBinding,
        registers: &mut impl SharedPhyContext,
        observer: &mut O,
    ) -> Result<PhyPwdetCompletion, PhyTargetPortError> {
        match binding {
            PhyPwdetExternalBinding::Mmio(binding) => {
                let completion = binding.execute_target(registers);
                if let PhyPwdetCompletion::SarSampled {
                    measurement_index,
                    sample_index,
                    value,
                    ..
                } = completion
                {
                    observer.power_detector_sample(measurement_index, sample_index, value);
                }
                Ok(completion)
            }
            PhyPwdetExternalBinding::Ready(binding) => Ok(binding.execute_target(registers)),
            PhyPwdetExternalBinding::Timer(binding) => {
                D::after_micros(u64::from(binding.micros())).await;
                Ok(binding.into_completion())
            }
            PhyPwdetExternalBinding::Pbus(mut binding) => {
                let mut started = false;
                for _ in 0..HARDWARE_EDGE_LIMIT {
                    if binding.start_target(registers).is_ok() {
                        started = true;
                        break;
                    }
                    D::after_micros(1).await;
                }
                if !started {
                    return Err(PhyTargetPortError::HardwareEdgeTimedOut);
                }
                for _ in 0..HARDWARE_EDGE_LIMIT {
                    D::after_micros(1).await;
                    match binding
                        .sample_target_once(registers)
                        .map_err(|_| PhyTargetPortError::UnexpectedBinding)?
                    {
                        PhyPwdetPbusObservation::Completed => {
                            return binding
                                .into_completion()
                                .map_err(|_| PhyTargetPortError::UnexpectedBinding);
                        }
                        PhyPwdetPbusObservation::StillPending => {}
                    }
                }
                Err(PhyTargetPortError::HardwareEdgeTimedOut)
            }
        }
    }

    async fn complete_baseband<P, O: PhyTargetObserver>(
        binding: PhyBbExternalBinding,
        platform: &mut P,
        registers: &mut impl PhyInitializationAccess,
        observer: &mut O,
    ) -> Result<PhyBbInitCompletion, PhyTargetPortError> {
        match binding {
            PhyBbExternalBinding::Mmio(binding) => {
                Ok(PhyBbInitCompletion::Mmio(binding.execute_target(registers)))
            }
            PhyBbExternalBinding::TxDc(binding) => Ok(PhyBbInitCompletion::TxDc(
                Self::complete_tx_dc(binding, registers, observer).await?,
            )),
            PhyBbExternalBinding::Pwdet(binding) => Ok(PhyBbInitCompletion::Pwdet(
                Self::complete_pwdet(binding, registers, observer).await?,
            )),
            PhyBbExternalBinding::TxCap(binding) => Ok(PhyBbInitCompletion::TxCap(
                Self::complete_tx_cap(binding, platform, registers).await?,
            )),
            PhyBbExternalBinding::Temperature(binding) => Ok(PhyBbInitCompletion::Temperature(
                Self::complete_temperature(binding, platform, registers).await?,
            )),
            PhyBbExternalBinding::TxPower(binding) => Ok(PhyBbInitCompletion::TxPower(
                Self::complete_tx_power(binding, platform, registers).await?,
            )),
            PhyBbExternalBinding::TxDcPwdet(binding) => Ok(PhyBbInitCompletion::TxDcPwdet(
                Self::complete_tx_dc_pwdet(binding, registers).await?,
            )),
            PhyBbExternalBinding::Dcode(binding) => Ok(PhyBbInitCompletion::Dcode(
                Self::complete_dcode(binding, platform, registers).await?,
            )),
            PhyBbExternalBinding::TxIq(binding) => Ok(PhyBbInitCompletion::TxIq(
                Self::complete_txiq(binding, platform, registers).await?,
            )),
            PhyBbExternalBinding::TxCfr(binding) => Ok(PhyBbInitCompletion::TxCfr(
                binding.execute_target(registers),
            )),
            PhyBbExternalBinding::BluetoothTxGain(binding) => {
                Ok(PhyBbInitCompletion::BluetoothTxGain(
                    Self::complete_bluetooth_tx_gain(binding, platform, registers, observer)
                        .await?,
                ))
            }
            PhyBbExternalBinding::PbusMemory(binding) => Ok(PhyBbInitCompletion::PbusMemory(
                binding
                    .execute_target(registers)
                    .map_err(|_| PhyTargetPortError::UnexpectedBinding)?,
            )),
            PhyBbExternalBinding::RxIq(binding) => Ok(PhyBbInitCompletion::RxIq(
                Self::complete_rxiq(binding, platform, registers).await?,
            )),
            PhyBbExternalBinding::RxSaturation(binding) => Ok(PhyBbInitCompletion::RxSaturation(
                Self::complete_rx_saturation(binding, registers).await?,
            )),
            PhyBbExternalBinding::RxGain(binding) => Ok(PhyBbInitCompletion::RxGain(
                Self::complete_rx_gain(binding, platform, registers).await?,
            )),
            PhyBbExternalBinding::Channel(binding) => Ok(PhyBbInitCompletion::Channel(
                Self::complete_channel(binding, platform, registers, observer).await?,
            )),
        }
    }

    async fn complete_rf<O: PhyTargetObserver>(
        binding: PhyColdExternalBinding,
        registers: &mut impl SharedPhyContext,
        observer: &mut O,
    ) -> Result<PhyRfInitPrefixCompletion, PhyTargetPortError> {
        match binding {
            PhyColdExternalBinding::I2c(mut binding) => {
                for _ in 0..HARDWARE_EDGE_LIMIT {
                    match binding.action() {
                        PhyColdI2cAction::StartRead { .. }
                        | PhyColdI2cAction::StartWrite { .. } => {
                            match binding.start_target(registers) {
                                Ok(()) => {}
                                Err(PhyColdI2cError::BusyAtStart) => D::after_micros(1).await,
                                Err(_) => return Err(PhyTargetPortError::UnexpectedBinding),
                            }
                        }
                        PhyColdI2cAction::AwaitReadCompletionEdge { .. }
                        | PhyColdI2cAction::AwaitWriteCompletionEdge { .. } => {
                            D::after_micros(1).await;
                            match binding
                                .observe_target_edge(registers)
                                .map_err(|_| PhyTargetPortError::UnexpectedBinding)?
                            {
                                PhyColdI2cObservation::EdgeConsumed
                                | PhyColdI2cObservation::StillPending => {}
                            }
                        }
                        PhyColdI2cAction::Complete(_) => {
                            return binding
                                .into_completion()
                                .map_err(|_| PhyTargetPortError::UnexpectedBinding);
                        }
                    }
                }
                Err(PhyTargetPortError::HardwareEdgeTimedOut)
            }
            PhyColdExternalBinding::Mmio(binding) => {
                let boundary = match binding.outer_action() {
                    PhyRfInitPrefixAction::ConfigureFeBbClock => Some(PhyRfBoundary::BeforeRfInit),
                    PhyRfInitPrefixAction::ConfigureI2cClockSelection { .. } => {
                        Some(PhyRfBoundary::AfterPbusClear)
                    }
                    PhyRfInitPrefixAction::ConfigureI2cMasterRegisters => {
                        Some(PhyRfBoundary::BeforeI2cMasterRegisterInit)
                    }
                    PhyRfInitPrefixAction::ConfigurePowerDetectorRegisters => {
                        Some(PhyRfBoundary::BeforePowerDetectorRegisterInit)
                    }
                    PhyRfInitPrefixAction::ConfigureFrontEndRegisters => {
                        Some(PhyRfBoundary::BeforeFrontEndRegisterInit)
                    }
                    PhyRfInitPrefixAction::ConfigureTemperatureSensorRead => {
                        Some(PhyRfBoundary::BeforeTemperatureSensorReadInit)
                    }
                    PhyRfInitPrefixAction::ConfigureTxPowerControlBackground => {
                        Some(PhyRfBoundary::BeforeTxPowerControlBackgroundInit)
                    }
                    PhyRfInitPrefixAction::ChannelFrequency(
                        crate::phy_frequency::PhyChannelFrequencyInitAction::ConfigureFrequencyRegisters {
                            ..
                        },
                    ) => Some(PhyRfBoundary::BeforeChannelFrequencyInit),
                    _ => None,
                };
                if let Some(boundary) = boundary {
                    observer.rf_boundary(boundary);
                }
                binding
                    .execute_target(registers)
                    .map_err(|error| match error {
                        crate::phy_cold::PhyColdLoweringError::HardwareRestoreInvariant => {
                            PhyTargetPortError::HardwareInvariant
                        }
                        _ => PhyTargetPortError::UnexpectedBinding,
                    })
            }
            PhyColdExternalBinding::Observation(binding) => {
                if binding.outer_action() == PhyRfInitPrefixAction::CaptureChannelFrequencyControl {
                    observer.rf_boundary(PhyRfBoundary::BeforeChannelFrequencyInit);
                }
                match binding.request() {
                    PhyColdObservationRequest::ObserveDcIqReadiness {
                        readiness_samples, ..
                    }
                    | PhyColdObservationRequest::ObserveSignalPowerReadiness {
                        readiness_samples,
                        ..
                    } if readiness_samples >= HARDWARE_EDGE_LIMIT => binding
                        .into_timeout_completion()
                        .map_err(|_| PhyTargetPortError::UnexpectedBinding),
                    PhyColdObservationRequest::ObserveDcIqReadiness { .. }
                    | PhyColdObservationRequest::ObserveSignalPowerReadiness { .. } => {
                        D::after_micros(1).await;
                        binding
                            .execute_target(registers)
                            .map_err(|_| PhyTargetPortError::UnexpectedBinding)
                    }
                    _ => binding
                        .execute_target(registers)
                        .map_err(|_| PhyTargetPortError::UnexpectedBinding),
                }
            }
            PhyColdExternalBinding::Pbus(mut binding) => {
                binding
                    .start_target(registers)
                    .map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
                for _ in 0..HARDWARE_EDGE_LIMIT {
                    D::after_micros(1).await;
                    match binding
                        .observe_target_edge(registers)
                        .map_err(|_| PhyTargetPortError::UnexpectedBinding)?
                    {
                        PhyColdPbusObservation::EdgeConsumed => {
                            return binding
                                .into_completion()
                                .map_err(|_| PhyTargetPortError::UnexpectedBinding);
                        }
                        PhyColdPbusObservation::StillPending => {}
                    }
                }
                Err(PhyTargetPortError::HardwareEdgeTimedOut)
            }
            PhyColdExternalBinding::Timer(binding) => {
                D::after_micros(u64::from(binding.micros())).await;
                binding
                    .into_elapsed_completion()
                    .map_err(|_| PhyTargetPortError::UnexpectedBinding)
            }
        }
    }

    async fn run_calibration_pbus<O: PhyTargetObserver>(
        mut child: PhyCalibrationPbusClearTransition,
        registers: &mut impl SharedPhyContext,
        observer: &mut O,
    ) -> Result<PhyCalibrationTrackingCompletion, PhyTargetPortError> {
        for _ in 0..RF_OPERATION_LIMIT {
            child = match child.commit() {
                Ok(completion) => return Ok(completion),
                Err(child) => child,
            };
            let binding = child
                .lower_external()
                .map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
            let completion = Self::complete_rf(binding, registers, observer).await?;
            child
                .advance_external(completion)
                .map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
        }
        Err(PhyTargetPortError::RfOperationLimit)
    }

    async fn run_calibration_dcode<P>(
        mut child: PhyCalibrationDcodeTransition,
        platform: &mut P,
        registers: &mut impl SharedPhyAccess,
    ) -> Result<PhyCalibrationTrackingCompletion, PhyTargetPortError> {
        for _ in 0..RF_OPERATION_LIMIT {
            child = match child.commit() {
                Ok(completion) => return Ok(completion),
                Err(child) => child,
            };
            let binding = child
                .lower_external()
                .map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
            let completion = Self::complete_dcode(binding, platform, registers).await?;
            child
                .advance(completion)
                .map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
        }
        Err(PhyTargetPortError::RfOperationLimit)
    }

    async fn run_calibration_rx_gain<P>(
        mut child: PhyCalibrationRxGainTransition,
        platform: &mut P,
        registers: &mut impl SharedPhyContext,
    ) -> Result<PhyCalibrationTrackingCompletion, PhyTargetPortError> {
        for _ in 0..RF_OPERATION_LIMIT {
            child = match child.commit() {
                Ok(completion) => return Ok(completion),
                Err(child) => child,
            };
            let binding = child
                .lower_external()
                .map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
            let completion = Self::complete_rx_gain(binding, platform, registers).await?;
            child
                .advance(completion)
                .map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
        }
        Err(PhyTargetPortError::RfOperationLimit)
    }

    async fn run_calibration_channel<P, O: PhyTargetObserver>(
        mut child: PhyCalibrationChannelTransition,
        platform: &mut P,
        registers: &mut impl SharedPhyAccess,
        observer: &mut O,
    ) -> Result<PhyCalibrationTrackingCompletion, PhyTargetPortError> {
        for _ in 0..RF_OPERATION_LIMIT {
            child = match child.commit() {
                Ok(completion) => return Ok(completion),
                Err(child) => child,
            };
            let binding = child
                .lower_external()
                .map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
            let completion = Self::complete_channel(binding, platform, registers, observer).await?;
            child
                .advance(completion)
                .map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
        }
        Err(PhyTargetPortError::RfOperationLimit)
    }

    async fn run_calibration_force_txrx(
        mut child: PhyCalibrationForceTxRxTransition,
        registers: &mut impl SharedPhyAccess,
    ) -> Result<PhyCalibrationTrackingCompletion, PhyTargetPortError> {
        for _ in 0..RF_OPERATION_LIMIT {
            child = match child.commit() {
                Ok(completion) => return Ok(completion),
                Err(child) => child,
            };
            let binding = child
                .lower_external()
                .map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
            let completion = match binding {
                PhyForceTxRxExternalBinding::Mmio(binding) => binding.execute_target(registers),
                PhyForceTxRxExternalBinding::Timer(binding) => {
                    D::after_micros(u64::from(binding.micros())).await;
                    binding.into_completion()
                }
            };
            child
                .advance(completion)
                .map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
        }
        Err(PhyTargetPortError::RfOperationLimit)
    }

    async fn run_calibration_tx_dc_pwdet(
        mut child: PhyCalibrationTxDcPwdetTransition,
        registers: &mut impl SharedPhyContext,
    ) -> Result<PhyCalibrationTrackingCompletion, PhyTargetPortError> {
        for _ in 0..RF_OPERATION_LIMIT {
            child = match child.commit() {
                Ok(completion) => return Ok(completion),
                Err(child) => child,
            };
            let binding = child
                .lower_external()
                .map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
            let completion = Self::complete_tx_dc_pwdet(binding, registers).await?;
            child
                .advance(completion)
                .map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
        }
        Err(PhyTargetPortError::RfOperationLimit)
    }

    async fn run_param_rfpll<P>(
        mut child: PhyParamTrackingRfpllTransition<'_>,
        platform: &mut P,
        registers: &mut impl SharedPhyAccess,
    ) -> Result<PhyParamTrackingCompletion, PhyTargetPortError> {
        for _ in 0..RF_OPERATION_LIMIT {
            child = match child.commit() {
                Ok(completion) => return Ok(completion),
                Err(child) => child,
            };
            let binding = child
                .lower_external()
                .map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
            let completion = Self::complete_rfpll_cap(binding, platform, registers).await?;
            child
                .advance(completion)
                .map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
        }
        Err(PhyTargetPortError::RfOperationLimit)
    }

    async fn run_param_tx_power<P>(
        mut child: PhyParamTrackingTxPowerTransition<'_>,
        platform: &mut P,
        registers: &mut impl SharedPhyAccess,
    ) -> Result<PhyParamTrackingCompletion, PhyTargetPortError> {
        for _ in 0..RF_OPERATION_LIMIT {
            child = match child.commit() {
                Ok(completion) => return Ok(completion),
                Err(child) => child,
            };
            let binding = child
                .lower_external()
                .map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
            let completion = binding.execute_target(platform, registers);
            child
                .advance(completion)
                .map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
        }
        Err(PhyTargetPortError::RfOperationLimit)
    }

    async fn run_param_wifi_i2c(
        mut child: PhyParamTrackingWifiI2cTransition<'_>,
        registers: &mut impl SharedPhyAccess,
    ) -> Result<PhyParamTrackingCompletion, PhyTargetPortError> {
        for _ in 0..RF_OPERATION_LIMIT {
            child = match child.commit() {
                Ok(completion) => return Ok(completion),
                Err(child) => child,
            };
            let binding = child
                .lower_external()
                .map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
            let completion = complete_masked_i2c::<D>(binding, registers).await?;
            child
                .advance(PhyWifiI2cTrackingCompletion::MaskedWrite(completion))
                .map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
        }
        Err(PhyTargetPortError::RfOperationLimit)
    }

    async fn run_param_temperature<P>(
        mut child: PhyParamTrackingTemperatureTransition<'_>,
        platform: &mut P,
        registers: &mut impl SharedPhyAccess,
    ) -> Result<PhyParamTrackingCompletion, PhyTargetPortError> {
        for _ in 0..RF_OPERATION_LIMIT {
            child = match child.commit() {
                Ok(completion) => return Ok(completion),
                Err(child) => child,
            };
            let binding = child
                .lower_external()
                .map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
            let completion = Self::complete_temperature(binding, platform, registers).await?;
            child
                .advance(completion)
                .map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
        }
        Err(PhyTargetPortError::RfOperationLimit)
    }
}

impl<'a, P, R, D, O> TargetPhyRegisterPort<'a, P, R, D, O> {
    /// Bind the complete target port to disjoint platform and PHY borrows.
    pub fn new(platform: &'a mut P, registers: &'a mut R, observer: O) -> Self {
        Self {
            platform,
            registers,
            observer,
            counters: PhyTargetPortCounters::default(),
            delay: PhantomData,
        }
    }

    /// Snapshot operation counts without releasing the radio borrow.
    pub const fn counters(&self) -> PhyTargetPortCounters {
        self.counters
    }
}

impl<'a, P, R, D, O> TargetPhyCalibrationTrackingPort<'a, P, R, D, O> {
    pub fn new(platform: &'a mut P, registers: &'a mut R, observer: &'a mut O) -> Self {
        Self {
            platform,
            registers,
            observer,
            delay: PhantomData,
        }
    }
}

impl<'a, P, R, D, O> TargetPhyParamTrackingPort<'a, P, R, D, O> {
    pub fn new(platform: &'a mut P, registers: &'a mut R, observer: O) -> Self {
        Self {
            platform,
            registers,
            observer,
            delay: PhantomData,
        }
    }
}

impl<P, R: PhyInitializationAccess, D: PhyAsyncDelay, O: PhyTargetObserver>
    PhyCalibrationTrackingPort for TargetPhyCalibrationTrackingPort<'_, P, R, D, O>
{
    type Error = PhyTargetPortError;

    async fn complete<'port, 'state>(
        &'port mut self,
        transition: &'port mut PhyParamTrackingCalibrationTransition<'state>,
    ) -> Result<PhyCalibrationTrackingCompletion, Self::Error> {
        self.observer.operation_started();
        let completion = match transition.action() {
            PhyCalibrationTrackingAction::ClearPbus => {
                TargetCompleter::<D>::run_calibration_pbus(
                    transition
                        .begin_pbus_clear()
                        .map_err(|_| PhyTargetPortError::UnexpectedBinding)?,
                    self.registers,
                    &mut *self.observer,
                )
                .await?
            }
            PhyCalibrationTrackingAction::CalibrateDcode => {
                TargetCompleter::<D>::run_calibration_dcode(
                    transition
                        .begin_dcode()
                        .map_err(|_| PhyTargetPortError::UnexpectedBinding)?,
                    self.platform,
                    self.registers,
                )
                .await?
            }
            PhyCalibrationTrackingAction::RecalibrateRxGain => {
                TargetCompleter::<D>::run_calibration_rx_gain(
                    transition
                        .begin_rx_gain_recalibration()
                        .map_err(|_| PhyTargetPortError::UnexpectedBinding)?,
                    self.platform,
                    self.registers,
                )
                .await?
            }
            PhyCalibrationTrackingAction::RestoreChipChannel { .. } => {
                TargetCompleter::<D>::run_calibration_channel(
                    transition
                        .begin_channel_restore()
                        .map_err(|_| PhyTargetPortError::UnexpectedBinding)?,
                    self.platform,
                    self.registers,
                    &mut *self.observer,
                )
                .await?
            }
            PhyCalibrationTrackingAction::ForceTxRxOff { .. } => {
                TargetCompleter::<D>::run_calibration_force_txrx(
                    transition
                        .begin_force_txrx()
                        .map_err(|_| PhyTargetPortError::UnexpectedBinding)?,
                    self.registers,
                )
                .await?
            }
            PhyCalibrationTrackingAction::CalibrateTxDcPwdet { .. } => {
                TargetCompleter::<D>::run_calibration_tx_dc_pwdet(
                    transition
                        .begin_tx_dc_pwdet()
                        .map_err(|_| PhyTargetPortError::UnexpectedBinding)?,
                    self.registers,
                )
                .await?
            }
            PhyCalibrationTrackingAction::PublishWifiTxGain { .. }
            | PhyCalibrationTrackingAction::PublishBluetoothIeee802154TxGain => transition
                .begin_tx_gain_publication()
                .map_err(|_| PhyTargetPortError::UnexpectedBinding)?
                .execute_target(self.registers),
            PhyCalibrationTrackingAction::SetHardwareFrequencyControl { .. }
            | PhyCalibrationTrackingAction::ConfigureBasebandChannel { .. }
            | PhyCalibrationTrackingAction::EnableMacBaseband
            | PhyCalibrationTrackingAction::RestoreTxGainCompensation => {
                match transition
                    .lower_external()
                    .map_err(|_| PhyTargetPortError::UnexpectedBinding)?
                {
                    PhyCalibrationTrackingExternalBinding::Register(binding) => {
                        binding.execute_target(self.registers)
                    }
                    PhyCalibrationTrackingExternalBinding::MacBaseband(binding) => {
                        binding.execute_target(self.registers)
                    }
                }
            }
            PhyCalibrationTrackingAction::Complete(_) | PhyCalibrationTrackingAction::Failed(_) => {
                return Err(PhyTargetPortError::UnexpectedBinding);
            }
        };
        self.observer.operation_completed();
        Ok(completion)
    }
}

impl<P, R: PhyInitializationAccess, D: PhyAsyncDelay, O: PhyTargetObserver> PhyParamTrackingPort
    for TargetPhyParamTrackingPort<'_, P, R, D, O>
{
    type Error = PhyTargetPortError;

    async fn complete<'port>(
        &'port mut self,
        pending: &'port mut PhyPendingTracking,
        state: &'port mut PhyState,
    ) -> Result<PhyParamTrackingCompletion, Self::Error> {
        self.observer.operation_started();
        let completion = match pending.action() {
            PhyParamTrackingAction::EnterCritical => PhyParamTrackingCompletion::EnteredCritical,
            PhyParamTrackingAction::ExitCritical => PhyParamTrackingCompletion::ExitedCritical,
            PhyParamTrackingAction::RfpllCapTrack { .. } => {
                TargetCompleter::<D>::run_param_rfpll(
                    pending
                        .begin_rfpll_cap_tracking(state)
                        .map_err(|_| PhyTargetPortError::UnexpectedBinding)?,
                    self.platform,
                    self.registers,
                )
                .await?
            }
            PhyParamTrackingAction::BluetoothIeee802154TxPowerTrack { .. }
            | PhyParamTrackingAction::WifiTxPowerTrack { .. } => {
                TargetCompleter::<D>::run_param_tx_power(
                    pending
                        .begin_tx_power_tracking(state)
                        .map_err(|_| PhyTargetPortError::UnexpectedBinding)?,
                    self.platform,
                    self.registers,
                )
                .await?
            }
            PhyParamTrackingAction::CalibrationTrack { .. } => {
                let mut child = pending
                    .begin_calibration_tracking(state)
                    .map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
                let mut port = TargetPhyCalibrationTrackingPort::<_, _, D, _>::new(
                    self.platform,
                    self.registers,
                    &mut self.observer,
                );
                run_phy_calibration_tracking(&mut child, &mut port)
                    .await
                    .map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
                child
                    .commit()
                    .map_err(|_| PhyTargetPortError::UnexpectedBinding)?
            }
            PhyParamTrackingAction::WifiI2cTrack => {
                TargetCompleter::<D>::run_param_wifi_i2c(
                    pending
                        .begin_wifi_i2c_tracking(state)
                        .map_err(|_| PhyTargetPortError::UnexpectedBinding)?,
                    self.registers,
                )
                .await?
            }
            PhyParamTrackingAction::TemperatureRead => {
                TargetCompleter::<D>::run_param_temperature(
                    pending
                        .begin_temperature_read(state)
                        .map_err(|_| PhyTargetPortError::UnexpectedBinding)?,
                    self.platform,
                    self.registers,
                )
                .await?
            }
            PhyParamTrackingAction::Complete(_) => {
                return Err(PhyTargetPortError::UnexpectedBinding);
            }
        };
        self.observer.operation_completed();
        Ok(completion)
    }
}

/// Execute one complete affine periodic-tracking request on ESP32-S31.
///
/// This is the fail-closed production boundary. It consumes the exact
/// [`RegisteredPhyPendingTracking`] whose platform, PAC capability, semantic
/// state, client set, and scheduler request all belong to one epoch. Success
/// returns the ordinary coupled radio only after the outer transition is terminal.
/// Any child, port or transition error consumes the request into
/// [`RegisteredPhyTrackPoisoned`] and retains the registered radio in an opaque
/// failure, so ambiguous hardware work cannot be paired with another state or
/// retried.
///
/// # Cancellation
///
/// Once polled, this future must run to a terminal result. Dropping it also
/// drops the unique pending client owner and therefore cannot create a retry
/// path, but the hardware epoch must be reset before reuse.
#[must_use = "periodic PHY tracking must be driven to a terminal result"]
#[allow(
    clippy::result_large_err,
    reason = "the allocation-free failure must retain the complete poisoned hardware epoch"
)]
pub async fn run_target_phy_param_tracking<P, D, O>(
    mut tracking: RegisteredPhyPendingTracking<P>,
    observer: O,
) -> Result<TargetPhyParamTrackingSuccess<P>, TargetPhyParamTrackingFailure<P>>
where
    D: PhyAsyncDelay,
    O: PhyTargetObserver,
{
    let result = {
        let (platform, registers, state, pending) = tracking.target_tracking_parts();
        let mut port = TargetPhyParamTrackingPort::<_, _, D, _>::new(platform, registers, observer);
        run_phy_param_tracking(pending, state, &mut port).await
    };
    let outcome = match result {
        Ok(outcome) => outcome,
        Err(error) => {
            return Err(TargetPhyParamTrackingFailure {
                poisoned: tracking.fail(),
                error: TargetPhyParamTrackingError::Run(error),
            });
        }
    };
    match tracking.into_registered_radio() {
        Ok(registered_radio) => Ok(TargetPhyParamTrackingSuccess {
            registered_radio,
            outcome,
        }),
        Err(tracking) => Err(TargetPhyParamTrackingFailure {
            poisoned: tracking.fail(),
            error: TargetPhyParamTrackingError::MissingCompletedOwner,
        }),
    }
}

/// Execute one immediate or periodic tracking request on the dedicated IEEE
/// 802.15.4 route.
///
/// The affine input owns the clocked role, target-registration proof, live PHY
/// state, IEEE client bit, and scheduler request. BTBB/timing initialization
/// cannot begin until this function returns terminal success.
#[must_use = "IEEE periodic PHY tracking must be driven to a terminal result"]
#[allow(
    clippy::result_large_err,
    reason = "the allocation-free failure retains the complete poisoned IEEE hardware epoch"
)]
pub async fn run_target_ieee802154_phy_param_tracking<P, D, O>(
    mut tracking: RegisteredIeee802154PendingTracking<P>,
    observer: O,
) -> Result<TargetIeee802154PhyParamTrackingSuccess<P>, TargetIeee802154PhyParamTrackingFailure<P>>
where
    D: PhyAsyncDelay,
    O: PhyTargetObserver,
{
    let result = {
        let (platform, mut registers, state, pending) = tracking.target_tracking_parts();
        let mut port =
            TargetPhyParamTrackingPort::<_, _, D, _>::new(platform, &mut registers, observer);
        run_phy_param_tracking(pending, state, &mut port).await
    };
    let outcome = match result {
        Ok(outcome) => outcome,
        Err(error) => {
            return Err(TargetIeee802154PhyParamTrackingFailure {
                poisoned: tracking.fail(),
                error: TargetPhyParamTrackingError::Run(error),
            });
        }
    };
    match tracking.into_client_owner() {
        Ok(owner) => Ok(TargetIeee802154PhyParamTrackingSuccess { owner, outcome }),
        Err(tracking) => Err(TargetIeee802154PhyParamTrackingFailure {
            poisoned: tracking.fail(),
            error: TargetPhyParamTrackingError::MissingCompletedOwner,
        }),
    }
}

impl<P, R: PhyInitializationAccess, D: PhyAsyncDelay, O: PhyTargetObserver> PhyRegisterPort
    for TargetPhyRegisterPort<'_, P, R, D, O>
{
    type Error = PhyTargetPortError;

    async fn complete(
        &mut self,
        binding: PhyRegisterExternalBinding,
    ) -> Result<PhyRegisterCompletion, Self::Error> {
        self.observer.operation_started();
        let result = match binding {
            PhyRegisterExternalBinding::Mmio(binding) => {
                self.counters.mmio += 1;
                Ok(PhyRegisterCompletion::Mmio(
                    binding.execute_target(self.platform, self.registers),
                ))
            }
            PhyRegisterExternalBinding::Timer(binding) => {
                D::after_micros(u64::from(binding.micros())).await;
                self.counters.delays += 1;
                Ok(binding.into_completion())
            }
            PhyRegisterExternalBinding::ResetSample(binding) => {
                self.counters.reset_samples += 1;
                Ok(binding.execute_target(self.registers))
            }
            PhyRegisterExternalBinding::Rf(binding) => {
                if self.counters.rf_operations >= RF_OPERATION_LIMIT {
                    Err(PhyTargetPortError::RfOperationLimit)
                } else {
                    let completion = TargetCompleter::<D>::complete_rf(
                        binding,
                        self.registers,
                        &mut self.observer,
                    )
                    .await?;
                    self.counters.rf_operations += 1;
                    Ok(PhyRegisterCompletion::Rf(completion))
                }
            }
            PhyRegisterExternalBinding::Baseband(binding) => {
                let completion = TargetCompleter::<D>::complete_baseband(
                    binding,
                    self.platform,
                    self.registers,
                    &mut self.observer,
                )
                .await?;
                self.counters.baseband_operations += 1;
                Ok(PhyRegisterCompletion::Baseband(completion))
            }
            PhyRegisterExternalBinding::Temperature(binding) => {
                Ok(PhyRegisterCompletion::Temperature(
                    TargetCompleter::<D>::complete_temperature(
                        binding,
                        self.platform,
                        self.registers,
                    )
                    .await?,
                ))
            }
            PhyRegisterExternalBinding::FinalI2c(binding) => {
                complete_final_i2c::<D>(binding, self.registers).await
            }
        };
        if result.is_ok() {
            self.observer.operation_completed();
        }
        result
    }
}

/// Drive the dedicated IEEE 802.15.4 owner through the recovered common-PHY
/// transition and the concrete ESP32-S31 target port.
///
/// This is the only production mint path from [`Ieee802154Clocked`] to
/// [`RegisteredIeee802154Clocked`]. It creates a fresh transition internally,
/// borrows the platform and shared-PHY register partitions from the same IEEE
/// owner, and accepts success only after the target executor returns a terminal
/// outcome and the transition yields its registered model owner.
///
/// # Cancellation
///
/// Integrations must drive this future to a terminal result. Once polled,
/// cancelling or dropping it may strand a partially applied hardware edge and
/// destroys the only software owner. The chip must be reset before another
/// radio owner is constructed; cancellation is never a retry path.
#[must_use = "IEEE 802.15.4 PHY registration must be driven to a terminal result"]
#[allow(
    clippy::result_large_err,
    reason = "fail-stop error retains the exact allocation-free IEEE owner and PHY transition"
)]
pub async fn run_target_ieee802154_phy_register<P, D, O>(
    mut owner: Ieee802154Clocked<P>,
    config: TargetIeee802154PhyRegisterConfig,
    observer: O,
) -> Result<TargetIeee802154PhyRegisterSuccess<P>, TargetIeee802154PhyRegisterFailure<P>>
where
    D: PhyAsyncDelay,
    O: PhyTargetObserver,
{
    let TargetIeee802154PhyRegisterConfig {
        calibration_identity,
        calibration_cache,
    } = config;
    let mut transition = PhyRegisterTransition::with_production_config_and_calibration(
        calibration_identity,
        calibration_cache,
    );

    let (result, counters) = {
        let (platform, mut registers) = owner.common_phy_parts();
        let mut port = TargetPhyRegisterPort::<_, _, D, _>::new(platform, &mut registers, observer);
        let result = run_phy_register(&mut transition, &mut port).await;
        (result, port.counters())
    };

    let outcome = match result {
        Ok(outcome) => outcome,
        Err(error) => {
            return Err(TargetIeee802154PhyRegisterFailure {
                _owner: owner,
                transition,
                counters,
                error: TargetIeee802154PhyRegisterError::Run(error),
            });
        }
    };

    let (state, calibration_cache) = match transition.into_model_parts() {
        Ok(parts) => parts,
        Err(transition) => {
            return Err(TargetIeee802154PhyRegisterFailure {
                _owner: owner,
                transition,
                counters,
                error: TargetIeee802154PhyRegisterError::MissingCompletedModelOwner,
            });
        }
    };

    Ok(TargetIeee802154PhyRegisterSuccess {
        registered_owner: RegisteredIeee802154Clocked::from_target_completion(
            owner,
            state,
            TargetRegistrationWitness::new(),
        ),
        calibration_cache,
        outcome,
        counters,
    })
}

/// Drive one opaque fresh attempt through the concrete ESP32-S31 target port.
///
/// Unlike [`crate::run_phy_register`], this function does not accept a caller-
/// supplied port or a raw caller-driven transition. The exact transition that
/// receives every concrete target completion and the exact powered radio epoch
/// remain hidden inside `attempt`. Terminal success produces one result which
/// retains that radio and [`crate::RegisteredPhyState`]. Any other result retains a
/// poisoned opaque owner unless the transition completed its failure cleanup.
///
/// # Cancellation
///
/// Integrations must run this future to completion. Once polled, cancelling or
/// dropping it may leave a hardware edge partially applied and destroys the
/// only software owner; it never returns registration proof. The peripheral or
/// chip must be reset out of band before a new `Radio` owner is established.
#[must_use = "target PHY registration must be driven to a terminal result"]
pub async fn run_target_phy_register<P, D, O>(
    mut attempt: TargetPhyRegisterAttempt<P>,
    observer: O,
) -> Result<TargetPhyRegisterSuccess<P>, TargetPhyRegisterFailure<P>>
where
    D: PhyAsyncDelay,
    O: PhyTargetObserver,
{
    let (result, counters) = {
        let (platform, registers) = attempt.radio.phy_hal_parts();
        let mut port = TargetPhyRegisterPort::<_, _, D, _>::new(platform, registers, observer);
        let result = run_phy_register(&mut attempt.transition, &mut port).await;
        (result, port.counters())
    };

    let outcome = match result {
        Ok(outcome) => outcome,
        Err(error) => {
            return Err(TargetPhyRegisterFailure {
                attempt,
                counters,
                error: TargetPhyRegisterError::Run(error),
            });
        }
    };

    let (state, calibration_cache) = match attempt.transition.into_model_parts() {
        Ok(parts) => parts,
        Err(transition) => {
            return Err(TargetPhyRegisterFailure {
                attempt: TargetPhyRegisterAttempt {
                    radio: attempt.radio,
                    transition,
                },
                counters,
                error: TargetPhyRegisterError::MissingCompletedModelOwner,
            });
        }
    };
    Ok(TargetPhyRegisterSuccess {
        registered_epoch: TargetRegisteredPhyEpoch::from_target_completion(
            attempt.radio,
            state,
            TargetRegistrationWitness::new(),
        ),
        calibration_cache,
        outcome,
        counters,
    })
}

/// Select a PHY channel with the same finite target contract used by cold
/// registration.
/// Select a PHY channel through a temporary borrow of the complete HAL owner.
pub async fn select_phy_channel_with_hal<D: PhyAsyncDelay, P, O: PhyTargetObserver>(
    state: &mut PhyState,
    channel_or_frequency: u16,
    cbw: u8,
    channel: &mut open_esp_radio_esp32s31_hal::channel::RadioChannelHal<'_, P>,
    observer: &mut O,
) -> Result<(), PhyTargetPortError> {
    TargetCompleter::<D>::select_channel_hal(state, channel_or_frequency, cbw, channel, observer)
        .await
}

/// Stop, retune and restart through a temporary borrow of the HAL owner.
pub async fn switch_phy_channel_with_hal_and_mac_restart<
    D: PhyAsyncDelay,
    P,
    O: PhyTargetObserver,
>(
    state: &mut PhyState,
    channel_or_frequency: u16,
    cbw: u8,
    channel: &mut open_esp_radio_esp32s31_hal::channel::RadioChannelHal<'_, P>,
    observer: &mut O,
) -> Result<(), PhyTargetPortError> {
    TargetCompleter::<D>::switch_channel_hal_with_mac_restart(
        state,
        channel_or_frequency,
        cbw,
        channel,
        observer,
    )
    .await
}
