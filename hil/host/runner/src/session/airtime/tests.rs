use super::*;
use open_esp_radio_hil_protocol::{WifiAccessPointEvidence, WifiAirtimePeer, WifiAirtimeReport};

fn peer() -> WifiAirtimePeerEvidence {
    WifiAirtimePeerEvidence {
        peer: WifiAirtimePeer::Group,
        grants: 3,
        granted_micros: 3000,
        settlements: 2,
        settled_grants_micros: 2000,
        charged_micros: 2500,
        cancellations: 1,
        cancelled_grants_micros: 1000,
        balance_after_last_event_micros: -500,
        outstanding: 0,
        outstanding_micros: 0,
        maximum_outstanding: 2,
    }
}
fn report() -> Event {
    Event::WifiAirtimeReport(WifiAirtimeReport {
        peer_records: 1,
        ..Default::default()
    })
}
fn stopped() -> Event {
    Event::WifiAccessPointStopped(WifiAccessPointEvidence::default())
}

#[test]
fn complete_accounting_accepts_debt_but_requires_every_reservation_closed() {
    assert!(validate([Event::WifiAirtimePeer(peer()), report(), stopped()].iter()).is_ok());
    let pending = WifiAirtimePeerEvidence {
        settlements: 1,
        settled_grants_micros: 1000,
        outstanding: 1,
        outstanding_micros: 1000,
        ..peer()
    };
    assert!(validate([Event::WifiAirtimePeer(pending), report(), stopped()].iter()).is_err());
}

#[test]
fn missing_duplicate_late_and_inconsistent_evidence_is_rejected() {
    assert!(validate([stopped()].iter()).is_err());
    assert!(validate([Event::WifiAirtimePeer(peer()), stopped(), report()].iter()).is_err());
    assert!(
        validate(
            [
                Event::WifiAirtimePeer(peer()),
                Event::WifiAirtimePeer(peer()),
                report(),
                stopped()
            ]
            .iter()
        )
        .is_err()
    );
    let wrong = WifiAirtimePeerEvidence {
        granted_micros: 3001,
        ..peer()
    };
    assert!(validate([Event::WifiAirtimePeer(wrong), report(), stopped()].iter()).is_err());
    for incomplete in [
        WifiAirtimeReport {
            peer_records: 2,
            ..Default::default()
        },
        WifiAirtimeReport {
            peer_records: 1,
            saturated: true,
            ..Default::default()
        },
        WifiAirtimeReport {
            peer_records: 1,
            dropped_events: 1,
            ..Default::default()
        },
    ] {
        assert!(
            validate(
                [
                    Event::WifiAirtimePeer(peer()),
                    Event::WifiAirtimeReport(incomplete),
                    stopped()
                ]
                .iter()
            )
            .is_err()
        );
    }
}
