//! Source-owned ESP32-S31 temperature-driven TX-power tracking.
//!
//! The pinned archive parent `phy_txpwr_cal_track_new` is 104 instructions in
//! 21 finite basic blocks. This module preserves its decision arithmetic,
//! state-commit point, BBPLL boundary and protocol-specific gain publication
//! order. The vendor's optional `phy_printf` call is diagnostic-only and has
//! no radio or retained-state effect, so it is deliberately absent.

use crate::{
    phy_math::{absolute_temperature, saturate_signed},
    phy_param_tracking::{PhyCalibrationTrackClass, temperature_to_tracking_power},
};

/// Caller-owned inputs formerly read from the shared `phy_param` image.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyTxPowerTrackingParameters {
    pub current_temperature: i16,
    pub reference_temperature: i16,
    pub previous_tracking_temperature: i16,
    pub previous_tracking_gain_base: i8,
    pub wifi_gain_base: i8,
    pub bluetooth_ieee802154_gain_base: i8,
    pub relaxed_threshold: bool,
}

/// One invocation of the shared Wi-Fi or Bluetooth/IEEE 802.15.4 parent.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyTxPowerTrackingRequest {
    pub class: PhyCalibrationTrackClass,
    pub enabled: bool,
    /// Passed only to the Wi-Fi gain publisher. The Bluetooth/IEEE child uses
    /// its fixed gain-memory bank and ignores this value.
    pub wifi_channel: u16,
}

/// Pure decision result before any retained state or hardware is changed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyTxPowerTrackingDecision {
    pub bounded_temperature: i16,
    pub threshold: u8,
    pub gain_base: i8,
    pub recomputed: bool,
    pub update_required: bool,
}

/// Compute the exact branch decision of `phy_txpwr_cal_track_new`.
pub const fn decide_tx_power_tracking(
    request: PhyTxPowerTrackingRequest,
    parameters: PhyTxPowerTrackingParameters,
) -> PhyTxPowerTrackingDecision {
    let reference_delta = (parameters.current_temperature as i32)
        .wrapping_sub(parameters.reference_temperature as i32);
    let threshold = if parameters.relaxed_threshold {
        10
    } else if absolute_temperature(reference_delta) > 7 {
        4
    } else {
        2
    };
    let upper = match request.class {
        PhyCalibrationTrackClass::Wifi => 80,
        PhyCalibrationTrackClass::BluetoothIeee802154 => 105,
    };
    let bounded_temperature =
        saturate_signed(parameters.current_temperature as i32, upper, -60) as i16;
    let tracking_delta =
        (bounded_temperature as i32).wrapping_sub(parameters.previous_tracking_temperature as i32);
    let recomputed = absolute_temperature(tracking_delta) >= threshold as u32;
    let gain_base = if recomputed {
        temperature_to_tracking_power(
            bounded_temperature,
            parameters.reference_temperature,
            request.class,
        )
    } else {
        parameters.previous_tracking_gain_base
    };
    let published_gain_base = match request.class {
        PhyCalibrationTrackClass::Wifi => parameters.wifi_gain_base,
        PhyCalibrationTrackClass::BluetoothIeee802154 => parameters.bluetooth_ieee802154_gain_base,
    };

    PhyTxPowerTrackingDecision {
        bounded_temperature,
        threshold,
        gain_base,
        recomputed,
        update_required: request.enabled && gain_base != published_gain_base,
    }
}

