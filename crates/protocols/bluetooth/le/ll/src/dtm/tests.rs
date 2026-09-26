use std::vec::Vec;

use oer_bluetooth_radio::{
    ConnectionAllowances, EventId, EventResult, RadioDuration, RadioInstant, RadioOutcome,
    RadioRequest, RadioTiming, TestChannel, TestPhy, TestReport, TxPower,
};

use super::{DTM_MAX_PAYLOAD, DtmPayloadPattern, DtmSession, DtmStartError, DtmStop, DtmTest};

const TIMING: RadioTiming = RadioTiming {
    preparation_lead: RadioDuration::from_micros(107),
    admission_guard: RadioDuration::from_micros(40),
    connection: ConnectionAllowances {
        local_sleep_clock_ppm: 500,
        widening_jitter: RadioDuration::from_micros(63),
        receive_guard: RadioDuration::from_micros(10),
        receive_tail: RadioDuration::from_micros(2),
        boundary_guard: RadioDuration::from_micros(1),
        first_event_guard: RadioDuration::from_micros(16),
        event_length: RadioDuration::from_micros(5047),
        first_event_length: RadioDuration::from_micros(5155),
    },
};

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn transmit() -> DtmTest {
    DtmTest::Transmit {
        channel: TestChannel::new(0).unwrap(),
        phy: TestPhy::Le1M,
        pattern: DtmPayloadPattern::Prbs9,
        length: 37,
    }
}

fn ended(session: &mut DtmSession, id: EventId, result: EventResult) {
    session.observe(RadioOutcome::EventEnded { id, result });
}

#[test]
fn prbs_patterns_match_the_vendor_table_fingerprints() {
    let mut payload = [0; 255];
    DtmPayloadPattern::Prbs9.fill(&mut payload);
    assert_eq!(
        payload[..16],
        [
            0xff, 0xc1, 0xfb, 0xe8, 0x4c, 0x90, 0x72, 0x8b, 0xe7, 0xb3, 0x51, 0x89, 0x63, 0xab,
            0x23, 0x23
        ]
    );
    assert_eq!(fnv1a64(&payload), 0x94db_648c_b178_dce3);
    DtmPayloadPattern::Prbs15.fill(&mut payload);
    assert_eq!(fnv1a64(&payload), 0x4655_41b8_492c_b9ba);
    for selector in 0..8 {
        let pattern = DtmPayloadPattern::from_hci_selector(selector).unwrap();
        assert_eq!(pattern.payload_type(), selector);
    }
    assert_eq!(DtmPayloadPattern::from_hci_selector(8), None);
}

#[test]
fn transmitter_packets_stay_on_the_interval_grid() {
    let mut session = DtmSession::new(TxPower::from_dbm(0), 10);
    session.start(transmit()).unwrap();
    assert_eq!(session.start(transmit()), Err(DtmStartError::Active));
    let mut payload = [0; DTM_MAX_PAYLOAD];
    let mut anchors = Vec::new();
    let mut now = 1_000;
    for _ in 0..3 {
        let Some(RadioRequest::TestTransmit(test)) =
            session.next_request(RadioInstant::from_micros(now), TIMING, &mut payload)
        else {
            panic!("a transmitter plans its next packet")
        };
        assert_eq!(test.window.duration(), RadioDuration::from_micros(376));
        assert_eq!(test.payload.len(), 37);
        assert_eq!(test.payload[..2], [0xff, 0xc1]);
        anchors.push(test.window.start().as_micros());
        let (id, end) = (test.id, test.window.end().as_micros());
        let mut other = [0; DTM_MAX_PAYLOAD];
        assert!(
            session
                .next_request(RadioInstant::from_micros(now), TIMING, &mut other)
                .is_none()
        );
        ended(&mut session, id, EventResult::Executed { anchor: None });
        // The next plan happens after the packet: late grid points are skipped.
        now = end + 900;
    }
    // 376 us of air make I(L) = 625 us.
    assert_eq!(anchors[0], 1_000 + 107 + 40 + 500);
    assert_eq!((anchors[1] - anchors[0]) % 625, 0);
    assert_eq!((anchors[2] - anchors[1]) % 625, 0);
    assert!(anchors[1] > anchors[0] + 625);
    assert_eq!(session.end(), DtmStop::Stopped { received: 0 });
    assert_eq!(session.counters().executed, 3);
}

#[test]
fn a_receiver_counts_packets_and_drains_before_it_stops() {
    let mut session = DtmSession::new(TxPower::from_dbm(0), 1);
    session
        .start(DtmTest::Receive {
            channel: TestChannel::new(0).unwrap(),
            phy: TestPhy::Le1M,
        })
        .unwrap();
    let mut payload = [0; DTM_MAX_PAYLOAD];
    let mut recurring = Vec::new();
    for report in [
        TestReport::Received { rssi_dbm: -50 },
        TestReport::Nothing,
        TestReport::Received { rssi_dbm: -52 },
    ] {
        let Some(RadioRequest::TestReceive(test)) =
            session.next_request(RadioInstant::from_micros(0), TIMING, &mut payload)
        else {
            panic!("a receiver plans its next window")
        };
        recurring.push(test.recurring);
        session.observe(RadioOutcome::TestReport {
            id: test.id,
            report,
        });
        ended(
            &mut session,
            test.id,
            EventResult::Executed { anchor: None },
        );
    }
    assert_eq!(recurring, [false, true, true]);
    let Some(RadioRequest::TestReceive(test)) =
        session.next_request(RadioInstant::from_micros(0), TIMING, &mut payload)
    else {
        panic!("the receiver keeps listening")
    };
    assert_eq!(session.end(), DtmStop::Draining(test.id));
    assert_eq!(session.drained(), None);
    assert!(
        session
            .next_request(RadioInstant::from_micros(0), TIMING, &mut payload)
            .is_none()
    );
    ended(&mut session, test.id, EventResult::NotExecuted);
    assert_eq!(session.drained(), Some(2));
    assert!(!session.is_active());
    assert_eq!(session.counters().empty, 1);
}

#[test]
fn coded_transmitters_are_refused_and_refusals_replan() {
    let mut session = DtmSession::new(TxPower::from_dbm(0), 1);
    assert_eq!(
        session.start(DtmTest::Transmit {
            channel: TestChannel::new(0).unwrap(),
            phy: TestPhy::LeCodedS8,
            pattern: DtmPayloadPattern::Prbs9,
            length: 37,
        }),
        Err(DtmStartError::UnsupportedPhy)
    );
    session.start(transmit()).unwrap();
    let mut payload = [0; DTM_MAX_PAYLOAD];
    assert!(
        session
            .next_request(RadioInstant::from_micros(0), TIMING, &mut payload)
            .is_some()
    );
    session.refused();
    assert!(
        session
            .next_request(RadioInstant::from_micros(0), TIMING, &mut payload)
            .is_some()
    );
}
