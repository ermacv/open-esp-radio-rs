//! Source-owned outer transition for ESP32-S31 periodic PHY tracking.
//!
//! Blobray resolves the complete pinned `phy_param_track_tot` body to 47
//! instructions and eleven basic blocks. This module preserves that body's
//! critical-section boundary, guards, child-call order, arguments and optional
//! branches. The child actions remain explicit obligations: completing this
//! transition does not claim that their still-unrecovered register effects are
//! implemented.
//!
//! The vendor function reads six bytes from a 508-byte `phy_param` image. The
//! live driver does not retain that ABI layout. Its only behaviorally relevant
//! projections are represented below as booleans and owned child inputs.

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
}
