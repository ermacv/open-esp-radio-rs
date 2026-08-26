//! Source-owned outer transition for ESP32-S31 periodic PHY tracking.
//!
//! Blobray resolves the complete pinned `phy_param_track_tot` body to 47
//! instructions and eleven basic blocks. This module preserves that body's
//! critical-section boundary, guards, child-call order, arguments and optional
//! branches. The child actions remain explicit obligations: completing this
//! transition does not claim that every child effect is implemented. The two
//! TX-power actions lower into the complete source-owned transition in
//! [`crate::phy_power_tracking`], and the final temperature action lowers into
//! [`crate::phy_temperature`]. RFPLL-cap, calibration and Wi-Fi PHY-I2C
//! children remain separate obligations.
//!
//! The vendor function reads six bytes from a 508-byte `phy_param` image. The
//! live driver does not retain that ABI layout. Its only behaviorally relevant
//! projections are represented below as booleans and owned child inputs.

use core::fmt;

/// Active protocol classes passed by the shared-PHY scheduler.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyParamTrackRequest {
    wifi: bool,
    bluetooth_ieee802154: bool,
}

impl PhyParamTrackRequest {
    pub const fn new(wifi: bool, bluetooth_ieee802154: bool) -> Self {
        Self {
            wifi,
            bluetooth_ieee802154,
        }
    }

    pub const fn wifi(self) -> bool {
        self.wifi
    }

    pub const fn bluetooth_ieee802154(self) -> bool {
        self.bluetooth_ieee802154
    }
}

/// Rust-owned projections consumed by the outer tracking function.
///
/// `tracking_inhibited` is the OR of the two vendor guard bytes. Collapsing
/// them is exact because the body never distinguishes which guard stopped the
/// operation. The remaining fields preserve the arguments and predicates of
/// the child calls without importing the vendor parameter-image layout.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyParamTrackingParameters {
    pub tracking_inhibited: bool,
    pub rfpll_cap_tracking_enabled: bool,
    pub shared_tracking_control: u8,
    pub bluetooth_ieee802154_power_control: u8,
    pub calibration_tracking_enabled: bool,
    pub relaxed_power_tracking_threshold: bool,
}

/// Calibration class passed as the second `phy_cal_param_track` argument.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyCalibrationTrackClass {
    Wifi,
    BluetoothIeee802154,
}

impl PhyCalibrationTrackClass {
    /// Exact class selector used by the pinned vendor child.
    pub const fn selector(self) -> u8 {
        match self {
            Self::Wifi => 0,
            Self::BluetoothIeee802154 => 1,
        }
    }
}

/// Exact non-I/O arithmetic of rev0 ROM `phy_temp_to_power` at
/// `0x2f82_5f80`.
///
/// The ROM first truncates the temperature subtraction to a signed 16-bit
/// delta. Positive deltas use divisor five for both classes; zero and negative
/// deltas use divisor three for Wi-Fi and four for Bluetooth/IEEE 802.15.4.
/// The quotient becomes the protocol gain-base byte and is finally truncated
/// and sign-extended from eight bits.
pub const fn temperature_to_tracking_power(
    current_temperature: i16,
    reference_temperature: i16,
    class: PhyCalibrationTrackClass,
) -> i8 {
    let delta = current_temperature.wrapping_sub(reference_temperature);
    let divisor = match (class, delta > 0) {
        (_, true) => 5,
        (PhyCalibrationTrackClass::Wifi, false) => 3,
        (PhyCalibrationTrackClass::BluetoothIeee802154, false) => 4,
    };
    (delta / divisor) as i8
}

/// One externally executed child of `phy_param_track_tot`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyParamTrackingAction {
    EnterCritical,
    RfpllCapTrack {
        shared_tracking_control: u8,
    },
    BluetoothIeee802154TxPowerTrack {
        power_control: u8,
        shared_tracking_control: u8,
    },
    CalibrationTrack {
        shared_tracking_control: u8,
        class: PhyCalibrationTrackClass,
    },
    WifiI2cTrack,
    WifiTxPowerTrack {
        enabled: bool,
        shared_tracking_control: u8,
    },
    TemperatureRead,
    ExitCritical,
    Complete(PhyParamTrackingOutcome),
}

