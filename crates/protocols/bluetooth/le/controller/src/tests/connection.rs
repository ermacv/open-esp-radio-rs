//! The peripheral connection from its indication to its end.

use std::vec::Vec;

use bt_hci::cmd::{Opcode, OpcodeGroup};
use oer_bluetooth_radio::{
    ConnectionEventTiming, DataPduKind, EventId, EventResult, RadioInstant, RadioOutcome,
    ReceivedPdu,
};

use super::{Harness, Request, SET_ADV_ENABLE, SET_EVENT_MASK, SUCCESS};

const DISCONNECT: Opcode = Opcode::new(OpcodeGroup::LINK_CONTROL, 0x0006);
const LTK_REPLY: Opcode = Opcode::new(OpcodeGroup::LE, 0x001a);

/// On-air start of the connection indication.
const INDICATION_AT: u64 = 20_000;
/// 30 ms interval, 1 s supervision timeout, 2.5 ms transmit window at offset 0.
const INTERVAL: u64 = 30_000;
const TIMEOUT: u64 = 1_000_000;

/// `CONNECT_IND` from a random initiator to the public advertiser, asking for
/// Channel Selection Algorithm #2 with a 50 ppm central clock.
fn connect_ind() -> Vec<u8> {
    let mut pdu = std::vec![0x05 | 0x20 | 0x40, 34];
    pdu.extend_from_slice(&[0xa1, 0xa2, 0xa3, 0xa4, 0xa5, 0xc6]); // InitA
    pdu.extend_from_slice(&[6, 5, 4, 3, 2, 1]); // AdvA
    pdu.extend_from_slice(&0x5a3c_71e9_u32.to_le_bytes()); // AA
    pdu.extend_from_slice(&[0x55, 0x66, 0x77]); // CRCInit
    pdu.push(2); // WinSize
    pdu.extend_from_slice(&0_u16.to_le_bytes()); // WinOffset
    pdu.extend_from_slice(&24_u16.to_le_bytes()); // Interval
    pdu.extend_from_slice(&0_u16.to_le_bytes()); // Latency
    pdu.extend_from_slice(&100_u16.to_le_bytes()); // Timeout
    pdu.extend_from_slice(&[0xff, 0xff, 0xff, 0xff, 0x1f]); // ChM
    pdu.push(7 | (5 << 5)); // Hop, SCA
    pdu
}

impl Harness {
    /// A connected peripheral whose first event is in progress.
    fn connected() -> (Self, EventId) {
        Self::connected_from(Self::configured())
    }

    fn connected_from(mut harness: Self) -> (Self, EventId) {
        // LE Meta plus Disconnection Complete, Encryption Change and Read
        // Remote Version Information Complete.
        let mask = [0x90, 0x08, 0, 0, 0, 0, 0, 0x20];
        assert_eq!(harness.command(SET_EVENT_MASK, &mask), Some(SUCCESS));
        let mut parameters = super::nonconnectable_parameters();
        parameters[4] = 0x00; // ADV_IND
        assert_eq!(
            harness.command(super::SET_ADV_PARAMS, &parameters),
            Some(SUCCESS)
        );
        harness.send(SET_ADV_ENABLE, &[1]);
        let Some(Request::ConfigureConnectable(adv_ind, scan_rsp)) = harness.step() else {
            panic!("a connectable set");
        };
        assert_eq!(adv_ind[0] & 0x0f, 0x00);
        assert_eq!(adv_ind[0] & 0x20, 0x20, "Channel Selection Algorithm #2");
        assert_eq!(scan_rsp[..8], [0x04, 6, 6, 5, 4, 3, 2, 1]);
        assert_eq!(harness.status_of(SET_ADV_ENABLE), Some(SUCCESS));
        let Some(Request::Advertise(event)) = harness.step() else {
            panic!("an advertising event");
        };
        harness.core.outcome(RadioOutcome::Received {
            id: event.id,
            pdu: ReceivedPdu {
                pdu: &connect_ind(),
                rssi_dbm: -50,
                captured_at: Some(RadioInstant::from_micros(INDICATION_AT)),
            },
        });
        let Some(Request::OpenConnection(configuration)) = harness.step() else {
            panic!("the connection opens");
        };
        assert_eq!(
            configuration.access_address.0,
            0x5a3c_71e9_u32.to_le_bytes()
        );
        assert_eq!(configuration.crc_init.0, [0x55, 0x66, 0x77]);
        assert_eq!(configuration.created_at.as_micros(), INDICATION_AT);
        let events = harness.drain();
        assert_eq!(events.len(), 1);
        assert_eq!(&events[0][..5], &[0x3e, 19, 0x01, SUCCESS, 0]);

        // The first event listens across the transmit window.
        let Some(Request::ConnectionEvent(first)) = harness.step() else {
            panic!("the first event");
        };
        let anchor = INDICATION_AT + 352 + 1_250;
        assert_eq!(first.window.start().as_micros(), anchor - 17);
        assert_eq!(
            first.timing,
            ConnectionEventTiming::First {
                transmit_window: oer_bluetooth_radio::RadioDuration::from_micros(2_500),
                timing_guard: oer_bluetooth_radio::RadioDuration::from_micros(16),
            }
        );
        assert_eq!(first.priority, 13);
        // Advertising stops: its event is cancelled and the set removed.
        assert_eq!(harness.step(), Some(Request::Cancel(event.id)));
        harness.end(event.id);
        assert_eq!(harness.step(), Some(Request::RemoveAdvertising));
        assert!(
            harness.drain().is_empty(),
            "advertising ends without an event"
        );
        (harness, first.id)
    }

