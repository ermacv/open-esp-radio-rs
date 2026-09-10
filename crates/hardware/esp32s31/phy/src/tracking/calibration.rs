//! Runtime RX calibration followed by one shared TX calibration transaction.
//!
//! The current S31 vendor graph has one RX reference and one shared TX
//! reference. Wi-Fi and BT/154 TX children run in that order inside the same
//! frequency/gain envelope. References are published only after terminal
//! restoration; grant arbitration remains the physical radio owner's job.

use crate::tracking::parameters::PhyCalibrationTrackClass;

pub mod decision;

/// Which measurement branch is selected; each branch retains its complete
/// hardware restoration sequence before returning maintenance access.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Scope {
    Both,
    Common,
    Transmit,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyCalibrationTrackingParameters {
    pub current_temperature: i16,
    pub common_reference_temperature: i16,
    pub transmit_reference_temperature: i16,
    pub threshold_override: Option<u8>,
    pub current_channel: u16,
    pub channel_bandwidth: u8,
    pub crystal_selector: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyCalibrationTrackingRequest {
    pub clients: super::parameters::PhyParamTrackRequest,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyCalibrationTrackingOutcome {
    pub clients: super::parameters::PhyParamTrackRequest,
    pub threshold: u8,
    pub common_reference_temperature: i16,
    pub transmit_reference_temperature: i16,
    pub common_updated: bool,
    pub transmit_updated: bool,
    pub dcode: Option<crate::analog::dcode::PhyDcodeOutcome>,
    pub rx_gain: Option<crate::rx::gain::PhyRxGainInitOutcome>,
    pub channel: Option<crate::channel::PhyChipChannelOutcome>,
    pub wifi_tx_dc_pwdet: Option<crate::tx::dc_power_detector::PhyTxDcPwdetOutcome>,
    pub bluetooth_ieee802154_tx_dc_pwdet: Option<crate::tx::dc_power_detector::PhyTxDcPwdetOutcome>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyCalibrationTrackingAction {
    ClearPbus,
    CalibrateDcode,
    RecalibrateRxGain,
    RestoreChipChannel { channel: u16, cbw: u8 },
    SetHardwareFrequencyControl { enabled: bool },
    AwaitSoftwareFrequencySettle,
    SetForcedDigitalGain { enabled: bool },
    ForceTxRxOff { enabled: bool },
    ConfigureBasebandChannel { cbw: u8 },
    CalibrateTxDcPwdet { class: PhyCalibrationTrackClass },
    PublishWifiTxGain { channel: u16 },
    PublishBluetoothIeee802154TxGain,
    DisableWifiBaseband,
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
    SoftwareFrequencySettled,
    ForcedDigitalGainSet { enabled: bool },
    ForceTxRxCompleted(PhyCalibrationForceTxRxCompletion),
    BasebandChannelConfigured { cbw: u8 },
    TxDcPwdetCalibrated(PhyCalibrationTxDcPwdetCompletion),
    TxGainPublished(PhyCalibrationTxGainCompletion),
    WifiBasebandDisabled,
    MacBasebandEnabled,
    TxGainCompensationRestored,
}

/// Opaque proof that both force-mode writes and both timer edges completed.
///
/// ```compile_fail
/// use oer_esp32s31_phy::tracking::calibration::{
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
/// use oer_esp32s31_phy::tracking::calibration::{
///     PhyCalibrationPbusClearCompletion, PhyCalibrationTrackingCompletion,
/// };
/// use oer_esp32s31_phy::analog::pbus::PhyPbusClearOutcome;
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
/// use oer_esp32s31_phy::tracking::calibration::{
///     PhyCalibrationDcodeCompletion, PhyCalibrationTrackingCompletion,
/// };
///
/// let forged = PhyCalibrationTrackingCompletion::DcodeCompleted(
///     PhyCalibrationDcodeCompletion {
///         result: Ok(oer_esp32s31_phy::analog::dcode::PhyDcodeOutcome {
///             codes: [0; 8],
///         }),
///     },
/// );
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyCalibrationDcodeCompletion {
    result: Result<crate::analog::dcode::PhyDcodeOutcome, crate::analog::dcode::PhyDcodeFailure>,
}

impl PhyCalibrationDcodeCompletion {
    /// Inspect measured codes or terminal failure without publishing parent state.
    pub const fn result(
        &self,
    ) -> Result<crate::analog::dcode::PhyDcodeOutcome, crate::analog::dcode::PhyDcodeFailure> {
        self.result
    }
}

/// Opaque terminal result of complete RX-DC calibration and table generation.
///
/// ```compile_fail
/// use oer_esp32s31_phy::tracking::calibration::{
///     PhyCalibrationRxGainCompletion, PhyCalibrationTrackingCompletion,
/// };
///
/// let forged = PhyCalibrationTrackingCompletion::RxGainRecalibrated(
///     PhyCalibrationRxGainCompletion {
///         result: Ok(oer_esp32s31_phy::rx::gain::PhyRxGainInitOutcome {
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
/// use oer_esp32s31_phy::tracking::calibration::{
///     PhyCalibrationChannelCompletion, PhyCalibrationTrackingCompletion,
/// };
///
/// let forged = PhyCalibrationTrackingCompletion::ChipChannelRestored(
///     PhyCalibrationChannelCompletion {
///         result: Err(oer_esp32s31_phy::channel::PhyChipChannelFailure::UnsupportedChannel(14)),
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
/// use oer_esp32s31_phy::tracking::calibration::{
///     PhyCalibrationTrackingCompletion, PhyCalibrationTxDcPwdetCompletion,
/// };
/// use oer_esp32s31_phy::tracking::parameters::PhyCalibrationTrackClass;
///
/// let forged = PhyCalibrationTrackingCompletion::TxDcPwdetCalibrated(
///     PhyCalibrationTxDcPwdetCompletion {
///         class: PhyCalibrationTrackClass::Wifi,
///         result: Ok(oer_esp32s31_phy::tx::dc_power_detector::PhyTxDcPwdetOutcome {
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
/// use oer_esp32s31_phy::tracking::calibration::{
///     PhyCalibrationTrackingCompletion, PhyCalibrationTxGainCompletion,
/// };
/// use oer_esp32s31_phy::tracking::parameters::PhyCalibrationTrackClass;
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
    CommonDisableBaseband,
    CommonRecalibrateRxGain,
    CommonRestoreChannel,
    CommonEnableMac,
    CommonRestoreTxGainCompensation,
    TxDisableHardwareFrequency,
    TxSoftwareFrequencySettle,
    TxForceTxRxOff,
    TxForceDigitalGain,
    TxReleaseDigitalGain,
    TxClearPbus,
    TxDisableBaseband,
    TxConfigureBasebandZero,
    TxCalibrateTxDcPwdet,
    TxPublishTxGain,
    TxRestoreBaseband,
    TxEnableMac,
    TxReleaseTxRxOff,
    TxEnableHardwareFrequency,
    RestoreTxGainCompensation,
    Complete,
    Failed,
}

/// Finite RX/TX parent for the current three-argument vendor child.
/// The shared TX reference advances once after all requested classes restore.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyCalibrationTrackingTransition {
    scope: Scope,
    request: PhyCalibrationTrackingRequest,
    parameters: PhyCalibrationTrackingParameters,
    threshold: u8,
    common_updated: bool,
    transmit_updated: bool,
    active_class: PhyCalibrationTrackClass,
    dcode: Option<crate::analog::dcode::PhyDcodeOutcome>,
    rx_gain: Option<crate::rx::gain::PhyRxGainInitOutcome>,
    channel: Option<crate::channel::PhyChipChannelOutcome>,
    tx_dc_pwdet: [Option<crate::tx::dc_power_detector::PhyTxDcPwdetOutcome>; 2],
    failure: Option<PhyCalibrationTrackingFailure>,
    step: Step,
}

impl PhyCalibrationTrackingTransition {
    pub const fn new(
        request: PhyCalibrationTrackingRequest,
        parameters: PhyCalibrationTrackingParameters,
    ) -> Self {
        Self::selected(request, parameters, Scope::Both)
    }

    pub(crate) const fn selected(
        request: PhyCalibrationTrackingRequest,
        parameters: PhyCalibrationTrackingParameters,
        scope: Scope,
    ) -> Self {
        let decision = parameters.decision();
        let threshold = decision.common.threshold;
        let step = if !matches!(scope, Scope::Transmit) && decision.common.is_due() {
            Step::CommonClearPbus
        } else if !matches!(scope, Scope::Common) && decision.transmit.is_due() {
            Step::TxDisableHardwareFrequency
        } else {
            Step::Complete
        };
        Self {
            scope,
            request,
            parameters,
            threshold,
            common_updated: false,
            transmit_updated: false,
            active_class: if request.clients.wifi() {
                PhyCalibrationTrackClass::Wifi
            } else {
                PhyCalibrationTrackClass::BluetoothIeee802154
            },
            dcode: None,
            rx_gain: None,
            channel: None,
            tx_dc_pwdet: [None; 2],
            failure: None,
            step,
        }
    }

    pub const fn action(self) -> PhyCalibrationTrackingAction {
        match self.step {
            Step::CommonClearPbus | Step::TxClearPbus => PhyCalibrationTrackingAction::ClearPbus,
            Step::CommonDisableBaseband | Step::TxDisableBaseband => {
                PhyCalibrationTrackingAction::DisableWifiBaseband
            }
            Step::CommonDcode => PhyCalibrationTrackingAction::CalibrateDcode,
            Step::CommonRecalibrateRxGain => PhyCalibrationTrackingAction::RecalibrateRxGain,
            Step::CommonRestoreChannel => PhyCalibrationTrackingAction::RestoreChipChannel {
                channel: self.parameters.current_channel,
                cbw: self.parameters.channel_bandwidth,
            },
            Step::CommonEnableMac | Step::TxEnableMac => {
                PhyCalibrationTrackingAction::EnableMacBaseband
            }
            Step::TxDisableHardwareFrequency => {
                PhyCalibrationTrackingAction::SetHardwareFrequencyControl { enabled: false }
            }
            Step::TxSoftwareFrequencySettle => {
                PhyCalibrationTrackingAction::AwaitSoftwareFrequencySettle
            }
            Step::TxForceDigitalGain => {
                PhyCalibrationTrackingAction::SetForcedDigitalGain { enabled: true }
            }
            Step::TxReleaseDigitalGain => {
                PhyCalibrationTrackingAction::SetForcedDigitalGain { enabled: false }
            }
            Step::TxForceTxRxOff => PhyCalibrationTrackingAction::ForceTxRxOff { enabled: true },
            Step::TxConfigureBasebandZero => {
                PhyCalibrationTrackingAction::ConfigureBasebandChannel { cbw: 0 }
            }
            Step::TxCalibrateTxDcPwdet => PhyCalibrationTrackingAction::CalibrateTxDcPwdet {
                class: self.active_class,
            },
            Step::TxPublishTxGain => match self.active_class {
                PhyCalibrationTrackClass::Wifi => PhyCalibrationTrackingAction::PublishWifiTxGain {
                    channel: self.parameters.current_channel,
                },
                PhyCalibrationTrackClass::BluetoothIeee802154 => {
                    PhyCalibrationTrackingAction::PublishBluetoothIeee802154TxGain
                }
            },
            Step::TxRestoreBaseband => PhyCalibrationTrackingAction::ConfigureBasebandChannel {
                cbw: self.parameters.channel_bandwidth,
            },
            Step::TxReleaseTxRxOff => PhyCalibrationTrackingAction::ForceTxRxOff { enabled: false },
            Step::TxEnableHardwareFrequency => {
                PhyCalibrationTrackingAction::SetHardwareFrequencyControl { enabled: true }
            }
            Step::CommonRestoreTxGainCompensation | Step::RestoreTxGainCompensation => {
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
                Step::CommonDisableBaseband,
                PhyCalibrationTrackingCompletion::WifiBasebandDisabled,
            ) => Step::CommonRecalibrateRxGain,
            (Step::TxDisableBaseband, PhyCalibrationTrackingCompletion::WifiBasebandDisabled) => {
                Step::TxConfigureBasebandZero
            }
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
                        Step::CommonDisableBaseband
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
                Step::CommonRestoreTxGainCompensation
            }
            (
                Step::CommonRestoreTxGainCompensation,
                PhyCalibrationTrackingCompletion::TxGainCompensationRestored,
            ) => {
                self.common_updated = true;
                self.first_transmit_step()
            }
            (
                Step::TxDisableHardwareFrequency,
                PhyCalibrationTrackingCompletion::HardwareFrequencyControlSet { enabled: false },
            ) => Step::TxSoftwareFrequencySettle,
            (
                Step::TxSoftwareFrequencySettle,
                PhyCalibrationTrackingCompletion::SoftwareFrequencySettled,
            ) => Step::TxForceTxRxOff,
            (
                Step::TxForceTxRxOff,
                PhyCalibrationTrackingCompletion::ForceTxRxCompleted(completion),
            ) if completion.enabled => Step::TxForceDigitalGain,
            (
                Step::TxForceDigitalGain,
                PhyCalibrationTrackingCompletion::ForcedDigitalGainSet { enabled: true },
            ) => Step::TxClearPbus,
            (
                Step::TxReleaseDigitalGain,
                PhyCalibrationTrackingCompletion::ForcedDigitalGainSet { enabled: false },
            ) => Step::TxReleaseTxRxOff,
            (
                Step::TxClearPbus,
                PhyCalibrationTrackingCompletion::PbusClearCompleted(completion),
            ) => match completion.outcome {
                crate::analog::pbus::PhyPbusClearOutcome::Cleared => Step::TxDisableBaseband,
                crate::analog::pbus::PhyPbusClearOutcome::ForceTestTimedOut(transaction) => {
                    self.failure = Some(PhyCalibrationTrackingFailure::PbusClearTimedOut(
                        transaction,
                    ));
                    Step::TxReleaseDigitalGain
                }
            },
            (
                Step::TxConfigureBasebandZero,
                PhyCalibrationTrackingCompletion::BasebandChannelConfigured { cbw: 0 },
            ) => {
                if self.request.clients.wifi() || self.request.clients.bluetooth_ieee802154() {
                    Step::TxCalibrateTxDcPwdet
                } else {
                    Step::TxRestoreBaseband
                }
            }
            (
                Step::TxCalibrateTxDcPwdet,
                PhyCalibrationTrackingCompletion::TxDcPwdetCalibrated(completion),
            ) if completion.class == self.active_class => match completion.result {
                Ok(outcome) => {
                    self.tx_dc_pwdet[self.active_class.selector() as usize] = Some(outcome);
                    Step::TxPublishTxGain
                }
                Err(failure) => {
                    self.failure = Some(PhyCalibrationTrackingFailure::TxDcPwdet(failure));
                    Step::TxReleaseDigitalGain
                }
            },
            (
                Step::TxPublishTxGain,
                PhyCalibrationTrackingCompletion::TxGainPublished(completion),
            ) if completion.class == self.active_class
                && completion.channel
                    == match self.active_class {
                        PhyCalibrationTrackClass::Wifi => Some(self.parameters.current_channel),
                        PhyCalibrationTrackClass::BluetoothIeee802154 => None,
                    } =>
            {
                if self.active_class == PhyCalibrationTrackClass::Wifi
                    && self.request.clients.bluetooth_ieee802154()
                {
                    self.active_class = PhyCalibrationTrackClass::BluetoothIeee802154;
                    Step::TxCalibrateTxDcPwdet
                } else {
                    Step::TxRestoreBaseband
                }
            }
            (
                Step::TxRestoreBaseband,
                PhyCalibrationTrackingCompletion::BasebandChannelConfigured { cbw },
            ) if cbw == self.parameters.channel_bandwidth => Step::TxEnableMac,
            (Step::TxEnableMac, PhyCalibrationTrackingCompletion::MacBasebandEnabled) => {
                Step::TxReleaseDigitalGain
            }
            (
                Step::TxReleaseTxRxOff,
                PhyCalibrationTrackingCompletion::ForceTxRxCompleted(completion),
            ) if !completion.enabled => Step::TxEnableHardwareFrequency,
            (
                Step::TxEnableHardwareFrequency,
                PhyCalibrationTrackingCompletion::HardwareFrequencyControlSet { enabled: true },
            ) => {
                self.transmit_updated = self.failure.is_none();
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
        let Some(tx_dc_pwdet) = self.tx_dc_pwdet[self.active_class.selector() as usize] else {
            return Err(PhyCalibrationTrackingChildError::IncompleteChildOutcome);
        };
        PhyCalibrationTxGainBinding::lower(self.action(), state, tx_dc_pwdet)
    }

    const fn first_transmit_step(self) -> Step {
        if !matches!(self.scope, Scope::Common) && self.parameters.decision().transmit.is_due() {
            Step::TxDisableHardwareFrequency
        } else {
            Step::Complete
        }
    }

    const fn outcome(self) -> PhyCalibrationTrackingOutcome {
        let current = self.parameters.current_temperature;
        PhyCalibrationTrackingOutcome {
            clients: self.request.clients,
            threshold: self.threshold,
            common_reference_temperature: if self.common_updated {
                current
            } else {
                self.parameters.common_reference_temperature
            },
            transmit_reference_temperature: if self.transmit_updated {
                current
            } else {
                self.parameters.transmit_reference_temperature
            },
            common_updated: self.common_updated,
            transmit_updated: self.transmit_updated,
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
            wifi_tx_dc_pwdet: if self.transmit_updated {
                self.tx_dc_pwdet[0]
            } else {
                None
            },
            bluetooth_ieee802154_tx_dc_pwdet: if self.transmit_updated {
                self.tx_dc_pwdet[1]
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
        registers: &mut impl oer_esp32s31_hal::owner::SharedPhyAccess,
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
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn transition_mut(
        &mut self,
    ) -> &mut crate::tx::dc_power_detector::PhyTxDcPwdetTransition {
        match &mut self.child {
            PhyCalibrationTxDcPwdetChild::Wifi(child) => child,
            PhyCalibrationTxDcPwdetChild::BluetoothIeee802154(child) => child.transition_mut(),
        }
    }

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
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn transition_mut(&mut self) -> &mut crate::rx::gain::PhyRxGainInitTransition {
        &mut self.child
    }

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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyCalibrationTrackingBindingError {
    UnsupportedAction,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RegisterAction {
    SetHardwareFrequencyControl { enabled: bool },
    SetForcedDigitalGain { enabled: bool },
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
            PhyCalibrationTrackingAction::SetForcedDigitalGain { enabled } => {
                RegisterAction::SetForcedDigitalGain { enabled }
            }
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
            RegisterAction::SetForcedDigitalGain { enabled } => {
                PhyCalibrationTrackingAction::SetForcedDigitalGain { enabled }
            }
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
        registers: &mut impl oer_esp32s31_hal::owner::SharedPhyAccess,
    ) -> PhyCalibrationTrackingCompletion {
        match self.action {
            RegisterAction::SetForcedDigitalGain { enabled } => {
                oer_esp32s31_hal::phy::baseband::configure_forced_digital_gain(
                    registers, enabled, -120, -120,
                );
                PhyCalibrationTrackingCompletion::ForcedDigitalGainSet { enabled }
            }
            RegisterAction::SetHardwareFrequencyControl { enabled } => {
                oer_esp32s31_hal::phy::frequency::set_baseband_mode(
                    registers,
                    if enabled { 0 } else { 2 },
                );
                PhyCalibrationTrackingCompletion::HardwareFrequencyControlSet { enabled }
            }
            RegisterAction::ConfigureBasebandChannel { cbw } => {
                oer_esp32s31_hal::phy::frequency::configure_channel_cbw(registers, cbw.into());
                PhyCalibrationTrackingCompletion::BasebandChannelConfigured { cbw }
            }
            RegisterAction::RestoreTxGainCompensation => {
                oer_esp32s31_hal::phy::baseband::restore_tx_gain_compensation(registers);
                PhyCalibrationTrackingCompletion::TxGainCompensationRestored
            }
        }
    }
}

/// Non-cloneable owner of a route-PAC baseband edge: disable Wi-Fi before
/// measurement, or execute the complete `phy_mac_enable_bb` restoration.
#[derive(Debug, Eq, PartialEq)]
pub struct PhyCalibrationTrackingMacBasebandBinding {
    enabled: bool,
}

impl PhyCalibrationTrackingMacBasebandBinding {
    pub const fn new(
        action: PhyCalibrationTrackingAction,
    ) -> Result<Self, PhyCalibrationTrackingBindingError> {
        match action {
            PhyCalibrationTrackingAction::EnableMacBaseband => Ok(Self { enabled: true }),
            PhyCalibrationTrackingAction::DisableWifiBaseband => Ok(Self { enabled: false }),
            _ => Err(PhyCalibrationTrackingBindingError::UnsupportedAction),
        }
    }

    pub const fn action(&self) -> PhyCalibrationTrackingAction {
        if self.enabled {
            PhyCalibrationTrackingAction::EnableMacBaseband
        } else {
            PhyCalibrationTrackingAction::DisableWifiBaseband
        }
    }

    #[cfg(target_arch = "riscv32")]
    pub fn execute_target<R: oer_esp32s31_hal::owner::PhyInitializationAccess>(
        self,
        registers: &mut R,
    ) -> PhyCalibrationTrackingCompletion {
        if self.enabled {
            oer_esp32s31_hal::phy::frequency::enable_mac_baseband(registers);
            PhyCalibrationTrackingCompletion::MacBasebandEnabled
        } else {
            // Current b88e4b76 phy_cal_param_track clears Wi-Fi enable after
            // DCODE (RX) or PBus clear (TX), before the measurement children.
            oer_esp32s31_hal::phy::frequency::set_wifi_enabled(registers, false);
            PhyCalibrationTrackingCompletion::WifiBasebandDisabled
        }
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
            PhyCalibrationTrackingAction::SetForcedDigitalGain { .. }
            | PhyCalibrationTrackingAction::SetHardwareFrequencyControl { .. }
            | PhyCalibrationTrackingAction::ConfigureBasebandChannel { .. }
            | PhyCalibrationTrackingAction::RestoreTxGainCompensation => {
                match PhyCalibrationTrackingRegisterBinding::new(action) {
                    Ok(binding) => Ok(Self::Register(binding)),
                    Err(error) => Err(error),
                }
            }
            PhyCalibrationTrackingAction::EnableMacBaseband
            | PhyCalibrationTrackingAction::DisableWifiBaseband => {
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
