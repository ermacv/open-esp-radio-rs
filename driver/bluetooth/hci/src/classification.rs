//! Portable command classification before chip-specific session execution.

use bt_hci::cmd::Opcode;

use crate::{
    BootstrapCommandCompleteEvent, HciCommandPacket, LeDtmCommand, LeDtmCommandCompleteEvent,
    OwnedBootstrapCommand,
    bootstrap::{BootstrapCommandDecodeError, invalid_parameters},
};

/// One finite result of classifying a validated HCI command packet.
///
/// Bootstrap and valid DTM commands are owned semantic tokens that may be
/// retained across asynchronous session transitions. Malformed responses are
/// complete owned packets that may be retained across Controller-to-Host
/// backpressure. Only [`Self::Unsupported`] borrows the receive buffer: an
/// outer router must inspect or consume it before that buffer is reused.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "a classified HCI command must be routed or answered exactly once"]
pub enum LeControllerCommandClassification<'packet> {
    /// A decoded bootstrap command awaits session-aware software dispatch.
    Bootstrap(OwnedBootstrapCommand),
    /// A known bootstrap opcode had malformed parameters and produced this response.
    MalformedBootstrap(BootstrapCommandCompleteEvent),
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
            Self::Bootstrap(command) => command.opcode(),
            Self::MalformedBootstrap(response) => response.opcode(),
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
/// is decoded only when it belongs to the closed bootstrap table; otherwise
/// the unchanged packet is returned to the outer router. Classification never
/// observes or advances a bootstrap epoch. No branch waits, publishes a
/// response, or claims hardware readiness.
pub fn classify_le_controller_command<'packet>(
    command: HciCommandPacket<'packet>,
) -> LeControllerCommandClassification<'packet> {
    match LeDtmCommand::decode(command) {
        Ok(command) => LeControllerCommandClassification::Dtm(command),
        Err(error) => match error.into_invalid_parameters_command_complete() {
            Ok(response) => LeControllerCommandClassification::MalformedDtm(response),
            Err(_) => match OwnedBootstrapCommand::decode(command) {
                Ok(command) => LeControllerCommandClassification::Bootstrap(command),
                Err(BootstrapCommandDecodeError::Malformed(kind)) => {
                    LeControllerCommandClassification::MalformedBootstrap(invalid_parameters(
                        kind.opcode(),
                    ))
                }
                Err(BootstrapCommandDecodeError::Unsupported) => {
                    LeControllerCommandClassification::Unsupported(command)
                }
            },
        },
    }
}

#[cfg(test)]
mod tests {
    use bt_hci::{
        cmd::{
            Cmd, Opcode, OpcodeGroup,
            controller_baseband::{Reset, SetEventMask},
            le::{LeSetAdvEnable, LeSetRandomAddr},
        },
        param::{BdAddr, Error as HciError, EventMask, Status},
    };

    use super::{LeControllerCommandClassification, classify_le_controller_command};
    use crate::{
        BluetoothPublicDeviceAddress, BootstrapCommand, BootstrapPhase, HciCommandPacket,
        LE_RECEIVER_TEST_V1_OPCODE, LE_TEST_END_OPCODE, LE_TRANSMITTER_TEST_V1_OPCODE,
        LeControllerBootstrap, LeControllerBootstrapConfig, LeDtmCommand, OwnedBootstrapCommand,
    };

    #[test]
    fn bootstrap_command_is_owned_without_advancing_software_state() {
        let mut bootstrap = bootstrap();
        let classified =
            classify_le_controller_command(HciCommandPacket::for_test(Reset::OPCODE, &[]));

        let LeControllerCommandClassification::Bootstrap(command) = classified else {
            panic!("Reset did not become an owned bootstrap command");
        };
        assert_eq!(command.kind(), BootstrapCommand::Reset);
        assert!(command.is_reset());
        assert_eq!(bootstrap.phase(), BootstrapPhase::AwaitingReset);

        let response = bootstrap.dispatch_owned(command);
        assert_eq!(response.status(), Status::SUCCESS);
        assert_eq!(bootstrap.phase(), BootstrapPhase::Configuring);
    }

    #[test]
    fn bootstrap_payload_is_typed_and_independent_of_receive_storage() {
        let mut parameters = [6, 5, 4, 3, 2, 0xc1];
        let command = match classify_le_controller_command(HciCommandPacket::for_test(
            LeSetRandomAddr::OPCODE,
            &parameters,
        )) {
            LeControllerCommandClassification::Bootstrap(command) => command,
            _ => panic!("random address did not become an owned bootstrap command"),
        };

        parameters.fill(0);
        let OwnedBootstrapCommand::LeSetRandomAddress(address) = command else {
            panic!("random address lost its semantic bootstrap variant");
        };
        assert_eq!(address, BdAddr::new([6, 5, 4, 3, 2, 0xc1]));
    }