/// Identity-bound acknowledgement of one tracking action.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyParamTrackingCompletion {
    EnteredCritical,
    RfpllCapTracked {
        shared_tracking_control: u8,
    },
    BluetoothIeee802154TxPowerTracked {
        power_control: u8,
        shared_tracking_control: u8,
    },
    CalibrationTracked {
        shared_tracking_control: u8,
        class: PhyCalibrationTrackClass,
    },
    WifiI2cTracked,
    WifiTxPowerTracked {
        enabled: bool,
        shared_tracking_control: u8,
    },
    TemperatureRead,
    ExitedCritical,
}

/// Observable outer-wrapper result. Child hardware postconditions are not
/// implied by this value; each action must be backed by its own transition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyParamTrackingOutcome {
    pub clients: PhyParamTrackRequest,
    pub tracking_inhibited: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyParamTrackingTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PhyParamTrackingStep {
    EnterCritical,
    RfpllCapTrack,
    BluetoothIeee802154TxPowerTrack,
    BluetoothIeee802154CalibrationTrack,
    WifiI2cTrack,
    WifiTxPowerTrack,
    WifiCalibrationTrack,
    TemperatureRead,
    ExitCritical,
    Complete,
}

/// Finite exact-order outer transition for one scheduler request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyParamTrackingTransition {
    request: PhyParamTrackRequest,
    parameters: PhyParamTrackingParameters,
    step: PhyParamTrackingStep,
}

impl PhyParamTrackingTransition {
    pub const fn new(
        request: PhyParamTrackRequest,
        parameters: PhyParamTrackingParameters,
    ) -> Self {
        Self {
            request,
            parameters,
            step: PhyParamTrackingStep::EnterCritical,
        }
    }

    pub const fn action(self) -> PhyParamTrackingAction {
        match self.step {
            PhyParamTrackingStep::EnterCritical => PhyParamTrackingAction::EnterCritical,
            PhyParamTrackingStep::RfpllCapTrack => PhyParamTrackingAction::RfpllCapTrack {
                shared_tracking_control: self.parameters.shared_tracking_control,
            },
            PhyParamTrackingStep::BluetoothIeee802154TxPowerTrack => {
                PhyParamTrackingAction::BluetoothIeee802154TxPowerTrack {
                    power_control: self.parameters.bluetooth_ieee802154_power_control,
                    shared_tracking_control: self.parameters.shared_tracking_control,
                }
            }
            PhyParamTrackingStep::BluetoothIeee802154CalibrationTrack => {
                PhyParamTrackingAction::CalibrationTrack {
                    shared_tracking_control: self.parameters.shared_tracking_control,
                    class: PhyCalibrationTrackClass::BluetoothIeee802154,
                }
            }
            PhyParamTrackingStep::WifiI2cTrack => PhyParamTrackingAction::WifiI2cTrack,
            PhyParamTrackingStep::WifiTxPowerTrack => PhyParamTrackingAction::WifiTxPowerTrack {
                enabled: true,
                shared_tracking_control: self.parameters.shared_tracking_control,
            },
            PhyParamTrackingStep::WifiCalibrationTrack => {
                PhyParamTrackingAction::CalibrationTrack {
                    shared_tracking_control: self.parameters.shared_tracking_control,
                    class: PhyCalibrationTrackClass::Wifi,
                }
            }
            PhyParamTrackingStep::TemperatureRead => PhyParamTrackingAction::TemperatureRead,
            PhyParamTrackingStep::ExitCritical => PhyParamTrackingAction::ExitCritical,
            PhyParamTrackingStep::Complete => {
                PhyParamTrackingAction::Complete(PhyParamTrackingOutcome {
                    clients: self.request,
                    tracking_inhibited: self.parameters.tracking_inhibited,
                })
            }
        }
    }