    fn receive(&mut self, id: EventId, pdu: &[u8]) {
        self.core.outcome(RadioOutcome::Received {
            id,
            pdu: ReceivedPdu {
                pdu,
                rssi_dbm: -40,
                captured_at: None,
            },
        });
    }

    fn end_at(&mut self, id: EventId, anchor: Option<u64>) {
        self.core.outcome(RadioOutcome::EventEnded {
            id,
            result: match anchor {
                Some(anchor) => EventResult::Executed {
                    anchor: Some(RadioInstant::from_micros(anchor)),
                },
                None => EventResult::NotExecuted,
            },
        });
    }

    fn acknowledge(&mut self) {
        self.core.outcome(RadioOutcome::TransmitAcknowledged(
            crate::peripheral::CONNECTION,
        ));
    }
}

#[test]
fn a_connection_answers_version_exchange_and_follows_the_received_anchor() {
    let (mut harness, first) = Harness::connected();
    // LL_VERSION_IND from the central.
    harness.receive(first, &[0x03, 6, 0x0c, 0x0c, 0x5f, 0x00, 0x01, 0x00]);
    let captured = INDICATION_AT + 352 + 1_250 + 700;
    harness.end_at(first, Some(captured));
    assert_eq!(
        harness.step(),
        Some(Request::Transmit(
            DataPduKind::Control,
            std::vec![0x0c, 0x0d, 0xff, 0xff, 0x01, 0x00]
        ))
    );
    let Some(Request::ConnectionEvent(second)) = harness.step() else {
        panic!("the second event");
    };
    // 30 ms after the received anchor, widened by 30 ms at 550 ppm.
    let widening = 30 * 550 / 1_000 + 63;
    assert_eq!(
        second.window.start().as_micros(),
        captured + INTERVAL - 10 - widening - 1
    );
    assert_eq!(
        second.timing,
        ConnectionEventTiming::Recurring {
            receive_wait: oer_bluetooth_radio::RadioDuration::from_micros(
                10 + 2 * widening as u32 + 2
            ),
        }
    );
    assert_eq!(second.priority, 8);
}

#[test]
fn acl_data_flows_both_ways_and_completes_host_packets() {
    let (mut harness, first) = Harness::connected();
    // Central data: a 3-octet L2CAP start fragment.
    harness.receive(first, &[0x02, 3, 0xaa, 0xbb, 0xcc]);
    let received = harness.drain();
    assert_eq!(received, [std::vec![0x00, 0x20, 3, 0, 0xaa, 0xbb, 0xcc]]);
    harness.end_at(first, Some(INDICATION_AT + 2_000));

    // Host data: 30 octets become a start and a continuation fragment.
    assert!(harness.core.is_acl_ready());
    let data: Vec<u8> = (0..30).collect();
    let packet = bt_hci::data::AclPacket::new(
        bt_hci::param::ConnHandle::new(0),
        bt_hci::data::AclPacketBoundary::FirstNonFlushable,
        bt_hci::data::AclBroadcastFlag::PointToPoint,
        &data,
    );
    harness.core.acl(packet);
    assert!(!harness.core.is_acl_ready());
    let Some(Request::Transmit(DataPduKind::Start, fragment)) = harness.step() else {
        panic!("the first fragment");
    };
    assert_eq!(fragment.len(), 27);
    let Some(Request::ConnectionEvent(second)) = harness.step() else {
        panic!("the second event");
    };
    harness.acknowledge();
    harness.end_at(second.id, Some(INDICATION_AT + 2_000 + INTERVAL));
    let Some(Request::Transmit(DataPduKind::Continuation, rest)) = harness.step() else {
        panic!("the continuation");
    };
    assert_eq!(rest, (27..30).collect::<Vec<u8>>());
    let Some(Request::ConnectionEvent(third)) = harness.step() else {
        panic!("the third event");
    };
    harness.acknowledge();
    // Number Of Completed Packets for the handle.
    assert_eq!(harness.drain(), [std::vec![0x13, 5, 1, 0, 0, 1, 0]]);
    assert!(harness.core.is_acl_ready());
    harness.end_at(third.id, None);
}

