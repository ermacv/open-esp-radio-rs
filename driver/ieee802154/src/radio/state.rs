//! The single finite owner that admits commands and validates backend events.
//! Validation errors and admission results describe this owner without retaining frames.

use super::{
    RequestId,
    capabilities::RadioCapabilities,
    channel::Channel,
    command::{CommandKind, Configuration, RadioCommand, TxMode},
    event::{RadioEvent, TxStatus},
};

/// Stable state to which an asynchronous operation returns.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum RestingState {
    /// Radio is enabled but not receiving.
    Sleeping,
    /// Radio is receiving on one channel.
    Receiving {
        /// Active receive channel.
        channel: Channel,
    },
}

/// Complete finite portable controller state.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum RadioState {
    /// Platform radio ownership is not acquired.
    Disabled,
    /// Enabled stable state.
    Resting(RestingState),
    /// One transmit request is owned by the backend.
    Transmitting {
        /// Active correlation identifier.
        id: RequestId,
        /// Transmit channel.
        channel: Channel,
        /// Stable state restored by terminal completion.
        resume: RestingState,
    },
    /// One energy scan is owned by the backend.
    EnergyScanning {
        /// Active correlation identifier.
        id: RequestId,
        /// Scan channel.
        channel: Channel,
        /// Stable state restored by terminal completion.
        resume: RestingState,
    },
    /// One standalone clear-channel assessment is owned by the backend.
    AssessingChannel {
        /// Active correlation identifier.
        id: RequestId,
        /// Assessed channel.
        channel: Channel,
        /// Stable state restored by terminal completion.
        resume: RestingState,
    },
}

/// Successful pure admission of one command.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct AcceptedCommand {
    /// Correlation identifier.
    pub id: RequestId,
    /// Admitted operation kind.
    pub kind: CommandKind,
    /// State before admission.
    pub previous: RadioState,
    /// State owned by the backend after admission.
    pub current: RadioState,
}

/// A command cannot be admitted in the current finite state/capability set.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum CommandError {
    /// Only enable is accepted while disabled.
    Disabled,
    /// Enable was requested for an already enabled controller.
    AlreadyEnabled,
    /// An asynchronous operation already owns the radio.
    Busy {
        /// Complete active state.
        state: RadioState,
    },
    /// The controller did not publish the required capability.
    Unsupported {
        /// Rejected operation kind.
        command: CommandKind,
        /// Missing capability flag.
        required: RadioCapabilities,
    },
}

/// A backend event does not match the operation/state it claims to complete.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum EventError {
    /// This event kind is invalid in the current state.
    Unexpected {
        /// Complete current state.
        state: RadioState,
    },
    /// A terminal event named a different operation.
    RequestMismatch {
        /// Expected active identifier.
        expected: RequestId,
        /// Identifier published by the backend.
        actual: RequestId,
    },
    /// Receive or acknowledgement metadata named a different channel.
    ChannelMismatch {
        /// Channel owned by the active state.
        expected: Channel,
        /// Channel published in metadata.
        actual: Channel,
    },
    /// A failed transmit must not carry a successful acknowledgement frame.
    AcknowledgementOnFailedTransmit,
    /// A fault identifier is inconsistent with the active operation.
    FaultRequestMismatch {
        /// Active identifier, if any.
        expected: Option<RequestId>,
        /// Identifier published by the fault event.
        actual: Option<RequestId>,
    },
}

/// Pure finite command/event admission state.
///
/// Fields are private so a caller cannot manufacture an active operation or
/// skip capability checks:
///
/// ```compile_fail
/// use open_esp_radio_ieee802154::{RadioCapabilities, RadioState, RadioStateMachine};
/// let forged = RadioStateMachine {
///     state: RadioState::Disabled,
///     capabilities: RadioCapabilities::NONE,
/// };
/// ```
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct RadioStateMachine {
    state: RadioState,
    capabilities: RadioCapabilities,
}

impl RadioStateMachine {
    /// Construct one disabled controller contract.
    pub const fn new(capabilities: RadioCapabilities) -> Self {
        Self {
            state: RadioState::Disabled,
            capabilities,
        }
    }