    pub fn advance(
        &mut self,
        completion: PhyParamTrackingCompletion,
    ) -> Result<(), PhyParamTrackingTransitionError> {
        let next = match (self.step, completion) {
            (PhyParamTrackingStep::EnterCritical, PhyParamTrackingCompletion::EnteredCritical) => {
                if self.parameters.tracking_inhibited {
                    PhyParamTrackingStep::ExitCritical
                } else if self.parameters.rfpll_cap_tracking_enabled {
                    PhyParamTrackingStep::RfpllCapTrack
                } else {
                    self.first_client_step()
                }
            }
            (
                PhyParamTrackingStep::RfpllCapTrack,
                PhyParamTrackingCompletion::RfpllCapTracked {
                    shared_tracking_control,
                },
            ) if shared_tracking_control == self.parameters.shared_tracking_control => {
                self.first_client_step()
            }
            (
                PhyParamTrackingStep::BluetoothIeee802154TxPowerTrack,
                PhyParamTrackingCompletion::BluetoothIeee802154TxPowerTracked {
                    power_control,
                    shared_tracking_control,
                },
            ) if power_control == self.parameters.bluetooth_ieee802154_power_control
                && shared_tracking_control == self.parameters.shared_tracking_control =>
            {
                if self.parameters.calibration_tracking_enabled {
                    PhyParamTrackingStep::BluetoothIeee802154CalibrationTrack
                } else {
                    self.first_wifi_step()
                }
            }
            (
                PhyParamTrackingStep::BluetoothIeee802154CalibrationTrack,
                PhyParamTrackingCompletion::CalibrationTracked {
                    shared_tracking_control,
                    class: PhyCalibrationTrackClass::BluetoothIeee802154,
                },
            ) if shared_tracking_control == self.parameters.shared_tracking_control => {
                self.first_wifi_step()
            }
            (PhyParamTrackingStep::WifiI2cTrack, PhyParamTrackingCompletion::WifiI2cTracked) => {
                PhyParamTrackingStep::WifiTxPowerTrack
            }
            (
                PhyParamTrackingStep::WifiTxPowerTrack,
                PhyParamTrackingCompletion::WifiTxPowerTracked {
                    enabled: true,
                    shared_tracking_control,
                },
            ) if shared_tracking_control == self.parameters.shared_tracking_control => {
                if self.parameters.calibration_tracking_enabled {
                    PhyParamTrackingStep::WifiCalibrationTrack
                } else {
                    PhyParamTrackingStep::TemperatureRead
                }
            }
            (
                PhyParamTrackingStep::WifiCalibrationTrack,
                PhyParamTrackingCompletion::CalibrationTracked {
                    shared_tracking_control,
                    class: PhyCalibrationTrackClass::Wifi,
                },
            ) if shared_tracking_control == self.parameters.shared_tracking_control => {
                PhyParamTrackingStep::TemperatureRead
            }
            (
                PhyParamTrackingStep::TemperatureRead,
                PhyParamTrackingCompletion::TemperatureRead,
            ) => PhyParamTrackingStep::ExitCritical,
            (PhyParamTrackingStep::ExitCritical, PhyParamTrackingCompletion::ExitedCritical) => {
                PhyParamTrackingStep::Complete
            }
            (PhyParamTrackingStep::Complete, _) => {
                return Err(PhyParamTrackingTransitionError::AlreadyComplete);
            }
            _ => return Err(PhyParamTrackingTransitionError::WrongCompletion),
        };
        self.step = next;
        Ok(())
    }

    /// Lower only the currently selected TX-power child, using the immutable
    /// parameter snapshot captured with this outer transition.
    pub fn begin_tx_power_tracking<'state>(
        &self,
        state: &'state mut crate::phy_state::PhyState,
    ) -> Result<PhyParamTrackingTxPowerTransition<'state>, PhyParamTrackingChildError> {
        PhyParamTrackingTxPowerTransition::lower(self.action(), self.parameters, state)
    }

    /// Lower only the final temperature child while retaining exclusive access
    /// to the live state that will receive its terminal outcome.
    pub fn begin_temperature_read<'state>(
        &self,
        state: &'state mut crate::phy_state::PhyState,
    ) -> Result<PhyParamTrackingTemperatureTransition<'state>, PhyParamTrackingChildError> {
        PhyParamTrackingTemperatureTransition::lower(self.action(), state)
    }

    const fn first_client_step(self) -> PhyParamTrackingStep {
        if self.request.bluetooth_ieee802154 {
            PhyParamTrackingStep::BluetoothIeee802154TxPowerTrack
        } else {
            self.first_wifi_step()
        }
    }

    const fn first_wifi_step(self) -> PhyParamTrackingStep {
        if self.request.wifi {
            PhyParamTrackingStep::WifiI2cTrack
        } else {
            PhyParamTrackingStep::TemperatureRead
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyParamTrackingChildError {
    UnsupportedAction,
}

/// One exact TX-power child selected by the outer tracking transition.
///
/// This owner deliberately exposes no way to manufacture the corresponding
/// outer completion. The child must first reach its terminal action and commit
/// that outcome into the same live [`crate::phy_state::PhyState`].
pub struct PhyParamTrackingTxPowerTransition<'state> {
    parent_action: PhyParamTrackingAction,
    child: crate::phy_power_tracking::PhyTxPowerTrackingTransition,
    state: &'state mut crate::phy_state::PhyState,
}

impl fmt::Debug for PhyParamTrackingTxPowerTransition<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PhyParamTrackingTxPowerTransition")
            .field("parent_action", &self.parent_action)
            .field("action", &self.child.action())
            .finish_non_exhaustive()
    }
}

