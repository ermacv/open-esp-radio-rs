//! Executor-neutral HCI codec for the supported LE Direct Test Mode commands.
//!
//! This module stops at the semantic HCI boundary. Decoding a command does not
//! start a radio, reserve a scheduler item, prove controller readiness or imply
//! that a test reached hardware. Consuming one of the typed command tokens to
//! build Command Complete only records a status supplied by the controller
//! owner after its own state transition has finished.

use bt_hci::{
    PacketKind,
    cmd::{Opcode, OpcodeGroup},
    param::{Error as HciError, Status},
};

use crate::{HciCommandPacket, HciControllerResponse};

/// Largest complete HCI Event body emitted for the supported DTM commands.
///
/// LE Test End returns Status followed by the two-octet packet count, making
/// its Command Complete event two octets larger than the start-test responses.
pub const LE_DTM_COMMAND_COMPLETE_EVENT_CAPACITY: usize = 8;

/// HCI LE Receiver Test v1 command opcode.
pub const LE_RECEIVER_TEST_V1_OPCODE: Opcode = Opcode::new(OpcodeGroup::LE, 0x001d);
/// HCI LE Transmitter Test v1 command opcode.
pub const LE_TRANSMITTER_TEST_V1_OPCODE: Opcode = Opcode::new(OpcodeGroup::LE, 0x001e);
/// HCI LE Test End command opcode.
pub const LE_TEST_END_OPCODE: Opcode = Opcode::new(OpcodeGroup::LE, 0x001f);
/// HCI LE Receiver Test v2 command opcode.
pub const LE_RECEIVER_TEST_V2_OPCODE: Opcode = Opcode::new(OpcodeGroup::LE, 0x0033);
/// HCI LE Transmitter Test v2 command opcode.
pub const LE_TRANSMITTER_TEST_V2_OPCODE: Opcode = Opcode::new(OpcodeGroup::LE, 0x0034);

/// One RF channel in the legacy LE test-channel domain.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeDtmChannel(u8);

impl LeDtmChannel {
    const LAST: u8 = 39;

    /// Decode the HCI channel parameter without assigning it to a radio.
    pub const fn from_hci_parameter(channel: u8) -> Option<Self> {
        if channel <= Self::LAST {
            Some(Self(channel))
        } else {
            None
        }
    }

    /// Return the semantic LE test-channel index.
    pub const fn index(self) -> u8 {
        self.0
    }
}

/// Payload pattern selected by a supported LE Transmitter Test command.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum LeDtmPayloadPattern {
    /// PRBS9.
    Prbs9 = 0,
    /// Repeated `11110000` in transmission order.
    Repeated11110000 = 1,
    /// Repeated `10101010` in transmission order.
    Repeated10101010 = 2,
    /// PRBS15.
    Prbs15 = 3,
    /// Repeated `11111111`.
    RepeatedAllOnes = 4,
    /// Repeated `00000000`.
    RepeatedAllZeros = 5,
    /// Repeated `00001111` in transmission order.
    Repeated00001111 = 6,
    /// Repeated `01010101` in transmission order.
    Repeated01010101 = 7,
}

impl LeDtmPayloadPattern {
    /// Decode the complete standard HCI selector domain.
    pub const fn from_hci_parameter(selector: u8) -> Option<Self> {
        match selector {
            0 => Some(Self::Prbs9),
            1 => Some(Self::Repeated11110000),
            2 => Some(Self::Repeated10101010),
            3 => Some(Self::Prbs15),
            4 => Some(Self::RepeatedAllOnes),
            5 => Some(Self::RepeatedAllZeros),
            6 => Some(Self::Repeated00001111),
            7 => Some(Self::Repeated01010101),
            _ => None,
        }
    }

    /// Return the HCI selector value.
    pub const fn hci_parameter(self) -> u8 {
        self as u8
    }
}

/// PHY selected by a normalized LE DTM start command.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum LeDtmPhy {
    /// LE 1M PHY.
    Le1M = 1,
    /// LE 2M PHY.
    Le2M = 2,
    /// Generic LE Coded receiver or S=8 transmitter.
    LeCoded = 3,
    /// LE Coded S=2 transmitter-only selection.
    LeCodedS2 = 4,
}

impl LeDtmPhy {
    const fn from_receiver_parameter(selector: u8) -> Option<Self> {
        match selector {
            1 => Some(Self::Le1M),
            2 => Some(Self::Le2M),
            3 => Some(Self::LeCoded),
            _ => None,
        }
    }

    const fn from_transmitter_parameter(selector: u8) -> Option<Self> {
        match selector {
            1 => Some(Self::Le1M),
            2 => Some(Self::Le2M),
            3 => Some(Self::LeCoded),
            4 => Some(Self::LeCodedS2),
            _ => None,
        }
    }

    /// Return the standard HCI selector.
    pub const fn hci_parameter(self) -> u8 {
        self as u8
    }
}

/// Expected modulation-index class for an LE Receiver Test.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeDtmModulationIndex {
    /// The transmitter uses the standard modulation-index requirements.
    Standard,
    /// The transmitter uses the stable modulation-index requirements.
    Stable,
}

impl LeDtmModulationIndex {
    const fn from_hci_parameter(parameter: u8) -> Option<Self> {
        match parameter {
            0 => Some(Self::Standard),
            1 => Some(Self::Stable),
            _ => None,
        }
    }
}

