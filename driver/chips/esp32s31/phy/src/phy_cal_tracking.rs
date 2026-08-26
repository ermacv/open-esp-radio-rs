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
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyCalibrationTrackingCompletion {
    PbusCleared,
    DcodeCalibrated,
    CommonCalibrationStateReset,
    RxGainTablePublished,
    ChipChannelRestored { channel: u16, cbw: u8 },
    HardwareFrequencyControlSet { enabled: bool },
    TxRxForcedOff { enabled: bool },
    ClassCalibrationStateReset,
    BasebandChannelConfigured { cbw: u8 },
    TxDcPwdetCalibrated { class: PhyCalibrationTrackClass },
    WifiTxGainPublished { channel: u16 },
    BluetoothIeee802154TxGainPublished,
    MacBasebandEnabled,
    TxGainCompensationRestored,
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
}

/// Finite exact-order parent for `phy_cal_param_track`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyCalibrationTrackingTransition {
    request: PhyCalibrationTrackingRequest,
    parameters: PhyCalibrationTrackingParameters,
    threshold: u8,
    common_updated: bool,
    class_updated: bool,
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
        }
    }

    pub fn advance(
        &mut self,
        completion: PhyCalibrationTrackingCompletion,
    ) -> Result<(), PhyCalibrationTrackingTransitionError> {
        self.step = match (self.step, completion) {
            (Step::CommonClearPbus, PhyCalibrationTrackingCompletion::PbusCleared) => {
                Step::CommonDcode
            }
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
                PhyCalibrationTrackingCompletion::TxRxForcedOff { enabled: true },
            ) => Step::ClassClearPbus,
            (Step::ClassClearPbus, PhyCalibrationTrackingCompletion::PbusCleared) => {
                Step::ClassReset
            }
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
                PhyCalibrationTrackingCompletion::TxRxForcedOff { enabled: false },
            ) => Step::ClassEnableHardwareFrequency,
            (
                Step::ClassEnableHardwareFrequency,
                PhyCalibrationTrackingCompletion::HardwareFrequencyControlSet { enabled: true },
            ) => {
                self.class_updated = true;
                Step::RestoreTxGainCompensation
            }
            (
                Step::RestoreTxGainCompensation,
                PhyCalibrationTrackingCompletion::TxGainCompensationRestored,
            ) => Step::Complete,
            (Step::Complete, _) => {
                return Err(PhyCalibrationTrackingTransitionError::AlreadyComplete);
            }
            _ => return Err(PhyCalibrationTrackingTransitionError::WrongCompletion),
        };
        Ok(())
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
                PhyCalibrationTrackingCompletion::PbusCleared
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
                PhyCalibrationTrackingCompletion::TxRxForcedOff { enabled }
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
            PhyCalibrationTrackingAction::Complete(_) => panic!("terminal action"),
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
            if matches!(action, PhyCalibrationTrackingAction::Complete(_)) {
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
}
