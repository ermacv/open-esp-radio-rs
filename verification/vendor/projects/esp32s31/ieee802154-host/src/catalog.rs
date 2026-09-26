//! Named vendor scenarios.
//!
//! Event bits, abort codes and frame layouts are the pinned ESP-IDF values
//! (`ieee802154_common_ll.h`, `esp_ieee802154.h`): an RX/TX buffer is
//! `[length, psdu...]`, the length counts the two FCS bytes, and a received
//! buffer carries RSSI and LQI in place of the FCS.

use crate::{
    record::Inputs,
    scenario::{Scenario, Step},
};

/// `IEEE802154_EVENT_*` bits.
pub mod event {
    pub const TX_DONE: u16 = 1 << 0;
    pub const RX_DONE: u16 = 1 << 1;
    pub const ACK_TX_DONE: u16 = 1 << 2;
    pub const ACK_RX_DONE: u16 = 1 << 3;
    pub const RX_ABORT: u16 = 1 << 4;
    pub const TX_ABORT: u16 = 1 << 5;
    pub const ED_DONE: u16 = 1 << 6;
    pub const TIMER0_OVERFLOW: u16 = 1 << 8;
}

/// `IEEE802154_RX_ABORT_BY_*` codes used by the catalog.
pub mod rx_abort {
    pub const CRC_ERROR: u8 = 3;
}

/// `IEEE802154_TX_ABORT_BY_*` codes used by the catalog.
pub mod tx_abort {
    pub const CCA_BUSY: u8 = 25;
}

/// Data frame, PAN-ID compression, short addresses, 2006 frame version.
/// `acknowledge` sets the ACK-request bit.
fn data_frame(acknowledge: bool) -> Vec<u8> {
    let control = if acknowledge { 0x61 } else { 0x41 };
    // length 12: FCF(2) sequence(1) PAN(2) destination(2) source(2) payload(1) FCS(2)
    vec![
        12, control, 0x88, 0x01, 0x34, 0x12, 0xff, 0xff, 0x78, 0x56, 0xaa, 0x00, 0x00,
    ]
}

/// Immediate ACK for sequence one, RSSI -60 and LQI 200 in place of the FCS.
fn received_ack() -> Vec<u8> {
    vec![5, 0x02, 0x00, 0x01, (-60i8) as u8, 200]
}

/// Received data frame requesting an ACK, RSSI -55 and LQI 180.
fn received_data_frame() -> Vec<u8> {
    let mut frame = data_frame(true);
    let len = frame.len();
    frame[len - 2] = (-55i8) as u8;
    frame[len - 1] = 180;
    frame
}

fn no_inputs() -> Inputs {
    Inputs::default()
}

fn enable() -> Vec<Step> {
    vec![Step::Enable]
}

fn transmit_without_ack() -> Vec<Step> {
    vec![
        Step::Enable,
        Step::Transmit {
            frame: data_frame(false),
            cca: false,
        },
        Step::Interrupt {
            events: event::TX_DONE,
            rx_abort: 0,
            tx_abort: 0,
        },
    ]
}

fn transmit_with_ack() -> Vec<Step> {
    vec![
        Step::Enable,
        Step::Transmit {
            frame: data_frame(true),
            cca: false,
        },
        Step::Interrupt {
            events: event::TX_DONE,
            rx_abort: 0,
            tx_abort: 0,
        },
        Step::DeliverFrame(received_ack()),
        Step::Interrupt {
            events: event::ACK_RX_DONE,
            rx_abort: 0,
            tx_abort: 0,
        },
    ]
}

fn transmit_with_cca_busy() -> Vec<Step> {
    vec![
        Step::Enable,
        Step::Transmit {
            frame: data_frame(false),
            cca: true,
        },
        Step::Interrupt {
            events: event::TX_ABORT,
            rx_abort: 0,
            tx_abort: tx_abort::CCA_BUSY,
        },
    ]
}

fn receive_with_auto_ack() -> Vec<Step> {
    vec![
        Step::Enable,
        Step::SetRxWhenIdle(true),
        Step::Receive,
        Step::Input("ieee802154_ll_get_tx_auto_ack", 1),
        Step::DeliverFrame(received_data_frame()),
        Step::Interrupt {
            events: event::RX_DONE,
            rx_abort: 0,
            tx_abort: 0,
        },
        Step::Interrupt {
            events: event::ACK_TX_DONE,
            rx_abort: 0,
            tx_abort: 0,
        },
        Step::ReceiveHandleDone,
    ]
}

fn receive_crc_error_restarts() -> Vec<Step> {
    vec![
        Step::Enable,
        Step::SetRxWhenIdle(true),
        Step::Receive,
        Step::Interrupt {
            events: event::RX_ABORT,
            rx_abort: rx_abort::CRC_ERROR,
            tx_abort: 0,
        },
    ]
}

fn energy_detect() -> Vec<Step> {
    vec![
        Step::Enable,
        Step::EnergyDetect(8),
        Step::Input("ieee802154_ll_get_ed_rss", (-70i64) as u64),
        Step::Interrupt {
            events: event::ED_DONE,
            rx_abort: 0,
            tx_abort: 0,
        },
    ]
}

fn cca_busy() -> Vec<Step> {
    vec![
        Step::Enable,
        Step::Cca,
        Step::Input("ieee802154_ll_is_cca_busy", 1),
        Step::Interrupt {
            events: event::ED_DONE,
            rx_abort: 0,
            tx_abort: 0,
        },
    ]
}

/// Every catalog scenario.
pub const SCENARIOS: &[Scenario] = &[
    Scenario {
        name: "enable",
        inputs: no_inputs,
        steps: enable,
    },
    Scenario {
        name: "transmit-without-ack",
        inputs: no_inputs,
        steps: transmit_without_ack,
    },
    Scenario {
        name: "transmit-with-ack",
        inputs: no_inputs,
        steps: transmit_with_ack,
    },
    Scenario {
        name: "transmit-with-cca-busy",
        inputs: no_inputs,
        steps: transmit_with_cca_busy,
    },
    Scenario {
        name: "receive-with-auto-ack",
        inputs: no_inputs,
        steps: receive_with_auto_ack,
    },
    Scenario {
        name: "receive-crc-error-restarts",
        inputs: no_inputs,
        steps: receive_crc_error_restarts,
    },
    Scenario {
        name: "energy-detect",
        inputs: no_inputs,
        steps: energy_detect,
    },
    Scenario {
        name: "cca-busy",
        inputs: no_inputs,
        steps: cca_busy,
    },
];
