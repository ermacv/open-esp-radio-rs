//! Portable command classification before chip-specific session execution.

use bt_hci::cmd::Opcode;

use crate::{
    BootstrapCommand, BootstrapCommandCompleteEvent, HciCommandPacket, LeControllerBootstrap,
    LeDtmCommand, LeDtmCommandCompleteEvent,
};

/// One finite result of classifying a validated HCI command packet.
///
/// Bootstrap and malformed-DTM responses are complete owned packets that may
/// be retained across Controller-to-Host backpressure. A valid DTM command is
/// an owned semantic token that may be retained across asynchronous hardware
/// transitions. Only [`Self::Unsupported`] borrows the receive buffer: an
/// outer router must inspect or consume it before that buffer is reused.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "a classified HCI command must be routed or answered exactly once"]
pub enum LeControllerCommandClassification<'packet> {
    /// Pure software bootstrap state was advanced and produced this response.
    Bootstrap(BootstrapCommandCompleteEvent),
    /// A validated DTM command awaits chip-specific session execution.
    Dtm(LeDtmCommand),
    /// A known DTM opcode had malformed parameters and produced this response.
    MalformedDtm(LeDtmCommandCompleteEvent),
    /// This portable command set did not claim the packet.
    ///
    /// The exact opcode and parameters remain available to an outer router.
    Unsupported(HciCommandPacket<'packet>),
}

impl LeControllerCommandClassification<'_> {
    /// Opcode retained by this exact classification result.
    pub const fn opcode(&self) -> Opcode {
        match self {
            Self::Bootstrap(response) => response.opcode(),
            Self::Dtm(command) => command.kind().opcode(),
            Self::MalformedDtm(response) => response.opcode(),
            Self::Unsupported(command) => command.opcode(),
        }
    }
}

/// Classify one validated command without executing radio or Link-Layer work.
///
/// DTM decoding runs first because it distinguishes malformed input for its
/// three known opcodes from an opcode outside that command family. The latter
/// is dispatched only when it belongs to the closed bootstrap table; otherwise
/// the unchanged packet is returned to the outer router. No branch waits,
/// publishes a response, or claims hardware readiness.
pub fn classify_le_controller_command<'packet>(
    bootstrap: &mut LeControllerBootstrap,
    command: HciCommandPacket<'packet>,
) -> LeControllerCommandClassification<'packet> {
    match LeDtmCommand::decode(command) {
        Ok(command) => LeControllerCommandClassification::Dtm(command),
        Err(error) => match error.into_invalid_parameters_command_complete() {
            Ok(response) => LeControllerCommandClassification::MalformedDtm(response),
            Err(_) if BootstrapCommand::supports(command.opcode()) => {
                LeControllerCommandClassification::Bootstrap(bootstrap.dispatch(command))
            }
            Err(_) => LeControllerCommandClassification::Unsupported(command),
        },
    }
}

#[cfg(test)]
mod tests {
    use bt_hci::{
        cmd::{Cmd, Opcode, OpcodeGroup, controller_baseband::Reset, le::LeSetAdvEnable},
        param::{Error as HciError, Status},
    };

    use super::{LeControllerCommandClassification, classify_le_controller_command};
    use crate::{
        BluetoothPublicDeviceAddress, BootstrapPhase, HciCommandPacket, LE_RECEIVER_TEST_V1_OPCODE,
        LE_TEST_END_OPCODE, LE_TRANSMITTER_TEST_V1_OPCODE, LeControllerBootstrap,
        LeControllerBootstrapConfig, LeDtmCommand,
    };

    #[test]
    fn bootstrap_command_produces_an_owned_software_response() {
        let mut bootstrap = bootstrap();
        let classified = classify_le_controller_command(
            &mut bootstrap,
            HciCommandPacket::for_test(Reset::OPCODE, &[]),
        );

        let LeControllerCommandClassification::Bootstrap(response) = classified else {
            panic!("Reset did not enter the bootstrap state machine");
        };
        assert_eq!(response.opcode(), Reset::OPCODE);
        assert_eq!(response.status(), Status::SUCCESS);
        assert_eq!(bootstrap.phase(), BootstrapPhase::Configuring);
    }

    #[test]
    fn valid_dtm_command_becomes_an_owned_semantic_token() {
        let mut bootstrap = bootstrap();
        let classified = classify_le_controller_command(
            &mut bootstrap,
            HciCommandPacket::for_test(LE_RECEIVER_TEST_V1_OPCODE, &[39]),
        );

        let LeControllerCommandClassification::Dtm(LeDtmCommand::ReceiverTestV1(command)) =
            classified
        else {
            panic!("valid receiver test did not become a semantic DTM command");
        };
        assert_eq!(command.channel().index(), 39);
        assert_eq!(bootstrap.phase(), BootstrapPhase::AwaitingReset);
    }

    #[test]
    fn every_malformed_known_dtm_family_produces_invalid_parameters() {
        let mut bootstrap = bootstrap();
        for (opcode, parameters) in [
            (LE_RECEIVER_TEST_V1_OPCODE, &[][..]),
            (LE_TRANSMITTER_TEST_V1_OPCODE, &[0, 1, 8][..]),
            (LE_TEST_END_OPCODE, &[0][..]),
        ] {
            let classified = classify_le_controller_command(
                &mut bootstrap,
                HciCommandPacket::for_test(opcode, parameters),
            );
            let LeControllerCommandClassification::MalformedDtm(response) = classified else {
                panic!("malformed known DTM command escaped its command family");
            };
            assert_eq!(response.opcode(), opcode);
            assert_eq!(
                response.status(),
                HciError::INVALID_HCI_PARAMETERS.to_status()
            );
        }
        assert_eq!(bootstrap.phase(), BootstrapPhase::AwaitingReset);
    }

    #[test]
    fn unsupported_packet_is_returned_unchanged_for_outer_routing() {
        let mut bootstrap = bootstrap();
        let parameters = [1];
        let classified = classify_le_controller_command(
            &mut bootstrap,
            HciCommandPacket::for_test(LeSetAdvEnable::OPCODE, &parameters),
        );

        assert_eq!(classified.opcode(), LeSetAdvEnable::OPCODE);
        let LeControllerCommandClassification::Unsupported(command) = classified else {
            panic!("an unsupported command was claimed by a closed command family");
        };
        assert_eq!(command.opcode(), LeSetAdvEnable::OPCODE);
        assert_eq!(command.parameters(), &parameters);
        assert_eq!(bootstrap.phase(), BootstrapPhase::AwaitingReset);
    }

    #[test]
    fn unrelated_opcode_group_is_not_collapsed_into_a_bootstrap_error() {
        let mut bootstrap = bootstrap();
        let opcode = Opcode::new(OpcodeGroup::VENDOR_SPECIFIC, 7);
        let classified = classify_le_controller_command(
            &mut bootstrap,
            HciCommandPacket::for_test(opcode, &[2, 3, 5]),
        );

        let LeControllerCommandClassification::Unsupported(command) = classified else {
            panic!("outer command family was claimed by the portable LE classifier");
        };
        assert_eq!(command.opcode(), opcode);
        assert_eq!(command.parameters(), &[2, 3, 5]);
    }

    fn bootstrap() -> LeControllerBootstrap {
        LeControllerBootstrap::new(
            LeControllerBootstrapConfig::new(
                BluetoothPublicDeviceAddress::from_canonical_bytes([2, 3, 5, 7, 11, 13]),
                27,
                1,
            )
            .expect("nonzero test profile"),
        )
    }
}
