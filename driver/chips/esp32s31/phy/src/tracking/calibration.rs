//! Exact orchestration model for periodic calibration tracking.
//!
//! The pinned `libphy.a[phy_track.o]::phy_cal_param_track` body is 602 bytes.
//! It evaluates three independent temperature references: common DCODE/RX
//! calibration, Wi-Fi TXDC/gain calibration, and shared Bluetooth/IEEE
//! 802.15.4 TXDC/gain calibration. This transition preserves the inclusive
//! threshold, protocol selector, hardware quiesce/restore order, and final
//! unconditional TX-gain-compensation restore without retaining vendor
//! parameter offsets.

use crate::tracking::parameters::PhyCalibrationTrackClass;

const DEFAULT_CALIBRATION_TRACKING_THRESHOLD: u8 = 30;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyCalibrationTrackingParameters {
    pub current_temperature: i16,
    pub common_reference_temperature: i16,
    pub wifi_reference_temperature: i16,
    pub bluetooth_ieee802154_reference_temperature: i16,
    pub threshold_override: Option<u8>,
    pub current_channel: u16,
    pub channel_bandwidth: u8,
    pub crystal_selector: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyCalibrationTrackingRequest {
    pub class: PhyCalibrationTrackClass,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyCalibrationTrackingOutcome {
    pub class: PhyCalibrationTrackClass,
    pub threshold: u8,
    pub common_reference_temperature: i16,
    pub wifi_reference_temperature: i16,
    pub bluetooth_ieee802154_reference_temperature: i16,
    pub common_updated: bool,
    pub class_updated: bool,
    pub dcode: Option<crate::analog::dcode::PhyDcodeOutcome>,
    pub rx_gain: Option<crate::rx::gain::PhyRxGainInitOutcome>,
    pub channel: Option<crate::channel::PhyChipChannelOutcome>,
    pub tx_dc_pwdet: Option<crate::tx::dc_power_detector::PhyTxDcPwdetOutcome>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyCalibrationTrackingAction {
    ClearPbus,
    CalibrateDcode,
    RecalibrateRxGain,
    RestoreChipChannel { channel: u16, cbw: u8 },
    SetHardwareFrequencyControl { enabled: bool },
    ForceTxRxOff { enabled: bool },
    ConfigureBasebandChannel { cbw: u8 },
    CalibrateTxDcPwdet { class: PhyCalibrationTrackClass },
    PublishWifiTxGain { channel: u16 },
    PublishBluetoothIeee802154TxGain,
    EnableMacBaseband,
    RestoreTxGainCompensation,
    Complete(PhyCalibrationTrackingOutcome),
    Failed(PhyCalibrationTrackingFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyCalibrationTrackingCompletion {
    PbusClearCompleted(PhyCalibrationPbusClearCompletion),
    DcodeCompleted(PhyCalibrationDcodeCompletion),
    RxGainRecalibrated(PhyCalibrationRxGainCompletion),
    ChipChannelRestored(PhyCalibrationChannelCompletion),
    HardwareFrequencyControlSet { enabled: bool },
    ForceTxRxCompleted(PhyCalibrationForceTxRxCompletion),
    BasebandChannelConfigured { cbw: u8 },
    TxDcPwdetCalibrated(PhyCalibrationTxDcPwdetCompletion),
    TxGainPublished(PhyCalibrationTxGainCompletion),
    MacBasebandEnabled,
    TxGainCompensationRestored,
}

/// Opaque proof that both force-mode writes and both timer edges completed.
///
/// ```compile_fail
/// use open_esp_radio_esp32s31_phy::tracking::calibration::{
///     PhyCalibrationForceTxRxCompletion, PhyCalibrationTrackingCompletion,
/// };
///
/// let forged = PhyCalibrationTrackingCompletion::ForceTxRxCompleted(
///     PhyCalibrationForceTxRxCompletion { enabled: true },
/// );
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyCalibrationForceTxRxCompletion {
    enabled: bool,
}

/// Opaque terminal result of the complete bounded PBus-clear child.
///
/// ```compile_fail
/// use open_esp_radio_esp32s31_phy::tracking::calibration::{
///     PhyCalibrationPbusClearCompletion, PhyCalibrationTrackingCompletion,
/// };
/// use open_esp_radio_esp32s31_phy::analog::pbus::PhyPbusClearOutcome;
///
/// let forged = PhyCalibrationTrackingCompletion::PbusClearCompleted(
///     PhyCalibrationPbusClearCompletion {
///         outcome: PhyPbusClearOutcome::Cleared,
///     },
/// );
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyCalibrationPbusClearCompletion {
    outcome: crate::analog::pbus::PhyPbusClearOutcome,
}

/// Opaque terminal result of the complete DCODE child.
///
/// ```compile_fail
/// use open_esp_radio_esp32s31_phy::tracking::calibration::{
///     PhyCalibrationDcodeCompletion, PhyCalibrationTrackingCompletion,
/// };
///
/// let forged = PhyCalibrationTrackingCompletion::DcodeCompleted(
///     PhyCalibrationDcodeCompletion {
///         result: Ok(open_esp_radio_esp32s31_phy::analog::dcode::PhyDcodeOutcome {
///             codes: [0; 8],
///         }),
///     },
/// );
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyCalibrationDcodeCompletion {
    result: Result<crate::analog::dcode::PhyDcodeOutcome, crate::analog::dcode::PhyDcodeFailure>,
}

/// Opaque terminal result of complete RX-DC calibration and table generation.
///
/// ```compile_fail
/// use open_esp_radio_esp32s31_phy::tracking::calibration::{
///     PhyCalibrationRxGainCompletion, PhyCalibrationTrackingCompletion,
/// };
///
/// let forged = PhyCalibrationTrackingCompletion::RxGainRecalibrated(
///     PhyCalibrationRxGainCompletion {
///         result: Ok(open_esp_radio_esp32s31_phy::rx::gain::PhyRxGainInitOutcome {
///             dc: None,
///             generated_tables: true,
///             wifi_last_index: 69,
///             shared_last_index: 75,
///         }),
///     },
/// );
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyCalibrationRxGainCompletion {
    result: Result<crate::rx::gain::PhyRxGainInitOutcome, crate::rx::gain::PhyRxGainInitFailure>,
}

/// Opaque terminal result of the complete chip-channel child.
///
/// ```compile_fail
/// use open_esp_radio_esp32s31_phy::tracking::calibration::{
///     PhyCalibrationChannelCompletion, PhyCalibrationTrackingCompletion,
/// };
///
/// let forged = PhyCalibrationTrackingCompletion::ChipChannelRestored(
///     PhyCalibrationChannelCompletion {
///         result: Err(open_esp_radio_esp32s31_phy::channel::PhyChipChannelFailure::UnsupportedChannel(14)),
///     },
/// );
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyCalibrationChannelCompletion {
    result: Result<crate::channel::PhyChipChannelOutcome, crate::channel::PhyChipChannelFailure>,
}

/// Opaque terminal result of the complete class-specific TXDC/PWDET child.
///
/// ```compile_fail
/// use open_esp_radio_esp32s31_phy::tracking::calibration::{
///     PhyCalibrationTrackingCompletion, PhyCalibrationTxDcPwdetCompletion,
/// };
/// use open_esp_radio_esp32s31_phy::tracking::parameters::PhyCalibrationTrackClass;
///
/// let forged = PhyCalibrationTrackingCompletion::TxDcPwdetCalibrated(
///     PhyCalibrationTxDcPwdetCompletion {
///         class: PhyCalibrationTrackClass::Wifi,
///         result: Ok(open_esp_radio_esp32s31_phy::tx::dc_power_detector::PhyTxDcPwdetOutcome {
///             dco: [[0; 4]; 3],
///             total_measurements: 0,
///         }),
///     },
/// );
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyCalibrationTxDcPwdetCompletion {
    class: PhyCalibrationTrackClass,
    result: Result<
        crate::tx::dc_power_detector::PhyTxDcPwdetOutcome,
        crate::tx::dc_power_detector::PhyTxDcPwdetFailure,
    >,
}

/// Opaque proof that the class-specific gain image derived from the pending
/// TXDC/PWDET result reached its PAC-backed publication edge.
///
/// ```compile_fail
/// use open_esp_radio_esp32s31_phy::tracking::calibration::{
///     PhyCalibrationTrackingCompletion, PhyCalibrationTxGainCompletion,
/// };
/// use open_esp_radio_esp32s31_phy::tracking::parameters::PhyCalibrationTrackClass;
///
/// let forged = PhyCalibrationTrackingCompletion::TxGainPublished(
///     PhyCalibrationTxGainCompletion {
///         class: PhyCalibrationTrackClass::Wifi,
///         channel: Some(11),
///     },
/// );
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyCalibrationTxGainCompletion {
    class: PhyCalibrationTrackClass,
    channel: Option<u16>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyCalibrationTrackingFailure {
    PbusClearTimedOut(crate::analog::pbus::PhyPbusForceTest),
    Dcode(crate::analog::dcode::PhyDcodeFailure),
    RxGain(crate::rx::gain::PhyRxGainInitFailure),
    Channel(crate::channel::PhyChipChannelFailure),
    TxDcPwdet(crate::tx::dc_power_detector::PhyTxDcPwdetFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyCalibrationTrackingTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Step {
    CommonClearPbus,
    CommonDcode,
    CommonRecalibrateRxGain,
    CommonRestoreChannel,
    CommonEnableMac,
    ClassDisableHardwareFrequency,
    ClassForceTxRxOff,
    ClassClearPbus,
    ClassConfigureBasebandZero,
    ClassCalibrateTxDcPwdet,
    ClassPublishTxGain,
    ClassRestoreBaseband,
    ClassEnableMac,
    ClassReleaseTxRxOff,
    ClassEnableHardwareFrequency,
    RestoreTxGainCompensation,
    Complete,
    Failed,
}

/// Finite exact-order parent for `phy_cal_param_track`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyCalibrationTrackingTransition {
    request: PhyCalibrationTrackingRequest,
    parameters: PhyCalibrationTrackingParameters,
    threshold: u8,
    common_updated: bool,
    class_updated: bool,
    dcode: Option<crate::analog::dcode::PhyDcodeOutcome>,
    rx_gain: Option<crate::rx::gain::PhyRxGainInitOutcome>,
    channel: Option<crate::channel::PhyChipChannelOutcome>,
    tx_dc_pwdet: Option<crate::tx::dc_power_detector::PhyTxDcPwdetOutcome>,
    failure: Option<PhyCalibrationTrackingFailure>,
    step: Step,
}

impl PhyCalibrationTrackingTransition {
    pub const fn new(
        request: PhyCalibrationTrackingRequest,
        parameters: PhyCalibrationTrackingParameters,
    ) -> Self {
        let threshold = match parameters.threshold_override {
            Some(value) => value,
            None => DEFAULT_CALIBRATION_TRACKING_THRESHOLD,
        };
        let common_due = temperature_delta(
            parameters.current_temperature,
            parameters.common_reference_temperature,
        ) >= threshold as u32;
        let step = if common_due {
            Step::CommonClearPbus
        } else if class_due(request, parameters, threshold) {
            Step::ClassDisableHardwareFrequency
        } else {
            Step::RestoreTxGainCompensation
        };
        Self {
            request,
            parameters,
            threshold,
            common_updated: false,
            class_updated: false,
            dcode: None,
            rx_gain: None,
            channel: None,
            tx_dc_pwdet: None,
            failure: None,
            step,
        }
    }

    pub const fn action(self) -> PhyCalibrationTrackingAction {
        match self.step {
            Step::CommonClearPbus | Step::ClassClearPbus => PhyCalibrationTrackingAction::ClearPbus,
            Step::CommonDcode => PhyCalibrationTrackingAction::CalibrateDcode,
            Step::CommonRecalibrateRxGain => PhyCalibrationTrackingAction::RecalibrateRxGain,
            Step::CommonRestoreChannel => PhyCalibrationTrackingAction::RestoreChipChannel {
                channel: self.parameters.current_channel,
                cbw: self.parameters.channel_bandwidth,
            },
            Step::CommonEnableMac | Step::ClassEnableMac => {
                PhyCalibrationTrackingAction::EnableMacBaseband
            }
            Step::ClassDisableHardwareFrequency => {
                PhyCalibrationTrackingAction::SetHardwareFrequencyControl { enabled: false }
            }
            Step::ClassForceTxRxOff => PhyCalibrationTrackingAction::ForceTxRxOff { enabled: true },
            Step::ClassConfigureBasebandZero => {
                PhyCalibrationTrackingAction::ConfigureBasebandChannel { cbw: 0 }
            }
            Step::ClassCalibrateTxDcPwdet => PhyCalibrationTrackingAction::CalibrateTxDcPwdet {
                class: self.request.class,
            },
            Step::ClassPublishTxGain => match self.request.class {
                PhyCalibrationTrackClass::Wifi => PhyCalibrationTrackingAction::PublishWifiTxGain {
                    channel: self.parameters.current_channel,
                },
                PhyCalibrationTrackClass::BluetoothIeee802154 => {
                    PhyCalibrationTrackingAction::PublishBluetoothIeee802154TxGain
                }
            },
            Step::ClassRestoreBaseband => PhyCalibrationTrackingAction::ConfigureBasebandChannel {
                cbw: self.parameters.channel_bandwidth,
            },
            Step::ClassReleaseTxRxOff => {
                PhyCalibrationTrackingAction::ForceTxRxOff { enabled: false }
            }
            Step::ClassEnableHardwareFrequency => {
                PhyCalibrationTrackingAction::SetHardwareFrequencyControl { enabled: true }
            }
            Step::RestoreTxGainCompensation => {
                PhyCalibrationTrackingAction::RestoreTxGainCompensation
            }
            Step::Complete => PhyCalibrationTrackingAction::Complete(self.outcome()),
            Step::Failed => match self.failure {
                Some(failure) => PhyCalibrationTrackingAction::Failed(failure),
                None => unreachable!(),
            },
        }
    }

    pub fn advance(
        &mut self,
        completion: PhyCalibrationTrackingCompletion,
    ) -> Result<(), PhyCalibrationTrackingTransitionError> {
        self.step = match (self.step, completion) {
            (
                Step::CommonClearPbus,
                PhyCalibrationTrackingCompletion::PbusClearCompleted(completion),
            ) => match completion.outcome {
                crate::analog::pbus::PhyPbusClearOutcome::Cleared => Step::CommonDcode,
                crate::analog::pbus::PhyPbusClearOutcome::ForceTestTimedOut(transaction) => {
                    self.failure = Some(PhyCalibrationTrackingFailure::PbusClearTimedOut(
                        transaction,
                    ));
                    Step::RestoreTxGainCompensation
                }
            },
            (Step::CommonDcode, PhyCalibrationTrackingCompletion::DcodeCompleted(completion)) => {
                match completion.result {
                    Ok(outcome) => {
                        self.dcode = Some(outcome);
                        Step::CommonRecalibrateRxGain
                    }
                    Err(failure) => {
                        self.failure = Some(PhyCalibrationTrackingFailure::Dcode(failure));
                        Step::RestoreTxGainCompensation
                    }
                }
            }
            (
                Step::CommonRecalibrateRxGain,
                PhyCalibrationTrackingCompletion::RxGainRecalibrated(completion),
            ) => match completion.result {
                Ok(outcome) => {
                    self.rx_gain = Some(outcome);
                    Step::CommonRestoreChannel
                }
                Err(failure) => {
                    self.failure = Some(PhyCalibrationTrackingFailure::RxGain(failure));
                    Step::RestoreTxGainCompensation
                }
            },
            (
                Step::CommonRestoreChannel,
                PhyCalibrationTrackingCompletion::ChipChannelRestored(completion),
            ) => match completion.result {
                Ok(outcome)
                    if outcome.channel == self.parameters.current_channel
                        && outcome.cbw == self.parameters.channel_bandwidth =>
                {
                    self.parameters.current_temperature = outcome.temperature.temperature;
                    self.channel = Some(outcome);
                    Step::CommonEnableMac
                }
                Ok(_) => return Err(PhyCalibrationTrackingTransitionError::WrongCompletion),
                Err(failure) => {
                    self.failure = Some(PhyCalibrationTrackingFailure::Channel(failure));
                    Step::RestoreTxGainCompensation
                }
            },
            (Step::CommonEnableMac, PhyCalibrationTrackingCompletion::MacBasebandEnabled) => {
                self.common_updated = true;
                self.first_class_step()
            }
            (
                Step::ClassDisableHardwareFrequency,
                PhyCalibrationTrackingCompletion::HardwareFrequencyControlSet { enabled: false },
            ) => Step::ClassForceTxRxOff,
            (
                Step::ClassForceTxRxOff,
                PhyCalibrationTrackingCompletion::ForceTxRxCompleted(completion),
            ) if completion.enabled => Step::ClassClearPbus,
            (
                Step::ClassClearPbus,
                PhyCalibrationTrackingCompletion::PbusClearCompleted(completion),
            ) => match completion.outcome {
                crate::analog::pbus::PhyPbusClearOutcome::Cleared => {
                    Step::ClassConfigureBasebandZero
                }
                crate::analog::pbus::PhyPbusClearOutcome::ForceTestTimedOut(transaction) => {
                    self.failure = Some(PhyCalibrationTrackingFailure::PbusClearTimedOut(
                        transaction,
                    ));
                    Step::ClassReleaseTxRxOff
                }
            },
            (
                Step::ClassConfigureBasebandZero,
                PhyCalibrationTrackingCompletion::BasebandChannelConfigured { cbw: 0 },
            ) => Step::ClassCalibrateTxDcPwdet,
            (
                Step::ClassCalibrateTxDcPwdet,
                PhyCalibrationTrackingCompletion::TxDcPwdetCalibrated(completion),
            ) if completion.class == self.request.class => match completion.result {
                Ok(outcome) => {
                    self.tx_dc_pwdet = Some(outcome);
                    Step::ClassPublishTxGain
                }
                Err(failure) => {
                    self.failure = Some(PhyCalibrationTrackingFailure::TxDcPwdet(failure));
                    Step::ClassReleaseTxRxOff
                }
            },
            (
                Step::ClassPublishTxGain,
                PhyCalibrationTrackingCompletion::TxGainPublished(completion),
            ) if completion.class == self.request.class
                && completion.channel
                    == match self.request.class {
                        PhyCalibrationTrackClass::Wifi => Some(self.parameters.current_channel),
                        PhyCalibrationTrackClass::BluetoothIeee802154 => None,
                    } =>
            {
                Step::ClassRestoreBaseband
            }
            (
                Step::ClassRestoreBaseband,
                PhyCalibrationTrackingCompletion::BasebandChannelConfigured { cbw },
            ) if cbw == self.parameters.channel_bandwidth => Step::ClassEnableMac,
            (Step::ClassEnableMac, PhyCalibrationTrackingCompletion::MacBasebandEnabled) => {
                Step::ClassReleaseTxRxOff
            }
            (
                Step::ClassReleaseTxRxOff,
                PhyCalibrationTrackingCompletion::ForceTxRxCompleted(completion),
            ) if !completion.enabled => Step::ClassEnableHardwareFrequency,
            (
                Step::ClassEnableHardwareFrequency,
                PhyCalibrationTrackingCompletion::HardwareFrequencyControlSet { enabled: true },
            ) => {
                self.class_updated = self.failure.is_none();
                Step::RestoreTxGainCompensation
            }
            (
                Step::RestoreTxGainCompensation,
                PhyCalibrationTrackingCompletion::TxGainCompensationRestored,
            ) => {
                if self.failure.is_some() {
                    Step::Failed
                } else {
                    Step::Complete
                }
            }
            (Step::Complete | Step::Failed, _) => {
                return Err(PhyCalibrationTrackingTransitionError::AlreadyComplete);
            }
            _ => return Err(PhyCalibrationTrackingTransitionError::WrongCompletion),
        };
        Ok(())
    }

    /// Lower the selected force/release action into both register and timer
    /// phases of complete `phy_force_txrx_off`.
    pub fn begin_force_txrx(
        &self,
    ) -> Result<PhyCalibrationForceTxRxTransition, PhyCalibrationTrackingChildError> {
        PhyCalibrationForceTxRxTransition::lower(self.action())
    }

    /// Lower either selected PBus-clear action into the existing complete
    /// cold-path hardware/timer binding graph.
    pub fn begin_pbus_clear(
        &self,
    ) -> Result<PhyCalibrationPbusClearTransition, PhyCalibrationTrackingChildError> {
        PhyCalibrationPbusClearTransition::lower(self.action())
    }

    /// Lower the selected common calibration step into the existing complete
    /// RFPLL/MMIO/PHY-I2C DCODE graph.
    pub fn begin_dcode(
        &self,
    ) -> Result<PhyCalibrationDcodeTransition, PhyCalibrationTrackingChildError> {
        PhyCalibrationDcodeTransition::lower(self.action(), self.parameters.crystal_selector)
    }

    /// Lower the selected common refresh into complete RX-DC calibration and
    /// two-bank RX-gain table generation.
    ///
    /// The vendor parent clears completion bits `0x80` and `0x200` immediately
    /// before calling `phy_set_rx_gain_table`. Force both semantic guards off
    /// here so a caller cannot accidentally turn the periodic refresh into a
    /// table-only or limits-only path.
    pub fn begin_rx_gain_recalibration(
        &self,
        mut parameters: crate::rx::gain::PhyRxGainInitParameters,
    ) -> Result<PhyCalibrationRxGainTransition, PhyCalibrationTrackingChildError> {
        parameters.dc_calibrated = false;
        parameters.tables_initialized = false;
        PhyCalibrationRxGainTransition::lower(self.action(), parameters)
    }

    /// Lower the selected common channel restore into complete asynchronous
    /// channel, temperature, PHY-I2C, TX-gain and cleanup hardware ownership.
    pub fn begin_channel_restore(
        &self,
        parameters: crate::channel::PhyChipChannelParameters,
    ) -> Result<PhyCalibrationChannelTransition, PhyCalibrationTrackingChildError> {
        PhyCalibrationChannelTransition::lower(self.action(), parameters)
    }

    /// Select the exact Wi-Fi or shared Bluetooth/IEEE 802.15.4 form of the
    /// complete `phy_txdc_cal_pwdet_init` hardware graph.
    pub fn begin_tx_dc_pwdet(
        &self,
        wifi_parameters: crate::tx::dc_power_detector::PhyTxDcPwdetParameters,
        bluetooth_ieee802154: crate::calibration::bluetooth::PhyBluetoothTxDcPwdetTransition,
    ) -> Result<PhyCalibrationTxDcPwdetTransition, PhyCalibrationTrackingChildError> {
        PhyCalibrationTxDcPwdetTransition::lower(
            self.action(),
            wifi_parameters,
            bluetooth_ieee802154,
        )
    }

    /// Capture the exact Wi-Fi or shared Bluetooth/IEEE 802.15.4 gain image
    /// from live calibration state and the pending TXDC result.
    pub fn begin_tx_gain_publication(
        &self,
        state: &crate::state::PhyState,
    ) -> Result<PhyCalibrationTxGainBinding, PhyCalibrationTrackingChildError> {
        let Some(tx_dc_pwdet) = self.tx_dc_pwdet else {
            return Err(PhyCalibrationTrackingChildError::IncompleteChildOutcome);
        };
        PhyCalibrationTxGainBinding::lower(self.action(), state, tx_dc_pwdet)
    }

    const fn first_class_step(self) -> Step {
        if class_due(self.request, self.parameters, self.threshold) {
            Step::ClassDisableHardwareFrequency
        } else {
            Step::RestoreTxGainCompensation
        }
    }

    const fn outcome(self) -> PhyCalibrationTrackingOutcome {
        let current = self.parameters.current_temperature;
        PhyCalibrationTrackingOutcome {
            class: self.request.class,
            threshold: self.threshold,
            common_reference_temperature: if self.common_updated {
                current
            } else {
                self.parameters.common_reference_temperature
            },
            wifi_reference_temperature: if self.class_updated
                && matches!(self.request.class, PhyCalibrationTrackClass::Wifi)
            {
                current
            } else {
                self.parameters.wifi_reference_temperature
            },
            bluetooth_ieee802154_reference_temperature: if self.class_updated
                && matches!(
                    self.request.class,
                    PhyCalibrationTrackClass::BluetoothIeee802154
                ) {
                current
            } else {
                self.parameters.bluetooth_ieee802154_reference_temperature
            },
            common_updated: self.common_updated,
            class_updated: self.class_updated,
            dcode: if self.common_updated {
                self.dcode
            } else {
                None
            },
            rx_gain: if self.common_updated {
                self.rx_gain
            } else {
                None
            },
            channel: if self.common_updated {
                self.channel
            } else {
                None
            },
            tx_dc_pwdet: if self.class_updated {
                self.tx_dc_pwdet
            } else {
                None
            },
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyCalibrationTrackingChildError {
    UnsupportedAction,
    IncompleteChildOutcome,
}

/// Non-cloneable calibration-child owner for complete `phy_force_txrx_off`.
#[derive(Debug, Eq, PartialEq)]
pub struct PhyCalibrationForceTxRxTransition {
    enabled: bool,
    child: crate::analog::pbus::PhyForceTxRxTransition,
}

/// Non-cloneable calibration-child owner for complete bounded PBus clear.
#[derive(Debug, Eq, PartialEq)]
pub struct PhyCalibrationPbusClearTransition {
    child: crate::analog::pbus::PhyPbusClearTransition,
}

/// Non-cloneable calibration-child owner for complete DCODE calibration.
#[derive(Debug, Eq, PartialEq)]
pub struct PhyCalibrationDcodeTransition {
    child: crate::analog::dcode::PhyDcodeTransition,
}

/// Non-cloneable calibration-child owner for complete RX-gain recalibration.
#[derive(Debug, Eq, PartialEq)]
pub struct PhyCalibrationRxGainTransition {
    child: crate::rx::gain::PhyRxGainInitTransition,
}

/// Non-cloneable calibration-child owner for complete channel restoration.
#[derive(Debug, Eq, PartialEq)]
pub struct PhyCalibrationChannelTransition {
    child: crate::channel::PhyChipChannelTransition,
}

#[derive(Debug, Eq, PartialEq)]
enum PhyCalibrationTxDcPwdetChild {
    Wifi(crate::tx::dc_power_detector::PhyTxDcPwdetTransition),
    BluetoothIeee802154(crate::calibration::bluetooth::PhyBluetoothTxDcPwdetTransition),
}

/// Non-cloneable owner of the selected complete class TXDC/PWDET graph.
#[derive(Debug, Eq, PartialEq)]
pub struct PhyCalibrationTxDcPwdetTransition {
    class: PhyCalibrationTrackClass,
    child: PhyCalibrationTxDcPwdetChild,
}

#[derive(Debug, Eq, PartialEq)]
enum PhyCalibrationTxGainPublication {
    Wifi {
        channel: u16,
        image: Option<crate::channel::PhyWifiTxGainImage>,
    },
    BluetoothIeee802154 {
        image: crate::calibration::bluetooth::PhyBluetoothTxGainImage,
    },
}

/// Non-cloneable owner of the selected direct gain-memory publication edge.
///
/// The image is captured before MMIO from the pending TXDC/PWDET result, so
/// callers cannot separate the parent completion identity from the DCO bytes
/// that were actually published.
#[derive(Debug, Eq, PartialEq)]
pub struct PhyCalibrationTxGainBinding {
    publication: PhyCalibrationTxGainPublication,
}

impl PhyCalibrationTxGainBinding {
    fn lower(
        parent_action: PhyCalibrationTrackingAction,
        state: &crate::state::PhyState,
        tx_dc_pwdet: crate::tx::dc_power_detector::PhyTxDcPwdetOutcome,
    ) -> Result<Self, PhyCalibrationTrackingChildError> {
        let publication = match parent_action {
            PhyCalibrationTrackingAction::PublishWifiTxGain { channel } => {
                PhyCalibrationTxGainPublication::Wifi {
                    channel,
                    image: state.wifi_calibration_gain_image(channel, tx_dc_pwdet),
                }
            }
            PhyCalibrationTrackingAction::PublishBluetoothIeee802154TxGain => {
                PhyCalibrationTxGainPublication::BluetoothIeee802154 {
                    image: state.bluetooth_ieee802154_calibration_gain_image(tx_dc_pwdet),
                }
            }
            _ => return Err(PhyCalibrationTrackingChildError::UnsupportedAction),
        };
        Ok(Self { publication })
    }

    pub const fn action(&self) -> PhyCalibrationTrackingAction {
        match self.publication {
            PhyCalibrationTxGainPublication::Wifi { channel, .. } => {
                PhyCalibrationTrackingAction::PublishWifiTxGain { channel }
            }
            PhyCalibrationTxGainPublication::BluetoothIeee802154 { .. } => {
                PhyCalibrationTrackingAction::PublishBluetoothIeee802154TxGain
            }
        }
    }

    #[cfg(target_arch = "riscv32")]
    pub fn execute_target(
        self,
        registers: &mut impl open_esp_radio_esp32s31_hal::SharedPhyAccess,
    ) -> PhyCalibrationTrackingCompletion {
        let completion = match self.publication {
            PhyCalibrationTxGainPublication::Wifi { channel, image } => {
                if let Some(image) = image {
                    crate::hardware::publish_phy_tx_gain_memory(registers, false, image);
                }
                PhyCalibrationTxGainCompletion {
                    class: PhyCalibrationTrackClass::Wifi,
                    channel: Some(channel),
                }
            }
            PhyCalibrationTxGainPublication::BluetoothIeee802154 { image } => {
                crate::hardware::publish_bluetooth_tx_gain_memory(registers, image);
                PhyCalibrationTxGainCompletion {
                    class: PhyCalibrationTrackClass::BluetoothIeee802154,
                    channel: None,
                }
            }
        };
        PhyCalibrationTrackingCompletion::TxGainPublished(completion)
    }
}

impl PhyCalibrationTxDcPwdetTransition {
    fn lower(
        parent_action: PhyCalibrationTrackingAction,
        wifi_parameters: crate::tx::dc_power_detector::PhyTxDcPwdetParameters,
        bluetooth_ieee802154: crate::calibration::bluetooth::PhyBluetoothTxDcPwdetTransition,
    ) -> Result<Self, PhyCalibrationTrackingChildError> {
        let PhyCalibrationTrackingAction::CalibrateTxDcPwdet { class } = parent_action else {
            return Err(PhyCalibrationTrackingChildError::UnsupportedAction);
        };
        let child = match class {
            PhyCalibrationTrackClass::Wifi => PhyCalibrationTxDcPwdetChild::Wifi(
                crate::tx::dc_power_detector::PhyTxDcPwdetTransition::new(wifi_parameters),
            ),
            PhyCalibrationTrackClass::BluetoothIeee802154 => {
                PhyCalibrationTxDcPwdetChild::BluetoothIeee802154(bluetooth_ieee802154)
            }
        };
        Ok(Self { class, child })
    }

    pub const fn class(&self) -> PhyCalibrationTrackClass {
        self.class
    }

    pub const fn action(&self) -> crate::tx::dc_power_detector::PhyTxDcPwdetAction {
        match &self.child {
            PhyCalibrationTxDcPwdetChild::Wifi(child) => child.action(),
            PhyCalibrationTxDcPwdetChild::BluetoothIeee802154(child) => child.action(),
        }
    }

    pub fn advance(
        &mut self,
        completion: crate::tx::dc_power_detector::PhyTxDcPwdetCompletion,
    ) -> Result<(), crate::tx::dc_power_detector::PhyTxDcPwdetTransitionError> {
        match &mut self.child {
            PhyCalibrationTxDcPwdetChild::Wifi(child) => child.advance(completion),
            PhyCalibrationTxDcPwdetChild::BluetoothIeee802154(child) => child.advance(completion),
        }
    }

    pub fn lower_external(
        &self,
    ) -> Result<
        crate::tx::dc_power_detector::PhyTxDcPwdetExternalBinding,
        crate::tx::dc_power_detector::PhyTxDcPwdetExternalBindingError,
    > {
        crate::tx::dc_power_detector::PhyTxDcPwdetExternalBinding::lower(self.action())
    }

    #[expect(
        clippy::result_large_err,
        reason = "the pending variant must return the allocation-free bounded calibration owner"
    )]
    pub fn commit(self) -> Result<PhyCalibrationTrackingCompletion, Self> {
        let result = match self.action() {
            crate::tx::dc_power_detector::PhyTxDcPwdetAction::Complete(outcome) => Ok(outcome),
            crate::tx::dc_power_detector::PhyTxDcPwdetAction::Failed(failure) => Err(failure),
            _ => return Err(self),
        };
        Ok(PhyCalibrationTrackingCompletion::TxDcPwdetCalibrated(
            PhyCalibrationTxDcPwdetCompletion {
                class: self.class,
                result,
            },
        ))
    }
}

impl PhyCalibrationChannelTransition {
    fn lower(
        parent_action: PhyCalibrationTrackingAction,
        parameters: crate::channel::PhyChipChannelParameters,
    ) -> Result<Self, PhyCalibrationTrackingChildError> {
        let PhyCalibrationTrackingAction::RestoreChipChannel { channel, cbw } = parent_action
        else {
            return Err(PhyCalibrationTrackingChildError::UnsupportedAction);
        };
        Ok(Self {
            child: crate::channel::PhyChipChannelTransition::new(
                crate::channel::PhyChipChannelRequest {
                    channel_or_frequency: channel,
                    cbw,
                    parameters,
                },
            ),
        })
    }

    pub const fn action(&self) -> crate::channel::PhyChipChannelAction {
        self.child.action()
    }

    pub fn advance(
        &mut self,
        completion: crate::channel::PhyChipChannelCompletion,
    ) -> Result<(), crate::channel::PhyChipChannelTransitionError> {
        self.child.advance(completion)
    }

    pub fn lower_external(
        &self,
    ) -> Result<
        crate::channel::PhyChipChannelExternalBinding,
        crate::channel::PhyChipChannelExternalBindingError,
    > {
        crate::channel::PhyChipChannelExternalBinding::lower(self.action())
    }

    #[expect(
        clippy::result_large_err,
        reason = "the pending variant must return the allocation-free complete channel owner"
    )]
    pub fn commit(self) -> Result<PhyCalibrationTrackingCompletion, Self> {
        let result = match self.child.action() {
            crate::channel::PhyChipChannelAction::Complete(outcome) => Ok(outcome),
            crate::channel::PhyChipChannelAction::Failed(failure) => Err(failure),
            _ => return Err(self),
        };
        Ok(PhyCalibrationTrackingCompletion::ChipChannelRestored(
            PhyCalibrationChannelCompletion { result },
        ))
    }
}

impl PhyCalibrationRxGainTransition {
    fn lower(
        parent_action: PhyCalibrationTrackingAction,
        parameters: crate::rx::gain::PhyRxGainInitParameters,
    ) -> Result<Self, PhyCalibrationTrackingChildError> {
        if parent_action != PhyCalibrationTrackingAction::RecalibrateRxGain {
            return Err(PhyCalibrationTrackingChildError::UnsupportedAction);
        }
        Ok(Self {
            child: crate::rx::gain::PhyRxGainInitTransition::new(parameters),
        })
    }

    pub fn action(&self) -> crate::rx::gain::PhyRxGainInitAction {
        self.child.action()
    }

    pub fn advance(
        &mut self,
        completion: crate::rx::gain::PhyRxGainInitCompletion,
    ) -> Result<(), crate::rx::gain::PhyRxGainInitTransitionError> {
        self.child.advance(completion)
    }

    pub fn lower_external(
        &self,
    ) -> Result<
        crate::rx::gain::PhyRxGainInitExternalBinding,
        crate::rx::gain::PhyRxGainExternalBindingError,
    > {
        crate::rx::gain::PhyRxGainInitExternalBinding::lower(self.action())
    }

    #[expect(
        clippy::result_large_err,
        reason = "the pending variant must return the non-allocating linear table owner"
    )]
    pub fn commit(self) -> Result<PhyCalibrationTrackingCompletion, Self> {
        let result = match self.child.action() {
            crate::rx::gain::PhyRxGainInitAction::Complete(outcome) => Ok(outcome),
            crate::rx::gain::PhyRxGainInitAction::Failed(failure) => Err(failure),
            _ => return Err(self),
        };
        Ok(PhyCalibrationTrackingCompletion::RxGainRecalibrated(
            PhyCalibrationRxGainCompletion { result },
        ))
    }
}

impl PhyCalibrationDcodeTransition {
    fn lower(
        parent_action: PhyCalibrationTrackingAction,
        crystal_selector: u8,
    ) -> Result<Self, PhyCalibrationTrackingChildError> {
        if parent_action != PhyCalibrationTrackingAction::CalibrateDcode {
            return Err(PhyCalibrationTrackingChildError::UnsupportedAction);
        }
        Ok(Self {
            child: crate::analog::dcode::PhyDcodeTransition::new(
                crate::analog::dcode::PhyDcodeParameters { crystal_selector },
            ),
        })
    }

    pub const fn action(&self) -> crate::analog::dcode::PhyDcodeAction {
        self.child.action()
    }

    pub fn advance(
        &mut self,
        completion: crate::analog::dcode::PhyDcodeCompletion,
    ) -> Result<(), crate::analog::dcode::PhyDcodeTransitionError> {
        self.child.advance(completion)
    }

    pub fn lower_external(
        &self,
    ) -> Result<
        crate::analog::dcode::PhyDcodeExternalBinding,
        crate::analog::dcode::PhyDcodeBindingError,
    > {
        crate::analog::dcode::PhyDcodeExternalBinding::lower(self.action())
    }

    pub fn commit(self) -> Result<PhyCalibrationTrackingCompletion, Self> {
        let result = match self.child.action() {
            crate::analog::dcode::PhyDcodeAction::Complete(outcome) => Ok(outcome),
            crate::analog::dcode::PhyDcodeAction::Failed(failure) => Err(failure),
            _ => return Err(self),
        };
        Ok(PhyCalibrationTrackingCompletion::DcodeCompleted(
            PhyCalibrationDcodeCompletion { result },
        ))
    }
}

impl PhyCalibrationPbusClearTransition {
    fn lower(
        parent_action: PhyCalibrationTrackingAction,
    ) -> Result<Self, PhyCalibrationTrackingChildError> {
        if parent_action != PhyCalibrationTrackingAction::ClearPbus {
            return Err(PhyCalibrationTrackingChildError::UnsupportedAction);
        }
        Ok(Self {
            child: crate::analog::pbus::PhyPbusClearTransition::new(),
        })
    }

    pub const fn action(&self) -> crate::analog::pbus::PhyPbusClearAction {
        self.child.action()
    }

    pub fn advance(
        &mut self,
        completion: crate::analog::pbus::PhyPbusClearCompletion,
    ) -> Result<(), crate::analog::pbus::PhyPbusClearTransitionError> {
        self.child.advance(completion)
    }

    pub fn advance_external(
        &mut self,
        completion: crate::analog::i2c::PhyRfInitPrefixCompletion,
    ) -> Result<(), crate::analog::pbus::PhyPbusClearTransitionError> {
        let crate::analog::i2c::PhyRfInitPrefixCompletion::PbusClear(completion) = completion
        else {
            return Err(crate::analog::pbus::PhyPbusClearTransitionError::WrongCompletion);
        };
        self.advance(completion)
    }

    pub fn lower_external(
        &self,
    ) -> Result<
        crate::calibration::cold::PhyColdExternalBinding,
        crate::calibration::cold::PhyColdLoweringError,
    > {
        crate::calibration::cold::PhyColdExternalBinding::lower(
            crate::analog::i2c::PhyRfInitPrefixAction::PbusClear(self.action()),
        )
    }

    pub fn commit(self) -> Result<PhyCalibrationTrackingCompletion, Self> {
        let crate::analog::pbus::PhyPbusClearAction::Complete(outcome) = self.child.action() else {
            return Err(self);
        };
        Ok(PhyCalibrationTrackingCompletion::PbusClearCompleted(
            PhyCalibrationPbusClearCompletion { outcome },
        ))
    }
}

impl PhyCalibrationForceTxRxTransition {
    fn lower(
        parent_action: PhyCalibrationTrackingAction,
    ) -> Result<Self, PhyCalibrationTrackingChildError> {
        let PhyCalibrationTrackingAction::ForceTxRxOff { enabled } = parent_action else {
            return Err(PhyCalibrationTrackingChildError::UnsupportedAction);
        };
        Ok(Self {
            enabled,
            child: crate::analog::pbus::PhyForceTxRxTransition::new(enabled),
        })
    }

    pub const fn parent_action(&self) -> PhyCalibrationTrackingAction {
        PhyCalibrationTrackingAction::ForceTxRxOff {
            enabled: self.enabled,
        }
    }

    pub const fn action(&self) -> crate::analog::pbus::PhyForceTxRxAction {
        self.child.action()
    }

    pub fn advance(
        &mut self,
        completion: crate::analog::pbus::PhyForceTxRxCompletion,
    ) -> Result<(), crate::analog::pbus::PhyForceTxRxTransitionError> {
        self.child.advance(completion)
    }

    pub fn lower_external(
        &self,
    ) -> Result<
        crate::analog::pbus::PhyForceTxRxExternalBinding,
        crate::analog::pbus::PhyForceTxRxBindingError,
    > {
        crate::analog::pbus::PhyForceTxRxExternalBinding::lower(self.action())
    }

    pub fn commit(self) -> Result<PhyCalibrationTrackingCompletion, Self> {
        let crate::analog::pbus::PhyForceTxRxAction::Complete { enabled } = self.child.action()
        else {
            return Err(self);
        };
        Ok(PhyCalibrationTrackingCompletion::ForceTxRxCompleted(
            PhyCalibrationForceTxRxCompletion { enabled },
        ))
    }
}

const fn class_due(
    request: PhyCalibrationTrackingRequest,
    parameters: PhyCalibrationTrackingParameters,
    threshold: u8,
) -> bool {
    let reference = match request.class {
        PhyCalibrationTrackClass::Wifi => parameters.wifi_reference_temperature,
        PhyCalibrationTrackClass::BluetoothIeee802154 => {
            parameters.bluetooth_ieee802154_reference_temperature
        }
    };
    temperature_delta(parameters.current_temperature, reference) >= threshold as u32
}

const fn temperature_delta(current: i16, reference: i16) -> u32 {
    crate::calibration::math::absolute_temperature((reference as i32).wrapping_sub(current as i32))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyCalibrationTrackingBindingError {
    UnsupportedAction,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RegisterAction {
    SetHardwareFrequencyControl { enabled: bool },
    ConfigureBasebandChannel { cbw: u8 },
    RestoreTxGainCompensation,
}

/// Non-cloneable owner of one finite register-only calibration edge.
#[derive(Debug, Eq, PartialEq)]
pub struct PhyCalibrationTrackingRegisterBinding {
    action: RegisterAction,
}

impl PhyCalibrationTrackingRegisterBinding {
    pub const fn new(
        action: PhyCalibrationTrackingAction,
    ) -> Result<Self, PhyCalibrationTrackingBindingError> {
        let action = match action {
            PhyCalibrationTrackingAction::SetHardwareFrequencyControl { enabled } => {
                RegisterAction::SetHardwareFrequencyControl { enabled }
            }
            PhyCalibrationTrackingAction::ConfigureBasebandChannel { cbw } => {
                RegisterAction::ConfigureBasebandChannel { cbw }
            }
            PhyCalibrationTrackingAction::RestoreTxGainCompensation => {
                RegisterAction::RestoreTxGainCompensation
            }
            _ => return Err(PhyCalibrationTrackingBindingError::UnsupportedAction),
        };
        Ok(Self { action })
    }

    pub const fn action(&self) -> PhyCalibrationTrackingAction {
        match self.action {
            RegisterAction::SetHardwareFrequencyControl { enabled } => {
                PhyCalibrationTrackingAction::SetHardwareFrequencyControl { enabled }
            }
            RegisterAction::ConfigureBasebandChannel { cbw } => {
                PhyCalibrationTrackingAction::ConfigureBasebandChannel { cbw }
            }
            RegisterAction::RestoreTxGainCompensation => {
                PhyCalibrationTrackingAction::RestoreTxGainCompensation
            }
        }
    }

    #[cfg(target_arch = "riscv32")]
    pub fn execute_target(
        self,
        registers: &mut impl open_esp_radio_esp32s31_hal::SharedPhyAccess,
    ) -> PhyCalibrationTrackingCompletion {
        match self.action {
            RegisterAction::SetHardwareFrequencyControl { enabled } => {
                open_esp_radio_esp32s31_hal::phy_frequency::set_hardware_control(
                    registers, enabled,
                );
                PhyCalibrationTrackingCompletion::HardwareFrequencyControlSet { enabled }
            }
            RegisterAction::ConfigureBasebandChannel { cbw } => {
                open_esp_radio_esp32s31_hal::phy_frequency::configure_channel_cbw(
                    registers,
                    cbw.into(),
                );
                PhyCalibrationTrackingCompletion::BasebandChannelConfigured { cbw }
            }
            RegisterAction::RestoreTxGainCompensation => {
                open_esp_radio_esp32s31_hal::phy_baseband::restore_tx_gain_compensation(registers);
                PhyCalibrationTrackingCompletion::TxGainCompensationRestored
            }
        }
    }
}

/// Non-cloneable owner of the route-PAC `phy_mac_enable_bb` edge.
#[derive(Debug, Eq, PartialEq)]
pub struct PhyCalibrationTrackingMacBasebandBinding;

impl PhyCalibrationTrackingMacBasebandBinding {
    pub const fn new(
        action: PhyCalibrationTrackingAction,
    ) -> Result<Self, PhyCalibrationTrackingBindingError> {
        match action {
            PhyCalibrationTrackingAction::EnableMacBaseband => Ok(Self),
            _ => Err(PhyCalibrationTrackingBindingError::UnsupportedAction),
        }
    }

    pub const fn action(&self) -> PhyCalibrationTrackingAction {
        PhyCalibrationTrackingAction::EnableMacBaseband
    }

    #[cfg(target_arch = "riscv32")]
    pub fn execute_target<R: open_esp_radio_esp32s31_hal::PhyInitializationAccess>(
        self,
        registers: &mut R,
    ) -> PhyCalibrationTrackingCompletion {
        open_esp_radio_esp32s31_hal::phy_frequency::enable_mac_baseband(registers);
        PhyCalibrationTrackingCompletion::MacBasebandEnabled
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum PhyCalibrationTrackingExternalBinding {
    Register(PhyCalibrationTrackingRegisterBinding),
    MacBaseband(PhyCalibrationTrackingMacBasebandBinding),
}

impl PhyCalibrationTrackingExternalBinding {
    pub const fn lower(
        action: PhyCalibrationTrackingAction,
    ) -> Result<Self, PhyCalibrationTrackingBindingError> {
        match action {
            PhyCalibrationTrackingAction::SetHardwareFrequencyControl { .. }
            | PhyCalibrationTrackingAction::ConfigureBasebandChannel { .. }
            | PhyCalibrationTrackingAction::RestoreTxGainCompensation => {
                match PhyCalibrationTrackingRegisterBinding::new(action) {
                    Ok(binding) => Ok(Self::Register(binding)),
                    Err(error) => Err(error),
                }
            }
            PhyCalibrationTrackingAction::EnableMacBaseband => {
                match PhyCalibrationTrackingMacBasebandBinding::new(action) {
                    Ok(binding) => Ok(Self::MacBaseband(binding)),
                    Err(error) => Err(error),
                }
            }
            _ => Err(PhyCalibrationTrackingBindingError::UnsupportedAction),
        }
    }
}

#[cfg(test)]
mod tests;
