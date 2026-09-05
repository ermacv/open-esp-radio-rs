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
mod tests;
