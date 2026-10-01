use super::*;
use crate::reports::{ReportConnectivity, ReportRoute};
use crate::{Error, LinkIdentity, OperationId, TxOutcome};
use oer_time::{Duration, Instant};
use std::vec::Vec;
fn link() -> LinkIdentity {
    LinkIdentity {
        peer: [2, 0, 0, 0, 0, 1],
        generation: 1,
    }
}
fn at(us: u64) -> Instant {
    Instant::from_micros(us)
}
fn transition(n: u8) -> EventReport<'static> {
    EventReport {
        token: 0,
        kind: EventType::TRANSITION,
        status: EventReportStatus::SUCCESSFUL,
        timing: Some(EventTiming {
            tsf: u64::from(n),
            utc_offset: [0; 10],
            time_error: [0; 5],
        }),
        data: EventData::Transition(TransitionEvent {
            source: link().peer,
            target: [2, 0, 0, 0, 0, 2],
            time_tu: u16::from(n),
            reason: 6,
            result: 0,
            source_rcpi: 90,
            source_rsni: 50,
            target_rcpi: 100,
            target_rsni: 60,
        }),
    }
}
fn request(dialog: u8, kind: EventType, limit: u8, conditions: &[u8]) -> Vec<u8> {
    let mut element = [0; 257];
    let n = EventRequest {
        token: 1,
        kind,
        response_limit: limit,
        conditions: Elements::parse(conditions).unwrap(),
    }
    .encode(&mut element)
    .unwrap();
    let mut result = std::vec![10, 0, dialog];
    result.extend_from_slice(&element[..n]);
    result
}
fn response(
    dialog: u8,
    mut r: EventReport<'_>,
    format: Option<FrequentTransitionFormat>,
) -> Vec<u8> {
    r.token = if dialog == 0 { 0 } else { 1 };
    let mut element = [0; 257];
    let n = if let Some(f) = format {
        r.encode_frequent(f, &mut element)
    } else {
        r.encode(&mut element)
    }
    .unwrap();
    let mut result = std::vec![10, 1, dialog];
    result.extend_from_slice(&element[..n]);
    result
}
fn received<const B: usize, const F: usize>(
    sta: &mut EventStation<B, F>,
    bytes: &[u8],
    now: Instant,
) -> OperationId {
    let EventStationEvent::Requested { id, .. } = sta.receive(link(), bytes, now).unwrap() else {
        panic!("new request")
    };
    id
}
fn reachable() -> ReportConnectivity {
    ReportConnectivity {
        requester_reachable: true,
        beacon_absent_since: Instant::EPOCH,
    }
}
fn ack<const B: usize, const F: usize>(sta: &mut EventStation<B, F>, now: Instant) -> Vec<u8> {
    let tx = sta.send_next(now).unwrap().unwrap();
    let bytes = sta
        .delivery(reachable(), now)
        .unwrap()
        .unwrap()
        .body
        .to_vec();
    sta.admitted(tx, now).unwrap();
    assert_eq!(
        sta.tx_completed(tx, TxOutcome::Acknowledged, now).unwrap(),
        EventStationEvent::Transmitted { id: tx }
    );
    bytes
}
#[test]
fn journal_retains_five_of_each_type_and_network_changes_are_explicit() {
    assert!(matches!(
        EventJournal::<4, 257>::new(NetworkIdentity(1)),
        Err(ReportError::InvalidJournalCapacity)
    ));
    let mut log = EventJournal::<5, 257>::new(NetworkIdentity(1)).unwrap();
    let rsna = EventReport {
        kind: EventType::RSNA,
        data: EventData::Rsna(RsnaEvent {
            target: link().peer,
            authentication: [0, 15, 172, 1],
            eap: EapMethod::Legacy(13),
            result: 0,
            rsne: &[48, 2, 1],
        }),
        ..transition(0)
    };
    for n in 0..8 {
        log.record(log.network(), transition(n), at(u64::from(n)))
            .unwrap();
        log.record(log.network(), rsna, at(u64::from(n))).unwrap();
    }
    assert_eq!(
        log.records(EventType::TRANSITION)
            .map(|(_, _, r)| r.timing.unwrap().tsf)
            .collect::<Vec<_>>(),
        [7, 6, 5, 4, 3]
    );
    assert_eq!(log.records(EventType::RSNA).count(), 5);
    let last = log.records(EventType::TRANSITION).next().unwrap().0;
    log.change_network(NetworkIdentity(1), at(10)).unwrap();
    assert!(log.get(last).is_some());
    log.change_network(NetworkIdentity(2), at(11)).unwrap();
    assert!(log.get(last).is_none());
    assert_eq!(log.records(EventType::RSNA).count(), 0);
    assert!(
        log.record(NetworkIdentity(1), transition(9), at(12))
            .is_err()
    );
    let next = log
        .record(NetworkIdentity(2), transition(9), at(12))
        .unwrap();
    assert!(next.sequence() > last.sequence());
    let mut small = EventJournal::<5, 8>::new(NetworkIdentity(1)).unwrap();
    assert!(
        small
            .record(NetworkIdentity(1), transition(1), at(1))
            .is_err()
    );
    assert_eq!(small.records(EventType::TRANSITION).count(), 0);
}
#[test]
fn peer_termination_removes_initiation_and_active_connection_time_is_computed() {
    let network = NetworkIdentity(1);
    let mut log = EventJournal::<5, 257>::new(network).unwrap();
    let peer = PeerLinkEvent {
        peer: [2, 0, 0, 0, 0, 3],
        operating_class: 81,
        channel: 6,
        tx_power_dbm: -3,
        connection_seconds: 0,
        status: PeerLinkStatus::DIRECT_ACTIVE,
    };
    let event = EventReport {
        kind: EventType::PEER_LINK,
        data: EventData::PeerLink(peer),
        ..transition(0)
    };
    let initial = log.record(network, event, at(1_000_000)).unwrap();
    let mut sta = EventStation::<128, 2>::new(
        link(),
        network,
        Duration::from_secs(1),
        FrequentTransitionFormat::StatusOnly,
    )
    .unwrap();
    let id = received(
        &mut sta,
        &request(1, EventType::PEER_LINK, 0, &[1, 2, 81, 0]),
        at(3_000_000),
    );
    sta.prepare_reports(id, &log, at(3_000_000)).unwrap();
    let bytes = ack(&mut sta, at(3_000_000));
    let frame = EventReportFrame::parse(&bytes).unwrap();
    let report = frame.reports().unwrap().next().unwrap();
    let EventData::PeerLink(active) = report.data else {
        panic!("peer report")
    };
    assert_eq!(active.connection_seconds, 2);
    log.record(
        network,
        EventReport {
            data: EventData::PeerLink(PeerLinkEvent {
                status: PeerLinkStatus::DIRECT_TERMINATED,
                connection_seconds: 4,
                ..peer
            }),
            ..event
        },
        at(5_000_000),
    )
    .unwrap();
    assert!(log.get(initial).is_none());
    assert_eq!(log.records(EventType::PEER_LINK).count(), 1);
}
#[test]
fn reports_apply_conditions_and_limits_and_never_split_an_element() {
    let network = NetworkIdentity(1);
    let mut log = EventJournal::<5, 257>::new(network).unwrap();
    for n in 1..=5 {
        log.record(network, transition(n), at(u64::from(n)))
            .unwrap();
    }
    let mut sta = EventStation::<64, 5>::new(
        link(),
        network,
        Duration::from_secs(1),
        FrequentTransitionFormat::StatusOnly,
    )
    .unwrap();
    let id = received(&mut sta, &request(1, EventType::TRANSITION, 0, &[]), at(10));
    sta.prepare_reports(id, &log, at(10)).unwrap();
    let mut values = Vec::new();
    while sta.send_next(at(10)).unwrap().is_some() {
        let delivery = sta.delivery(reachable(), at(10)).unwrap().unwrap();
        let id = delivery.id;
        let frame = EventReportFrame::parse(delivery.body).unwrap();
        let reports = frame.reports().unwrap().collect::<Vec<_>>();
        assert_eq!(reports.len(), 1);
        values.push(reports[0].timing.unwrap().tsf);
        sta.admitted(id, at(10)).unwrap();
        sta.tx_completed(id, TxOutcome::Acknowledged, at(10))
            .unwrap();
    }
    assert_eq!(values, [5, 4, 3, 2, 1]);
    let id = received(
        &mut sta,
        &request(2, EventType::TRANSITION, 2, &[2, 2, 4, 0, 200, 1, 9]),
        at(11),
    );
    sta.prepare_reports(id, &log, at(11)).unwrap();
    assert_eq!(
        EventReportFrame::parse(&ack(&mut sta, at(11)))
            .unwrap()
            .reports()
            .unwrap()
            .next()
            .unwrap()
            .timing
            .unwrap()
            .tsf,
        5
    );
    assert_eq!(
        EventReportFrame::parse(&ack(&mut sta, at(11)))
            .unwrap()
            .reports()
            .unwrap()
            .next()
            .unwrap()
            .timing
            .unwrap()
            .tsf,
        4
    );
    assert_eq!(sta.send_next(at(11)).unwrap(), None);
}
#[test]
fn full_snapshot_preflight_and_supersession_do_not_publish_partial_results() {
    let network = NetworkIdentity(1);
    let mut log = EventJournal::<5, 257>::new(network).unwrap();
    for n in 1..=2 {
        log.record(network, transition(n), at(u64::from(n)))
            .unwrap();
    }
    let mut sta = EventStation::<64, 1>::new(
        link(),
        network,
        Duration::from_secs(1),
        FrequentTransitionFormat::StatusOnly,
    )
    .unwrap();
    let old = received(&mut sta, &request(1, EventType::TRANSITION, 0, &[]), at(10));
    assert_eq!(
        sta.prepare_reports(old, &log, at(10)),
        Err(ReportError::Full)
    );
    assert_eq!(sta.send_next(at(10)).unwrap(), None);
    let id = received(&mut sta, &request(2, EventType::TRANSITION, 1, &[]), at(11));
    sta.prepare_reports(id, &log, at(11)).unwrap();
    let tx = sta.send_next(at(11)).unwrap().unwrap();
    sta.admitted(tx, at(11)).unwrap();
    let next = received(&mut sta, &request(3, EventType::TRANSITION, 1, &[]), at(12));
    assert_eq!(
        sta.tx_completed(tx, TxOutcome::Failed, at(0)).unwrap(),
        EventStationEvent::Ignored
    );
    assert_ne!(next, id);
    assert_eq!(
        sta.prepare_reports(old, &log, at(12)),
        Err(ReportError::Protocol(Error::WrongOperation))
    );
    let mut changed = request(3, EventType::TRANSITION, 2, &[]);
    assert!(sta.receive(link(), &changed, at(12)).is_err());
    changed[2] = 4;
    assert_eq!(
        sta.receive(
            LinkIdentity {
                generation: 0,
                ..link()
            },
            &[],
            at(0)
        )
        .unwrap(),
        EventStationEvent::Ignored
    );
}
#[test]
fn frequent_transition_uses_explicit_format_and_deduplicates_event_identity() {
    for format in [
        FrequentTransitionFormat::StatusOnly,
        FrequentTransitionFormat::WithLastEvent,
    ] {
        let network = NetworkIdentity(1);
        let mut log = EventJournal::<5, 257>::new(network).unwrap();
        let mut sta =
            EventStation::<128, 2>::new(link(), network, Duration::from_secs(1), format).unwrap();
        let bytes = request(1, EventType::TRANSITION, 0, &[4, 3, 2, 0xe8, 3]);
        let id = received(&mut sta, &bytes, at(0));
        sta.prepare_reports(id, &log, at(0)).unwrap();
        ack(&mut sta, at(0));
        let first = log.record(network, transition(1), at(1000)).unwrap();
        assert_eq!(sta.transition_logged(&log, first, at(1000)).unwrap(), 0);
        let second = log.record(network, transition(2), at(2000)).unwrap();
        assert_eq!(sta.transition_logged(&log, second, at(2000)).unwrap(), 1);
        assert_eq!(sta.transition_logged(&log, second, at(2000)).unwrap(), 0);
        let bytes = ack(&mut sta, at(2000));
        let frame = EventReportFrame::parse(&bytes).unwrap();
        let r = frame.reports().unwrap().next().unwrap();
        assert_eq!(r.status, EventReportStatus::FREQUENT_TRANSITION);
        assert_eq!(
            r.timing.is_some(),
            format == FrequentTransitionFormat::WithLastEvent
        );
        let third = log.record(network, transition(3), at(3000)).unwrap();
        assert_eq!(sta.transition_logged(&log, third, at(3000)).unwrap(), 1);
        ack(&mut sta, at(3000));
        assert_eq!(sta.next_deadline(), None);
    }
}
#[test]
fn requester_retains_alert_subscription_and_separate_autonomous_history() {
    let mut ap = EventRequester::<128, 1>::new(link(), Duration::from_secs(1)).unwrap();
    let bytes = request(1, EventType::TRANSITION, 0, &[4, 3, 2, 0xe8, 3]);
    let id = ap
        .request(EventRequestFrame::parse(&bytes).unwrap(), at(0))
        .unwrap()
        .id;
    ap.admitted(id, at(0)).unwrap();
    let empty = EventReport {
        token: 1,
        kind: EventType::TRANSITION,
        status: EventReportStatus::SUCCESSFUL,
        timing: None,
        data: EventData::Empty,
    };
    let initial = response(1, empty, None);
    assert_eq!(
        ap.receive(link(), &initial, at(0)).unwrap(),
        ReportWindowEvent::ReportReceived { id }
    );
    assert_eq!(
        ap.tx_completed(id, TxOutcome::Failed, at(0)).unwrap(),
        ReportWindowEvent::Ignored
    );
    assert_eq!(
        ap.poll(at(1_000_000)).unwrap(),
        ReportWindowEvent::WindowClosed { id, frames: 1 }
    );
    let frequent = response(
        1,
        EventReport {
            status: EventReportStatus::FREQUENT_TRANSITION,
            ..empty
        },
        Some(FrequentTransitionFormat::StatusOnly),
    );
    assert_eq!(
        ap.receive(link(), &frequent, at(2_000_000)).unwrap(),
        ReportWindowEvent::MonitoringReportReceived { id }
    );
    assert_eq!(
        ap.receive(link(), &frequent, at(2_000_000)).unwrap(),
        ReportWindowEvent::Ignored
    );
    let with_event = response(
        1,
        EventReport {
            status: EventReportStatus::FREQUENT_TRANSITION,
            ..transition(2)
        },
        Some(FrequentTransitionFormat::WithLastEvent),
    );
    assert_eq!(
        ap.receive(link(), &with_event, at(2_000_000)),
        Err(ReportError::Full)
    );
    ap.clear_monitoring_reports();
    ap.receive(link(), &with_event, at(2_000_000)).unwrap();
    assert_eq!(ap.monitoring_reports().count(), 1);
    assert_eq!(
        ap.tx_completed(id, TxOutcome::Failed, at(0)).unwrap(),
        ReportWindowEvent::Ignored
    );
    let autonomous = response(0, transition(2), None);
    assert_eq!(
        ap.receive(link(), &autonomous, at(3_000_000)).unwrap(),
        ReportWindowEvent::AutonomousReceived
    );
    assert_eq!(ap.autonomous_reports().count(), 1);
    assert_eq!(ap.reports().count(), 1);
    ap.clear_autonomous_reports();
    assert_eq!(ap.autonomous_reports().count(), 0);
    ap.cancel();
    assert_eq!(
        ap.receive(link(), &frequent, at(3_000_000)).unwrap(),
        ReportWindowEvent::Ignored
    );
}
#[test]
fn uri_route_requires_beacon_absence_and_failed_tx_retries_the_same_body() {
    let network = NetworkIdentity(1);
    let log = EventJournal::<5, 257>::new(network).unwrap();
    let mut sta = EventStation::<128, 2>::new(
        link(),
        network,
        Duration::from_secs(120),
        FrequentTransitionFormat::WithLastEvent,
    )
    .unwrap();
    let mut bytes = request(1, EventType::TRANSITION, 0, &[]);
    bytes.extend_from_slice(&[141, 2, 1, b'x']);
    let id = received(&mut sta, &bytes, at(0));
    sta.prepare_reports(id, &log, at(0)).unwrap();
    let tx = sta.send_next(at(0)).unwrap().unwrap();
    let offline = ReportConnectivity {
        requester_reachable: false,
        beacon_absent_since: at(0),
    };
    assert_eq!(sta.delivery(offline, at(59_000_000)).unwrap(), None);
    assert_eq!(
        sta.delivery(offline, at(60_000_000))
            .unwrap()
            .unwrap()
            .route,
        ReportRoute::Uri { uri: b"x" }
    );
    assert_eq!(
        sta.delivery(reachable(), at(61_000_000))
            .unwrap()
            .unwrap()
            .route,
        ReportRoute::Action {
            destination: link().peer
        }
    );
    let original = sta
        .delivery(reachable(), at(61_000_000))
        .unwrap()
        .unwrap()
        .body
        .to_vec();
    sta.admitted(tx, at(61_000_000)).unwrap();
    assert_eq!(
        sta.tx_completed(tx, TxOutcome::Failed, at(61_000_000))
            .unwrap(),
        EventStationEvent::TxFailed { id: tx }
    );
    let retry = sta.send_next(at(61_000_000)).unwrap().unwrap();
    assert_ne!(retry, tx);
    assert_eq!(
        sta.delivery(reachable(), at(61_000_000))
            .unwrap()
            .unwrap()
            .body,
        original
    );
    assert_eq!(
        sta.poll(at(181_000_000)).unwrap(),
        EventStationEvent::TimedOut { id: retry }
    );
    assert!(sta.send_next(at(181_000_000)).unwrap().is_some());
}
#[test]
fn unsupported_event_types_and_unattainable_thresholds_are_reported_incapable() {
    let network = NetworkIdentity(1);
    let log = EventJournal::<5, 257>::new(network).unwrap();
    let mut sta = EventStation::<128, 2>::new(
        link(),
        network,
        Duration::from_secs(1),
        FrequentTransitionFormat::StatusOnly,
    )
    .unwrap();
    for (dialog, kind, conditions) in [
        (1, EventType(4), &[4, 0, 4, 0][..]),
        (2, EventType::TRANSITION, &[4, 3, 6, 1, 0][..]),
    ] {
        let bytes = request(dialog, kind, 0, conditions);
        let id = received(&mut sta, &bytes, at(0));
        sta.prepare_reports(id, &log, at(0)).unwrap();
        let response = ack(&mut sta, at(0));
        let frame = EventReportFrame::parse(&response).unwrap();
        assert_eq!(
            frame.reports().unwrap().next().unwrap().status,
            EventReportStatus::INCAPABLE
        );
    }
    let mut ap = EventRequester::<128, 2>::new(link(), Duration::from_secs(1)).unwrap();
    let bytes = request(1, EventType::RSNA, 0, &[]);
    let id = ap
        .request(EventRequestFrame::parse(&bytes).unwrap(), at(0))
        .unwrap()
        .id;
    ap.admitted(id, at(0)).unwrap();
    let report = EventReport {
        kind: EventType::RSNA,
        status: EventReportStatus::INCAPABLE,
        timing: None,
        data: EventData::Empty,
        ..transition(0)
    };
    ap.receive(link(), &response(1, report, None), at(0))
        .unwrap();
    assert!(ap.peer_incapable(EventType::RSNA));
    assert_eq!(
        ap.request(
            EventRequestFrame::parse(&request(2, EventType::RSNA, 0, &[])).unwrap(),
            at(0)
        ),
        Err(ReportError::UnsupportedRequest)
    );
}
#[test]
fn refused_event_subscription_never_emits_frequent_alerts() {
    let network = NetworkIdentity(1);
    let mut log = EventJournal::<5, 257>::new(network).unwrap();
    let mut sta = EventStation::<128, 2>::new(
        link(),
        network,
        Duration::from_secs(1),
        FrequentTransitionFormat::StatusOnly,
    )
    .unwrap();
    let id = received(
        &mut sta,
        &request(1, EventType::TRANSITION, 0, &[4, 3, 1, 1, 0]),
        at(0),
    );
    sta.prepare_with_admission(id, &log, at(0), |_| EventAdmission::Refused)
        .unwrap();
    let bytes = ack(&mut sta, at(0));
    let frame = EventReportFrame::parse(&bytes).unwrap();
    assert_eq!(
        frame.reports().unwrap().next().unwrap().status,
        EventReportStatus::REFUSED
    );
    let event = log.record(network, transition(1), at(1)).unwrap();
    assert_eq!(sta.transition_logged(&log, event, at(1)).unwrap(), 0);
    assert_eq!(sta.send_next(at(1)).unwrap(), None);
}