/// A decoded command in the closed supported LE DTM HCI subset.
#[derive(Debug, Eq, PartialEq)]
pub enum LeDtmCommand {
    /// Begin a normalized receiver test.
    ReceiverTest(LeReceiverTestCommand),
    /// Begin a normalized transmitter test.
    TransmitterTest(LeTransmitterTestCommand),
    /// End the active LE test.
    TestEnd(LeTestEndCommand),
}

impl LeDtmCommand {
    /// Decode one semantic command already admitted by the HCI packet boundary.
    pub fn decode(command: HciCommandPacket<'_>) -> Result<Self, LeDtmCommandDecodeError> {
        Self::decode_body(command.opcode(), command.parameters())
    }

    /// Decode an opcode and the complete command-parameter body.
    ///
    /// This entry point is useful for Controller owners which already split the HCI
    /// command header and for focused malformed-body tests. It applies the same
    /// closed opcode and exact-length policy as [`Self::decode`].
    pub fn decode_body(opcode: Opcode, parameters: &[u8]) -> Result<Self, LeDtmCommandDecodeError> {
        if opcode == LE_RECEIVER_TEST_V1_OPCODE {
            require_parameter_length(LeDtmCommandKind::ReceiverTestV1, parameters, 1)?;
            let channel = LeDtmChannel::from_hci_parameter(parameters[0]).ok_or(
                LeDtmCommandDecodeError::ChannelOutsideTestDomain {
                    command: LeDtmCommandKind::ReceiverTestV1,
                    channel: parameters[0],
                },
            )?;
            Ok(Self::ReceiverTest(LeReceiverTestCommand {
                kind: LeDtmCommandKind::ReceiverTestV1,
                channel,
                phy: LeDtmPhy::Le1M,
                modulation_index: LeDtmModulationIndex::Standard,
            }))
        } else if opcode == LE_TRANSMITTER_TEST_V1_OPCODE {
            require_parameter_length(LeDtmCommandKind::TransmitterTestV1, parameters, 3)?;
            let channel = LeDtmChannel::from_hci_parameter(parameters[0]).ok_or(
                LeDtmCommandDecodeError::ChannelOutsideTestDomain {
                    command: LeDtmCommandKind::TransmitterTestV1,
                    channel: parameters[0],
                },
            )?;
            let pattern = LeDtmPayloadPattern::from_hci_parameter(parameters[2]).ok_or(
                LeDtmCommandDecodeError::UnsupportedPayloadPattern {
                    command: LeDtmCommandKind::TransmitterTestV1,
                    selector: parameters[2],
                },
            )?;
            Ok(Self::TransmitterTest(LeTransmitterTestCommand {
                kind: LeDtmCommandKind::TransmitterTestV1,
                channel,
                payload_length: parameters[1],
                pattern,
                phy: LeDtmPhy::Le1M,
            }))
        } else if opcode == LE_RECEIVER_TEST_V2_OPCODE {
            require_parameter_length(LeDtmCommandKind::ReceiverTestV2, parameters, 3)?;
            let channel = LeDtmChannel::from_hci_parameter(parameters[0]).ok_or(
                LeDtmCommandDecodeError::ChannelOutsideTestDomain {
                    command: LeDtmCommandKind::ReceiverTestV2,
                    channel: parameters[0],
                },
            )?;
            let phy = LeDtmPhy::from_receiver_parameter(parameters[1]).ok_or(
                LeDtmCommandDecodeError::UnsupportedPhy {
                    command: LeDtmCommandKind::ReceiverTestV2,
                    selector: parameters[1],
                },
            )?;
            let modulation_index = LeDtmModulationIndex::from_hci_parameter(parameters[2]).ok_or(
                LeDtmCommandDecodeError::UnsupportedModulationIndex {
                    command: LeDtmCommandKind::ReceiverTestV2,
                    parameter: parameters[2],
                },
            )?;
            Ok(Self::ReceiverTest(LeReceiverTestCommand {
                kind: LeDtmCommandKind::ReceiverTestV2,
                channel,
                phy,
                modulation_index,
            }))
        } else if opcode == LE_TRANSMITTER_TEST_V2_OPCODE {
            require_parameter_length(LeDtmCommandKind::TransmitterTestV2, parameters, 4)?;
            let channel = LeDtmChannel::from_hci_parameter(parameters[0]).ok_or(
                LeDtmCommandDecodeError::ChannelOutsideTestDomain {
                    command: LeDtmCommandKind::TransmitterTestV2,
                    channel: parameters[0],
                },
            )?;
            let pattern = LeDtmPayloadPattern::from_hci_parameter(parameters[2]).ok_or(
                LeDtmCommandDecodeError::UnsupportedPayloadPattern {
                    command: LeDtmCommandKind::TransmitterTestV2,
                    selector: parameters[2],
                },
            )?;
            let phy = LeDtmPhy::from_transmitter_parameter(parameters[3]).ok_or(
                LeDtmCommandDecodeError::UnsupportedPhy {
                    command: LeDtmCommandKind::TransmitterTestV2,
                    selector: parameters[3],
                },
            )?;
            Ok(Self::TransmitterTest(LeTransmitterTestCommand {
                kind: LeDtmCommandKind::TransmitterTestV2,
                channel,
                payload_length: parameters[1],
                pattern,
                phy,
            }))
        } else if opcode == LE_TEST_END_OPCODE {
            require_parameter_length(LeDtmCommandKind::TestEnd, parameters, 0)?;
            Ok(Self::TestEnd(LeTestEndCommand { private: () }))
        } else {
            Err(LeDtmCommandDecodeError::UnsupportedOpcode { opcode })
        }
    }

