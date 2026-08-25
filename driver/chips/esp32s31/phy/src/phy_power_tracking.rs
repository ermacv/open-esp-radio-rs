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
    pub previous_tracking_adjustment: i8,
    pub wifi_adjustment: i8,
    pub bluetooth_ieee802154_adjustment: i8,
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
    pub adjustment: i8,
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
    let adjustment = if recomputed {
        temperature_to_tracking_power(
            bounded_temperature,
            parameters.reference_temperature,
            request.class,
        )
    } else {
        parameters.previous_tracking_adjustment
    };
    let published_adjustment = match request.class {
        PhyCalibrationTrackClass::Wifi => parameters.wifi_adjustment,
        PhyCalibrationTrackClass::BluetoothIeee802154 => parameters.bluetooth_ieee802154_adjustment,
    };

    PhyTxPowerTrackingDecision {
        bounded_temperature,
        threshold,
        adjustment,
        recomputed,
        update_required: request.enabled && adjustment != published_adjustment,
    }
}

/// Retained result committed only after the gain child and BBPLL tail finish.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyTxPowerTrackingOutcome {
    pub class: PhyCalibrationTrackClass,
    pub gain_updated: bool,
    pub tracking_temperature: i16,
    pub tracking_adjustment: i8,
    pub wifi_adjustment: i8,
    pub bluetooth_ieee802154_adjustment: i8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxPowerTrackingAction {
    SetBbpllCalibration { enabled: bool },
    PublishWifiGain { channel: u16, adjustment: i8 },
    PublishBluetoothIeee802154Gain { adjustment: i8 },
    Complete(PhyTxPowerTrackingOutcome),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxPowerTrackingCompletion {
    BbpllCalibrationSet { enabled: bool },
    WifiGainPublished { channel: u16, adjustment: i8 },
    BluetoothIeee802154GainPublished { adjustment: i8 },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxPowerTrackingTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PhyTxPowerTrackingStep {
    BbpllOn,
    PublishGain,
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
            tracking_adjustment: parameters.previous_tracking_adjustment,
            wifi_adjustment: parameters.wifi_adjustment,
            bluetooth_ieee802154_adjustment: parameters.bluetooth_ieee802154_adjustment,
        };
        let step = if decision.update_required {
            outcome.tracking_temperature = parameters.current_temperature;
            outcome.tracking_adjustment = decision.adjustment;
            match request.class {
                PhyCalibrationTrackClass::Wifi => {
                    outcome.wifi_adjustment = decision.adjustment;
                }
                PhyCalibrationTrackClass::BluetoothIeee802154 => {
                    outcome.bluetooth_ieee802154_adjustment = decision.adjustment;
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
            PhyTxPowerTrackingStep::PublishGain => match self.request.class {
                PhyCalibrationTrackClass::Wifi => PhyTxPowerTrackingAction::PublishWifiGain {
                    channel: self.request.wifi_channel,
                    adjustment: self.decision.adjustment,
                },
                PhyCalibrationTrackClass::BluetoothIeee802154 => {
                    PhyTxPowerTrackingAction::PublishBluetoothIeee802154Gain {
                        adjustment: self.decision.adjustment,
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
            ) => PhyTxPowerTrackingStep::PublishGain,
            (
                PhyTxPowerTrackingStep::PublishGain,
                PhyTxPowerTrackingCompletion::WifiGainPublished {
                    channel,
                    adjustment,
                },
            ) if self.request.class == PhyCalibrationTrackClass::Wifi
                && channel == self.request.wifi_channel
                && adjustment == self.decision.adjustment =>
            {
                PhyTxPowerTrackingStep::BbpllOff
            }
            (
                PhyTxPowerTrackingStep::PublishGain,
                PhyTxPowerTrackingCompletion::BluetoothIeee802154GainPublished { adjustment },
            ) if self.request.class == PhyCalibrationTrackClass::BluetoothIeee802154
                && adjustment == self.decision.adjustment =>
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

#[cfg(test)]
mod tests {
    use super::*;

    const PARAMETERS: PhyTxPowerTrackingParameters = PhyTxPowerTrackingParameters {
        current_temperature: 25,
        reference_temperature: 0,
        previous_tracking_temperature: 0,
        previous_tracking_adjustment: -3,
        wifi_adjustment: 1,
        bluetooth_ieee802154_adjustment: 2,
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
        assert_eq!(decision.adjustment, PARAMETERS.previous_tracking_adjustment);

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
    fn disabled_or_equal_adjustment_commits_nothing() {
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
                tracking_adjustment: PARAMETERS.previous_tracking_adjustment,
                wifi_adjustment: PARAMETERS.wifi_adjustment,
                bluetooth_ieee802154_adjustment: PARAMETERS.bluetooth_ieee802154_adjustment,
            })
        );

        let same = PhyTxPowerTrackingParameters {
            bluetooth_ieee802154_adjustment: 5,
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
                tracking_adjustment: same.previous_tracking_adjustment,
                wifi_adjustment: same.wifi_adjustment,
                bluetooth_ieee802154_adjustment: same.bluetooth_ieee802154_adjustment,
            })
        );
    }

    #[test]
    fn bluetooth_ieee802154_update_owns_bbpll_and_gain_order() {
        let mut transition = PhyTxPowerTrackingTransition::new(
            request(PhyCalibrationTrackClass::BluetoothIeee802154),
            PARAMETERS,
        );
        assert_eq!(transition.decision().adjustment, 5);
        assert_eq!(
            transition.action(),
            PhyTxPowerTrackingAction::SetBbpllCalibration { enabled: true }
        );
        transition
            .advance(PhyTxPowerTrackingCompletion::BbpllCalibrationSet { enabled: true })
            .unwrap();
        assert_eq!(
            transition.action(),
            PhyTxPowerTrackingAction::PublishBluetoothIeee802154Gain { adjustment: 5 }
        );
        transition
            .advance(
                PhyTxPowerTrackingCompletion::BluetoothIeee802154GainPublished { adjustment: 5 },
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
                tracking_adjustment: 5,
                wifi_adjustment: PARAMETERS.wifi_adjustment,
                bluetooth_ieee802154_adjustment: 5,
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
            transition.advance(PhyTxPowerTrackingCompletion::WifiGainPublished {
                channel: 6,
                adjustment: 5,
            }),
            Err(PhyTxPowerTrackingTransitionError::WrongCompletion)
        );
        assert_eq!(
            transition.action(),
            PhyTxPowerTrackingAction::PublishWifiGain {
                channel: 11,
                adjustment: 5,
            }
        );
    }
}
