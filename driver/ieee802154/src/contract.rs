use crate::{
    Channel, Configuration, EnergyScanRequest, RadioCapabilities, RadioFault, ReceivedFrame,
    RequestId, TxMode, TxRequest, TxStatus,
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

/// Portable Host-to-radio operation.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum RadioCommand<'frame> {
    /// Acquire the radio and enter sleep.
    Enable {
        /// Caller-owned correlation identifier.
        id: RequestId,
    },
    /// Release an enabled, non-busy radio.
    Disable {
        /// Caller-owned correlation identifier.
        id: RequestId,
    },
    /// Leave receive mode and enter sleep.
    Sleep {
        /// Caller-owned correlation identifier.
        id: RequestId,
    },
    /// Enter receive mode on one channel.
    Receive {
        /// Caller-owned correlation identifier.
        id: RequestId,
        /// Requested receive channel.
        channel: Channel,
    },
    /// Apply one portable address/filter/power setting.
    Configure {
        /// Caller-owned correlation identifier.
        id: RequestId,
        /// Complete setting update.
        configuration: Configuration,
    },
    /// Transfer a borrowed frame to the backend for the duration of command
    /// admission. A hardware adapter must copy or otherwise retain the bytes
    /// under its own explicit ownership before returning from admission.
    Transmit(TxRequest<'frame>),
    /// Perform one bounded energy scan.
    EnergyScan(EnergyScanRequest),
    /// Perform one standalone clear-channel assessment.
    ClearChannelAssessment {
        /// Caller-owned correlation identifier.
        id: RequestId,
        /// Channel to assess.
        channel: Channel,
    },
}

impl RadioCommand<'_> {
    /// Return the caller-owned correlation identifier.
    pub const fn id(self) -> RequestId {
        match self {
            Self::Enable { id }
            | Self::Disable { id }
            | Self::Sleep { id }
            | Self::Receive { id, .. }
            | Self::Configure { id, .. }
            | Self::ClearChannelAssessment { id, .. } => id,
            Self::Transmit(request) => request.id,
            Self::EnergyScan(request) => request.id,
        }
    }

    /// Return the finite operation kind without retaining frame bytes.
    pub const fn kind(self) -> CommandKind {
        match self {
            Self::Enable { .. } => CommandKind::Enable,
            Self::Disable { .. } => CommandKind::Disable,
            Self::Sleep { .. } => CommandKind::Sleep,
            Self::Receive { .. } => CommandKind::Receive,
            Self::Configure { .. } => CommandKind::Configure,
            Self::Transmit(_) => CommandKind::Transmit,
            Self::EnergyScan(_) => CommandKind::EnergyScan,
            Self::ClearChannelAssessment { .. } => CommandKind::ClearChannelAssessment,
        }
    }
}

/// Frame-free command discriminator suitable for bounded mailboxes and logs.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum CommandKind {
    /// Enable operation.
    Enable,
    /// Disable operation.
    Disable,
    /// Sleep operation.
    Sleep,
    /// Receive operation.
    Receive,
    /// Configuration operation.
    Configure,
    /// Transmit operation.
    Transmit,
    /// Energy-scan operation.
    EnergyScan,
    /// Standalone CCA operation.
    ClearChannelAssessment,
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

