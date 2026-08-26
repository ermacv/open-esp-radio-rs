//! Exact orchestration model for periodic calibration tracking.
//!
//! The pinned `libphy.a[phy_track.o]::phy_cal_param_track` body is 602 bytes.
//! It evaluates three independent temperature references: common DCODE/RX
//! calibration, Wi-Fi TXDC/gain calibration, and shared Bluetooth/IEEE
//! 802.15.4 TXDC/gain calibration. This transition preserves the inclusive
//! threshold, protocol selector, hardware quiesce/restore order, and final
//! unconditional TX-gain-compensation restore without retaining vendor
//! parameter offsets.

use crate::phy_param_tracking::PhyCalibrationTrackClass;

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
    pub dcode: Option<crate::phy_dcode::PhyDcodeOutcome>,
    pub rx_gain: Option<crate::phy_rx_gain::PhyRxGainInitOutcome>,
    pub channel: Option<crate::phy_channel::PhyChipChannelOutcome>,
    pub tx_dc_pwdet: Option<crate::phy_txdc_pwdet::PhyTxDcPwdetOutcome>,
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
    WifiTxGainPublished { channel: u16 },
    BluetoothIeee802154TxGainPublished,
    MacBasebandEnabled,
    TxGainCompensationRestored,
}

/// Opaque proof that both force-mode writes and both timer edges completed.
///
/// ```compile_fail
/// use open_esp_radio_esp32s31_phy::phy_cal_tracking::{
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
/// use open_esp_radio_esp32s31_phy::phy_cal_tracking::{
///     PhyCalibrationPbusClearCompletion, PhyCalibrationTrackingCompletion,
/// };
/// use open_esp_radio_esp32s31_phy::phy_pbus::PhyPbusClearOutcome;
///
/// let forged = PhyCalibrationTrackingCompletion::PbusClearCompleted(
///     PhyCalibrationPbusClearCompletion {
///         outcome: PhyPbusClearOutcome::Cleared,
///     },
/// );
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyCalibrationPbusClearCompletion {
    outcome: crate::phy_pbus::PhyPbusClearOutcome,
}

/// Opaque terminal result of the complete DCODE child.
///
/// ```compile_fail
/// use open_esp_radio_esp32s31_phy::phy_cal_tracking::{
///     PhyCalibrationDcodeCompletion, PhyCalibrationTrackingCompletion,
/// };
///
/// let forged = PhyCalibrationTrackingCompletion::DcodeCompleted(
///     PhyCalibrationDcodeCompletion {
///         result: Ok(open_esp_radio_esp32s31_phy::phy_dcode::PhyDcodeOutcome {
///             codes: [0; 8],
///         }),
///     },
/// );
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyCalibrationDcodeCompletion {
    result: Result<crate::phy_dcode::PhyDcodeOutcome, crate::phy_dcode::PhyDcodeFailure>,
}

/// Opaque terminal result of complete RX-DC calibration and table generation.
///
/// ```compile_fail
/// use open_esp_radio_esp32s31_phy::phy_cal_tracking::{
///     PhyCalibrationRxGainCompletion, PhyCalibrationTrackingCompletion,
/// };
///
/// let forged = PhyCalibrationTrackingCompletion::RxGainRecalibrated(
///     PhyCalibrationRxGainCompletion {
///         result: Ok(open_esp_radio_esp32s31_phy::phy_rx_gain::PhyRxGainInitOutcome {
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
    result:
        Result<crate::phy_rx_gain::PhyRxGainInitOutcome, crate::phy_rx_gain::PhyRxGainInitFailure>,
}

/// Opaque terminal result of the complete chip-channel child.
///
/// ```compile_fail
/// use open_esp_radio_esp32s31_phy::phy_cal_tracking::{
///     PhyCalibrationChannelCompletion, PhyCalibrationTrackingCompletion,
/// };
///
/// let forged = PhyCalibrationTrackingCompletion::ChipChannelRestored(
///     PhyCalibrationChannelCompletion {
///         result: Err(open_esp_radio_esp32s31_phy::phy_channel::PhyChipChannelFailure::UnsupportedChannel(14)),
///     },
/// );
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyCalibrationChannelCompletion {
    result: Result<
        crate::phy_channel::PhyChipChannelOutcome,
        crate::phy_channel::PhyChipChannelFailure,
    >,
}

