use bt_hci::{
    cmd::{
        Cmd, Opcode, OpcodeGroup,
        controller_baseband::{Reset, SetEventMask},
        le::{
            LeSetAdvData, LeSetAdvEnable, LeSetAdvParams, LeSetRandomAddr, LeSetScanResponseData,
        },
    },
    param::{BdAddr, Error as HciError, EventMask, Status},
};

use super::{LeControllerCommandClassification, classify_le_controller_command};
use crate::{
    BluetoothPublicDeviceAddress, BootstrapCommand, BootstrapPhase, HciCommandPacket,
    LE_RECEIVER_TEST_V1_OPCODE, LE_RECEIVER_TEST_V2_OPCODE, LE_TEST_END_OPCODE,
    LE_TRANSMITTER_TEST_V1_OPCODE, LE_TRANSMITTER_TEST_V2_OPCODE, LeControllerBootstrap,
    LeControllerBootstrapConfig, LeDtmCommand, LeLegacyAdvertisingConfigurationCommand,
    OwnedBootstrapCommand,
};

#[test]
fn bootstrap_command_is_owned_without_advancing_software_state() {
    let mut bootstrap = bootstrap();
    let classified = classify_le_controller_command(HciCommandPacket::for_test(Reset::OPCODE, &[]));

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

    let classified = classify_le_controller_command(HciCommandPacket::for_test(Reset::OPCODE, &[]));
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

    let classified =
        classify_le_controller_command(HciCommandPacket::for_test(SetEventMask::OPCODE, &[0; 7]));

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

    let LeControllerCommandClassification::Dtm(LeDtmCommand::ReceiverTest(command)) = classified
    else {
        panic!("valid receiver test did not become a semantic DTM command");
    };
    assert_eq!(command.channel().index(), 39);
}

#[test]
fn advertising_configuration_and_enable_are_owned() {
    let parameters = classify_le_controller_command(HciCommandPacket::for_test(
        LeSetAdvParams::OPCODE,
        &[
            0x20, 0x00, 0x40, 0x00, 0x03, 0x00, 0x00, 0, 0, 0, 0, 0, 0, 0x07, 0x00,
        ],
    ));
    assert!(matches!(
        parameters,
        LeControllerCommandClassification::LegacyAdvertisingConfiguration(
            LeLegacyAdvertisingConfigurationCommand::SetParameters(_)
        )
    ));

    let data =
        classify_le_controller_command(HciCommandPacket::for_test(LeSetAdvData::OPCODE, &[0; 32]));
    assert!(matches!(
        data,
        LeControllerCommandClassification::LegacyAdvertisingConfiguration(
            LeLegacyAdvertisingConfigurationCommand::SetData(_)
        )
    ));

    let scan_response = classify_le_controller_command(HciCommandPacket::for_test(
        LeSetScanResponseData::OPCODE,
        &[0; 32],
    ));
    assert!(matches!(
        scan_response,
        LeControllerCommandClassification::LegacyAdvertisingConfiguration(
            LeLegacyAdvertisingConfigurationCommand::SetScanResponseData(_)
        )
    ));

    let enable =
        classify_le_controller_command(HciCommandPacket::for_test(LeSetAdvEnable::OPCODE, &[1]));
    assert!(matches!(
        enable,
        LeControllerCommandClassification::LegacyAdvertisingEnable(_)
    ));
}

#[test]
fn malformed_claimed_advertising_configuration_has_exact_status() {
    for opcode in [LeSetAdvData::OPCODE, LeSetScanResponseData::OPCODE] {
        let classified =
            classify_le_controller_command(HciCommandPacket::for_test(opcode, &[32; 32]));
        let LeControllerCommandClassification::MalformedLegacyAdvertising(response) = classified
        else {
            panic!("invalid advertising data escaped its claimed family");
        };
        assert_eq!(response.opcode(), opcode);
        assert_eq!(
            response.status(),
            HciError::INVALID_HCI_PARAMETERS.to_status()
        );
    }
}

#[test]
fn known_dtm_rejections_retain_their_required_status() {
    for (opcode, parameters, status) in [
        (
            LE_RECEIVER_TEST_V1_OPCODE,
            &[][..],
            HciError::INVALID_HCI_PARAMETERS.to_status(),
        ),
        (
            LE_TRANSMITTER_TEST_V1_OPCODE,
            &[0, 1, 8][..],
            HciError::INVALID_HCI_PARAMETERS.to_status(),
        ),
        (
            LE_RECEIVER_TEST_V2_OPCODE,
            &[0, 4, 0][..],
            HciError::UNSUPPORTED.to_status(),
        ),
        (
            LE_TRANSMITTER_TEST_V2_OPCODE,
            &[0, 1, 0, 5][..],
            HciError::UNSUPPORTED.to_status(),
        ),
        (
            LE_TEST_END_OPCODE,
            &[0][..],
            HciError::INVALID_HCI_PARAMETERS.to_status(),
        ),
    ] {
        let classified =
            classify_le_controller_command(HciCommandPacket::for_test(opcode, parameters));
        let LeControllerCommandClassification::MalformedDtm(response) = classified else {
            panic!("malformed known DTM command escaped its command family");
        };
        assert_eq!(response.opcode(), opcode);
        assert_eq!(response.status(), status);
    }
}

#[test]
fn malformed_enable_response_is_owned_across_receive_storage_reuse() {
    let mut parameters = [2];
    let classified = classify_le_controller_command(HciCommandPacket::for_test(
        LeSetAdvEnable::OPCODE,
        &parameters,
    ));
    parameters.fill(0);

    assert_eq!(classified.opcode(), LeSetAdvEnable::OPCODE);
    let LeControllerCommandClassification::MalformedLegacyAdvertising(response) = classified else {
        panic!("malformed Enable did not produce its owned response");
    };
    assert_eq!(response.opcode(), LeSetAdvEnable::OPCODE);
    assert_eq!(
        response.status(),
        HciError::INVALID_HCI_PARAMETERS.to_status()
    );
}

#[test]
fn unrelated_opcode_group_produces_an_exact_unknown_command_completion() {
    let opcode = Opcode::new(OpcodeGroup::VENDOR_SPECIFIC, 7);
    let classified = classify_le_controller_command(HciCommandPacket::for_test(opcode, &[2, 3, 5]));

    let LeControllerCommandClassification::Unsupported(response) = classified else {
        panic!("unclaimed opcode did not produce an owned terminal response");
    };
    assert_eq!(response.opcode(), opcode);
    assert_eq!(response.status(), HciError::UNKNOWN_CMD.to_status());
    assert_eq!(response.as_bytes().len(), 6);
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
