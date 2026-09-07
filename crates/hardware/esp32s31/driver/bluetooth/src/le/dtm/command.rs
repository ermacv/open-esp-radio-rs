//! Pure semantic HCI-to-chip DTM command projection.

#![forbid(unsafe_code)]

use crate::le::dtm::{DtmChannel, DtmPayloadLength, DtmPayloadPattern, DtmPhy};

use oer_bluetooth_hci::{
    LeDtmModulationIndex, LeDtmPhy, LeReceiverTestCommand, LeTransmitterTestCommand,
};

pub(crate) struct DtmFirstTransmitterProgram {
    pub(crate) pattern: DtmPayloadPattern,
    pub(crate) length: DtmPayloadLength,
    pub(crate) channel: DtmChannel,
    pub(crate) phy: DtmPhy,
    pub(crate) requested_interval_micros: u16,
}

pub(crate) struct DtmFirstReceiverProgram {
    pub(crate) channel: DtmChannel,
    pub(crate) phy: DtmPhy,
}

pub(crate) fn transmitter_program(
    command: &LeTransmitterTestCommand,
) -> DtmFirstTransmitterProgram {
    let pattern = match command.payload_pattern() {
        oer_bluetooth_hci::LeDtmPayloadPattern::Prbs9 => DtmPayloadPattern::Prbs9,
        oer_bluetooth_hci::LeDtmPayloadPattern::Repeated11110000 => {
            DtmPayloadPattern::Repeated11110000
        }
        oer_bluetooth_hci::LeDtmPayloadPattern::Repeated10101010 => {
            DtmPayloadPattern::Repeated10101010
        }
        oer_bluetooth_hci::LeDtmPayloadPattern::Prbs15 => DtmPayloadPattern::Prbs15,
        oer_bluetooth_hci::LeDtmPayloadPattern::RepeatedAllOnes => {
            DtmPayloadPattern::RepeatedAllOnes
        }
        oer_bluetooth_hci::LeDtmPayloadPattern::RepeatedAllZeros => {
            DtmPayloadPattern::RepeatedAllZeros
        }
        oer_bluetooth_hci::LeDtmPayloadPattern::Repeated00001111 => {
            DtmPayloadPattern::Repeated00001111
        }
        oer_bluetooth_hci::LeDtmPayloadPattern::Repeated01010101 => {
            DtmPayloadPattern::Repeated01010101
        }
    };
    DtmFirstTransmitterProgram {
        pattern,
        length: DtmPayloadLength::from_hci_image(command.payload_length()),
        channel: DtmChannel::new(command.channel().index())
            .expect("semantic HCI DTM channel is inside the chip domain"),
        phy: chip_phy(command.phy()),
        requested_interval_micros: 0,
    }
}

pub(crate) fn receiver_program(command: &LeReceiverTestCommand) -> DtmFirstReceiverProgram {
    // The HCI parameter is a test-transmitter assumption. A receiver may use
    // it as an optimization, but the reviewed S31 RX context materializes only
    // channel and PHY; both valid assumptions therefore share one projection.
    match command.modulation_index() {
        LeDtmModulationIndex::Standard | LeDtmModulationIndex::Stable => {}
    }
    DtmFirstReceiverProgram {
        channel: DtmChannel::new(command.channel().index())
            .expect("semantic HCI DTM channel is inside the chip domain"),
        phy: chip_phy(command.phy()),
    }
}

const fn chip_phy(phy: LeDtmPhy) -> DtmPhy {
    match phy {
        LeDtmPhy::Le1M => DtmPhy::Le1M,
        LeDtmPhy::Le2M => DtmPhy::Le2M,
        LeDtmPhy::LeCoded => DtmPhy::LeCoded,
        LeDtmPhy::LeCodedS2 => DtmPhy::LeCodedS2,
    }
}

#[cfg(test)]
mod tests;
