use super::*;

fn round_trip<E: Event + Copy + PartialEq + core::fmt::Debug>(event: E) {
    assert_eq!(E::decode(event.encode()), Some(event));
}

#[test]
fn every_station_event_decodes_to_what_it_encoded() {
    for verdict in [
        BeaconVerdict::Published,
        BeaconVerdict::Unextracted,
        BeaconVerdict::Rejected,
        BeaconVerdict::NoMetadata,
    ] {
        for rssi_dbm in [None, Some(-90), Some(0), Some(127), Some(-128)] {
            round_trip(BeaconDispatch {
                verdict,
                rssi_dbm,
                timestamp_tsf_low: 0xdead_beef,
            });
        }
    }
    round_trip(ControlMailbox {
        event: ControlEventKind::PowerSaveData,
        op: MailboxOp::Overflowed,
    });
    round_trip(PowerInputTrace {
        input: PowerInputKind::Tbtt,
        control_event_waiting: true,
    });
    round_trip(NetworkTxPowerTrace {
        offer: true,
        control_event_waiting: false,
    });
    round_trip(BeaconMonitorTrace {
        op: BeaconMonitorOp::Lost,
        deadline_micros_low: 6_000_000,
    });
    for reason in [
        ConnectedDisconnectReason::BeaconLoss,
        ConnectedDisconnectReason::PeerDeauthentication { reason_code: 7 },
        ConnectedDisconnectReason::PeerDisassociation { reason_code: 0 },
        ConnectedDisconnectReason::ControlMailboxOverflow,
        ConnectedDisconnectReason::ActiveStateRestoreFailed,
        ConnectedDisconnectReason::GroupKeyHandshakeFailed,
        ConnectedDisconnectReason::SaQueryTimeout,
    ] {
        round_trip(ControlExit { reason });
    }
}

#[test]
fn words_no_station_event_encodes_fail_to_decode() {
    assert_eq!(BeaconDispatch::decode([4, 0]), None);
    assert_eq!(BeaconDispatch::decode([1 << 17, 0]), None);
    assert_eq!(ControlMailbox::decode([10, 0]), None);
    assert_eq!(ControlMailbox::decode([0, 3]), None);
    assert_eq!(PowerInputTrace::decode([0, 2]), None);
    assert_eq!(NetworkTxPowerTrace::decode([2, 0]), None);
    assert_eq!(BeaconMonitorTrace::decode([4, 0]), None);
    assert_eq!(ControlExit::decode([0, 1]), None);
    assert_eq!(ControlExit::decode([1, 0x1_0000]), None);
    assert_eq!(ControlExit::decode([7, 0]), None);
}

#[test]
fn station_events_have_distinct_kinds_and_channels() {
    let kinds = [
        BeaconDispatch::KIND,
        ControlMailbox::KIND,
        PowerInputTrace::KIND,
        NetworkTxPowerTrace::KIND,
        BeaconMonitorTrace::KIND,
        ControlExit::KIND,
    ];
    let channels = [
        BeaconDispatch::CHANNEL,
        ControlMailbox::CHANNEL,
        PowerInputTrace::CHANNEL,
        NetworkTxPowerTrace::CHANNEL,
        BeaconMonitorTrace::CHANNEL,
        ControlExit::CHANNEL,
    ];
    for (index, kind) in kinds.iter().enumerate() {
        assert_eq!(kind.domain, Domain::Ieee80211);
        assert!(!kinds[..index].contains(kind));
        assert!(!channels[..index].contains(&channels[index]));
    }
}