impl<'state> PhyParamTrackingTxPowerTransition<'state> {
    fn lower(
        parent_action: PhyParamTrackingAction,
        parameters: PhyParamTrackingParameters,
        state: &'state mut crate::phy_state::PhyState,
    ) -> Result<Self, PhyParamTrackingChildError> {
        let request = match parent_action {
            PhyParamTrackingAction::BluetoothIeee802154TxPowerTrack { power_control, .. } => {
                crate::phy_power_tracking::PhyTxPowerTrackingRequest {
                    class: PhyCalibrationTrackClass::BluetoothIeee802154,
                    enabled: power_control != 0,
                    wifi_channel: state.current_wifi_channel(),
                }
            }
            PhyParamTrackingAction::WifiTxPowerTrack { enabled, .. } => {
                crate::phy_power_tracking::PhyTxPowerTrackingRequest {
                    class: PhyCalibrationTrackClass::Wifi,
                    enabled,
                    wifi_channel: state.current_wifi_channel(),
                }
            }
            _ => return Err(PhyParamTrackingChildError::UnsupportedAction),
        };
        Ok(Self {
            parent_action,
            child: crate::phy_power_tracking::PhyTxPowerTrackingTransition::new(
                request,
                state.tx_power_tracking_parameters(parameters.relaxed_power_tracking_threshold),
            ),
            state,
        })
    }

    pub const fn parent_action(&self) -> PhyParamTrackingAction {
        self.parent_action
    }

    pub const fn action(&self) -> crate::phy_power_tracking::PhyTxPowerTrackingAction {
        self.child.action()
    }

    pub fn advance(
        &mut self,
        completion: crate::phy_power_tracking::PhyTxPowerTrackingCompletion,
    ) -> Result<(), crate::phy_power_tracking::PhyTxPowerTrackingTransitionError> {
        self.child.advance(completion)
    }

    /// Bind the current child action to the existing target HAL/PAC edge while
    /// the exact live state remains exclusively borrowed by this transaction.
    pub fn lower_external(
        &self,
    ) -> Result<
        crate::phy_power_tracking::PhyTxPowerTrackingExternalBinding,
        crate::phy_power_tracking::PhyTxPowerTrackingBindingError,
    > {
        crate::phy_power_tracking::PhyTxPowerTrackingExternalBinding::lower(
            self.action(),
            self.state,
        )
    }

    pub const fn state(&self) -> &crate::phy_state::PhyState {
        self.state
    }

    /// Commit the terminal child result and mint its identity-bound parent
    /// completion. An incomplete child is returned unchanged.
    pub fn commit(self) -> Result<PhyParamTrackingCompletion, Self> {
        let crate::phy_power_tracking::PhyTxPowerTrackingAction::Complete(outcome) =
            self.child.action()
        else {
            return Err(self);
        };
        self.state.apply_tx_power_tracking_outcome(outcome);
        Ok(match self.parent_action {
            PhyParamTrackingAction::BluetoothIeee802154TxPowerTrack {
                power_control,
                shared_tracking_control,
            } => PhyParamTrackingCompletion::BluetoothIeee802154TxPowerTracked {
                power_control,
                shared_tracking_control,
            },
            PhyParamTrackingAction::WifiTxPowerTrack {
                enabled,
                shared_tracking_control,
            } => PhyParamTrackingCompletion::WifiTxPowerTracked {
                enabled,
                shared_tracking_control,
            },
            _ => unreachable!(),
        })
    }
}