    /// Kind of this decoded command without inspecting its parameters.
    pub const fn kind(&self) -> LeDtmCommandKind {
        match self {
            Self::ReceiverTest(command) => command.kind(),
            Self::TransmitterTest(command) => command.kind(),
            Self::TestEnd(_) => LeDtmCommandKind::TestEnd,
        }
    }

    /// Apply the reviewed command policy for an idle DTM session.
    ///
    /// A start command remains an owned semantic request for the hardware
    /// runner. Test End succeeds without starting or stopping hardware and
    /// reports the idle session's zero packet count. The policy exposes no
    /// caller-selected HCI status.
    pub fn into_idle_session_disposition(self) -> LeDtmIdleSessionDisposition {
        match self {
            Self::ReceiverTest(command) => LeDtmIdleSessionDisposition::StartReceiver(command),
            Self::TransmitterTest(command) => {
                LeDtmIdleSessionDisposition::StartTransmitter(command)
            }
            Self::TestEnd(command) => {
                LeDtmIdleSessionDisposition::CompleteNoTest(command.into_ended_command_complete(0))
            }
        }
    }

    /// Apply the reviewed command policy for an active DTM session.
    ///
    /// Test End remains owned until the chip-specific runner has quiesced the
    /// active graph and obtained its terminal packet count. A second receiver
    /// or transmitter start is rejected as Controller Busy without replacing
    /// or mutating that active session.
    pub fn into_active_session_disposition(self) -> LeDtmActiveSessionDisposition {
        match self {
            Self::ReceiverTest(command) => LeDtmActiveSessionDisposition::RejectControllerBusy(
                LeDtmCommandCompleteEvent::without_return_parameters(
                    command.opcode(),
                    HciError::CONTROLLER_BUSY.to_status(),
                ),
            ),
            Self::TransmitterTest(command) => LeDtmActiveSessionDisposition::RejectControllerBusy(
                LeDtmCommandCompleteEvent::without_return_parameters(
                    command.opcode(),
                    HciError::CONTROLLER_BUSY.to_status(),
                ),
            ),
            Self::TestEnd(command) => LeDtmActiveSessionDisposition::End(command),
        }
    }
}

/// Reviewed outcome of routing one DTM command while no test is active.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "the idle DTM command must start hardware or publish its response"]
pub enum LeDtmIdleSessionDisposition {
    /// Start one receiver test through the chip-specific hardware runner.
    StartReceiver(LeReceiverTestCommand),
    /// Start one transmitter test through the chip-specific hardware runner.
    StartTransmitter(LeTransmitterTestCommand),
    /// Publish the successful zero-count response for Test End without a test.
    CompleteNoTest(LeDtmCommandCompleteEvent),
}

/// Reviewed outcome of routing one DTM command while a test is active.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "the active DTM command must enter Test End or publish its rejection"]
pub enum LeDtmActiveSessionDisposition {
    /// Quiesce the active hardware owner before completing this Test End.
    End(LeTestEndCommand),
    /// Publish the fixed Controller Busy response while retaining the session.
    RejectControllerBusy(LeDtmCommandCompleteEvent),
}

/// Closed command identity used in decode diagnostics.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeDtmCommandKind {
    /// LE Receiver Test v1.
    ReceiverTestV1,
    /// LE Transmitter Test v1.
    TransmitterTestV1,
    /// LE Receiver Test v2.
    ReceiverTestV2,
    /// LE Transmitter Test v2.
    TransmitterTestV2,
    /// LE Test End.
    TestEnd,
}

impl LeDtmCommandKind {
    /// Exact HCI opcode represented by this semantic command kind.
    pub const fn opcode(self) -> Opcode {
        match self {
            Self::ReceiverTestV1 => LE_RECEIVER_TEST_V1_OPCODE,
            Self::TransmitterTestV1 => LE_TRANSMITTER_TEST_V1_OPCODE,
            Self::ReceiverTestV2 => LE_RECEIVER_TEST_V2_OPCODE,
            Self::TransmitterTestV2 => LE_TRANSMITTER_TEST_V2_OPCODE,
            Self::TestEnd => LE_TEST_END_OPCODE,
        }
    }
}

/// Why a semantic LE DTM command body was rejected.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeDtmCommandDecodeError {
    /// The opcode is outside the supported DTM command codec.
    UnsupportedOpcode {
        /// Unrecognized opcode.
        opcode: Opcode,
    },
    /// A known command did not contain its complete exact parameter body.
    InvalidParameterLength {
        /// Command whose body was malformed.
        command: LeDtmCommandKind,
        /// Required body length.
        expected: usize,
        /// Supplied body length.
        actual: usize,
    },
    /// The channel parameter is reserved rather than an LE test channel.
    ChannelOutsideTestDomain {
        /// Known command whose channel was rejected.
        command: LeDtmCommandKind,
        /// Rejected HCI channel value.
        channel: u8,
    },
    /// The transmitter pattern selector is outside the standard domain.
    UnsupportedPayloadPattern {
        /// Known command whose payload selector was rejected.
        command: LeDtmCommandKind,
        /// Rejected HCI selector value.
        selector: u8,
    },
    /// The PHY selector is reserved for this command version or role.
    UnsupportedPhy {
        /// Known command whose PHY selector was rejected.
        command: LeDtmCommandKind,
        /// Rejected HCI selector value.
        selector: u8,
    },
    /// The receiver modulation-index parameter is reserved.
    UnsupportedModulationIndex {
        /// Known command whose parameter was rejected.
        command: LeDtmCommandKind,
        /// Rejected HCI parameter value.
        parameter: u8,
    },
}