/// Retained result committed only after the gain child and BBPLL tail finish.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyTxPowerTrackingOutcome {
    pub class: PhyCalibrationTrackClass,
    pub gain_updated: bool,
    pub tracking_temperature: i16,
    pub tracking_gain_base: i8,
    pub wifi_gain_base: i8,
    pub bluetooth_ieee802154_gain_base: i8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxPowerTrackingAction {
    SetBbpllCalibration { enabled: bool },
    RegenerateWifiGain { channel: u16, gain_base: i8 },
    RegenerateBluetoothIeee802154Gain { gain_base: i8 },
    Complete(PhyTxPowerTrackingOutcome),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxPowerTrackingCompletion {
    BbpllCalibrationSet { enabled: bool },
    WifiGainRegenerated { channel: u16, gain_base: i8 },
    BluetoothIeee802154GainRegenerated { gain_base: i8 },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxPowerTrackingTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PhyTxPowerTrackingStep {
    BbpllOn,
    RegenerateGain,
    BbpllOff,
    Complete,
}

/// Exact finite I/O ordering of archive `phy_txpwr_cal_track_new`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyTxPowerTrackingTransition {
    request: PhyTxPowerTrackingRequest,
    decision: PhyTxPowerTrackingDecision,
    outcome: PhyTxPowerTrackingOutcome,
    step: PhyTxPowerTrackingStep,
}

impl PhyTxPowerTrackingTransition {
    pub const fn new(
        request: PhyTxPowerTrackingRequest,
        parameters: PhyTxPowerTrackingParameters,
    ) -> Self {
        let decision = decide_tx_power_tracking(request, parameters);
        let mut outcome = PhyTxPowerTrackingOutcome {
            class: request.class,
            gain_updated: decision.update_required,
            tracking_temperature: parameters.previous_tracking_temperature,
            tracking_gain_base: parameters.previous_tracking_gain_base,
            wifi_gain_base: parameters.wifi_gain_base,
            bluetooth_ieee802154_gain_base: parameters.bluetooth_ieee802154_gain_base,
        };
        let step = if decision.update_required {
            outcome.tracking_temperature = parameters.current_temperature;
            outcome.tracking_gain_base = decision.gain_base;
            match request.class {
                PhyCalibrationTrackClass::Wifi => {
                    outcome.wifi_gain_base = decision.gain_base;
                }
                PhyCalibrationTrackClass::BluetoothIeee802154 => {
                    outcome.bluetooth_ieee802154_gain_base = decision.gain_base;
                }
            }
            PhyTxPowerTrackingStep::BbpllOn
        } else {
            PhyTxPowerTrackingStep::Complete
        };

        Self {
            request,
            decision,
            outcome,
            step,
        }
    }

    pub const fn decision(self) -> PhyTxPowerTrackingDecision {
        self.decision
    }

    pub const fn action(self) -> PhyTxPowerTrackingAction {
        match self.step {
            PhyTxPowerTrackingStep::BbpllOn => {
                PhyTxPowerTrackingAction::SetBbpllCalibration { enabled: true }
            }
            PhyTxPowerTrackingStep::RegenerateGain => match self.request.class {
                PhyCalibrationTrackClass::Wifi => PhyTxPowerTrackingAction::RegenerateWifiGain {
                    channel: self.request.wifi_channel,
                    gain_base: self.decision.gain_base,
                },
                PhyCalibrationTrackClass::BluetoothIeee802154 => {
                    PhyTxPowerTrackingAction::RegenerateBluetoothIeee802154Gain {
                        gain_base: self.decision.gain_base,
                    }
                }
            },
            PhyTxPowerTrackingStep::BbpllOff => {
                PhyTxPowerTrackingAction::SetBbpllCalibration { enabled: false }
            }
            PhyTxPowerTrackingStep::Complete => PhyTxPowerTrackingAction::Complete(self.outcome),
        }
    }

    pub fn advance(
        &mut self,
        completion: PhyTxPowerTrackingCompletion,
    ) -> Result<(), PhyTxPowerTrackingTransitionError> {
        self.step = match (self.step, completion) {
            (
                PhyTxPowerTrackingStep::BbpllOn,
                PhyTxPowerTrackingCompletion::BbpllCalibrationSet { enabled: true },
            ) => PhyTxPowerTrackingStep::RegenerateGain,
            (
                PhyTxPowerTrackingStep::RegenerateGain,
                PhyTxPowerTrackingCompletion::WifiGainRegenerated { channel, gain_base },
            ) if self.request.class == PhyCalibrationTrackClass::Wifi
                && channel == self.request.wifi_channel
                && gain_base == self.decision.gain_base =>
            {
                PhyTxPowerTrackingStep::BbpllOff
            }
            (
                PhyTxPowerTrackingStep::RegenerateGain,
                PhyTxPowerTrackingCompletion::BluetoothIeee802154GainRegenerated { gain_base },
            ) if self.request.class == PhyCalibrationTrackClass::BluetoothIeee802154
                && gain_base == self.decision.gain_base =>
            {
                PhyTxPowerTrackingStep::BbpllOff
            }
            (
                PhyTxPowerTrackingStep::BbpllOff,
                PhyTxPowerTrackingCompletion::BbpllCalibrationSet { enabled: false },
            ) => PhyTxPowerTrackingStep::Complete,
            (PhyTxPowerTrackingStep::Complete, _) => {
                return Err(PhyTxPowerTrackingTransitionError::AlreadyComplete);
            }
            _ => return Err(PhyTxPowerTrackingTransitionError::WrongCompletion),
        };
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxPowerTrackingBindingError {
    UnsupportedAction,
}

#[derive(Debug, Eq, PartialEq)]
enum PhyTxPowerTrackingExternalOperation {
    SetBbpllCalibration {
        enabled: bool,
    },
    RegenerateWifiGain {
        channel: u16,
        gain_base: i8,
        image: Option<crate::phy_channel::PhyWifiTxGainImage>,
    },
    RegenerateBluetoothIeee802154Gain {
        gain_base: i8,
        image: crate::phy_bluetooth::PhyBluetoothTxGainImage,
    },
}

/// Non-cloneable owner of one external power-tracking edge.
///
/// Lowering captures a gain image from the live typed state before MMIO. This
/// prevents a later state change from separating the completion identity from
/// the bytes actually published. A missing Wi-Fi image is the reviewed vendor
/// `tx_gain_skip_publication` path and still completes the regeneration child.
#[derive(Debug, Eq, PartialEq)]
pub struct PhyTxPowerTrackingExternalBinding {
    operation: PhyTxPowerTrackingExternalOperation,
}

impl PhyTxPowerTrackingExternalBinding {
    pub fn lower(
        action: PhyTxPowerTrackingAction,
        state: &crate::phy_state::PhyState,
    ) -> Result<Self, PhyTxPowerTrackingBindingError> {
        let operation = match action {
            PhyTxPowerTrackingAction::SetBbpllCalibration { enabled } => {
                PhyTxPowerTrackingExternalOperation::SetBbpllCalibration { enabled }
            }
            PhyTxPowerTrackingAction::RegenerateWifiGain { channel, gain_base } => {
                PhyTxPowerTrackingExternalOperation::RegenerateWifiGain {
                    channel,
                    gain_base,
                    image: state.wifi_tracking_gain_image(channel, gain_base),
                }
            }
            PhyTxPowerTrackingAction::RegenerateBluetoothIeee802154Gain { gain_base } => {
                PhyTxPowerTrackingExternalOperation::RegenerateBluetoothIeee802154Gain {
                    gain_base,
                    image: state.bluetooth_ieee802154_tracking_gain_image(gain_base),
                }
            }
            PhyTxPowerTrackingAction::Complete(_) => {
                return Err(PhyTxPowerTrackingBindingError::UnsupportedAction);
            }
        };
        Ok(Self { operation })
    }

    pub const fn action(&self) -> PhyTxPowerTrackingAction {
        match self.operation {
            PhyTxPowerTrackingExternalOperation::SetBbpllCalibration { enabled } => {
                PhyTxPowerTrackingAction::SetBbpllCalibration { enabled }
            }
            PhyTxPowerTrackingExternalOperation::RegenerateWifiGain {
                channel, gain_base, ..
            } => PhyTxPowerTrackingAction::RegenerateWifiGain { channel, gain_base },
            PhyTxPowerTrackingExternalOperation::RegenerateBluetoothIeee802154Gain {
                gain_base,
                ..
            } => PhyTxPowerTrackingAction::RegenerateBluetoothIeee802154Gain { gain_base },
        }
    }

    #[cfg(target_arch = "riscv32")]
    pub fn execute_target<P: open_esp_radio_esp32s31_hal::phy_i2c::PhyI2cMasterControl>(
        self,
        platform: &mut P,
        registers: &mut impl open_esp_radio_esp32s31_hal::SharedPhyAccess,
    ) -> PhyTxPowerTrackingCompletion {
        match self.operation {
            PhyTxPowerTrackingExternalOperation::SetBbpllCalibration { enabled } => {
                open_esp_radio_esp32s31_hal::phy_i2c::configure_bbpll_calibration(
                    platform, enabled,
                );
                PhyTxPowerTrackingCompletion::BbpllCalibrationSet { enabled }
            }
            PhyTxPowerTrackingExternalOperation::RegenerateWifiGain {
                channel,
                gain_base,
                image,
            } => {
                if let Some(image) = image {
                    crate::phy_hardware::publish_phy_tx_gain_memory(registers, false, image);
                }
                PhyTxPowerTrackingCompletion::WifiGainRegenerated { channel, gain_base }
            }
            PhyTxPowerTrackingExternalOperation::RegenerateBluetoothIeee802154Gain {
                gain_base,
                image,
            } => {
                crate::phy_hardware::publish_bluetooth_tx_gain_memory(registers, image);
                PhyTxPowerTrackingCompletion::BluetoothIeee802154GainRegenerated { gain_base }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PARAMETERS: PhyTxPowerTrackingParameters = PhyTxPowerTrackingParameters {
        current_temperature: 25,
        reference_temperature: 0,
        previous_tracking_temperature: 0,
        previous_tracking_gain_base: -3,
        wifi_gain_base: 1,
        bluetooth_ieee802154_gain_base: 2,
        relaxed_threshold: false,
    };

    const fn request(class: PhyCalibrationTrackClass) -> PhyTxPowerTrackingRequest {
        PhyTxPowerTrackingRequest {
            class,
            enabled: true,
            wifi_channel: 11,
        }
    }

    #[test]
    fn decision_preserves_threshold_and_temperature_boundaries() {
        let decision = decide_tx_power_tracking(
            request(PhyCalibrationTrackClass::Wifi),
            PhyTxPowerTrackingParameters {
                current_temperature: 81,
                reference_temperature: 74,
                previous_tracking_temperature: 76,
                ..PARAMETERS
            },
        );
        assert_eq!(decision.bounded_temperature, 80);
        assert_eq!(decision.threshold, 2);
        assert!(decision.recomputed);

        let decision = decide_tx_power_tracking(
            request(PhyCalibrationTrackClass::BluetoothIeee802154),
            PhyTxPowerTrackingParameters {
                current_temperature: 106,
                reference_temperature: 98,
                previous_tracking_temperature: 101,
                relaxed_threshold: true,
                ..PARAMETERS
            },
        );
        assert_eq!(decision.bounded_temperature, 105);
        assert_eq!(decision.threshold, 10);
        assert!(!decision.recomputed);
        assert_eq!(decision.gain_base, PARAMETERS.previous_tracking_gain_base);

        let decision = decide_tx_power_tracking(
            request(PhyCalibrationTrackClass::Wifi),
            PhyTxPowerTrackingParameters {
                current_temperature: 8,
                reference_temperature: 0,
                previous_tracking_temperature: 4,
                ..PARAMETERS
            },
        );
        assert_eq!(decision.threshold, 4);
        assert!(decision.recomputed);
    }

    #[test]
    fn disabled_or_equal_gain_base_commits_nothing() {
        let mut disabled = request(PhyCalibrationTrackClass::BluetoothIeee802154);
        disabled.enabled = false;
        let transition = PhyTxPowerTrackingTransition::new(disabled, PARAMETERS);
        assert!(transition.decision().recomputed);
        assert_eq!(
            transition.action(),
            PhyTxPowerTrackingAction::Complete(PhyTxPowerTrackingOutcome {
                class: PhyCalibrationTrackClass::BluetoothIeee802154,
                gain_updated: false,
                tracking_temperature: PARAMETERS.previous_tracking_temperature,
                tracking_gain_base: PARAMETERS.previous_tracking_gain_base,
                wifi_gain_base: PARAMETERS.wifi_gain_base,
                bluetooth_ieee802154_gain_base: PARAMETERS.bluetooth_ieee802154_gain_base,
            })
        );

        let same = PhyTxPowerTrackingParameters {
            bluetooth_ieee802154_gain_base: 5,
            ..PARAMETERS
        };
        assert_eq!(
            PhyTxPowerTrackingTransition::new(
                request(PhyCalibrationTrackClass::BluetoothIeee802154),
                same,
            )
            .action(),
            PhyTxPowerTrackingAction::Complete(PhyTxPowerTrackingOutcome {
                class: PhyCalibrationTrackClass::BluetoothIeee802154,
                gain_updated: false,
                tracking_temperature: same.previous_tracking_temperature,
                tracking_gain_base: same.previous_tracking_gain_base,
                wifi_gain_base: same.wifi_gain_base,
                bluetooth_ieee802154_gain_base: same.bluetooth_ieee802154_gain_base,
            })
        );
    }

    #[test]
    fn bluetooth_ieee802154_update_owns_bbpll_and_gain_order() {
        let mut transition = PhyTxPowerTrackingTransition::new(
            request(PhyCalibrationTrackClass::BluetoothIeee802154),
            PARAMETERS,
        );
        assert_eq!(transition.decision().gain_base, 5);
        assert_eq!(
            transition.action(),
            PhyTxPowerTrackingAction::SetBbpllCalibration { enabled: true }
        );
        transition
            .advance(PhyTxPowerTrackingCompletion::BbpllCalibrationSet { enabled: true })
            .unwrap();
        assert_eq!(
            transition.action(),
            PhyTxPowerTrackingAction::RegenerateBluetoothIeee802154Gain { gain_base: 5 }
        );
        transition
            .advance(
                PhyTxPowerTrackingCompletion::BluetoothIeee802154GainRegenerated { gain_base: 5 },
            )
            .unwrap();
        assert_eq!(
            transition.action(),
            PhyTxPowerTrackingAction::SetBbpllCalibration { enabled: false }
        );
        transition
            .advance(PhyTxPowerTrackingCompletion::BbpllCalibrationSet { enabled: false })
            .unwrap();
        assert_eq!(
            transition.action(),
            PhyTxPowerTrackingAction::Complete(PhyTxPowerTrackingOutcome {
                class: PhyCalibrationTrackClass::BluetoothIeee802154,
                gain_updated: true,
                tracking_temperature: PARAMETERS.current_temperature,
                tracking_gain_base: 5,
                wifi_gain_base: PARAMETERS.wifi_gain_base,
                bluetooth_ieee802154_gain_base: 5,
            })
        );
    }

    #[test]
    fn wifi_update_binds_channel_and_rejects_foreign_completion() {
        let mut transition =
            PhyTxPowerTrackingTransition::new(request(PhyCalibrationTrackClass::Wifi), PARAMETERS);
        transition
            .advance(PhyTxPowerTrackingCompletion::BbpllCalibrationSet { enabled: true })
            .unwrap();
        assert_eq!(
            transition.advance(PhyTxPowerTrackingCompletion::WifiGainRegenerated {
                channel: 6,
                gain_base: 5,
            }),
            Err(PhyTxPowerTrackingTransitionError::WrongCompletion)
        );
        assert_eq!(
            transition.action(),
            PhyTxPowerTrackingAction::RegenerateWifiGain {
                channel: 11,
                gain_base: 5,
            }
        );
    }

    #[test]
    fn external_binding_captures_live_typed_gain_images_and_rejects_terminal() {
        let state = crate::phy_state::PhyState::new(crate::phy_state::PhyConfig::production());
        let wifi_action = PhyTxPowerTrackingAction::RegenerateWifiGain {
            channel: 11,
            gain_base: 5,
        };
        let wifi = PhyTxPowerTrackingExternalBinding::lower(wifi_action, &state).unwrap();
        assert_eq!(wifi.action(), wifi_action);
        assert_eq!(
            wifi.operation,
            PhyTxPowerTrackingExternalOperation::RegenerateWifiGain {
                channel: 11,
                gain_base: 5,
                image: state.wifi_tracking_gain_image(11, 5),
            }
        );

        let bluetooth_action =
            PhyTxPowerTrackingAction::RegenerateBluetoothIeee802154Gain { gain_base: -7 };
        let bluetooth = PhyTxPowerTrackingExternalBinding::lower(bluetooth_action, &state).unwrap();
        assert_eq!(bluetooth.action(), bluetooth_action);
        assert_eq!(
            bluetooth.operation,
            PhyTxPowerTrackingExternalOperation::RegenerateBluetoothIeee802154Gain {
                gain_base: -7,
                image: state.bluetooth_ieee802154_tracking_gain_image(-7),
            }
        );

        assert_eq!(
            PhyTxPowerTrackingExternalBinding::lower(
                PhyTxPowerTrackingAction::Complete(PhyTxPowerTrackingOutcome {
                    class: PhyCalibrationTrackClass::Wifi,
                    gain_updated: false,
                    tracking_temperature: 0,
                    tracking_gain_base: 0,
                    wifi_gain_base: 0,
                    bluetooth_ieee802154_gain_base: 0,
                }),
                &state,
            ),
            Err(PhyTxPowerTrackingBindingError::UnsupportedAction)
        );
    }
}
