use super::*;
use crate::TxOutcome::*;

fn time(us: u64) -> Instant {
    Instant::from_micros(us)
}
fn ap() -> LinkIdentity {
    LinkIdentity {
        peer: [2, 0, 0, 0, 0, 1],
        generation: 4,
    }
}
fn sta() -> LinkIdentity {
    LinkIdentity {
        peer: [2, 0, 0, 0, 0, 2],
        generation: 4,
    }
}
const REPORT: &[u8] = &[
    52, 16, 2, 0, 0, 0, 0, 9, 0, 0, 0, 0, 81, 6, 7, 3, 1, 200, 221, 1, 42,
];

#[test]
fn full_request_response_cache_and_early_rx_before_tx_completion() {
    let mut requester = NeighborReportRequester::<128>::new(
        ap(),
        Duration::from_micros(100),
        Duration::from_micros(200),
    )
    .unwrap();
    let mut responder =
        NeighborReportResponder::<128>::new(sta(), Duration::from_micros(100)).unwrap();
    let request = NeighborReportRequest {
        dialog_token: 7,
        elements: Elements::parse(&[0, 3, b'a', 0, b'b', 38, 3, 9, 0, 8]).unwrap(),
    };
    let tx = requester.request(time(0), request).unwrap();
    let body = requester.transmission().unwrap().body.to_vec();
    requester.admitted(tx, time(1)).unwrap();
    let incoming = responder.receive(sta(), &body).unwrap().unwrap();
    assert_eq!(incoming.ssid().unwrap(), Some(&b"a\0b"[..]));
    assert_eq!(incoming.elements.unique(38).unwrap(), Some(&[9, 0, 8][..]));
    let reply = responder
        .respond(time(2), incoming, Elements::parse(REPORT).unwrap())
        .unwrap();
    let body = responder.transmission().unwrap().body.to_vec();
    responder.admitted(reply, time(3)).unwrap();
    assert_eq!(
        requester.receive(ap(), &body, time(4)).unwrap(),
        NeighborEvent::ReportsReceived
    );
    assert_eq!(
        requester.tx_completed(tx, Failed, time(5)).unwrap(),
        NeighborEvent::Ignored
    );
    assert_eq!(
        responder
            .tx_completed(reply, Acknowledged, time(5))
            .unwrap(),
        DialogEvent::Transmitted
    );
    let cache = requester.reports(time(203)).unwrap();
    assert_eq!(
        cache.reports().next().unwrap().unwrap().bssid,
        [2, 0, 0, 0, 0, 9]
    );
    assert_eq!(cache.elements.unique(221).unwrap(), Some(&[42][..]));
    assert!(requester.reports(time(204)).is_none());
    assert_eq!(
        requester.poll(time(204)).unwrap(),
        NeighborEvent::CacheExpired
    );
}

#[test]
fn unadmitted_foreign_wrong_token_and_malformed_responses_do_not_complete() {
    let mut owner = NeighborReportRequester::<64>::new(
        ap(),
        Duration::from_micros(100),
        Duration::from_micros(200),
    )
    .unwrap();
    let id = owner
        .request(
            time(0),
            NeighborReportRequest {
                dialog_token: 2,
                elements: Elements::EMPTY,
            },
        )
        .unwrap();
    assert_eq!(
        owner.receive(ap(), &[5, 5, 2], time(1)).unwrap(),
        NeighborEvent::Ignored
    );
    assert_eq!(
        owner.tx_completed(id, Acknowledged, time(1)),
        Err(Error::NotAdmitted)
    );
    owner.admitted(id, time(1)).unwrap();
    let stale = LinkIdentity {
        generation: 3,
        ..ap()
    };
    assert_eq!(
        owner.receive(stale, &[5, 5, 2], time(2)).unwrap(),
        NeighborEvent::Ignored
    );
    assert_eq!(
        owner.receive(sta(), &[5, 5, 2], time(2)).unwrap(),
        NeighborEvent::Ignored
    );
    assert_eq!(
        owner.receive(ap(), &[5, 5, 3], time(2)).unwrap(),
        NeighborEvent::Ignored
    );
    assert!(owner.receive(ap(), &[5, 5, 2, 52, 13, 0], time(2)).is_err());
    assert_eq!(owner.next_deadline(), Some(time(100)));
    assert_eq!(owner.poll(time(100)).unwrap(), NeighborEvent::TimedOut);
}

#[test]
fn oversized_response_and_stale_completion_cannot_replace_new_dialog() {
    let mut owner = NeighborReportRequester::<16>::new(
        ap(),
        Duration::from_micros(100),
        Duration::from_micros(200),
    )
    .unwrap();
    let first = owner
        .request(
            time(0),
            NeighborReportRequest {
                dialog_token: 1,
                elements: Elements::EMPTY,
            },
        )
        .unwrap();
    owner.admitted(first, time(0)).unwrap();
    assert_eq!(owner.cancel(), NeighborEvent::Cancelled);
    let second = owner
        .request(
            time(1),
            NeighborReportRequest {
                dialog_token: 2,
                elements: Elements::EMPTY,
            },
        )
        .unwrap();
    owner.admitted(second, time(1)).unwrap();
    assert_eq!(
        owner.tx_completed(first, Failed, time(2)).unwrap(),
        NeighborEvent::Ignored
    );
    let body: std::vec::Vec<_> = [5, 5, 2]
        .into_iter()
        .chain(REPORT.iter().copied())
        .collect();
    assert!(matches!(
        owner.receive(ap(), &body, time(3)),
        Err(Error::FrameTooLarge { .. })
    ));
    assert_eq!(owner.next_deadline(), Some(time(101)));
    assert_eq!(
        owner.receive(ap(), &[5, 5, 2], time(4)).unwrap(),
        NeighborEvent::ReportsReceived
    );
    assert_eq!(
        owner.receive(ap(), &[5, 5, 2], time(5)).unwrap(),
        NeighborEvent::Ignored
    );
}

#[test]
fn deadlines_and_capacity_fail_without_admitting_work() {
    let mut owner = NeighborReportRequester::<3>::new(
        ap(),
        Duration::from_micros(10),
        Duration::from_micros(20),
    )
    .unwrap();
    assert!(
        owner
            .request(
                time(u64::MAX),
                NeighborReportRequest {
                    dialog_token: 1,
                    elements: Elements::EMPTY
                }
            )
            .is_err()
    );
    assert!(owner.transmission().is_none());
    assert!(
        owner
            .request(
                time(0),
                NeighborReportRequest {
                    dialog_token: 1,
                    elements: Elements::parse(&[0, 1, 1]).unwrap()
                }
            )
            .is_err()
    );
    assert!(owner.transmission().is_none());
    let id = owner
        .request(
            time(0),
            NeighborReportRequest {
                dialog_token: 1,
                elements: Elements::EMPTY,
            },
        )
        .unwrap();
    assert_eq!(owner.admitted(id, time(10)), Err(Error::NoPendingOperation));
    assert_eq!(owner.poll(time(10)).unwrap(), NeighborEvent::TimedOut);
}