/// Opaque terminal result of the complete class-specific TXDC/PWDET child.
///
/// ```compile_fail
/// use open_esp_radio_esp32s31_phy::phy_cal_tracking::{
///     PhyCalibrationTrackingCompletion, PhyCalibrationTxDcPwdetCompletion,
/// };
/// use open_esp_radio_esp32s31_phy::phy_param_tracking::PhyCalibrationTrackClass;
///
/// let forged = PhyCalibrationTrackingCompletion::TxDcPwdetCalibrated(
///     PhyCalibrationTxDcPwdetCompletion {
///         class: PhyCalibrationTrackClass::Wifi,
///         result: Ok(open_esp_radio_esp32s31_phy::phy_txdc_pwdet::PhyTxDcPwdetOutcome {
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
        crate::phy_txdc_pwdet::PhyTxDcPwdetOutcome,
        crate::phy_txdc_pwdet::PhyTxDcPwdetFailure,
    >,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyCalibrationTrackingFailure {
    PbusClearTimedOut(crate::phy_pbus::PhyPbusForceTest),
    Dcode(crate::phy_dcode::PhyDcodeFailure),
    RxGain(crate::phy_rx_gain::PhyRxGainInitFailure),
    Channel(crate::phy_channel::PhyChipChannelFailure),
    TxDcPwdet(crate::phy_txdc_pwdet::PhyTxDcPwdetFailure),
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
    dcode: Option<crate::phy_dcode::PhyDcodeOutcome>,
    rx_gain: Option<crate::phy_rx_gain::PhyRxGainInitOutcome>,
    channel: Option<crate::phy_channel::PhyChipChannelOutcome>,
    tx_dc_pwdet: Option<crate::phy_txdc_pwdet::PhyTxDcPwdetOutcome>,
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
                crate::phy_pbus::PhyPbusClearOutcome::Cleared => Step::CommonDcode,
                crate::phy_pbus::PhyPbusClearOutcome::ForceTestTimedOut(transaction) => {
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
                crate::phy_pbus::PhyPbusClearOutcome::Cleared => Step::ClassConfigureBasebandZero,
                crate::phy_pbus::PhyPbusClearOutcome::ForceTestTimedOut(transaction) => {
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
                PhyCalibrationTrackingCompletion::WifiTxGainPublished { channel },
            ) if self.request.class == PhyCalibrationTrackClass::Wifi
                && channel == self.parameters.current_channel =>
            {
                Step::ClassRestoreBaseband
            }
            (
                Step::ClassPublishTxGain,
                PhyCalibrationTrackingCompletion::BluetoothIeee802154TxGainPublished,
            ) if self.request.class == PhyCalibrationTrackClass::BluetoothIeee802154 => {
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
        mut parameters: crate::phy_rx_gain::PhyRxGainInitParameters,
    ) -> Result<PhyCalibrationRxGainTransition, PhyCalibrationTrackingChildError> {
        parameters.dc_calibrated = false;
        parameters.tables_initialized = false;
        PhyCalibrationRxGainTransition::lower(self.action(), parameters)
    }

    /// Lower the selected common channel restore into complete asynchronous
    /// channel, temperature, PHY-I2C, TX-gain and cleanup hardware ownership.
    pub fn begin_channel_restore(
        &self,
        parameters: crate::phy_channel::PhyChipChannelParameters,
    ) -> Result<PhyCalibrationChannelTransition, PhyCalibrationTrackingChildError> {
        PhyCalibrationChannelTransition::lower(self.action(), parameters)
    }

    /// Select the exact Wi-Fi or shared Bluetooth/IEEE 802.15.4 form of the
    /// complete `phy_txdc_cal_pwdet_init` hardware graph.
    pub fn begin_tx_dc_pwdet(
        &self,
        wifi_parameters: crate::phy_txdc_pwdet::PhyTxDcPwdetParameters,
        bluetooth_ieee802154: crate::phy_bluetooth::PhyBluetoothTxDcPwdetTransition,
    ) -> Result<PhyCalibrationTxDcPwdetTransition, PhyCalibrationTrackingChildError> {
        PhyCalibrationTxDcPwdetTransition::lower(
            self.action(),
            wifi_parameters,
            bluetooth_ieee802154,
        )
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
}

/// Non-cloneable calibration-child owner for complete `phy_force_txrx_off`.
#[derive(Debug, Eq, PartialEq)]
pub struct PhyCalibrationForceTxRxTransition {
    enabled: bool,
    child: crate::phy_pbus::PhyForceTxRxTransition,
}

/// Non-cloneable calibration-child owner for complete bounded PBus clear.
#[derive(Debug, Eq, PartialEq)]
pub struct PhyCalibrationPbusClearTransition {
    child: crate::phy_pbus::PhyPbusClearTransition,
}

/// Non-cloneable calibration-child owner for complete DCODE calibration.
#[derive(Debug, Eq, PartialEq)]
pub struct PhyCalibrationDcodeTransition {
    child: crate::phy_dcode::PhyDcodeTransition,
}

/// Non-cloneable calibration-child owner for complete RX-gain recalibration.
#[derive(Debug, Eq, PartialEq)]
pub struct PhyCalibrationRxGainTransition {
    child: crate::phy_rx_gain::PhyRxGainInitTransition,
}

/// Non-cloneable calibration-child owner for complete channel restoration.
#[derive(Debug, Eq, PartialEq)]
pub struct PhyCalibrationChannelTransition {
    child: crate::phy_channel::PhyChipChannelTransition,
}

#[derive(Debug, Eq, PartialEq)]
enum PhyCalibrationTxDcPwdetChild {
    Wifi(crate::phy_txdc_pwdet::PhyTxDcPwdetTransition),
    BluetoothIeee802154(crate::phy_bluetooth::PhyBluetoothTxDcPwdetTransition),
}

/// Non-cloneable owner of the selected complete class TXDC/PWDET graph.
#[derive(Debug, Eq, PartialEq)]
pub struct PhyCalibrationTxDcPwdetTransition {
    class: PhyCalibrationTrackClass,
    child: PhyCalibrationTxDcPwdetChild,
}

impl PhyCalibrationTxDcPwdetTransition {
    fn lower(
        parent_action: PhyCalibrationTrackingAction,
        wifi_parameters: crate::phy_txdc_pwdet::PhyTxDcPwdetParameters,
        bluetooth_ieee802154: crate::phy_bluetooth::PhyBluetoothTxDcPwdetTransition,
    ) -> Result<Self, PhyCalibrationTrackingChildError> {
        let PhyCalibrationTrackingAction::CalibrateTxDcPwdet { class } = parent_action else {
            return Err(PhyCalibrationTrackingChildError::UnsupportedAction);
        };
        let child = match class {
            PhyCalibrationTrackClass::Wifi => PhyCalibrationTxDcPwdetChild::Wifi(
                crate::phy_txdc_pwdet::PhyTxDcPwdetTransition::new(wifi_parameters),
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

    pub const fn action(&self) -> crate::phy_txdc_pwdet::PhyTxDcPwdetAction {
        match &self.child {
            PhyCalibrationTxDcPwdetChild::Wifi(child) => child.action(),
            PhyCalibrationTxDcPwdetChild::BluetoothIeee802154(child) => child.action(),
        }
    }

    pub fn advance(
        &mut self,
        completion: crate::phy_txdc_pwdet::PhyTxDcPwdetCompletion,
    ) -> Result<(), crate::phy_txdc_pwdet::PhyTxDcPwdetTransitionError> {
        match &mut self.child {
            PhyCalibrationTxDcPwdetChild::Wifi(child) => child.advance(completion),
            PhyCalibrationTxDcPwdetChild::BluetoothIeee802154(child) => child.advance(completion),
        }
    }

    pub fn lower_external(
        &self,
    ) -> Result<
        crate::phy_txdc_pwdet::PhyTxDcPwdetExternalBinding,
        crate::phy_txdc_pwdet::PhyTxDcPwdetExternalBindingError,
    > {
        crate::phy_txdc_pwdet::PhyTxDcPwdetExternalBinding::lower(self.action())
    }

    #[expect(
        clippy::result_large_err,
        reason = "the pending variant must return the allocation-free bounded calibration owner"
    )]
    pub fn commit(self) -> Result<PhyCalibrationTrackingCompletion, Self> {
        let result = match self.action() {
            crate::phy_txdc_pwdet::PhyTxDcPwdetAction::Complete(outcome) => Ok(outcome),
            crate::phy_txdc_pwdet::PhyTxDcPwdetAction::Failed(failure) => Err(failure),
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
        parameters: crate::phy_channel::PhyChipChannelParameters,
    ) -> Result<Self, PhyCalibrationTrackingChildError> {
        let PhyCalibrationTrackingAction::RestoreChipChannel { channel, cbw } = parent_action
        else {
            return Err(PhyCalibrationTrackingChildError::UnsupportedAction);
        };
        Ok(Self {
            child: crate::phy_channel::PhyChipChannelTransition::new(
                crate::phy_channel::PhyChipChannelRequest {
                    channel_or_frequency: channel,
                    cbw,
                    parameters,
                },
            ),
        })
    }

    pub const fn action(&self) -> crate::phy_channel::PhyChipChannelAction {
        self.child.action()
    }

    pub fn advance(
        &mut self,
        completion: crate::phy_channel::PhyChipChannelCompletion,
    ) -> Result<(), crate::phy_channel::PhyChipChannelTransitionError> {
        self.child.advance(completion)
    }

    pub fn lower_external(
        &self,
    ) -> Result<
        crate::phy_channel::PhyChipChannelExternalBinding,
        crate::phy_channel::PhyChipChannelExternalBindingError,
    > {
        crate::phy_channel::PhyChipChannelExternalBinding::lower(self.action())
    }

    #[expect(
        clippy::result_large_err,
        reason = "the pending variant must return the allocation-free complete channel owner"
    )]
    pub fn commit(self) -> Result<PhyCalibrationTrackingCompletion, Self> {
        let result = match self.child.action() {
            crate::phy_channel::PhyChipChannelAction::Complete(outcome) => Ok(outcome),
            crate::phy_channel::PhyChipChannelAction::Failed(failure) => Err(failure),
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
        parameters: crate::phy_rx_gain::PhyRxGainInitParameters,
    ) -> Result<Self, PhyCalibrationTrackingChildError> {
        if parent_action != PhyCalibrationTrackingAction::RecalibrateRxGain {
            return Err(PhyCalibrationTrackingChildError::UnsupportedAction);
        }
        Ok(Self {
            child: crate::phy_rx_gain::PhyRxGainInitTransition::new(parameters),
        })
    }

    pub fn action(&self) -> crate::phy_rx_gain::PhyRxGainInitAction {
        self.child.action()
    }

    pub fn advance(
        &mut self,
        completion: crate::phy_rx_gain::PhyRxGainInitCompletion,
    ) -> Result<(), crate::phy_rx_gain::PhyRxGainInitTransitionError> {
        self.child.advance(completion)
    }

    pub fn lower_external(
        &self,
    ) -> Result<
        crate::phy_rx_gain::PhyRxGainInitExternalBinding,
        crate::phy_rx_gain::PhyRxGainExternalBindingError,
    > {
        crate::phy_rx_gain::PhyRxGainInitExternalBinding::lower(self.action())
    }

    #[expect(
        clippy::result_large_err,
        reason = "the pending variant must return the non-allocating linear table owner"
    )]
    pub fn commit(self) -> Result<PhyCalibrationTrackingCompletion, Self> {
        let result = match self.child.action() {
            crate::phy_rx_gain::PhyRxGainInitAction::Complete(outcome) => Ok(outcome),
            crate::phy_rx_gain::PhyRxGainInitAction::Failed(failure) => Err(failure),
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
            child: crate::phy_dcode::PhyDcodeTransition::new(
                crate::phy_dcode::PhyDcodeParameters { crystal_selector },
            ),
        })
    }

    pub const fn action(&self) -> crate::phy_dcode::PhyDcodeAction {
        self.child.action()
    }

    pub fn advance(
        &mut self,
        completion: crate::phy_dcode::PhyDcodeCompletion,
    ) -> Result<(), crate::phy_dcode::PhyDcodeTransitionError> {
        self.child.advance(completion)
    }

    pub fn lower_external(
        &self,
    ) -> Result<crate::phy_dcode::PhyDcodeExternalBinding, crate::phy_dcode::PhyDcodeBindingError>
    {
        crate::phy_dcode::PhyDcodeExternalBinding::lower(self.action())
    }

    pub fn commit(self) -> Result<PhyCalibrationTrackingCompletion, Self> {
        let result = match self.child.action() {
            crate::phy_dcode::PhyDcodeAction::Complete(outcome) => Ok(outcome),
            crate::phy_dcode::PhyDcodeAction::Failed(failure) => Err(failure),
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
            child: crate::phy_pbus::PhyPbusClearTransition::new(),
        })
    }

    pub const fn action(&self) -> crate::phy_pbus::PhyPbusClearAction {
        self.child.action()
    }

    pub fn advance(
        &mut self,
        completion: crate::phy_pbus::PhyPbusClearCompletion,
    ) -> Result<(), crate::phy_pbus::PhyPbusClearTransitionError> {
        self.child.advance(completion)
    }

    pub fn advance_external(
        &mut self,
        completion: crate::phy_i2c::PhyRfInitPrefixCompletion,
    ) -> Result<(), crate::phy_pbus::PhyPbusClearTransitionError> {
        let crate::phy_i2c::PhyRfInitPrefixCompletion::PbusClear(completion) = completion else {
            return Err(crate::phy_pbus::PhyPbusClearTransitionError::WrongCompletion);
        };
        self.advance(completion)
    }

    pub fn lower_external(
        &self,
    ) -> Result<crate::phy_cold::PhyColdExternalBinding, crate::phy_cold::PhyColdLoweringError>
    {
        crate::phy_cold::PhyColdExternalBinding::lower(
            crate::phy_i2c::PhyRfInitPrefixAction::PbusClear(self.action()),
        )
    }

    pub fn commit(self) -> Result<PhyCalibrationTrackingCompletion, Self> {
        let crate::phy_pbus::PhyPbusClearAction::Complete(outcome) = self.child.action() else {
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
            child: crate::phy_pbus::PhyForceTxRxTransition::new(enabled),
        })
    }

    pub const fn parent_action(&self) -> PhyCalibrationTrackingAction {
        PhyCalibrationTrackingAction::ForceTxRxOff {
            enabled: self.enabled,
        }
    }

    pub const fn action(&self) -> crate::phy_pbus::PhyForceTxRxAction {
        self.child.action()
    }

    pub fn advance(
        &mut self,
        completion: crate::phy_pbus::PhyForceTxRxCompletion,
    ) -> Result<(), crate::phy_pbus::PhyForceTxRxTransitionError> {
        self.child.advance(completion)
    }

    pub fn lower_external(
        &self,
    ) -> Result<
        crate::phy_pbus::PhyForceTxRxExternalBinding,
        crate::phy_pbus::PhyForceTxRxBindingError,
    > {
        crate::phy_pbus::PhyForceTxRxExternalBinding::lower(self.action())
    }

    pub fn commit(self) -> Result<PhyCalibrationTrackingCompletion, Self> {
        let crate::phy_pbus::PhyForceTxRxAction::Complete { enabled } = self.child.action() else {
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
    crate::phy_math::absolute_temperature((reference as i32).wrapping_sub(current as i32))
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

/// Non-cloneable owner of the mixed platform/register `phy_mac_enable_bb`
/// edge.
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
    pub fn execute_target<
        P: open_esp_radio_esp32s31_hal::wifi_bb::PhyWifiBbControl,
        R: open_esp_radio_esp32s31_hal::PhyInitializationAccess,
    >(
        self,
        platform: &mut P,
        registers: &mut R,
    ) -> PhyCalibrationTrackingCompletion {
        open_esp_radio_esp32s31_hal::phy_frequency::enable_mac_baseband(platform, registers);
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
mod tests {
    use super::*;
    use std::vec::Vec;

    const PARAMETERS: PhyCalibrationTrackingParameters = PhyCalibrationTrackingParameters {
        current_temperature: 50,
        common_reference_temperature: 20,
        wifi_reference_temperature: 20,
        bluetooth_ieee802154_reference_temperature: 20,
        threshold_override: None,
        current_channel: 11,
        channel_bandwidth: 1,
        crystal_selector: 0x31,
    };

    const RX_GAIN_OUTCOME: crate::phy_rx_gain::PhyRxGainInitOutcome =
        crate::phy_rx_gain::PhyRxGainInitOutcome {
            dc: Some(crate::phy_rx_gain_cal::PhyRxGainDcOutcome {
                wifi_index_dc: [[0; 2]; 8],
                wifi_dc_base: [0; 2],
                shared_index_dc: [[0; 2]; 11],
                rxbb_dc_adjustments: [[0; 2]; 6],
            }),
            generated_tables: true,
            wifi_last_index: 69,
            shared_last_index: 75,
        };

    const fn channel_outcome(
        channel: u16,
        cbw: u8,
        temperature: i16,
    ) -> crate::phy_channel::PhyChipChannelOutcome {
        crate::phy_channel::PhyChipChannelOutcome {
            channel,
            frequency_mhz: 2_407 + channel * 5,
            cbw,
            init_complete: cbw != 0,
            temperature: crate::phy_temperature::PhyTemperatureOutcome {
                temperature,
                sensor_index: 2,
                next_dac: 15,
            },
        }
    }

    const fn tx_dc_pwdet_outcome(
        class: PhyCalibrationTrackClass,
    ) -> crate::phy_txdc_pwdet::PhyTxDcPwdetOutcome {
        let value = match class {
            PhyCalibrationTrackClass::Wifi => 0x111,
            PhyCalibrationTrackClass::BluetoothIeee802154 => 0x222,
        };
        crate::phy_txdc_pwdet::PhyTxDcPwdetOutcome {
            dco: [[value; 4]; 3],
            total_measurements: 144,
        }
    }

    fn completion(action: PhyCalibrationTrackingAction) -> PhyCalibrationTrackingCompletion {
        match action {
            PhyCalibrationTrackingAction::ClearPbus => {
                PhyCalibrationTrackingCompletion::PbusClearCompleted(
                    PhyCalibrationPbusClearCompletion {
                        outcome: crate::phy_pbus::PhyPbusClearOutcome::Cleared,
                    },
                )
            }
            PhyCalibrationTrackingAction::CalibrateDcode => {
                PhyCalibrationTrackingCompletion::DcodeCompleted(PhyCalibrationDcodeCompletion {
                    result: Ok(crate::phy_dcode::PhyDcodeOutcome { codes: [7; 8] }),
                })
            }
            PhyCalibrationTrackingAction::RecalibrateRxGain => {
                PhyCalibrationTrackingCompletion::RxGainRecalibrated(
                    PhyCalibrationRxGainCompletion {
                        result: Ok(RX_GAIN_OUTCOME),
                    },
                )
            }
            PhyCalibrationTrackingAction::RestoreChipChannel { channel, cbw } => {
                PhyCalibrationTrackingCompletion::ChipChannelRestored(
                    PhyCalibrationChannelCompletion {
                        result: Ok(channel_outcome(
                            channel,
                            cbw,
                            PARAMETERS.current_temperature,
                        )),
                    },
                )
            }
            PhyCalibrationTrackingAction::SetHardwareFrequencyControl { enabled } => {
                PhyCalibrationTrackingCompletion::HardwareFrequencyControlSet { enabled }
            }
            PhyCalibrationTrackingAction::ForceTxRxOff { enabled } => {
                PhyCalibrationTrackingCompletion::ForceTxRxCompleted(
                    PhyCalibrationForceTxRxCompletion { enabled },
                )
            }
            PhyCalibrationTrackingAction::ConfigureBasebandChannel { cbw } => {
                PhyCalibrationTrackingCompletion::BasebandChannelConfigured { cbw }
            }
            PhyCalibrationTrackingAction::CalibrateTxDcPwdet { class } => {
                PhyCalibrationTrackingCompletion::TxDcPwdetCalibrated(
                    PhyCalibrationTxDcPwdetCompletion {
                        class,
                        result: Ok(tx_dc_pwdet_outcome(class)),
                    },
                )
            }
            PhyCalibrationTrackingAction::PublishWifiTxGain { channel } => {
                PhyCalibrationTrackingCompletion::WifiTxGainPublished { channel }
            }
            PhyCalibrationTrackingAction::PublishBluetoothIeee802154TxGain => {
                PhyCalibrationTrackingCompletion::BluetoothIeee802154TxGainPublished
            }
            PhyCalibrationTrackingAction::EnableMacBaseband => {
                PhyCalibrationTrackingCompletion::MacBasebandEnabled
            }
            PhyCalibrationTrackingAction::RestoreTxGainCompensation => {
                PhyCalibrationTrackingCompletion::TxGainCompensationRestored
            }
            PhyCalibrationTrackingAction::Complete(_) | PhyCalibrationTrackingAction::Failed(_) => {
                panic!("terminal action")
            }
        }
    }

    fn run(
        request: PhyCalibrationTrackingRequest,
        parameters: PhyCalibrationTrackingParameters,
    ) -> Vec<PhyCalibrationTrackingAction> {
        let mut transition = PhyCalibrationTrackingTransition::new(request, parameters);
        let mut actions = Vec::new();
        for _ in 0..24 {
            let action = transition.action();
            actions.push(action);
            if matches!(
                action,
                PhyCalibrationTrackingAction::Complete(_) | PhyCalibrationTrackingAction::Failed(_)
            ) {
                return actions;
            }
            transition.advance(completion(action)).unwrap();
        }
        panic!("calibration tracking exceeded its finite path")
    }

    #[test]
    fn wifi_inclusive_threshold_runs_common_then_wifi_and_restores_every_guard() {
        let actions = run(
            PhyCalibrationTrackingRequest {
                class: PhyCalibrationTrackClass::Wifi,
            },
            PARAMETERS,
        );
        assert_eq!(actions[0], PhyCalibrationTrackingAction::ClearPbus);
        assert_eq!(actions[1], PhyCalibrationTrackingAction::CalibrateDcode);
        assert!(
            actions.contains(&PhyCalibrationTrackingAction::RestoreChipChannel {
                channel: 11,
                cbw: 1,
            })
        );
        assert!(
            actions.contains(&PhyCalibrationTrackingAction::CalibrateTxDcPwdet {
                class: PhyCalibrationTrackClass::Wifi,
            })
        );
        assert_eq!(
            actions[actions.len() - 2],
            PhyCalibrationTrackingAction::RestoreTxGainCompensation
        );
        let PhyCalibrationTrackingAction::Complete(outcome) = actions[actions.len() - 1] else {
            panic!("missing terminal outcome")
        };
        assert!(outcome.common_updated);
        assert!(outcome.class_updated);
        assert_eq!(
            outcome.dcode,
            Some(crate::phy_dcode::PhyDcodeOutcome { codes: [7; 8] })
        );
        assert_eq!(outcome.rx_gain, Some(RX_GAIN_OUTCOME));
        assert_eq!(outcome.channel, Some(channel_outcome(11, 1, 50)));
        assert_eq!(
            outcome.tx_dc_pwdet,
            Some(tx_dc_pwdet_outcome(PhyCalibrationTrackClass::Wifi))
        );
        assert_eq!(outcome.common_reference_temperature, 50);
        assert_eq!(outcome.wifi_reference_temperature, 50);
        assert_eq!(outcome.bluetooth_ieee802154_reference_temperature, 20);
    }

    #[test]
    fn bluetooth_class_uses_its_own_reference_and_skips_common_when_below_threshold() {
        let actions = run(
            PhyCalibrationTrackingRequest {
                class: PhyCalibrationTrackClass::BluetoothIeee802154,
            },
            PhyCalibrationTrackingParameters {
                common_reference_temperature: 50,
                wifi_reference_temperature: 0,
                bluetooth_ieee802154_reference_temperature: 20,
                ..PARAMETERS
            },
        );
        assert_eq!(
            actions[0],
            PhyCalibrationTrackingAction::SetHardwareFrequencyControl { enabled: false }
        );
        assert!(!actions.contains(&PhyCalibrationTrackingAction::CalibrateDcode));
        assert!(actions.contains(&PhyCalibrationTrackingAction::PublishBluetoothIeee802154TxGain));
    }

    #[test]
    fn override_below_delta_skips_all_calibration_but_never_skips_final_restore() {
        let actions = run(
            PhyCalibrationTrackingRequest {
                class: PhyCalibrationTrackClass::Wifi,
            },
            PhyCalibrationTrackingParameters {
                threshold_override: Some(31),
                ..PARAMETERS
            },
        );
        assert_eq!(
            actions,
            [
                PhyCalibrationTrackingAction::RestoreTxGainCompensation,
                PhyCalibrationTrackingAction::Complete(PhyCalibrationTrackingOutcome {
                    class: PhyCalibrationTrackClass::Wifi,
                    threshold: 31,
                    common_reference_temperature: 20,
                    wifi_reference_temperature: 20,
                    bluetooth_ieee802154_reference_temperature: 20,
                    common_updated: false,
                    class_updated: false,
                    dcode: None,
                    rx_gain: None,
                    channel: None,
                    tx_dc_pwdet: None,
                }),
            ]
        );
    }

    #[test]
    fn wrong_completion_preserves_action_and_terminal_rejects_more_work() {
        let mut transition = PhyCalibrationTrackingTransition::new(
            PhyCalibrationTrackingRequest {
                class: PhyCalibrationTrackClass::Wifi,
            },
            PARAMETERS,
        );
        assert_eq!(
            transition.advance(PhyCalibrationTrackingCompletion::MacBasebandEnabled),
            Err(PhyCalibrationTrackingTransitionError::WrongCompletion)
        );
        assert_eq!(transition.action(), PhyCalibrationTrackingAction::ClearPbus);
        while !matches!(
            transition.action(),
            PhyCalibrationTrackingAction::Complete(_)
        ) {
            transition.advance(completion(transition.action())).unwrap();
        }
        assert_eq!(
            transition.advance(PhyCalibrationTrackingCompletion::TxGainCompensationRestored),
            Err(PhyCalibrationTrackingTransitionError::AlreadyComplete)
        );
    }

    #[test]
    fn external_lowering_owns_only_complete_direct_hardware_leaves() {
        let direct = [
            PhyCalibrationTrackingAction::SetHardwareFrequencyControl { enabled: false },
            PhyCalibrationTrackingAction::SetHardwareFrequencyControl { enabled: true },
            PhyCalibrationTrackingAction::ConfigureBasebandChannel { cbw: 0 },
            PhyCalibrationTrackingAction::ConfigureBasebandChannel { cbw: 0x13 },
            PhyCalibrationTrackingAction::EnableMacBaseband,
            PhyCalibrationTrackingAction::RestoreTxGainCompensation,
        ];
        for action in direct {
            let binding = PhyCalibrationTrackingExternalBinding::lower(action).unwrap();
            let lowered = match &binding {
                PhyCalibrationTrackingExternalBinding::Register(binding) => binding.action(),
                PhyCalibrationTrackingExternalBinding::MacBaseband(binding) => binding.action(),
            };
            assert_eq!(lowered, action);
        }

        let unresolved = [
            PhyCalibrationTrackingAction::ClearPbus,
            PhyCalibrationTrackingAction::CalibrateDcode,
            PhyCalibrationTrackingAction::RecalibrateRxGain,
            PhyCalibrationTrackingAction::RestoreChipChannel {
                channel: 11,
                cbw: 1,
            },
            PhyCalibrationTrackingAction::ForceTxRxOff { enabled: true },
            PhyCalibrationTrackingAction::CalibrateTxDcPwdet {
                class: PhyCalibrationTrackClass::Wifi,
            },
            PhyCalibrationTrackingAction::PublishWifiTxGain { channel: 11 },
            PhyCalibrationTrackingAction::PublishBluetoothIeee802154TxGain,
            PhyCalibrationTrackingAction::Complete(PhyCalibrationTrackingOutcome {
                class: PhyCalibrationTrackClass::Wifi,
                threshold: 30,
                common_reference_temperature: 20,
                wifi_reference_temperature: 20,
                bluetooth_ieee802154_reference_temperature: 20,
                common_updated: false,
                class_updated: false,
                dcode: None,
                rx_gain: None,
                channel: None,
                tx_dc_pwdet: None,
            }),
            PhyCalibrationTrackingAction::Failed(PhyCalibrationTrackingFailure::PbusClearTimedOut(
                crate::phy_pbus::PhyPbusForceTest::new(4, 1, 0),
            )),
        ];
        for action in unresolved {
            assert_eq!(
                PhyCalibrationTrackingExternalBinding::lower(action),
                Err(PhyCalibrationTrackingBindingError::UnsupportedAction)
            );
        }
    }

    #[test]
    fn force_txrx_parent_proof_requires_both_writes_and_timer_edges() {
        let mut transition = PhyCalibrationTrackingTransition::new(
            PhyCalibrationTrackingRequest {
                class: PhyCalibrationTrackClass::BluetoothIeee802154,
            },
            PhyCalibrationTrackingParameters {
                common_reference_temperature: 50,
                wifi_reference_temperature: 0,
                bluetooth_ieee802154_reference_temperature: 20,
                ..PARAMETERS
            },
        );
        transition
            .advance(
                PhyCalibrationTrackingCompletion::HardwareFrequencyControlSet { enabled: false },
            )
            .unwrap();

        let child = transition.begin_force_txrx().unwrap();
        assert_eq!(child.parent_action(), transition.action());
        let mut child = child.commit().unwrap_err();
        loop {
            let completion = match child.lower_external() {
                Ok(crate::phy_pbus::PhyForceTxRxExternalBinding::Mmio(binding)) => {
                    let crate::phy_pbus::PhyForceTxRxAction::Configure { enabled, phase } =
                        binding.action()
                    else {
                        panic!("force MMIO binding lost its identity")
                    };
                    crate::phy_pbus::PhyForceTxRxCompletion::Configured { enabled, phase }
                }
                Ok(crate::phy_pbus::PhyForceTxRxExternalBinding::Timer(binding)) => {
                    assert_eq!(binding.micros(), 1);
                    binding.into_completion()
                }
                Err(_) => break,
            };
            child.advance(completion).unwrap();
        }
        let completion = child.commit().unwrap();
        transition.advance(completion).unwrap();
        assert_eq!(transition.action(), PhyCalibrationTrackingAction::ClearPbus);
    }

    #[test]
    fn dcode_parent_proof_starts_the_existing_complete_hardware_graph() {
        let mut transition = PhyCalibrationTrackingTransition::new(
            PhyCalibrationTrackingRequest {
                class: PhyCalibrationTrackClass::Wifi,
            },
            PARAMETERS,
        );
        transition
            .advance(completion(PhyCalibrationTrackingAction::ClearPbus))
            .unwrap();

        let child = transition.begin_dcode().unwrap();
        assert!(matches!(
            child.action(),
            crate::phy_dcode::PhyDcodeAction::Rfpll(_)
        ));
        assert!(matches!(
            child.lower_external(),
            Ok(crate::phy_dcode::PhyDcodeExternalBinding::Rfpll(_))
        ));
        assert!(child.commit().is_err());
    }

    #[test]
    fn dcode_failure_restores_gain_without_publishing_common_progress() {
        let mut transition = PhyCalibrationTrackingTransition::new(
            PhyCalibrationTrackingRequest {
                class: PhyCalibrationTrackClass::Wifi,
            },
            PARAMETERS,
        );
        transition
            .advance(completion(PhyCalibrationTrackingAction::ClearPbus))
            .unwrap();
        let failure = crate::phy_dcode::PhyDcodeFailure::Rfpll {
            calibration_index: 0,
            failure: crate::phy_rfpll::RfpllFrequencyFailure::FrequencyReadyDeadlineExceeded {
                samples: 100,
            },
        };
        transition
            .advance(PhyCalibrationTrackingCompletion::DcodeCompleted(
                PhyCalibrationDcodeCompletion {
                    result: Err(failure),
                },
            ))
            .unwrap();
        assert_eq!(
            transition.action(),
            PhyCalibrationTrackingAction::RestoreTxGainCompensation
        );
        transition
            .advance(PhyCalibrationTrackingCompletion::TxGainCompensationRestored)
            .unwrap();
        assert_eq!(
            transition.action(),
            PhyCalibrationTrackingAction::Failed(PhyCalibrationTrackingFailure::Dcode(failure))
        );
    }

    #[test]
    fn rx_gain_parent_proof_forces_dc_and_tables_through_complete_hardware_graph() {
        let mut transition = PhyCalibrationTrackingTransition::new(
            PhyCalibrationTrackingRequest {
                class: PhyCalibrationTrackClass::Wifi,
            },
            PARAMETERS,
        );
        for action in [
            PhyCalibrationTrackingAction::ClearPbus,
            PhyCalibrationTrackingAction::CalibrateDcode,
        ] {
            assert_eq!(transition.action(), action);
            transition.advance(completion(action)).unwrap();
        }

        let child = transition
            .begin_rx_gain_recalibration(crate::phy_rx_gain::PhyRxGainInitParameters {
                dc_calibrated: true,
                tables_initialized: true,
                dc: crate::phy_rx_gain_cal::PhyRxGainDcParameters {
                    crystal_selector: 0x31,
                    pbus_rx_path_value: 3,
                    rx_saturation_detected: false,
                },
                memory: crate::phy_bb::PhyRxGainMemoryParameters {
                    parameter_002: 3,
                    wifi_index_dc: [[0; 2]; 8],
                    wifi_dc_base: [0; 2],
                    shared_index_dc: [[0; 2]; 11],
                    rxbb_dc_adjustments: [[0; 2]; 6],
                    wifi_auxiliary: 0,
                },
            })
            .unwrap();
        assert_eq!(
            child.action(),
            crate::phy_rx_gain::PhyRxGainInitAction::CaptureAndClearDcControl
        );
        assert!(matches!(
            child.lower_external(),
            Ok(crate::phy_rx_gain::PhyRxGainInitExternalBinding::Mmio(_))
        ));
        assert!(child.commit().is_err());
    }

    #[test]
    fn channel_parent_proof_starts_the_complete_async_hardware_graph() {
        let mut transition = PhyCalibrationTrackingTransition::new(
            PhyCalibrationTrackingRequest {
                class: PhyCalibrationTrackClass::Wifi,
            },
            PARAMETERS,
        );
        for action in [
            PhyCalibrationTrackingAction::ClearPbus,
            PhyCalibrationTrackingAction::CalibrateDcode,
            PhyCalibrationTrackingAction::RecalibrateRxGain,
        ] {
            assert_eq!(transition.action(), action);
            transition.advance(completion(action)).unwrap();
        }

        let state = crate::phy_state::PhyState::new(crate::phy_state::PhyConfig::production());
        let child = transition
            .begin_channel_restore(state.channel_parameters())
            .unwrap();
        assert_eq!(
            child.action(),
            crate::phy_channel::PhyChipChannelAction::SetAgc { enabled: false }
        );
        assert!(matches!(
            child.lower_external(),
            Ok(crate::phy_channel::PhyChipChannelExternalBinding::Mmio(_))
        ));
        assert!(child.commit().is_err());
    }

    #[test]
    fn restored_channel_temperature_drives_following_class_threshold_and_references() {
        let mut transition = PhyCalibrationTrackingTransition::new(
            PhyCalibrationTrackingRequest {
                class: PhyCalibrationTrackClass::Wifi,
            },
            PhyCalibrationTrackingParameters {
                wifi_reference_temperature: 30,
                ..PARAMETERS
            },
        );
        for action in [
            PhyCalibrationTrackingAction::ClearPbus,
            PhyCalibrationTrackingAction::CalibrateDcode,
            PhyCalibrationTrackingAction::RecalibrateRxGain,
        ] {
            transition.advance(completion(action)).unwrap();
        }
        transition
            .advance(PhyCalibrationTrackingCompletion::ChipChannelRestored(
                PhyCalibrationChannelCompletion {
                    result: Ok(channel_outcome(11, 1, 60)),
                },
            ))
            .unwrap();
        transition
            .advance(PhyCalibrationTrackingCompletion::MacBasebandEnabled)
            .unwrap();
        assert_eq!(
            transition.action(),
            PhyCalibrationTrackingAction::SetHardwareFrequencyControl { enabled: false }
        );

        loop {
            let action = transition.action();
            if let PhyCalibrationTrackingAction::Complete(outcome) = action {
                assert_eq!(outcome.common_reference_temperature, 60);
                assert_eq!(outcome.wifi_reference_temperature, 60);
                assert_eq!(outcome.channel, Some(channel_outcome(11, 1, 60)));
                break;
            }
            transition.advance(completion(action)).unwrap();
        }
    }

    #[test]
    fn class_tx_dc_pwdet_parent_selects_both_complete_hardware_graphs() {
        for class in [
            PhyCalibrationTrackClass::Wifi,
            PhyCalibrationTrackClass::BluetoothIeee802154,
        ] {
            let mut transition = PhyCalibrationTrackingTransition::new(
                PhyCalibrationTrackingRequest { class },
                PhyCalibrationTrackingParameters {
                    common_reference_temperature: 50,
                    ..PARAMETERS
                },
            );
            while !matches!(
                transition.action(),
                PhyCalibrationTrackingAction::CalibrateTxDcPwdet { .. }
            ) {
                let action = transition.action();
                transition.advance(completion(action)).unwrap();
            }

            let state = crate::phy_state::PhyState::new(crate::phy_state::PhyConfig::production());
            let child = transition
                .begin_tx_dc_pwdet(
                    state.tx_dc_pwdet_parameters(),
                    state.bluetooth_tx_dc_pwdet_transition(),
                )
                .unwrap();
            assert_eq!(child.class(), class);
            assert_eq!(
                child.action(),
                crate::phy_txdc_pwdet::PhyTxDcPwdetAction::CaptureRegisters
            );
            assert!(matches!(
                child.lower_external(),
                Ok(crate::phy_txdc_pwdet::PhyTxDcPwdetExternalBinding::Mmio(_))
            ));
            assert!(child.commit().is_err());
        }
    }

    #[test]
    fn tx_dc_pwdet_failure_runs_outer_force_frequency_and_gain_cleanup() {
        let mut transition = PhyCalibrationTrackingTransition::new(
            PhyCalibrationTrackingRequest {
                class: PhyCalibrationTrackClass::BluetoothIeee802154,
            },
            PhyCalibrationTrackingParameters {
                common_reference_temperature: 50,
                ..PARAMETERS
            },
        );
        while !matches!(
            transition.action(),
            PhyCalibrationTrackingAction::CalibrateTxDcPwdet { .. }
        ) {
            let action = transition.action();
            transition.advance(completion(action)).unwrap();
        }
        let failure = crate::phy_txdc_pwdet::PhyTxDcPwdetFailure::PbusTimedOut(
            crate::phy_pbus::PhyPbusForceTest::new(4, 1, 0),
        );
        transition
            .advance(PhyCalibrationTrackingCompletion::TxDcPwdetCalibrated(
                PhyCalibrationTxDcPwdetCompletion {
                    class: PhyCalibrationTrackClass::BluetoothIeee802154,
                    result: Err(failure),
                },
            ))
            .unwrap();
        for expected in [
            PhyCalibrationTrackingAction::ForceTxRxOff { enabled: false },
            PhyCalibrationTrackingAction::SetHardwareFrequencyControl { enabled: true },
            PhyCalibrationTrackingAction::RestoreTxGainCompensation,
        ] {
            assert_eq!(transition.action(), expected);
            transition.advance(completion(expected)).unwrap();
        }
        assert_eq!(
            transition.action(),
            PhyCalibrationTrackingAction::Failed(PhyCalibrationTrackingFailure::TxDcPwdet(failure))
        );
    }

    #[test]
    fn rx_gain_failure_restores_gain_without_publishing_common_progress() {
        let mut transition = PhyCalibrationTrackingTransition::new(
            PhyCalibrationTrackingRequest {
                class: PhyCalibrationTrackClass::Wifi,
            },
            PARAMETERS,
        );
        for action in [
            PhyCalibrationTrackingAction::ClearPbus,
            PhyCalibrationTrackingAction::CalibrateDcode,
        ] {
            transition.advance(completion(action)).unwrap();
        }
        let failure = crate::phy_rx_gain::PhyRxGainInitFailure::Publish(
            crate::phy_rx_gain::PhyRxGainPublishFailure::PbusTimedOut {
                bank: crate::phy_bb::PhyRxGainBank::Wifi,
                transaction: crate::phy_pbus::PhyPbusForceTest::new(4, 1, 0),
            },
        );
        transition
            .advance(PhyCalibrationTrackingCompletion::RxGainRecalibrated(
                PhyCalibrationRxGainCompletion {
                    result: Err(failure),
                },
            ))
            .unwrap();
        assert_eq!(
            transition.action(),
            PhyCalibrationTrackingAction::RestoreTxGainCompensation
        );
        transition
            .advance(PhyCalibrationTrackingCompletion::TxGainCompensationRestored)
            .unwrap();
        assert_eq!(
            transition.action(),
            PhyCalibrationTrackingAction::Failed(PhyCalibrationTrackingFailure::RxGain(failure))
        );
    }

    #[test]
    fn class_pbus_timeout_restores_force_frequency_and_gain_before_failure() {
        let mut transition = PhyCalibrationTrackingTransition::new(
            PhyCalibrationTrackingRequest {
                class: PhyCalibrationTrackClass::BluetoothIeee802154,
            },
            PhyCalibrationTrackingParameters {
                common_reference_temperature: 50,
                wifi_reference_temperature: 0,
                bluetooth_ieee802154_reference_temperature: 20,
                ..PARAMETERS
            },
        );
        transition
            .advance(
                PhyCalibrationTrackingCompletion::HardwareFrequencyControlSet { enabled: false },
            )
            .unwrap();
        transition
            .advance(PhyCalibrationTrackingCompletion::ForceTxRxCompleted(
                PhyCalibrationForceTxRxCompletion { enabled: true },
            ))
            .unwrap();

        let child = transition.begin_pbus_clear().unwrap();
        let mut child = child.commit().unwrap_err();
        let crate::phy_cold::PhyColdExternalBinding::Mmio(binding) =
            child.lower_external().unwrap()
        else {
            panic!("PBus clear must begin with debug-mode MMIO")
        };
        child
            .advance_external(binding.into_completion().unwrap())
            .unwrap();

        let crate::phy_pbus::PhyPbusClearAction::ForceTest(transaction) = child.action() else {
            panic!("PBus clear did not publish its first transaction")
        };
        let crate::phy_cold::PhyColdExternalBinding::Pbus(mut binding) =
            child.lower_external().unwrap()
        else {
            panic!("force-test action did not lower to a PBus owner")
        };
        binding.started().unwrap();
        child
            .advance_external(binding.into_timeout_completion().unwrap())
            .unwrap();
        let completion = child.commit().unwrap();
        transition.advance(completion).unwrap();

        assert_eq!(
            transition.action(),
            PhyCalibrationTrackingAction::ForceTxRxOff { enabled: false }
        );
        transition
            .advance(PhyCalibrationTrackingCompletion::ForceTxRxCompleted(
                PhyCalibrationForceTxRxCompletion { enabled: false },
            ))
            .unwrap();
        assert_eq!(
            transition.action(),
            PhyCalibrationTrackingAction::SetHardwareFrequencyControl { enabled: true }
        );
        transition
            .advance(
                PhyCalibrationTrackingCompletion::HardwareFrequencyControlSet { enabled: true },
            )
            .unwrap();
        assert_eq!(
            transition.action(),
            PhyCalibrationTrackingAction::RestoreTxGainCompensation
        );
        transition
            .advance(PhyCalibrationTrackingCompletion::TxGainCompensationRestored)
            .unwrap();
        assert_eq!(
            transition.action(),
            PhyCalibrationTrackingAction::Failed(PhyCalibrationTrackingFailure::PbusClearTimedOut(
                transaction
            ))
        );
        assert_eq!(
            transition.advance(PhyCalibrationTrackingCompletion::TxGainCompensationRestored),
            Err(PhyCalibrationTrackingTransitionError::AlreadyComplete)
        );
    }
}