impl LeDtmCommandDecodeError {
    /// Convert rejected input for a known DTM opcode into its required
    /// command completion.
    ///
    /// An unsupported opcode remains owned by the error so the outer Controller
    /// router can dispatch another command family or return Unknown HCI Command.
    pub fn into_command_complete(self) -> Result<LeDtmCommandCompleteEvent, Self> {
        let (command, status) = match self {
            Self::UnsupportedOpcode { .. } => return Err(self),
            Self::InvalidParameterLength { command, .. }
            | Self::ChannelOutsideTestDomain { command, .. }
            | Self::UnsupportedPayloadPattern { command, .. }
            | Self::UnsupportedModulationIndex { command, .. } => {
                (command, HciError::INVALID_HCI_PARAMETERS.to_status())
            }
            Self::UnsupportedPhy { command, .. } => (command, HciError::UNSUPPORTED.to_status()),
        };
        Ok(LeDtmCommandCompleteEvent::without_return_parameters(
            command.opcode(),
            status,
        ))
    }
}

/// Normalized validated LE Receiver Test command awaiting controller execution.
#[derive(Debug, Eq, PartialEq)]
pub struct LeReceiverTestCommand {
    kind: LeDtmCommandKind,
    channel: LeDtmChannel,
    phy: LeDtmPhy,
    modulation_index: LeDtmModulationIndex,
}

impl LeReceiverTestCommand {
    const fn kind(&self) -> LeDtmCommandKind {
        self.kind
    }

    /// Exact HCI command version retained for response identity.
    pub const fn opcode(&self) -> Opcode {
        self.kind.opcode()
    }

    /// Requested LE test channel.
    pub const fn channel(&self) -> LeDtmChannel {
        self.channel
    }

    /// Requested receiver PHY.
    pub const fn phy(&self) -> LeDtmPhy {
        self.phy
    }

    /// Expected transmitter modulation-index class.
    pub const fn modulation_index(&self) -> LeDtmModulationIndex {
        self.modulation_index
    }

    /// Consume the command after its first receiver event reached hardware.
    pub fn into_started_command_complete(self) -> LeDtmCommandCompleteEvent {
        LeDtmCommandCompleteEvent::without_return_parameters(self.kind.opcode(), Status::SUCCESS)
    }

    pub(crate) fn into_hardware_failure_command_complete(self) -> LeDtmCommandCompleteEvent {
        LeDtmCommandCompleteEvent::without_return_parameters(
            self.kind.opcode(),
            HciError::HARDWARE_FAILURE.to_status(),
        )
    }
}

/// Normalized validated LE Transmitter Test command awaiting controller execution.
#[derive(Debug, Eq, PartialEq)]
pub struct LeTransmitterTestCommand {
    kind: LeDtmCommandKind,
    channel: LeDtmChannel,
    payload_length: u8,
    pattern: LeDtmPayloadPattern,
    phy: LeDtmPhy,
}

impl LeTransmitterTestCommand {
    const fn kind(&self) -> LeDtmCommandKind {
        self.kind
    }

    /// Exact HCI command version retained for response identity.
    pub const fn opcode(&self) -> Opcode {
        self.kind.opcode()
    }

    /// Requested LE test channel.
    pub const fn channel(&self) -> LeDtmChannel {
        self.channel
    }

    /// Complete eight-bit HCI payload-length parameter.
    pub const fn payload_length(&self) -> u8 {
        self.payload_length
    }

    /// Requested standard payload pattern.
    pub const fn payload_pattern(&self) -> LeDtmPayloadPattern {
        self.pattern
    }

    /// Requested transmitter PHY.
    pub const fn phy(&self) -> LeDtmPhy {
        self.phy
    }

    /// Consume the command after its first transmitter event reached hardware.
    pub fn into_started_command_complete(self) -> LeDtmCommandCompleteEvent {
        LeDtmCommandCompleteEvent::without_return_parameters(self.kind.opcode(), Status::SUCCESS)
    }

    pub(crate) fn into_hardware_failure_command_complete(self) -> LeDtmCommandCompleteEvent {
        LeDtmCommandCompleteEvent::without_return_parameters(
            self.kind.opcode(),
            HciError::HARDWARE_FAILURE.to_status(),
        )
    }
}

/// Validated LE Test End command awaiting the controller's terminal report.
#[derive(Debug, Eq, PartialEq)]
pub struct LeTestEndCommand {
    private: (),
}

impl LeTestEndCommand {
    /// Consume the pending command after the controller has ended the test.
    ///
    /// `packet_count` is deliberately not inferred here. The chip-specific
    /// terminal owner supplies zero for a transmitter test or the accumulated
    /// accepted count for a receiver test. This codec only serializes that
    /// semantic report in HCI little-endian order.
    pub fn into_ended_command_complete(self, packet_count: u16) -> LeDtmCommandCompleteEvent {
        LeDtmCommandCompleteEvent::with_packet_count(
            LE_TEST_END_OPCODE,
            Status::SUCCESS,
            packet_count,
        )
    }
}

