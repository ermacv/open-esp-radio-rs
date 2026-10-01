use super::*;
use crate::tclas::ClassifierPacket;
use crate::{Error, LinkIdentity, OperationId, TxOutcome};
use oer_ieee80211_mac::roaming::*;
use oer_time::{Duration, Instant};
fn link() -> LinkIdentity {
    LinkIdentity {
        peer: [2, 0, 0, 0, 0, 1],
        generation: 1,
    }
}
fn request_bytes(token: u8) -> std::vec::Vec<u8> {
    let mut storage = [0; 32];
    let n = TclasRule {
        user_priority: 0,
        mask: 4,
        parameters: TclasParameters::Ethernet {
            source: [0; 6],
            destination: [0; 6],
            ether_type: 0x0800,
        },
    }
    .encode(&mut storage)
    .unwrap();
    let mut bytes = std::vec![10, 13, token, 91, (4 + n) as u8, 7, 3, 1, n as u8];
    bytes.extend_from_slice(&storage[..n]);
    bytes
}
fn response(token: u8) -> [u8; 9] {
    [10, 14, token, 92, 4, 1, 2, 0, 7]
}
fn input() -> TrafficInput<'static> {
    TrafficInput {
        fields: ClassifierPacket {
            source: [2, 0, 0, 0, 0, 2],
            destination: link().peer,
            ether_type: 0x0800,
            ip: None,
        },
        payload: &[],
        vlan_tci: None,
        eapol_key: false,
    }
}
#[test]
fn multiple_tfs_classifiers_without_processing_default_to_all() {
    // Two Ethernet TCLAS: IPv4 EtherType and the peer destination, without
    // Processing. Unlike a DMS Add, this is a complete valid TFS request.
    let mut request = [
        std::vec![10, 13, 1, 91, 42, 7, 0, 1, 38],
        std::vec![14, 17, 0, 0, 4, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 8, 0],
        std::vec![14, 17, 0, 0, 2, 0, 0, 0, 0, 0, 0, 2, 0, 0, 0, 0, 1, 0, 0],
    ]
    .concat();
    let accepted = [92, 4, 1, 2, 0, 7];
    let mut filters = TrafficFilters::<128, 4>::new(link());
    for any in [false, true] {
        if any {
            request.extend_from_slice(&[44, 1, 1]);
            request[4] += 3;
            request[8] += 3;
        }
        let parsed = TfsRequestFrame::parse(&request).unwrap();
        let mut encoded = [0; 128];
        let n = parsed.encode(&mut encoded).unwrap();
        assert_eq!(&encoded[..n], request);
        filters
            .install(link(), parsed.elements, Elements::parse(&accepted).unwrap())
            .unwrap();
        for (ether_type, destination, matches_all, matches_any) in [
            (0x0800, link().peer, true, true),
            (0x0800, [2, 0, 0, 0, 0, 2], false, true),
            (0x86dd, link().peer, false, true),
            (0x86dd, [2, 0, 0, 0, 0, 2], false, false),
        ] {
            let mut packet = input();
            packet.fields.ether_type = ether_type;
            packet.fields.destination = destination;
            let decision = filters.evaluate(link(), packet).unwrap();
            assert_eq!(
                decision.discard_individual,
                !(if any { matches_any } else { matches_all })
            );
            filters.cancel(decision.id).unwrap();
        }
    }
}
fn receive(ap: &mut TfsAccessPoint<128, 4>, bytes: &[u8], now: Instant) -> OperationId {
    let TfsApEvent::Requested { id, .. } = ap.receive(link(), bytes, now).unwrap() else {
        panic!("new request")
    };
    id
}
#[test]
fn standalone_cancel_and_rejected_replacements_are_distinct_from_sleep_admission() {
    let mut filters = TrafficFilters::<128, 4>::new(link());
    let request = request_bytes(0);
    let denial = Elements::parse(&[92, 4, 1, 2, 1, 7]).unwrap();
    filters
        .install(
            link(),
            TfsRequestFrame::parse(&request).unwrap().elements,
            denial,
        )
        .unwrap();
    assert_eq!(
        validate_sleep_filters(TfsRequestFrame::parse(&request).unwrap().elements, denial),
        Err(TrafficError::NoAcceptedFilters)
    );
    let d = filters.evaluate(link(), input()).unwrap();
    assert!(!d.discard_individual);
    filters.cancel(d.id).unwrap();
    filters
        .install(link(), Elements::EMPTY, Elements::EMPTY)
        .unwrap();
    assert!(filters.negotiation().0.as_bytes().is_empty());
}
#[test]
fn replay_does_not_reinstall_a_deleted_filter_or_extend_the_request_lease() {
    let mut ap = TfsAccessPoint::<128, 4>::new(link(), Duration::from_secs(1)).unwrap();
    let now = Instant::EPOCH;
    let bytes = request_bytes(0);
    let id = receive(&mut ap, &bytes, now);
    let tx = ap
        .respond(id, TfsResponseFrame::parse(&response(0)).unwrap(), now)
        .unwrap();
    ap.admitted(tx, now).unwrap();
    ap.tx_completed(tx, TxOutcome::Acknowledged, now).unwrap();
    let d = ap.evaluate(link(), input()).unwrap();
    ap.traffic_applied(d.id, true, true).unwrap();
    let later = now.checked_add(Duration::from_millis(900)).unwrap();
    assert!(matches!(
        ap.receive(link(), &bytes, later).unwrap(),
        TfsApEvent::ReplayQueued { .. }
    ));
    assert_eq!(
        ap.next_deadline(),
        Some(now.checked_add(Duration::from_secs(1)).unwrap())
    );
    let d = ap.evaluate(link(), input()).unwrap();
    assert!(d.discard_individual);
    ap.cancel_traffic(d.id).unwrap();
}
#[test]
fn response_preflight_and_latest_request_preserve_filter_ownership() {
    let mut ap = TfsAccessPoint::<128, 4>::new(link(), Duration::from_secs(1)).unwrap();
    let now = Instant::EPOCH;
    let old = receive(&mut ap, &request_bytes(1), now);
    let missing = TfsResponseFrame {
        dialog_token: 1,
        elements: Elements::EMPTY,
    };
    assert!(ap.respond(old, missing, now).is_err());
    assert!(ap.transmission().is_none());
    assert!(ap.negotiation().0.as_bytes().is_empty());
    let latest = receive(&mut ap, &request_bytes(2), now);
    assert_ne!(old, latest);
    assert_eq!(
        ap.respond(old, TfsResponseFrame::parse(&response(2)).unwrap(), now),
        Err(TfsError::Protocol(Error::WrongOperation))
    );
    let mut conflict = request_bytes(2);
    conflict[6] = 8;
    assert_eq!(
        ap.receive(link(), &conflict, now),
        Err(TfsError::Protocol(Error::ConflictingDialog))
    );
}
#[test]
fn station_handles_response_before_tx_callback_and_lost_admitted_replacements() {
    let mut sta = TfsStation::<128>::new(link(), Duration::from_secs(1)).unwrap();
    let now = Instant::EPOCH;
    let queued = sta
        .request(TfsRequestFrame::parse(&request_bytes(1)).unwrap(), now)
        .unwrap();
    sta.cancel();
    assert_eq!(sta.state(), TfsState::Unconfigured);
    assert_eq!(
        sta.tx_completed(queued, TxOutcome::Failed, now).unwrap(),
        TfsStationEvent::Ignored
    );
    let id = sta
        .request(TfsRequestFrame::parse(&request_bytes(2)).unwrap(), now)
        .unwrap();
    sta.admitted(id, now).unwrap();
    assert_eq!(
        sta.receive(link(), &response(2), now).unwrap(),
        TfsStationEvent::ResponseReceived { id }
    );
    assert_eq!(sta.state(), TfsState::Configured);
    assert_eq!(
        sta.tx_completed(id, TxOutcome::Failed, now).unwrap(),
        TfsStationEvent::Ignored
    );
    let id = sta
        .request(
            TfsRequestFrame {
                dialog_token: 3,
                elements: Elements::EMPTY,
            },
            now,
        )
        .unwrap();
    sta.admitted(id, now).unwrap();
    assert_eq!(
        sta.poll(now.checked_add(Duration::from_secs(1)).unwrap())
            .unwrap(),
        TfsStationEvent::RecoveryRequired { id }
    );
    assert_eq!(sta.state(), TfsState::Uncertain);
}
#[test]
fn station_correlates_complete_statuses_and_notifications_atomically() {
    let mut sta = TfsStation::<128>::new(link(), Duration::from_secs(1)).unwrap();
    let now = Instant::EPOCH;
    let id = sta
        .request(TfsRequestFrame::parse(&request_bytes(0)).unwrap(), now)
        .unwrap();
    sta.admitted(id, now).unwrap();
    assert!(
        sta.receive(link(), &[10, 14, 0, 92, 4, 1, 2, 0, 8], now)
            .is_err()
    );
    assert_eq!(
        sta.next_deadline(),
        Some(now.checked_add(Duration::from_secs(1)).unwrap())
    );
    sta.receive(link(), &response(0), now).unwrap();
    assert_eq!(
        sta.receive_notify(link(), &[10, 15, 2, 7, 8], now),
        Err(TfsError::UnknownNotification(8))
    );
    assert_eq!(
        sta.receive_notify(link(), &[10, 15, 1, 7], now)
            .unwrap()
            .unwrap()
            .ids(),
        [7]
    );
    assert_eq!(
        sta.receive_notify(
            LinkIdentity {
                generation: 0,
                ..link()
            },
            &[],
            now
        )
        .unwrap(),
        None
    );
    assert!(sta.receive_notify(link(), &[10, 15, 1, 7], now).is_err());
}