/// Backend-to-Host observation.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum RadioEvent<'frame> {
    /// One frame was received while the state machine owns receive mode.
    Received(ReceivedFrame<'frame>),
    /// Terminal transmit completion.
    TransmitDone {
        /// Correlation identifier from the accepted request.
        id: RequestId,
        /// Portable completion category.
        status: TxStatus,
        /// Optional received acknowledgement MAC bytes and metadata.
        acknowledgement: Option<ReceivedFrame<'frame>>,
    },
    /// Terminal energy scan completion.
    EnergyScanDone {
        /// Correlation identifier from the accepted request.
        id: RequestId,
        /// Maximum observed RSSI normalized to dBm.
        maximum_rssi_dbm: i8,
    },
    /// Terminal standalone CCA completion.
    ClearChannelAssessmentDone {
        /// Correlation identifier from the accepted request.
        id: RequestId,
        /// Whether the assessment found the channel idle.
        idle: bool,
    },
    /// Fail-closed backend fault. A valid fault disables the state machine.
    Fault {
        /// Active operation identifier, or `None` outside an operation.
        id: Option<RequestId>,
        /// Portable fault category.
        fault: RadioFault,
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
mod tests {
    use super::*;
    use crate::{FcsStatus, FramePending, FrameView, RxMetadata, SecurityStatus};

    const ID: RequestId = RequestId::new(7);

    fn channel(raw: u8) -> Channel {
        Channel::new(raw).unwrap()
    }

    fn metadata(channel: Channel) -> RxMetadata {
        RxMetadata {
            channel,
            rssi_dbm: -42,
            link_quality: 211,
            timestamp: None,
            fcs: FcsStatus::Valid,
            security: SecurityStatus::Unprocessed,
            frame_pending: crate::FramePending::Unavailable,
        }
    }

    fn enabled(capabilities: RadioCapabilities) -> RadioStateMachine {
        let mut machine = RadioStateMachine::new(capabilities);
        machine.admit(RadioCommand::Enable { id: ID }).unwrap();
        machine
    }

    #[test]
    fn finite_enable_receive_sleep_disable_path_is_exact() {
        let mut machine = RadioStateMachine::new(RadioCapabilities::NONE);
        assert_eq!(machine.state(), RadioState::Disabled);
        let enable = machine.admit(RadioCommand::Enable { id: ID }).unwrap();
        assert_eq!(enable.previous, RadioState::Disabled);
        assert_eq!(machine.state(), RadioState::Resting(RestingState::Sleeping));

        machine
            .admit(RadioCommand::Receive {
                id: RequestId::new(8),
                channel: channel(15),
            })
            .unwrap();
        assert_eq!(
            machine.state(),
            RadioState::Resting(RestingState::Receiving {
                channel: channel(15)
            })
        );
        machine
            .admit(RadioCommand::Sleep {
                id: RequestId::new(9),
            })
            .unwrap();
        machine
            .admit(RadioCommand::Disable {
                id: RequestId::new(10),
            })
            .unwrap();
        assert_eq!(machine.state(), RadioState::Disabled);
    }

    #[test]
    fn transmit_correlates_completion_and_restores_receive() {
        let capabilities = RadioCapabilities::CSMA_CA
            | RadioCapabilities::HARDWARE_ACKNOWLEDGEMENT
            | RadioCapabilities::TRANSMIT_POWER;
        let mut machine = enabled(capabilities);
        machine
            .admit(RadioCommand::Receive {
                id: RequestId::new(1),
                channel: channel(20),
            })
            .unwrap();
        let bytes = [0x61, 0x88, 0x2a];
        let request = TxRequest {
            id: ID,
            frame: FrameView::new(&bytes).unwrap(),
            channel: channel(20),
            mode: TxMode::CsmaCa { max_backoffs: 4 },
            transmit_power_dbm: Some(3),
        };
        machine.admit(RadioCommand::Transmit(request)).unwrap();
        assert_eq!(
            machine.admit(RadioCommand::Sleep {
                id: RequestId::new(99)
            }),
            Err(CommandError::Busy {
                state: machine.state()
            })
        );
        assert_eq!(
            machine.observe(RadioEvent::TransmitDone {
                id: RequestId::new(6),
                status: TxStatus::Success,
                acknowledgement: None,
            }),
            Err(EventError::RequestMismatch {
                expected: ID,
                actual: RequestId::new(6),
            })
        );
        machine
            .observe(RadioEvent::TransmitDone {
                id: ID,
                status: TxStatus::Success,
                acknowledgement: None,
            })
            .unwrap();
        assert_eq!(
            machine.state(),
            RadioState::Resting(RestingState::Receiving {
                channel: channel(20)
            })
        );
    }

    #[test]
    fn unsupported_operations_leave_state_unchanged() {
        let mut machine = enabled(RadioCapabilities::NONE);
        let before = machine.state();
        assert_eq!(
            machine.admit(RadioCommand::EnergyScan(EnergyScanRequest {
                id: ID,
                channel: channel(11),
                duration_us: 128,
            })),
            Err(CommandError::Unsupported {
                command: CommandKind::EnergyScan,
                required: RadioCapabilities::ENERGY_SCAN,
            })
        );
        assert_eq!(machine.state(), before);
    }

    #[test]
    fn acknowledgement_capability_is_derived_only_from_the_fcf() {
        let no_ack_bytes = [0x01];
        let mut no_ack_machine = enabled(RadioCapabilities::NONE);
        no_ack_machine
            .admit(RadioCommand::Transmit(TxRequest {
                id: ID,
                frame: FrameView::new(&no_ack_bytes).unwrap(),
                channel: channel(15),
                mode: TxMode::Direct,
                transmit_power_dbm: None,
            }))
            .unwrap();

        let ack_bytes = [0x21];
        let mut ack_machine = enabled(RadioCapabilities::NONE);
        assert_eq!(
            ack_machine.admit(RadioCommand::Transmit(TxRequest {
                id: ID,
                frame: FrameView::new(&ack_bytes).unwrap(),
                channel: channel(15),
                mode: TxMode::Direct,
                transmit_power_dbm: None,
            })),
            Err(CommandError::Unsupported {
                command: CommandKind::Transmit,
                required: RadioCapabilities::HARDWARE_ACKNOWLEDGEMENT,
            })
        );
    }

    #[test]
    fn receive_and_ack_channels_are_checked() {
        let mut machine = enabled(RadioCapabilities::NONE);
        machine
            .admit(RadioCommand::Receive {
                id: ID,
                channel: channel(11),
            })
            .unwrap();
        let bytes = [0x02];
        let received = ReceivedFrame {
            frame: FrameView::new(&bytes).unwrap(),
            metadata: metadata(channel(12)),
        };
        assert_eq!(
            machine.observe(RadioEvent::Received(received)),
            Err(EventError::ChannelMismatch {
                expected: channel(11),
                actual: channel(12),
            })
        );
        assert_eq!(
            machine.state(),
            RadioState::Resting(RestingState::Receiving {
                channel: channel(11)
            })
        );
    }

    #[test]
    fn matching_fault_disables_and_mismatched_fault_is_rejected() {
        let capabilities = RadioCapabilities::ENERGY_SCAN;
        let mut machine = enabled(capabilities);
        machine
            .admit(RadioCommand::EnergyScan(EnergyScanRequest {
                id: ID,
                channel: channel(26),
                duration_us: 64,
            }))
            .unwrap();
        assert_eq!(
            machine.observe(RadioEvent::Fault {
                id: None,
                fault: RadioFault::StateLost,
            }),
            Err(EventError::FaultRequestMismatch {
                expected: Some(ID),
                actual: None,
            })
        );
        machine
            .observe(RadioEvent::Fault {
                id: Some(ID),
                fault: RadioFault::StateLost,
            })
            .unwrap();
        assert_eq!(machine.state(), RadioState::Disabled);
    }

    #[test]
    fn failed_transmit_cannot_publish_an_acknowledgement() {
        let capabilities = RadioCapabilities::HARDWARE_ACKNOWLEDGEMENT;
        let mut machine = enabled(capabilities);
        let frame_bytes = [0x21];
        machine
            .admit(RadioCommand::Transmit(TxRequest {
                id: ID,
                frame: FrameView::new(&frame_bytes).unwrap(),
                channel: channel(15),
                mode: TxMode::Direct,
                transmit_power_dbm: None,
            }))
            .unwrap();
        let ack_bytes = [2];
        let ack = ReceivedFrame {
            frame: FrameView::new(&ack_bytes).unwrap(),
            metadata: RxMetadata {
                frame_pending: FramePending::Clear,
                ..metadata(channel(15))
            },
        };
        assert_eq!(
            machine.observe(RadioEvent::TransmitDone {
                id: ID,
                status: TxStatus::NoAcknowledgement,
                acknowledgement: Some(ack),
            }),
            Err(EventError::AcknowledgementOnFailedTransmit)
        );
    }
}