/// Complete Command Complete HCI Event built after a DTM state transition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeDtmCommandCompleteEvent {
    bytes: [u8; LE_DTM_COMMAND_COMPLETE_EVENT_CAPACITY],
    length: usize,
    opcode: Opcode,
    status: Status,
    packet_count: Option<u16>,
}

impl LeDtmCommandCompleteEvent {
    fn without_return_parameters(opcode: Opcode, status: Status) -> Self {
        let mut event = Self::header(opcode, status, 0);
        event.length = 6;
        event
    }

    fn with_packet_count(opcode: Opcode, status: Status, packet_count: u16) -> Self {
        let mut event = Self::header(opcode, status, 2);
        event.bytes[6..8].copy_from_slice(&packet_count.to_le_bytes());
        event.length = 8;
        event.packet_count = Some(packet_count);
        event
    }

    fn header(opcode: Opcode, status: Status, return_parameter_length: u8) -> Self {
        let mut bytes = [0; LE_DTM_COMMAND_COMPLETE_EVENT_CAPACITY];
        bytes[0] = 0x0e;
        bytes[1] = 4 + return_parameter_length;
        bytes[2] = 1;
        bytes[3..5].copy_from_slice(&opcode.to_raw().to_le_bytes());
        bytes[5] = status.into_inner();
        Self {
            bytes,
            length: 0,
            opcode,
            status,
            packet_count: None,
        }
    }

    /// Complete HCI Event body without an H4 packet indicator.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.length]
    }

    /// Opcode copied from the consumed typed command token.
    pub const fn opcode(&self) -> Opcode {
        self.opcode
    }

    /// Controller-supplied completion status.
    pub const fn status(&self) -> Status {
        self.status
    }

    /// Test End packet count, absent for Receiver/Transmitter Test start.
    pub const fn packet_count(&self) -> Option<u16> {
        self.packet_count
    }
}

impl HciControllerResponse for LeDtmCommandCompleteEvent {
    fn kind(&self) -> PacketKind {
        PacketKind::Event
    }

    fn as_bytes(&self) -> &[u8] {
        self.as_bytes()
    }
}

fn require_parameter_length(
    command: LeDtmCommandKind,
    parameters: &[u8],
    expected: usize,
) -> Result<(), LeDtmCommandDecodeError> {
    if parameters.len() == expected {
        Ok(())
    } else {
        Err(LeDtmCommandDecodeError::InvalidParameterLength {
            command,
            expected,
            actual: parameters.len(),
        })
    }
}

#[cfg(test)]
mod tests {
    use bt_hci::{
        ControllerToHostPacket, FromHciBytes, PacketKind,
        cmd::{
            Cmd, Opcode,
            le::{LeReadSupportedStates, LeReceiverTestV2, LeTestEnd, LeTransmitterTestV2},
        },
        event::{CommandComplete, CommandCompleteWithStatus, EventKind},
        param::{Error as HciError, Status},
        transport::Transport,
    };
    use embassy_futures::block_on;
    use embassy_sync::blocking_mutex::raw::NoopRawMutex;

    use crate::{HostToControllerFrame, InProcessHciChannel};

    use super::{
        LE_RECEIVER_TEST_V1_OPCODE, LE_RECEIVER_TEST_V2_OPCODE, LE_TEST_END_OPCODE,
        LE_TRANSMITTER_TEST_V1_OPCODE, LE_TRANSMITTER_TEST_V2_OPCODE,
        LeDtmActiveSessionDisposition, LeDtmCommand, LeDtmCommandDecodeError, LeDtmCommandKind,
        LeDtmIdleSessionDisposition, LeDtmModulationIndex, LeDtmPayloadPattern, LeDtmPhy,
    };

    type TestChannel = InProcessHciChannel<NoopRawMutex, 1, 1, 16>;

    #[test]
    fn legacy_transmitter_opcode_does_not_collide_with_read_supported_states() {
        assert_eq!(LE_TRANSMITTER_TEST_V1_OPCODE.to_raw(), 0x201e);
        assert_ne!(LE_TRANSMITTER_TEST_V1_OPCODE, LeReadSupportedStates::OPCODE);
    }

    #[test]
    fn all_command_bodies_decode_semantically_and_typed_test_end_crosses_the_boundary() {
        let receiver = LeDtmCommand::decode_body(LE_RECEIVER_TEST_V1_OPCODE, &[39]).unwrap();
        let LeDtmCommand::ReceiverTest(receiver) = receiver else {
            panic!("receiver command changed semantic kind");
        };
        assert_eq!(receiver.channel().index(), 39);

        let transmitter =
            LeDtmCommand::decode_body(LE_TRANSMITTER_TEST_V1_OPCODE, &[7, 255, 6]).unwrap();
        let LeDtmCommand::TransmitterTest(transmitter) = transmitter else {
            panic!("transmitter command changed semantic kind");
        };
        assert_eq!(transmitter.channel().index(), 7);
        assert_eq!(transmitter.payload_length(), 255);
        assert_eq!(
            transmitter.payload_pattern(),
            LeDtmPayloadPattern::Repeated00001111
        );

        let LeDtmCommand::ReceiverTest(receiver) =
            cross_hci_boundary(&LeReceiverTestV2::new(12, 3, 1))
        else {
            panic!("enhanced receiver command changed semantic kind");
        };
        assert_eq!(receiver.opcode(), LE_RECEIVER_TEST_V2_OPCODE);
        assert_eq!(receiver.phy(), LeDtmPhy::LeCoded);
        assert_eq!(receiver.modulation_index(), LeDtmModulationIndex::Stable);

        let LeDtmCommand::TransmitterTest(transmitter) =
            cross_hci_boundary(&LeTransmitterTestV2::new(18, 255, 4, 4))
        else {
            panic!("enhanced transmitter command changed semantic kind");
        };
        assert_eq!(transmitter.opcode(), LE_TRANSMITTER_TEST_V2_OPCODE);
        assert_eq!(transmitter.phy(), LeDtmPhy::LeCodedS2);

        let test_end = cross_hci_boundary(&LeTestEnd::new());
        assert!(matches!(test_end, LeDtmCommand::TestEnd(_)));
    }

