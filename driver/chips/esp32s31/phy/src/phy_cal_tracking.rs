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
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyCalibrationTrackingAction {
    ClearPbus,
    CalibrateDcode,
    ResetCommonCalibrationState,
    PublishRxGainTable,
    RestoreChipChannel { channel: u16, cbw: u8 },
    SetHardwareFrequencyControl { enabled: bool },
    ForceTxRxOff { enabled: bool },
    ResetClassCalibrationState,
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
    DcodeCalibrated,
    CommonCalibrationStateReset,
    RxGainTablePublished,
    ChipChannelRestored { channel: u16, cbw: u8 },
    HardwareFrequencyControlSet { enabled: bool },
    ForceTxRxCompleted(PhyCalibrationForceTxRxCompletion),
    ClassCalibrationStateReset,
    BasebandChannelConfigured { cbw: u8 },
    TxDcPwdetCalibrated { class: PhyCalibrationTrackClass },
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyCalibrationTrackingFailure {
    PbusClearTimedOut(crate::phy_pbus::PhyPbusForceTest),
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
    CommonReset,
    CommonPublishRxGain,
    CommonRestoreChannel,
    CommonEnableMac,
    ClassDisableHardwareFrequency,
    ClassForceTxRxOff,
    ClassClearPbus,
    ClassReset,
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
            failure: None,
            step,
        }
    }

    pub const fn action(self) -> PhyCalibrationTrackingAction {
        match self.step {
            Step::CommonClearPbus | Step::ClassClearPbus => PhyCalibrationTrackingAction::ClearPbus,
            Step::CommonDcode => PhyCalibrationTrackingAction::CalibrateDcode,
            Step::CommonReset => PhyCalibrationTrackingAction::ResetCommonCalibrationState,
            Step::CommonPublishRxGain => PhyCalibrationTrackingAction::PublishRxGainTable,
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
            Step::ClassReset => PhyCalibrationTrackingAction::ResetClassCalibrationState,
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
            (Step::CommonDcode, PhyCalibrationTrackingCompletion::DcodeCalibrated) => {
                Step::CommonReset
            }
            (Step::CommonReset, PhyCalibrationTrackingCompletion::CommonCalibrationStateReset) => {
                Step::CommonPublishRxGain
            }
            (Step::CommonPublishRxGain, PhyCalibrationTrackingCompletion::RxGainTablePublished) => {
                Step::CommonRestoreChannel
            }
            (
                Step::CommonRestoreChannel,
                PhyCalibrationTrackingCompletion::ChipChannelRestored { channel, cbw },
            ) if channel == self.parameters.current_channel
                && cbw == self.parameters.channel_bandwidth =>
            {
                Step::CommonEnableMac
            }
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
                crate::phy_pbus::PhyPbusClearOutcome::Cleared => Step::ClassReset,
                crate::phy_pbus::PhyPbusClearOutcome::ForceTestTimedOut(transaction) => {
                    self.failure = Some(PhyCalibrationTrackingFailure::PbusClearTimedOut(
                        transaction,
                    ));
                    Step::ClassReleaseTxRxOff
                }
            },
            (Step::ClassReset, PhyCalibrationTrackingCompletion::ClassCalibrationStateReset) => {
                Step::ClassConfigureBasebandZero
            }
            (
                Step::ClassConfigureBasebandZero,
                PhyCalibrationTrackingCompletion::BasebandChannelConfigured { cbw: 0 },
            ) => Step::ClassCalibrateTxDcPwdet,
            (
                Step::ClassCalibrateTxDcPwdet,
                PhyCalibrationTrackingCompletion::TxDcPwdetCalibrated { class },
            ) if class == self.request.class => Step::ClassPublishTxGain,
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
    parent_action: PhyCalibrationTrackingAction,
    child: crate::phy_pbus::PhyForceTxRxTransition,
}

/// Non-cloneable calibration-child owner for complete bounded PBus clear.
#[derive(Debug, Eq, PartialEq)]
pub struct PhyCalibrationPbusClearTransition {
    child: crate::phy_pbus::PhyPbusClearTransition,
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
            parent_action,
            child: crate::phy_pbus::PhyForceTxRxTransition::new(enabled),
        })
    }

    pub const fn parent_action(&self) -> PhyCalibrationTrackingAction {
        self.parent_action
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
    };

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
                PhyCalibrationTrackingCompletion::DcodeCalibrated
            }
            PhyCalibrationTrackingAction::ResetCommonCalibrationState => {
                PhyCalibrationTrackingCompletion::CommonCalibrationStateReset
            }
            PhyCalibrationTrackingAction::PublishRxGainTable => {
                PhyCalibrationTrackingCompletion::RxGainTablePublished
            }
            PhyCalibrationTrackingAction::RestoreChipChannel { channel, cbw } => {
                PhyCalibrationTrackingCompletion::ChipChannelRestored { channel, cbw }
            }
            PhyCalibrationTrackingAction::SetHardwareFrequencyControl { enabled } => {
                PhyCalibrationTrackingCompletion::HardwareFrequencyControlSet { enabled }
            }
            PhyCalibrationTrackingAction::ForceTxRxOff { enabled } => {
                PhyCalibrationTrackingCompletion::ForceTxRxCompleted(
                    PhyCalibrationForceTxRxCompletion { enabled },
                )
            }
            PhyCalibrationTrackingAction::ResetClassCalibrationState => {
                PhyCalibrationTrackingCompletion::ClassCalibrationStateReset
            }
            PhyCalibrationTrackingAction::ConfigureBasebandChannel { cbw } => {
                PhyCalibrationTrackingCompletion::BasebandChannelConfigured { cbw }
            }
            PhyCalibrationTrackingAction::CalibrateTxDcPwdet { class } => {
                PhyCalibrationTrackingCompletion::TxDcPwdetCalibrated { class }
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
            PhyCalibrationTrackingAction::ResetCommonCalibrationState,
            PhyCalibrationTrackingAction::PublishRxGainTable,
            PhyCalibrationTrackingAction::RestoreChipChannel {
                channel: 11,
                cbw: 1,
            },
            PhyCalibrationTrackingAction::ForceTxRxOff { enabled: true },
            PhyCalibrationTrackingAction::ResetClassCalibrationState,
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
