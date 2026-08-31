//! Pure semantic HCI-to-chip DTM command projection.

#![forbid(unsafe_code)]

use open_esp_radio_bluetooth_hci::{
    LeDtmModulationIndex, LeDtmPhy, LeReceiverTestCommand, LeTransmitterTestCommand,
};

use crate::{
    BluetoothDtmChannel, BluetoothDtmPayloadLength, BluetoothDtmPayloadPattern, BluetoothDtmPhy,
};

pub(crate) struct BluetoothDtmFirstTransmitterProgram {
    pub(crate) pattern: BluetoothDtmPayloadPattern,
    pub(crate) length: BluetoothDtmPayloadLength,
    pub(crate) channel: BluetoothDtmChannel,
    pub(crate) phy: BluetoothDtmPhy,
    pub(crate) requested_interval_micros: u16,
}

pub(crate) struct BluetoothDtmFirstReceiverProgram {
    pub(crate) channel: BluetoothDtmChannel,
    pub(crate) phy: BluetoothDtmPhy,
}

pub(crate) fn transmitter_program(
    command: &LeTransmitterTestCommand,
) -> BluetoothDtmFirstTransmitterProgram {
    let pattern = match command.payload_pattern() {
        open_esp_radio_bluetooth_hci::LeDtmPayloadPattern::Prbs9 => {
            BluetoothDtmPayloadPattern::Prbs9
        }
        open_esp_radio_bluetooth_hci::LeDtmPayloadPattern::Repeated11110000 => {
            BluetoothDtmPayloadPattern::Repeated11110000
        }
        open_esp_radio_bluetooth_hci::LeDtmPayloadPattern::Repeated10101010 => {
            BluetoothDtmPayloadPattern::Repeated10101010
        }
        open_esp_radio_bluetooth_hci::LeDtmPayloadPattern::Prbs15 => {
            BluetoothDtmPayloadPattern::Prbs15
        }
        open_esp_radio_bluetooth_hci::LeDtmPayloadPattern::RepeatedAllOnes => {
            BluetoothDtmPayloadPattern::RepeatedAllOnes
        }
        open_esp_radio_bluetooth_hci::LeDtmPayloadPattern::RepeatedAllZeros => {
            BluetoothDtmPayloadPattern::RepeatedAllZeros
        }
        open_esp_radio_bluetooth_hci::LeDtmPayloadPattern::Repeated00001111 => {
            BluetoothDtmPayloadPattern::Repeated00001111
        }
        open_esp_radio_bluetooth_hci::LeDtmPayloadPattern::Repeated01010101 => {
            BluetoothDtmPayloadPattern::Repeated01010101
        }
    };
    BluetoothDtmFirstTransmitterProgram {
        pattern,
        length: BluetoothDtmPayloadLength::from_hci_image(command.payload_length()),
        channel: BluetoothDtmChannel::new(command.channel().index())
            .expect("semantic HCI DTM channel is inside the chip domain"),
        phy: chip_phy(command.phy()),
        requested_interval_micros: 0,
    }
}

pub(crate) fn receiver_program(
    command: &LeReceiverTestCommand,
) -> BluetoothDtmFirstReceiverProgram {
    // The HCI parameter is a test-transmitter assumption. A receiver may use
    // it as an optimization, but the reviewed S31 RX context materializes only
    // channel and PHY; both valid assumptions therefore share one projection.
    match command.modulation_index() {
        LeDtmModulationIndex::Standard | LeDtmModulationIndex::Stable => {}
    }
    BluetoothDtmFirstReceiverProgram {
        channel: BluetoothDtmChannel::new(command.channel().index())
            .expect("semantic HCI DTM channel is inside the chip domain"),
        phy: chip_phy(command.phy()),
    }
}

const fn chip_phy(phy: LeDtmPhy) -> BluetoothDtmPhy {
    match phy {
        LeDtmPhy::Le1M => BluetoothDtmPhy::Le1M,
        LeDtmPhy::Le2M => BluetoothDtmPhy::Le2M,
        LeDtmPhy::LeCoded => BluetoothDtmPhy::LeCoded,
        LeDtmPhy::LeCodedS2 => BluetoothDtmPhy::LeCodedS2,
    }
}

#[cfg(test)]
mod tests {
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
}