    #[test]
    fn v2_phy_and_modulation_domains_normalize_without_version_specific_tokens() {
        for (selector, expected) in [
            (1, LeDtmPhy::Le1M),
            (2, LeDtmPhy::Le2M),
            (3, LeDtmPhy::LeCoded),
        ] {
            for (parameter, modulation) in [
                (0, LeDtmModulationIndex::Standard),
                (1, LeDtmModulationIndex::Stable),
            ] {
                let LeDtmCommand::ReceiverTest(command) = LeDtmCommand::decode_body(
                    LE_RECEIVER_TEST_V2_OPCODE,
                    &[39, selector, parameter],
                )
                .expect("the standard receiver mode must normalize") else {
                    unreachable!()
                };
                assert_eq!(command.phy(), expected);
                assert_eq!(command.modulation_index(), modulation);
            }
        }

        for (selector, expected) in [
            (1, LeDtmPhy::Le1M),
            (2, LeDtmPhy::Le2M),
            (3, LeDtmPhy::LeCoded),
            (4, LeDtmPhy::LeCodedS2),
        ] {
            let LeDtmCommand::TransmitterTest(command) =
                LeDtmCommand::decode_body(LE_TRANSMITTER_TEST_V2_OPCODE, &[39, 255, 7, selector])
                    .expect("the standard transmitter mode must normalize")
            else {
                unreachable!()
            };
            assert_eq!(command.phy(), expected);
        }
    }

    #[test]
    fn exact_parameter_lengths_fail_closed_without_partial_decode() {
        for (opcode, parameters, command, expected) in [
            (
                LE_RECEIVER_TEST_V1_OPCODE,
                &[][..],
                LeDtmCommandKind::ReceiverTestV1,
                1,
            ),
            (
                LE_RECEIVER_TEST_V1_OPCODE,
                &[0, 1][..],
                LeDtmCommandKind::ReceiverTestV1,
                1,
            ),
            (
                LE_TRANSMITTER_TEST_V1_OPCODE,
                &[0, 0][..],
                LeDtmCommandKind::TransmitterTestV1,
                3,
            ),
            (
                LE_RECEIVER_TEST_V2_OPCODE,
                &[0, 1][..],
                LeDtmCommandKind::ReceiverTestV2,
                3,
            ),
            (
                LE_RECEIVER_TEST_V2_OPCODE,
                &[0, 1, 0, 0][..],
                LeDtmCommandKind::ReceiverTestV2,
                3,
            ),
            (
                LE_TRANSMITTER_TEST_V2_OPCODE,
                &[0, 0, 0][..],
                LeDtmCommandKind::TransmitterTestV2,
                4,
            ),
            (
                LE_TRANSMITTER_TEST_V2_OPCODE,
                &[0, 0, 0, 1, 0][..],
                LeDtmCommandKind::TransmitterTestV2,
                4,
            ),
            (
                LE_TRANSMITTER_TEST_V1_OPCODE,
                &[0, 0, 0, 0][..],
                LeDtmCommandKind::TransmitterTestV1,
                3,
            ),
            (LE_TEST_END_OPCODE, &[0][..], LeDtmCommandKind::TestEnd, 0),
        ] {
            assert_eq!(
                LeDtmCommand::decode_body(opcode, parameters),
                Err(LeDtmCommandDecodeError::InvalidParameterLength {
                    command,
                    expected,
                    actual: parameters.len(),
                })
            );
        }
    }