    /// Return the immutable backend capability set.
    pub const fn capabilities(&self) -> RadioCapabilities {
        self.capabilities
    }

    /// Return the complete current state.
    pub const fn state(&self) -> RadioState {
        self.state
    }

    /// Validate and admit one command, advancing state exactly once.
    ///
    /// A backend must retain any borrowed transmit bytes before this call
    /// returns. The state machine itself never retains those bytes.
    pub fn admit(&mut self, command: RadioCommand<'_>) -> Result<AcceptedCommand, CommandError> {
        let previous = self.state;
        let kind = command.kind();
        let id = command.id();
        let current = match command {
            RadioCommand::Enable { .. } => match previous {
                RadioState::Disabled => RadioState::Resting(RestingState::Sleeping),
                _ => return Err(CommandError::AlreadyEnabled),
            },
            _ if previous == RadioState::Disabled => return Err(CommandError::Disabled),
            RadioCommand::Disable { .. } => match resting(previous) {
                Some(_) => RadioState::Disabled,
                None => return Err(CommandError::Busy { state: previous }),
            },
            RadioCommand::Sleep { .. } => match resting(previous) {
                Some(_) => RadioState::Resting(RestingState::Sleeping),
                None => return Err(CommandError::Busy { state: previous }),
            },
            RadioCommand::Receive { channel, .. } => match resting(previous) {
                Some(_) => RadioState::Resting(RestingState::Receiving { channel }),
                None => return Err(CommandError::Busy { state: previous }),
            },
            RadioCommand::Configure { configuration, .. } => {
                let Some(_) = resting(previous) else {
                    return Err(CommandError::Busy { state: previous });
                };
                if !self.capabilities.supports_configuration(configuration) {
                    let required = required_configuration_capability(configuration);
                    return Err(CommandError::Unsupported {
                        command: kind,
                        required,
                    });
                }
                previous
            }
            RadioCommand::Transmit(request) => {
                let Some(resume) = resting(previous) else {
                    return Err(CommandError::Busy { state: previous });
                };
                if !self.capabilities.supports_tx_mode(request.mode) {
                    return Err(CommandError::Unsupported {
                        command: kind,
                        required: required_tx_capability(request.mode),
                    });
                }
                if request.frame.acknowledgement_requested()
                    && !self
                        .capabilities
                        .contains(RadioCapabilities::HARDWARE_ACKNOWLEDGEMENT)
                {
                    return Err(CommandError::Unsupported {
                        command: kind,
                        required: RadioCapabilities::HARDWARE_ACKNOWLEDGEMENT,
                    });
                }
                if request.transmit_power_dbm.is_some()
                    && !self
                        .capabilities
                        .contains(RadioCapabilities::TRANSMIT_POWER)
                {
                    return Err(CommandError::Unsupported {
                        command: kind,
                        required: RadioCapabilities::TRANSMIT_POWER,
                    });
                }
                RadioState::Transmitting {
                    id,
                    channel: request.channel,
                    resume,
                }
            }
            RadioCommand::EnergyScan(request) => {
                let Some(resume) = resting(previous) else {
                    return Err(CommandError::Busy { state: previous });
                };
                require_capability(self.capabilities, kind, RadioCapabilities::ENERGY_SCAN)?;
                RadioState::EnergyScanning {
                    id,
                    channel: request.channel,
                    resume,
                }
            }
            RadioCommand::ClearChannelAssessment { channel, .. } => {
                let Some(resume) = resting(previous) else {
                    return Err(CommandError::Busy { state: previous });
                };
                require_capability(
                    self.capabilities,
                    kind,
                    RadioCapabilities::CLEAR_CHANNEL_ASSESSMENT,
                )?;
                RadioState::AssessingChannel {
                    id,
                    channel,
                    resume,
                }
            }
        };

        self.state = current;
        Ok(AcceptedCommand {
            id,
            kind,
            previous,
            current,
        })
    }