#[test]
fn host_disconnect_terminates_after_the_central_acknowledged() {
    let (mut harness, first) = Harness::connected();
    harness.end_at(first, Some(INDICATION_AT + 2_000));
    assert_eq!(
        harness.command(DISCONNECT, &[0, 0, 0x13]),
        Some(SUCCESS),
        "Command Status"
    );
    assert_eq!(
        harness.step(),
        Some(Request::Transmit(
            DataPduKind::Control,
            std::vec![0x02, 0x13]
        ))
    );
    let Some(Request::ConnectionEvent(second)) = harness.step() else {
        panic!("the second event");
    };
    harness.acknowledge();
    harness.end_at(second.id, Some(INDICATION_AT + 2_000 + INTERVAL));
    assert_eq!(harness.step(), Some(Request::CloseConnection));
    // Disconnection Complete reports the local Host.
    assert_eq!(harness.drain(), [std::vec![0x05, 4, 0, 0, 0, 0x16]]);
    assert!(!harness.core.wants_radio());
    assert!(
        harness.core.is_acl_ready(),
        "data without a connection is dropped"
    );
}

#[test]
fn the_central_terminates_the_connection() {
    let (mut harness, first) = Harness::connected();
    harness.receive(first, &[0x03, 2, 0x02, 0x13]);
    harness.end_at(first, Some(INDICATION_AT + 2_000));
    assert_eq!(harness.step(), Some(Request::CloseConnection));
    assert_eq!(harness.drain(), [std::vec![0x05, 4, 0, 0, 0, 0x13]]);
}

#[test]
fn six_silent_events_fail_the_establishment() {
    let (mut harness, first) = Harness::connected();
    harness.end_at(first, None);
    for _ in 0..5 {
        let Some(Request::ConnectionEvent(event)) = harness.step() else {
            panic!("another event");
        };
        // Before a packet arrives, the transmit window stays uncertain.
        let ConnectionEventTiming::Recurring { receive_wait } = event.timing else {
            panic!("a recurring event");
        };
        assert!(receive_wait.as_micros() > 2_500);
        harness.end_at(event.id, None);
    }
    assert_eq!(harness.step(), Some(Request::CloseConnection));
    assert_eq!(harness.drain(), [std::vec![0x05, 4, 0, 0, 0, 0x3e]]);
}

#[test]
fn the_supervision_timeout_ends_a_silent_connection() {
    let (mut harness, first) = Harness::connected();
    let captured = INDICATION_AT + 2_000;
    harness.end_at(first, Some(captured));
    let mut events = 0;
    loop {
        match harness.step() {
            Some(Request::ConnectionEvent(event)) => {
                events += 1;
                harness.end_at(event.id, None);
            }
            Some(Request::CloseConnection) => break,
            other => panic!("unexpected {other:?}"),
        }
    }
    assert_eq!(events as u64, TIMEOUT / INTERVAL);
    assert_eq!(harness.drain(), [std::vec![0x05, 4, 0, 0, 0, 0x08]]);
}

#[test]
fn encryption_start_asks_the_host_for_its_key() {
    let (mut harness, first) = Harness::connected();
    // LL_ENC_REQ: Rand, EDIV, SKDm, IVm.
    let mut request = std::vec![0x03, 23, 0x03];
    request.extend_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
    request.extend_from_slice(&0x1234_u16.to_le_bytes());
    request.extend_from_slice(&[9; 8]);
    request.extend_from_slice(&[7; 4]);
    harness.receive(first, &request);
    harness.end_at(first, Some(INDICATION_AT + 2_000));
    let Some(Request::Transmit(DataPduKind::Control, response)) = harness.step() else {
        panic!("LL_ENC_RSP");
    };
    // SKDs and IVs come from the random source.
    assert_eq!(response[0], 0x04);
    assert_eq!(&response[1..9], &[1, 2, 3, 4, 5, 6, 7, 8]);
    assert_eq!(&response[9..13], &[1, 2, 3, 4]);
    let Some(Request::ConnectionEvent(second)) = harness.step() else {
        panic!("the second event");
    };
    harness.acknowledge();
    // LE Long Term Key Request.
    let events = harness.drain();
    assert_eq!(events.len(), 1);
    assert_eq!(&events[0][..5], &[0x3e, 13, 0x05, 0, 0]);
    assert_eq!(&events[0][5..13], &[1, 2, 3, 4, 5, 6, 7, 8]);
    harness.end_at(second.id, Some(INDICATION_AT + 2_000 + INTERVAL));

    let mut reply = std::vec![0, 0];
    reply.extend_from_slice(&[0x42; 16]);
    assert_eq!(harness.command(LTK_REPLY, &reply), Some(SUCCESS));
    // LL_START_ENC_REQ, still unencrypted.
    assert_eq!(
        harness.step(),
        Some(Request::Transmit(DataPduKind::Control, std::vec![0x05]))
    );
}