    #[test]
    fn reserved_semantic_values_are_not_forwarded_to_a_chip() {
        assert_eq!(
            LeDtmCommand::decode_body(LE_RECEIVER_TEST_V1_OPCODE, &[40]),
            Err(LeDtmCommandDecodeError::ChannelOutsideTestDomain {
                command: LeDtmCommandKind::ReceiverTestV1,
                channel: 40,
            })
        );
        assert_eq!(
            LeDtmCommand::decode_body(LE_TRANSMITTER_TEST_V1_OPCODE, &[0, 1, 8]),
            Err(LeDtmCommandDecodeError::UnsupportedPayloadPattern {
                command: LeDtmCommandKind::TransmitterTestV1,
                selector: 8,
            })
        );
        assert_eq!(
            LeDtmCommand::decode_body(LE_RECEIVER_TEST_V2_OPCODE, &[0, 4, 0]),
            Err(LeDtmCommandDecodeError::UnsupportedPhy {
                command: LeDtmCommandKind::ReceiverTestV2,
                selector: 4,
            })
        );
        assert!(matches!(
            LeDtmCommand::decode_body(LE_RECEIVER_TEST_V2_OPCODE, &[0, 0, 0]),
            Err(LeDtmCommandDecodeError::UnsupportedPhy {
                command: LeDtmCommandKind::ReceiverTestV2,
                selector: 0,
            })
        ));
        assert_eq!(
            LeDtmCommand::decode_body(LE_RECEIVER_TEST_V2_OPCODE, &[0, 1, 2]),
            Err(LeDtmCommandDecodeError::UnsupportedModulationIndex {
                command: LeDtmCommandKind::ReceiverTestV2,
                parameter: 2,
            })
        );
        assert_eq!(
            LeDtmCommand::decode_body(LE_TRANSMITTER_TEST_V2_OPCODE, &[0, 1, 0, 5]),
            Err(LeDtmCommandDecodeError::UnsupportedPhy {
                command: LeDtmCommandKind::TransmitterTestV2,
                selector: 5,
            })
        );
        assert!(matches!(
            LeDtmCommand::decode_body(LE_TRANSMITTER_TEST_V2_OPCODE, &[0, 1, 0, 0]),
            Err(LeDtmCommandDecodeError::UnsupportedPhy {
                command: LeDtmCommandKind::TransmitterTestV2,
                selector: 0,
            })
        ));
        assert!(matches!(
            LeDtmCommand::decode_body(Opcode::UNSOLICITED, &[]),
            Err(LeDtmCommandDecodeError::UnsupportedOpcode { .. })
        ));
    }

    #[test]
    fn rejected_known_opcodes_build_specified_command_completions() {
        for (opcode, parameters) in [
            (LE_RECEIVER_TEST_V1_OPCODE, &[][..]),
            (LE_RECEIVER_TEST_V1_OPCODE, &[40][..]),
            (LE_TRANSMITTER_TEST_V1_OPCODE, &[0, 1, 8][..]),
            (LE_RECEIVER_TEST_V2_OPCODE, &[0, 1, 2][..]),
            (LE_TEST_END_OPCODE, &[0][..]),
        ] {
            let response = LeDtmCommand::decode_body(opcode, parameters)
                .expect_err("malformed known command must fail closed")
                .into_command_complete()
                .expect("known DTM opcode must retain its response identity");
            let observed = parse_command_complete(response.as_bytes());
            assert_eq!(observed.cmd_opcode, opcode);
            assert_eq!(
                observed.status,
                HciError::INVALID_HCI_PARAMETERS.to_status()
            );
            assert!(observed.return_param_bytes.is_empty());
        }

        for (opcode, parameters) in [
            (LE_RECEIVER_TEST_V2_OPCODE, &[0, 4, 0][..]),
            (LE_TRANSMITTER_TEST_V2_OPCODE, &[0, 1, 0, 5][..]),
        ] {
            let response = LeDtmCommand::decode_body(opcode, parameters)
                .expect_err("a reserved PHY must fail closed")
                .into_command_complete()
                .expect("the known DTM opcode retains response identity");
            let observed = parse_command_complete(response.as_bytes());
            assert_eq!(observed.cmd_opcode, opcode);
            assert_eq!(observed.status, HciError::UNSUPPORTED.to_status());
        }

        let unsupported = LeDtmCommand::decode_body(Opcode::UNSOLICITED, &[])
            .expect_err("unsupported opcode remains outside the DTM response scope");
        assert_eq!(
            unsupported.into_command_complete(),
            Err(LeDtmCommandDecodeError::UnsupportedOpcode {
                opcode: Opcode::UNSOLICITED,
            })
        );
    }

    #[test]
    fn successful_starts_roundtrip_through_bt_hci_event_types() {
        let LeDtmCommand::ReceiverTest(receiver) =
            LeDtmCommand::decode_body(LE_RECEIVER_TEST_V1_OPCODE, &[3]).unwrap()
        else {
            unreachable!()
        };
        let receiver_complete = receiver.into_started_command_complete();
        let observed = parse_command_complete(receiver_complete.as_bytes());
        assert_eq!(observed.cmd_opcode, LE_RECEIVER_TEST_V1_OPCODE);
        assert_eq!(observed.status, Status::SUCCESS);
        assert!(observed.return_param_bytes.is_empty());

        let LeDtmCommand::TransmitterTest(transmitter) =
            LeDtmCommand::decode_body(LE_TRANSMITTER_TEST_V1_OPCODE, &[5, 37, 2]).unwrap()
        else {
            unreachable!()
        };
        let transmitter_complete = transmitter.into_started_command_complete();
        let observed = parse_command_complete(transmitter_complete.as_bytes());
        assert_eq!(observed.cmd_opcode, LE_TRANSMITTER_TEST_V1_OPCODE);
        assert_eq!(observed.status, Status::SUCCESS);
        assert!(observed.return_param_bytes.is_empty());

        let LeDtmCommand::ReceiverTest(receiver) =
            LeDtmCommand::decode_body(LE_RECEIVER_TEST_V2_OPCODE, &[3, 2, 0]).unwrap()
        else {
            unreachable!()
        };
        let receiver_complete = receiver.into_started_command_complete();
        let observed = parse_command_complete(receiver_complete.as_bytes());
        assert_eq!(observed.cmd_opcode, LE_RECEIVER_TEST_V2_OPCODE);
        assert_eq!(observed.status, Status::SUCCESS);

        let LeDtmCommand::TransmitterTest(transmitter) =
            LeDtmCommand::decode_body(LE_TRANSMITTER_TEST_V2_OPCODE, &[5, 37, 2, 3]).unwrap()
        else {
            unreachable!()
        };
        let transmitter_complete = transmitter.into_started_command_complete();
        let observed = parse_command_complete(transmitter_complete.as_bytes());
        assert_eq!(observed.cmd_opcode, LE_TRANSMITTER_TEST_V2_OPCODE);
        assert_eq!(observed.status, Status::SUCCESS);
    }