    /// Validate one backend event and apply its exact state transition.
    pub fn observe(&mut self, event: RadioEvent<'_>) -> Result<(), EventError> {
        let next = match (self.state, event) {
            (
                RadioState::Resting(RestingState::Receiving { channel }),
                RadioEvent::Received(rx),
            ) => {
                require_channel(channel, rx.metadata.channel)?;
                self.state
            }
            (
                RadioState::Transmitting {
                    id: expected,
                    channel,
                    resume,
                },
                RadioEvent::TransmitDone {
                    id,
                    status,
                    acknowledgement,
                },
            ) => {
                require_id(expected, id)?;
                if status != TxStatus::Success && acknowledgement.is_some() {
                    return Err(EventError::AcknowledgementOnFailedTransmit);
                }
                if let Some(acknowledgement) = acknowledgement {
                    require_channel(channel, acknowledgement.metadata.channel)?;
                }
                RadioState::Resting(resume)
            }
            (
                RadioState::EnergyScanning {
                    id: expected,
                    resume,
                    ..
                },
                RadioEvent::EnergyScanDone { id, .. },
            ) => {
                require_id(expected, id)?;
                RadioState::Resting(resume)
            }
            (
                RadioState::AssessingChannel {
                    id: expected,
                    resume,
                    ..
                },
                RadioEvent::ClearChannelAssessmentDone { id, .. },
            ) => {
                require_id(expected, id)?;
                RadioState::Resting(resume)
            }
            (state, RadioEvent::Fault { id, .. }) => {
                let expected = active_id(state);
                if id != expected {
                    return Err(EventError::FaultRequestMismatch {
                        expected,
                        actual: id,
                    });
                }
                RadioState::Disabled
            }
            (state, _) => return Err(EventError::Unexpected { state }),
        };
        self.state = next;
        Ok(())
    }
}

const fn resting(state: RadioState) -> Option<RestingState> {
    if let RadioState::Resting(resting) = state {
        Some(resting)
    } else {
        None
    }
}

const fn active_id(state: RadioState) -> Option<RequestId> {
    match state {
        RadioState::Transmitting { id, .. }
        | RadioState::EnergyScanning { id, .. }
        | RadioState::AssessingChannel { id, .. } => Some(id),
        RadioState::Disabled | RadioState::Resting(_) => None,
    }
}

const fn required_tx_capability(mode: TxMode) -> RadioCapabilities {
    match mode {
        TxMode::Direct => RadioCapabilities::NONE,
        TxMode::ClearChannelAssessment => RadioCapabilities::CLEAR_CHANNEL_ASSESSMENT,
        TxMode::CsmaCa { .. } => RadioCapabilities::CSMA_CA,
        TxMode::Scheduled { .. } => RadioCapabilities::SCHEDULED_TRANSMIT,
    }
}

const fn required_configuration_capability(configuration: Configuration) -> RadioCapabilities {
    match configuration {
        Configuration::Promiscuous(_) => RadioCapabilities::PROMISCUOUS,
        Configuration::AutomaticAcknowledgement(_) => RadioCapabilities::AUTOMATIC_ACKNOWLEDGEMENT,
        Configuration::TransmitPowerDbm(_) => RadioCapabilities::TRANSMIT_POWER,
        Configuration::PanId(_)
        | Configuration::ShortAddress(_)
        | Configuration::ExtendedAddress(_) => RadioCapabilities::NONE,
    }
}

fn require_capability(
    capabilities: RadioCapabilities,
    command: CommandKind,
    required: RadioCapabilities,
) -> Result<(), CommandError> {
    if capabilities.contains(required) {
        Ok(())
    } else {
        Err(CommandError::Unsupported { command, required })
    }
}

fn require_id(expected: RequestId, actual: RequestId) -> Result<(), EventError> {
    if expected == actual {
        Ok(())
    } else {
        Err(EventError::RequestMismatch { expected, actual })
    }
}

fn require_channel(expected: Channel, actual: Channel) -> Result<(), EventError> {
    if expected == actual {
        Ok(())
    } else {
        Err(EventError::ChannelMismatch { expected, actual })
    }
}

#[cfg(test)]
mod tests;