    #[test]
    fn active_reset_can_be_held_until_the_session_policy_dispatches_it() {
        let mut bootstrap = bootstrap();
        assert_eq!(
            bootstrap
                .dispatch_owned(OwnedBootstrapCommand::Reset)
                .status(),
            Status::SUCCESS
        );
        let requested_mask = EventMask::new().enable_hardware_error(true);
        assert_eq!(
            bootstrap
                .dispatch_owned(OwnedBootstrapCommand::SetEventMask(requested_mask))
                .status(),
            Status::SUCCESS
        );

        let classified =
            classify_le_controller_command(HciCommandPacket::for_test(Reset::OPCODE, &[]));
        assert_eq!(bootstrap.event_mask(), requested_mask);
        let LeControllerCommandClassification::Bootstrap(reset) = classified else {
            panic!("active Reset did not remain an owned policy input");
        };
        assert!(reset.is_reset());
        assert_eq!(bootstrap.event_mask(), requested_mask);

        assert_eq!(bootstrap.dispatch_owned(reset).status(), Status::SUCCESS);
        assert_eq!(bootstrap.event_mask(), EventMask::new());
    }

    #[test]
    fn malformed_known_bootstrap_is_owned_without_touching_an_epoch() {
        let mut bootstrap = bootstrap();
        let requested_mask = EventMask::new().enable_hardware_error(true);
        assert_eq!(
            bootstrap
                .dispatch_owned(OwnedBootstrapCommand::Reset)
                .status(),
            Status::SUCCESS
        );
        assert_eq!(
            bootstrap
                .dispatch_owned(OwnedBootstrapCommand::SetEventMask(requested_mask))
                .status(),
            Status::SUCCESS
        );

        let classified = classify_le_controller_command(HciCommandPacket::for_test(
            SetEventMask::OPCODE,
            &[0; 7],
        ));

        let LeControllerCommandClassification::MalformedBootstrap(response) = classified else {
            panic!("malformed bootstrap command escaped its command family");
        };
        assert_eq!(response.opcode(), SetEventMask::OPCODE);
        assert_eq!(
            response.status(),
            HciError::INVALID_HCI_PARAMETERS.to_status()
        );
        assert_eq!(bootstrap.phase(), BootstrapPhase::Configuring);
        assert_eq!(bootstrap.event_mask(), requested_mask);
    }

    #[test]
    fn valid_dtm_command_becomes_an_owned_semantic_token() {
        let classified = classify_le_controller_command(HciCommandPacket::for_test(
            LE_RECEIVER_TEST_V1_OPCODE,
            &[39],
        ));

        let LeControllerCommandClassification::Dtm(LeDtmCommand::ReceiverTestV1(command)) =
            classified
        else {
            panic!("valid receiver test did not become a semantic DTM command");
        };
        assert_eq!(command.channel().index(), 39);
    }

    #[test]
    fn every_malformed_known_dtm_family_produces_invalid_parameters() {
        for (opcode, parameters) in [
            (LE_RECEIVER_TEST_V1_OPCODE, &[][..]),
            (LE_TRANSMITTER_TEST_V1_OPCODE, &[0, 1, 8][..]),
            (LE_TEST_END_OPCODE, &[0][..]),
        ] {
            let classified =
                classify_le_controller_command(HciCommandPacket::for_test(opcode, parameters));
            let LeControllerCommandClassification::MalformedDtm(response) = classified else {
                panic!("malformed known DTM command escaped its command family");
            };
            assert_eq!(response.opcode(), opcode);
            assert_eq!(
                response.status(),
                HciError::INVALID_HCI_PARAMETERS.to_status()
            );
        }
    }

    #[test]
    fn unsupported_packet_is_returned_unchanged_for_outer_routing() {
        let parameters = [1];
        let classified = classify_le_controller_command(HciCommandPacket::for_test(
            LeSetAdvEnable::OPCODE,
            &parameters,
        ));

        assert_eq!(classified.opcode(), LeSetAdvEnable::OPCODE);
        let LeControllerCommandClassification::Unsupported(command) = classified else {
            panic!("an unsupported command was claimed by a closed command family");
        };
        assert_eq!(command.opcode(), LeSetAdvEnable::OPCODE);
        assert_eq!(command.parameters(), &parameters);
    }

    #[test]
    fn unrelated_opcode_group_is_not_collapsed_into_a_bootstrap_error() {
        let opcode = Opcode::new(OpcodeGroup::VENDOR_SPECIFIC, 7);
        let classified =
            classify_le_controller_command(HciCommandPacket::for_test(opcode, &[2, 3, 5]));

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