    #[test]
    fn idle_policy_retains_starts_and_completes_empty_test_end() {
        let receiver = LeDtmCommand::decode_body(LE_RECEIVER_TEST_V1_OPCODE, &[17]).unwrap();
        let LeDtmIdleSessionDisposition::StartReceiver(receiver) =
            receiver.into_idle_session_disposition()
        else {
            panic!("an idle receiver start did not retain its command")
        };
        assert_eq!(receiver.channel().index(), 17);

        let transmitter =
            LeDtmCommand::decode_body(LE_TRANSMITTER_TEST_V1_OPCODE, &[9, 31, 2]).unwrap();
        let LeDtmIdleSessionDisposition::StartTransmitter(transmitter) =
            transmitter.into_idle_session_disposition()
        else {
            panic!("an idle transmitter start did not retain its command")
        };
        assert_eq!(transmitter.channel().index(), 9);
        assert_eq!(transmitter.payload_length(), 31);

        let test_end = cross_hci_boundary(&LeTestEnd::new());
        let LeDtmIdleSessionDisposition::CompleteNoTest(response) =
            test_end.into_idle_session_disposition()
        else {
            panic!("idle Test End did not produce its terminal response")
        };
        let observed = parse_command_complete(response.as_bytes());
        assert_eq!(observed.cmd_opcode, LeTestEnd::OPCODE);
        assert_eq!(observed.status, Status::SUCCESS);
        assert_eq!(observed.return_params::<LeTestEnd>().unwrap(), 0);
    }

    #[test]
    fn active_policy_rejects_both_starts_as_controller_busy() {
        for (command, expected_opcode) in [
            (
                LeDtmCommand::decode_body(LE_RECEIVER_TEST_V1_OPCODE, &[3]).unwrap(),
                LE_RECEIVER_TEST_V1_OPCODE,
            ),
            (
                LeDtmCommand::decode_body(LE_TRANSMITTER_TEST_V1_OPCODE, &[5, 37, 2]).unwrap(),
                LE_TRANSMITTER_TEST_V1_OPCODE,
            ),
            (
                LeDtmCommand::decode_body(LE_RECEIVER_TEST_V2_OPCODE, &[3, 2, 0]).unwrap(),
                LE_RECEIVER_TEST_V2_OPCODE,
            ),
            (
                LeDtmCommand::decode_body(LE_TRANSMITTER_TEST_V2_OPCODE, &[5, 37, 2, 4]).unwrap(),
                LE_TRANSMITTER_TEST_V2_OPCODE,
            ),
        ] {
            let LeDtmActiveSessionDisposition::RejectControllerBusy(response) =
                command.into_active_session_disposition()
            else {
                panic!("a second active-session start escaped busy rejection")
            };
            let observed = parse_command_complete(response.as_bytes());
            assert_eq!(observed.cmd_opcode, expected_opcode);
            assert_eq!(observed.status, HciError::CONTROLLER_BUSY.to_status());
            assert!(observed.return_param_bytes.is_empty());
        }
    }

    #[test]
    fn active_test_end_stays_owned_until_terminal_count_is_available() {
        let command = cross_hci_boundary(&LeTestEnd::new());
        let LeDtmActiveSessionDisposition::End(command) = command.into_active_session_disposition()
        else {
            panic!("active Test End was completed before hardware quiescence")
        };

        let response = command.into_ended_command_complete(0x3412);
        let observed = parse_command_complete(response.as_bytes());
        assert_eq!(observed.status, Status::SUCCESS);
        assert_eq!(observed.return_params::<LeTestEnd>().unwrap(), 0x3412);
    }

    fn cross_hci_boundary<T: bt_hci::transport::PacketToController>(command: &T) -> LeDtmCommand {
        let mut channel = TestChannel::new();
        let (host, controller) = channel.split();
        block_on(async {
            host.write(command).await.unwrap();
            let mut buffer = [0; 16];
            let HostToControllerFrame::Command(command) =
                controller.receive(&mut buffer).await.unwrap()
            else {
                panic!("typed HCI command changed packet class");
            };
            LeDtmCommand::decode(command).unwrap()
        })
    }

    fn parse_command_complete(bytes: &[u8]) -> CommandCompleteWithStatus<'_> {
        let (packet, remaining) =
            ControllerToHostPacket::from_hci_bytes_with_kind(PacketKind::Event, bytes).unwrap();
        assert!(remaining.is_empty());
        let ControllerToHostPacket::Event(event) = packet else {
            panic!("Command Complete changed packet class");
        };
        assert_eq!(event.kind, EventKind::CommandComplete);
        let complete = CommandComplete::from_hci_bytes_complete(event.data).unwrap();
        complete.try_into().unwrap()
    }
}