/// Final temperature child selected by the outer periodic transition.
///
/// A failed or incomplete sensor transaction retains this owner and cannot
/// mint `TemperatureRead`; the caller must poison the outer scheduler epoch.
pub struct PhyParamTrackingTemperatureTransition<'state> {
    child: crate::phy_temperature::PhyTemperatureTransition,
    state: &'state mut crate::phy_state::PhyState,
}

impl fmt::Debug for PhyParamTrackingTemperatureTransition<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PhyParamTrackingTemperatureTransition")
            .field("action", &self.child.action())
            .finish_non_exhaustive()
    }
}

impl<'state> PhyParamTrackingTemperatureTransition<'state> {
    fn lower(
        parent_action: PhyParamTrackingAction,
        state: &'state mut crate::phy_state::PhyState,
    ) -> Result<Self, PhyParamTrackingChildError> {
        if parent_action != PhyParamTrackingAction::TemperatureRead {
            return Err(PhyParamTrackingChildError::UnsupportedAction);
        }
        Ok(Self {
            child: crate::phy_temperature::PhyTemperatureTransition::new(),
            state,
        })
    }

    pub const fn action(&self) -> crate::phy_temperature::PhyTemperatureAction {
        self.child.action()
    }

    pub fn advance(
        &mut self,
        completion: crate::phy_temperature::PhyTemperatureCompletion,
    ) -> Result<(), crate::phy_temperature::PhyTemperatureTransitionError> {
        self.child.advance(completion)
    }

    pub fn lower_external(
        &self,
    ) -> Result<
        crate::phy_temperature::PhyTemperatureExternalBinding,
        crate::phy_temperature::PhyTemperatureBindingError,
    > {
        crate::phy_temperature::PhyTemperatureExternalBinding::lower(self.action())
    }

    pub const fn state(&self) -> &crate::phy_state::PhyState {
        self.state
    }

