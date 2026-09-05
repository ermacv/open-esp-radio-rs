use open_esp_radio_bluetooth_hci::{
    LE_RECEIVER_TEST_V1_OPCODE, LE_RECEIVER_TEST_V2_OPCODE, LE_TRANSMITTER_TEST_V1_OPCODE,
    LE_TRANSMITTER_TEST_V2_OPCODE, LeDtmCommand,
};

use super::*;

#[test]
fn semantic_transmitter_command_projects_exactly_once_into_typed_chip_inputs() {
    let LeDtmCommand::TransmitterTest(command) =
        LeDtmCommand::decode_body(LE_TRANSMITTER_TEST_V1_OPCODE, &[39, 255, 7])
            .expect("the boundary accepts the maximum typed v1 program")
    else {
        panic!("the transmitter opcode changed semantic role");
    };

    let program = transmitter_program(&command);
    assert_eq!(program.channel.hci_image(), 39);
    assert_eq!(program.length.hci_image(), 255);
    assert_eq!(
        program.pattern,
        BluetoothDtmPayloadPattern::Repeated01010101
    );
    assert_eq!(program.phy, BluetoothDtmPhy::Le1M);
    assert_eq!(program.requested_interval_micros, 0);
    assert_eq!(command.channel().index(), 39);
}

#[test]
fn semantic_receiver_command_projects_to_the_legacy_one_m_phy() {
    let LeDtmCommand::ReceiverTest(command) =
        LeDtmCommand::decode_body(LE_RECEIVER_TEST_V1_OPCODE, &[0])
            .expect("the boundary accepts the first typed v1 channel")
    else {
        panic!("the receiver opcode changed semantic role");
    };

    let program = receiver_program(&command);
    assert_eq!(program.channel.hci_image(), 0);
    assert_eq!(program.phy, BluetoothDtmPhy::Le1M);
    assert_eq!(command.channel().index(), 0);
}

#[test]
fn enhanced_commands_project_all_reviewed_phy_modes() {
    for (selector, expected) in [
        (1, BluetoothDtmPhy::Le1M),
        (2, BluetoothDtmPhy::Le2M),
        (3, BluetoothDtmPhy::LeCoded),
    ] {
        for modulation_index in [0, 1] {
            let LeDtmCommand::ReceiverTest(command) = LeDtmCommand::decode_body(
                LE_RECEIVER_TEST_V2_OPCODE,
                &[7, selector, modulation_index],
            )
            .expect("the enhanced receiver mode is inside the reviewed chip domain") else {
                panic!("the receiver opcode changed semantic role");
            };
            assert_eq!(receiver_program(&command).phy, expected);
        }
    }

    for (selector, expected) in [
        (1, BluetoothDtmPhy::Le1M),
        (2, BluetoothDtmPhy::Le2M),
        (3, BluetoothDtmPhy::LeCoded),
        (4, BluetoothDtmPhy::LeCodedS2),
    ] {
        let LeDtmCommand::TransmitterTest(command) =
            LeDtmCommand::decode_body(LE_TRANSMITTER_TEST_V2_OPCODE, &[7, 37, 2, selector])
                .expect("the enhanced transmitter mode is inside the reviewed chip domain")
        else {
            panic!("the transmitter opcode changed semantic role");
        };
        assert_eq!(transmitter_program(&command).phy, expected);
    }
}