#[test]
fn unsolicited_matching_alerts_cannot_activate_an_unadmitted_subscription() {
    let mut ap = EventRequester::<128, 2>::new(link(), Duration::from_secs(1)).unwrap();
    let bytes = request(1, EventType::TRANSITION, 0, &[4, 3, 1, 1, 0]);
    let id = ap
        .request(EventRequestFrame::parse(&bytes).unwrap(), at(0))
        .unwrap()
        .id;
    let empty = EventReport {
        status: EventReportStatus::FREQUENT_TRANSITION,
        timing: None,
        data: EventData::Empty,
        ..transition(0)
    };
    let alert = response(1, empty, Some(FrequentTransitionFormat::StatusOnly));
    assert_eq!(
        ap.receive(link(), &alert, at(0)).unwrap(),
        ReportWindowEvent::Ignored
    );
    assert_eq!(ap.monitoring_reports().count(), 0);
    assert_eq!(ap.reports().count(), 0);
    ap.admitted(id, at(0)).unwrap();
    ap.receive(link(), &alert, at(0)).unwrap();
    assert_eq!(ap.reports().count(), 1);
}

#[test]
fn frequent_subscriptions_are_admitted_independently_by_event_token() {
    for reversed in [false, true] {
        let mut requester = EventRequester::<128, 2>::new(link(), Duration::from_secs(1)).unwrap();
        // Two frequent-transition requests use the same type, with distinct tokens.
        let bytes = [
            10, 0, 1, 78, 8, 1, 0, 0, 4, 3, 2, 0xe8, 3, 78, 8, 2, 0, 0, 4, 3, 2, 0xe8, 3,
        ];
        let id = requester
            .request(EventRequestFrame::parse(&bytes).unwrap(), at(0))
            .unwrap()
            .id;
        requester.admitted(id, at(0)).unwrap();
        let accepted = [79, 3, 1, 0, 0];
        let refused = [79, 3, 2, 0, 2];
        let mut initial = std::vec![10, 1, 1];
        for element in if reversed {
            [refused, accepted]
        } else {
            [accepted, refused]
        } {
            initial.extend_from_slice(&element);
        }
        requester.receive(link(), &initial, at(0)).unwrap();
        assert_eq!(
            requester.poll(at(1_000_000)).unwrap(),
            ReportWindowEvent::WindowClosed { id, frames: 1 }
        );
        assert_eq!(
            requester
                .receive(link(), &[10, 1, 1, 79, 3, 2, 0, 4], at(2_000_000))
                .unwrap(),
            ReportWindowEvent::Ignored
        );
        assert_eq!(
            requester
                .receive(link(), &[10, 1, 1, 79, 3, 1, 0, 4], at(2_000_000))
                .unwrap(),
            ReportWindowEvent::MonitoringReportReceived { id }
        );
        assert_eq!(requester.monitoring_reports().count(), 1);
    }
}