    /// Commit a successful sensor outcome and mint the exact parent
    /// completion. Failed and incomplete children are returned unchanged.
    pub fn commit(self) -> Result<PhyParamTrackingCompletion, Self> {
        let crate::phy_temperature::PhyTemperatureAction::Complete(outcome) = self.child.action()
        else {
            return Err(self);
        };
        self.state.apply_temperature_outcome(outcome);
        Ok(PhyParamTrackingCompletion::TemperatureRead)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec::Vec;

    const PARAMETERS: PhyParamTrackingParameters = PhyParamTrackingParameters {
        tracking_inhibited: false,
        rfpll_cap_tracking_enabled: true,
        shared_tracking_control: 0x29,
        bluetooth_ieee802154_power_control: 0x51,
        calibration_tracking_enabled: true,
        relaxed_power_tracking_threshold: false,
    };

    fn completion(action: PhyParamTrackingAction) -> PhyParamTrackingCompletion {
        match action {
            PhyParamTrackingAction::EnterCritical => PhyParamTrackingCompletion::EnteredCritical,
            PhyParamTrackingAction::RfpllCapTrack {
                shared_tracking_control,
            } => PhyParamTrackingCompletion::RfpllCapTracked {
                shared_tracking_control,
            },
            PhyParamTrackingAction::BluetoothIeee802154TxPowerTrack {
                power_control,
                shared_tracking_control,
            } => PhyParamTrackingCompletion::BluetoothIeee802154TxPowerTracked {
                power_control,
                shared_tracking_control,
            },
            PhyParamTrackingAction::CalibrationTrack {
                shared_tracking_control,
                class,
            } => PhyParamTrackingCompletion::CalibrationTracked {
                shared_tracking_control,
                class,
            },
            PhyParamTrackingAction::WifiI2cTrack => PhyParamTrackingCompletion::WifiI2cTracked,
            PhyParamTrackingAction::WifiTxPowerTrack {
                enabled,
                shared_tracking_control,
            } => PhyParamTrackingCompletion::WifiTxPowerTracked {
                enabled,
                shared_tracking_control,
            },
            PhyParamTrackingAction::TemperatureRead => PhyParamTrackingCompletion::TemperatureRead,
            PhyParamTrackingAction::ExitCritical => PhyParamTrackingCompletion::ExitedCritical,
            PhyParamTrackingAction::Complete(_) => panic!("terminal action has no completion"),
        }
    }

    fn run(
        request: PhyParamTrackRequest,
        parameters: PhyParamTrackingParameters,
    ) -> Vec<PhyParamTrackingAction> {
        let mut transition = PhyParamTrackingTransition::new(request, parameters);
        let mut actions = Vec::new();
        for _ in 0..10 {
            let action = transition.action();
            actions.push(action);
            if matches!(action, PhyParamTrackingAction::Complete(_)) {
                return actions;
            }
            transition.advance(completion(action)).unwrap();
        }
        panic!("finite tracking transition exceeded its maximum path length")
    }

    #[test]
    fn ieee802154_only_preserves_exact_child_order() {
        assert_eq!(
            run(PhyParamTrackRequest::new(false, true), PARAMETERS),
            [
                PhyParamTrackingAction::EnterCritical,
                PhyParamTrackingAction::RfpllCapTrack {
                    shared_tracking_control: 0x29,
                },
                PhyParamTrackingAction::BluetoothIeee802154TxPowerTrack {
                    power_control: 0x51,
                    shared_tracking_control: 0x29,
                },
                PhyParamTrackingAction::CalibrationTrack {
                    shared_tracking_control: 0x29,
                    class: PhyCalibrationTrackClass::BluetoothIeee802154,
                },
                PhyParamTrackingAction::TemperatureRead,
                PhyParamTrackingAction::ExitCritical,
                PhyParamTrackingAction::Complete(PhyParamTrackingOutcome {
                    clients: PhyParamTrackRequest::new(false, true),
                    tracking_inhibited: false,
                }),
            ]
        );
    }

    #[test]
    fn both_classes_run_bluetooth_before_wifi_and_temperature_last() {
        assert_eq!(
            run(PhyParamTrackRequest::new(true, true), PARAMETERS),
            [
                PhyParamTrackingAction::EnterCritical,
                PhyParamTrackingAction::RfpllCapTrack {
                    shared_tracking_control: 0x29,
                },
                PhyParamTrackingAction::BluetoothIeee802154TxPowerTrack {
                    power_control: 0x51,
                    shared_tracking_control: 0x29,
                },
                PhyParamTrackingAction::CalibrationTrack {
                    shared_tracking_control: 0x29,
                    class: PhyCalibrationTrackClass::BluetoothIeee802154,
                },
                PhyParamTrackingAction::WifiI2cTrack,
                PhyParamTrackingAction::WifiTxPowerTrack {
                    enabled: true,
                    shared_tracking_control: 0x29,
                },
                PhyParamTrackingAction::CalibrationTrack {
                    shared_tracking_control: 0x29,
                    class: PhyCalibrationTrackClass::Wifi,
                },
                PhyParamTrackingAction::TemperatureRead,
                PhyParamTrackingAction::ExitCritical,
                PhyParamTrackingAction::Complete(PhyParamTrackingOutcome {
                    clients: PhyParamTrackRequest::new(true, true),
                    tracking_inhibited: false,
                }),
            ]
        );
    }

    #[test]
    fn guard_exits_critical_section_without_running_children() {
        let mut parameters = PARAMETERS;
        parameters.tracking_inhibited = true;
        assert_eq!(
            run(PhyParamTrackRequest::new(true, true), parameters),
            [
                PhyParamTrackingAction::EnterCritical,
                PhyParamTrackingAction::ExitCritical,
                PhyParamTrackingAction::Complete(PhyParamTrackingOutcome {
                    clients: PhyParamTrackRequest::new(true, true),
                    tracking_inhibited: true,
                }),
            ]
        );
    }

    #[test]
    fn disabled_optional_branches_are_absent() {
        let parameters = PhyParamTrackingParameters {
            rfpll_cap_tracking_enabled: false,
            calibration_tracking_enabled: false,
            ..PARAMETERS
        };
        assert_eq!(
            run(PhyParamTrackRequest::new(false, true), parameters),
            [
                PhyParamTrackingAction::EnterCritical,
                PhyParamTrackingAction::BluetoothIeee802154TxPowerTrack {
                    power_control: 0x51,
                    shared_tracking_control: 0x29,
                },
                PhyParamTrackingAction::TemperatureRead,
                PhyParamTrackingAction::ExitCritical,
                PhyParamTrackingAction::Complete(PhyParamTrackingOutcome {
                    clients: PhyParamTrackRequest::new(false, true),
                    tracking_inhibited: false,
                }),
            ]
        );
    }

    #[test]
    fn wrong_completion_does_not_advance_and_terminal_rejects_more_work() {
        let mut transition =
            PhyParamTrackingTransition::new(PhyParamTrackRequest::new(false, true), PARAMETERS);
        assert_eq!(
            transition.advance(PhyParamTrackingCompletion::TemperatureRead),
            Err(PhyParamTrackingTransitionError::WrongCompletion)
        );
        assert_eq!(transition.action(), PhyParamTrackingAction::EnterCritical);

        while !matches!(transition.action(), PhyParamTrackingAction::Complete(_)) {
            transition.advance(completion(transition.action())).unwrap();
        }
        assert_eq!(
            transition.advance(PhyParamTrackingCompletion::ExitedCritical),
            Err(PhyParamTrackingTransitionError::AlreadyComplete)
        );
    }

    #[test]
    fn failed_temperature_child_cannot_complete_parent_or_mutate_state() {
        let mut parameters = PARAMETERS;
        parameters.rfpll_cap_tracking_enabled = false;
        let mut transition =
            PhyParamTrackingTransition::new(PhyParamTrackRequest::new(false, false), parameters);
        transition
            .advance(PhyParamTrackingCompletion::EnteredCritical)
            .unwrap();
        assert_eq!(transition.action(), PhyParamTrackingAction::TemperatureRead);

        let mut state = crate::phy_state::PhyState::new(crate::phy_state::PhyConfig::production());
        let mut temperature = transition.begin_temperature_read(&mut state).unwrap();
        let crate::phy_temperature::PhyTemperatureAction::ReadMasked {
            address,
            high_bit,
            low_bit,
        } = temperature.action()
        else {
            panic!("temperature child did not begin with its DAC read")
        };
        temperature
            .advance(
                crate::phy_temperature::PhyTemperatureCompletion::MaskedRead {
                    address,
                    high_bit,
                    low_bit,
                    value: 3,
                },
            )
            .unwrap();
        assert_eq!(
            temperature.action(),
            crate::phy_temperature::PhyTemperatureAction::Failed(
                crate::phy_temperature::PhyTemperatureFailure::InvalidDac(3),
            )
        );
        let temperature = temperature.commit().unwrap_err();
        assert_eq!(
            temperature
                .state()
                .tx_power_tracking_parameters(false)
                .current_temperature,
            0
        );
    }

    #[test]
    fn temperature_to_power_matches_all_signed_16_bit_deltas() {
        for raw_delta in 0..=u16::MAX {
            let delta = raw_delta as i16;
            let expected_wifi = ((i32::from(delta) / 3) as u8) as i8;
            let expected_bluetooth_ieee802154 = ((i32::from(delta) / 4) as u8) as i8;
            let expected_positive = ((i32::from(delta) / 5) as u8) as i8;

            assert_eq!(
                temperature_to_tracking_power(
                    0,
                    0_i16.wrapping_sub(delta),
                    PhyCalibrationTrackClass::Wifi
                ),
                if delta > 0 {
                    expected_positive
                } else {
                    expected_wifi
                },
            );
            assert_eq!(
                temperature_to_tracking_power(
                    0,
                    0_i16.wrapping_sub(delta),
                    PhyCalibrationTrackClass::BluetoothIeee802154,
                ),
                if delta > 0 {
                    expected_positive
                } else {
                    expected_bluetooth_ieee802154
                },
            );
        }

        assert_eq!(
            temperature_to_tracking_power(
                i16::MIN,
                1,
                PhyCalibrationTrackClass::BluetoothIeee802154,
            ),
            (32_767_i16 / 5) as i8,
        );
    }
}