#[test]
fn a_connection_update_moves_the_anchor_at_its_instant() {
    let (mut harness, first) = Harness::connected();
    // LL_CONNECTION_UPDATE_IND: WinSize 1, WinOffset 2, Interval 40 (50 ms),
    // Latency 0, Timeout 200, Instant 2.
    let mut update = std::vec![0x03, 12, 0x00, 1];
    update.extend_from_slice(&2_u16.to_le_bytes());
    update.extend_from_slice(&40_u16.to_le_bytes());
    update.extend_from_slice(&0_u16.to_le_bytes());
    update.extend_from_slice(&200_u16.to_le_bytes());
    update.extend_from_slice(&2_u16.to_le_bytes());
    harness.receive(first, &update);
    let captured = INDICATION_AT + 2_000;
    harness.end_at(first, Some(captured));
    // Event 1 keeps the old interval.
    let Some(Request::ConnectionEvent(second)) = harness.step() else {
        panic!("event 1");
    };
    let second_anchor = captured + INTERVAL;
    harness.end_at(second.id, Some(second_anchor));
    // Event 2 is the instant: one old interval plus the new window offset,
    // listening across the new transmit window.
    let Some(Request::ConnectionEvent(instant)) = harness.step() else {
        panic!("the instant");
    };
    let anchor = second_anchor + INTERVAL + 2 * 1_250;
    let widening = 32 * 550 / 1_000 + 63;
    assert_eq!(
        instant.window.start().as_micros(),
        anchor - 10 - widening - 1
    );
    assert_eq!(
        instant.timing,
        ConnectionEventTiming::Recurring {
            receive_wait: oer_bluetooth_radio::RadioDuration::from_micros(
                10 + 2 * widening as u32 + 1_250 + 2
            ),
        }
    );
    harness.end_at(instant.id, Some(anchor + 300));
    // LE Connection Update Complete with the new parameters.
    assert_eq!(
        harness.drain(),
        [std::vec![0x3e, 10, 0x03, 0, 0, 0, 40, 0, 0, 0, 200, 0]]
    );
    let Some(Request::ConnectionEvent(after)) = harness.step() else {
        panic!("the event after the instant");
    };
    assert!(after.window.start().as_micros() > anchor + 300 + 50_000 - 200);
}

#[test]
fn reset_drops_the_connection_without_disconnection_complete() {
    let (mut harness, first) = Harness::connected();
    assert_eq!(harness.command(super::RESET, &[]), None);
    assert_eq!(harness.step(), Some(Request::Cancel(first)));
    harness.end_at(first, None);
    assert_eq!(harness.step(), Some(Request::CloseConnection));
    assert_eq!(harness.status_of(super::RESET), Some(SUCCESS));
    assert!(!harness.core.wants_radio());
}

#[test]
fn host_credits_hold_received_data_and_the_next_event() {
    const FLOW_CONTROL: Opcode = Opcode::new(OpcodeGroup::CONTROL_BASEBAND, 0x0031);
    const HOST_BUFFER_SIZE: Opcode = Opcode::new(OpcodeGroup::CONTROL_BASEBAND, 0x0033);
    const HOST_COMPLETED: Opcode = Opcode::new(OpcodeGroup::CONTROL_BASEBAND, 0x0035);
    let mut harness = Harness::configured();
    assert_eq!(harness.command(FLOW_CONTROL, &[0x01]), Some(SUCCESS));
    // 27-octet packets, one ACL buffer.
    assert_eq!(
        harness.command(HOST_BUFFER_SIZE, &[27, 0, 0, 1, 0, 0, 0]),
        Some(SUCCESS)
    );
    let (mut harness, first) = Harness::connected_from(harness);
    harness.receive(first, &[0x02, 1, 0xaa]);
    harness.receive(first, &[0x02, 1, 0xbb]);
    assert_eq!(harness.drain(), [std::vec![0x00, 0x20, 1, 0, 0xaa]]);
    harness.end_at(first, Some(INDICATION_AT + 2_000));
    // Held data keeps the next event from being planned.
    assert!(!harness.core.wants_radio());
    harness.send(HOST_COMPLETED, &[1, 0, 0, 1, 0]);
    assert_eq!(harness.drain(), [std::vec![0x00, 0x20, 1, 0, 0xbb]]);
    assert!(matches!(harness.step(), Some(Request::ConnectionEvent(_))));
}
