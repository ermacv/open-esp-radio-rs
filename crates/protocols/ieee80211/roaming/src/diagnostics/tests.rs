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
fn at(seconds: u32) -> Instant {
    Instant::EPOCH
        .checked_add(Duration::from_secs(seconds))
        .unwrap()
}
fn req_element(token: u8, kind: DiagnosticType, seconds: u16, info: &[u8]) -> Vec<u8> {
    let mut bytes = [0; 257];
    let n = DiagnosticRequest {
        token,
        kind,
        timeout_seconds: seconds,
        information: Elements::parse(info).unwrap(),
    }
    .encode(&mut bytes)
    .unwrap();
    bytes[..n].to_vec()
}
fn request(dialog: u8, token: u8, kind: DiagnosticType, seconds: u16, info: &[u8]) -> Vec<u8> {
    let mut bytes = std::vec![10, 2, dialog];
    bytes.extend_from_slice(&req_element(token, kind, seconds, info));
    bytes
}
fn result(token: u8, kind: DiagnosticType, status: u8, info: &[u8]) -> Vec<u8> {
    let mut bytes = [0; 257];
    let n = DiagnosticReport {
        token,
        kind,
        status: DiagnosticReportStatus(status),
        information: Elements::parse(info).unwrap(),
    }
    .encode(&mut bytes)
    .unwrap();
    bytes[..n].to_vec()
}
fn frame(dialog: u8, elements: &[u8]) -> Vec<u8> {
    let mut bytes = std::vec![10, 3, dialog];
    bytes.extend_from_slice(elements);
    bytes
}
fn ap_info() -> Vec<u8> {
    let mut bytes = [0; 10];
    DiagnosticAp {
        bssid: [2, 0, 0, 0, 0, 2],
        operating_class: 81,
        channel: 6,
    }
    .encode(&mut bytes)
    .unwrap();
    bytes.to_vec()
}
fn active_request(kind: DiagnosticType) -> Vec<u8> {
    let mut info = ap_info();
    if kind == DiagnosticType::IEEE8021X {
        info.extend_from_slice(&[7, 8, 254, 0, 0, 1, 0, 0, 0, 2, 0, 2, 2, 3]);
    }
    info.extend_from_slice(&[15, 1, 7]);
    info
}
fn active_result(kind: DiagnosticType) -> Vec<u8> {
    let mut info = ap_info();
    info.extend_from_slice(&[17, 2, 0, 0]);
    if kind == DiagnosticType::IEEE8021X {
        info.extend_from_slice(&[7, 8, 254, 0, 0, 1, 0, 0, 0, 2, 0, 2, 2, 3]);
    }
    info
}
fn received<const B: usize, const J: usize, const F: usize>(
    sta: &mut DiagnosticStation<B, J, F>,
    bytes: &[u8],
    now: Instant,
) -> OperationId {
    let DiagnosticStationEvent::Requested { id, .. } = sta
        .receive(link(), bytes, now, |_| DiagnosticAdmission::Accept)
        .unwrap()
    else {
        panic!("new request")
    };
    id
}
fn reachable() -> ReportConnectivity {
    ReportConnectivity {
        requester_reachable: true,
        beacon_absent_since: at(0),
    }
}
fn ack<const B: usize, const J: usize, const F: usize>(
    sta: &mut DiagnosticStation<B, J, F>,
    now: Instant,
) -> Vec<u8> {
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
        DiagnosticStationEvent::Transmitted { id: tx }
    );
    bytes
}
#[test]
fn configuration_profiles_pack_complete_elements_and_requester_keeps_all_frames() {
    let mut sta =
        DiagnosticStation::<24, 2, 2>::new(link(), NetworkIdentity(1), Duration::from_secs(1))
            .unwrap();
    let bytes = request(1, 0, DiagnosticType::CONFIGURATION, 10, &[]);
    received(&mut sta, &bytes, at(0));
    let work = sta.works().next().unwrap().id;
    let mut reports = result(
        0,
        DiagnosticType::CONFIGURATION,
        0,
        &[15, 1, 1, 18, 2, b'a', 0],
    );
    reports.extend_from_slice(&result(
        0,
        DiagnosticType::CONFIGURATION,
        0,
        &[15, 1, 2, 18, 2, b'b', 0],
    ));
    sta.complete_work(work, Elements::parse(&reports).unwrap(), at(0))
        .unwrap();
    let mut ap = DiagnosticRequester::<24, 2>::new(link(), Duration::from_secs(10)).unwrap();
    let id = ap
        .request(DiagnosticRequestFrame::parse(&bytes).unwrap(), at(0))
        .unwrap()
        .id;
    ap.admitted(id, at(0)).unwrap();
    for _ in 0..2 {
        let bytes = ack(&mut sta, at(0));
        let frame = DiagnosticReportFrame::parse(&bytes).unwrap();
        assert_eq!(frame.reports().unwrap().count(), 1);
        assert_eq!(
            ap.receive(link(), &bytes, at(0)).unwrap(),
            ReportWindowEvent::ReportReceived { id }
        );
    }
    assert_eq!(sta.send_next(at(0)).unwrap(), None);
    assert_eq!(ap.reports().count(), 2);
    assert_eq!(
        ap.tx_completed(id, TxOutcome::Failed, at(0)).unwrap(),
        ReportWindowEvent::Ignored
    );
    assert_eq!(
        ap.poll(at(10)).unwrap(),
        ReportWindowEvent::WindowClosed { id, frames: 2 }
    );
    assert_eq!(ap.reports().count(), 2);
}
#[test]
fn manufacturer_repetitions_and_unknown_information_are_not_dropped() {
    let mut sta =
        DiagnosticStation::<128, 1, 1>::new(link(), NetworkIdentity(1), Duration::from_secs(1))
            .unwrap();
    received(
        &mut sta,
        &request(1, 1, DiagnosticType::MANUFACTURER, 10, &[]),
        at(0),
    );
    let work = sta.works().next().unwrap().id;
    let info = [
        10, 2, b'A', b'B', 3, 3, 1, 2, b'x', 3, 3, 2, 3, b'y', 5, 1, 4, 5, 1, 5, 6, 1, 1, 6, 1, 2,
        200, 2, 8, 9,
    ];
    let reports = result(1, DiagnosticType::MANUFACTURER, 0, &info);
    sta.complete_work(work, Elements::parse(&reports).unwrap(), at(0))
        .unwrap();
    let bytes = ack(&mut sta, at(0));
    let frame = DiagnosticReportFrame::parse(&bytes).unwrap();
    assert_eq!(
        frame
            .reports()
            .unwrap()
            .next()
            .unwrap()
            .information
            .as_bytes(),
        info
    );
}
#[test]
fn result_capacity_and_correlation_are_preflighted_before_completing_work() {
    let mut sta =
        DiagnosticStation::<24, 2, 1>::new(link(), NetworkIdentity(1), Duration::from_secs(1))
            .unwrap();
    received(
        &mut sta,
        &request(1, 1, DiagnosticType::CONFIGURATION, 10, &[]),
        at(0),
    );
    let work = sta.works().next().unwrap().id;
    let valid = result(
        1,
        DiagnosticType::CONFIGURATION,
        0,
        &[15, 1, 1, 18, 2, b'a', 0],
    );
    let mut all = valid.clone();
    all.extend_from_slice(&result(
        1,
        DiagnosticType::CONFIGURATION,
        0,
        &[15, 1, 2, 18, 2, b'b', 0],
    ));
    assert_eq!(
        sta.complete_work(work, Elements::parse(&all).unwrap(), at(0)),
        Err(ReportError::Full)
    );
    assert_eq!(sta.works().count(), 1);
    assert_eq!(sta.send_next(at(0)).unwrap(), None);
    let wrong = result(2, DiagnosticType::CONFIGURATION, 0, &[15, 1, 1]);
    assert_eq!(
        sta.complete_work(work, Elements::parse(&wrong).unwrap(), at(0)),
        Err(ReportError::UnexpectedToken(2))
    );
    assert_eq!(sta.works().count(), 1);
    assert_eq!(
        sta.complete_work(work, Elements::EMPTY, at(0)),
        Err(ReportError::IncompleteResults)
    );
    sta.complete_work(work, Elements::parse(&valid).unwrap(), at(0))
        .unwrap();
    assert_eq!(sta.works().count(), 0);
}
#[test]
fn active_diagnostics_require_admission_target_and_authentication_results() {
    for kind in [DiagnosticType::ASSOCIATION, DiagnosticType::IEEE8021X] {
        let mut sta =
            DiagnosticStation::<256, 2, 2>::new(link(), NetworkIdentity(1), Duration::from_secs(1))
                .unwrap();
        received(
            &mut sta,
            &request(1, 1, kind, 10, &active_request(kind)),
            at(0),
        );
        let work = sta.works().next().unwrap();
        assert_eq!(work.request.profile_id().unwrap(), Some(7));
        let id = work.id;
        let reports = result(1, kind, 0, &active_result(kind));
        assert_eq!(
            sta.complete_work(id, Elements::parse(&reports).unwrap(), at(0)),
            Err(ReportError::Protocol(Error::NotAdmitted))
        );
        sta.admitted_work(id, at(0)).unwrap();
        let missing = result(1, kind, 0, &ap_info());
        assert_eq!(
            sta.complete_work(id, Elements::parse(&missing).unwrap(), at(0)),
            Err(ReportError::InvalidResult)
        );
        let mut wrong = active_result(kind);
        wrong[7] = 3;
        let wrong = result(1, kind, 0, &wrong);
        assert_eq!(
            sta.complete_work(id, Elements::parse(&wrong).unwrap(), at(0)),
            Err(ReportError::InvalidResult)
        );
        sta.complete_work(id, Elements::parse(&reports).unwrap(), at(0))
            .unwrap();
        ack(&mut sta, at(0));
    }
}
#[test]
fn task_timeouts_are_exact_and_expired_reports_are_never_transmitted() {
    let mut sta =
        DiagnosticStation::<128, 3, 3>::new(link(), NetworkIdentity(1), Duration::from_secs(10))
            .unwrap();
    let mut bytes = request(1, 1, DiagnosticType::MANUFACTURER, 1, &[]);
    bytes.extend_from_slice(&req_element(2, DiagnosticType::CONFIGURATION, 5, &[]));
    received(&mut sta, &bytes, at(0));
    let work = sta.works().next().unwrap().id;
    assert_eq!(sta.next_deadline(), Some(at(1)));
    assert_eq!(
        sta.poll(at(1)).unwrap(),
        DiagnosticStationEvent::WorkExpired {
            id: work,
            admitted: false
        }
    );
    let report = result(1, DiagnosticType::MANUFACTURER, 0, &[]);
    assert_eq!(
        sta.complete_work(work, Elements::parse(&report).unwrap(), at(1)),
        Err(ReportError::ExpiredWork)
    );
    assert_eq!(sta.works().count(), 1);
    let remaining = sta.works().next().unwrap().id;
    let report = result(2, DiagnosticType::CONFIGURATION, 0, &[15, 1, 1]);
    sta.complete_work(remaining, Elements::parse(&report).unwrap(), at(2))
        .unwrap();
    assert_eq!(sta.next_deadline(), Some(at(5)));
    assert_eq!(sta.send_next(at(5)), Err(ReportError::ExpiredWork));
    assert_eq!(
        sta.poll(at(5)).unwrap(),
        DiagnosticStationEvent::ReportExpired { id: remaining }
    );
    assert_eq!(sta.send_next(at(5)).unwrap(), None);
    let zero = received(
        &mut sta,
        &request(2, 0, DiagnosticType::MANUFACTURER, 0, &[]),
        at(5),
    );
    let work = sta.works().next().unwrap().id;
    assert_eq!(work.request(), zero);
    assert_eq!(
        sta.poll(at(5)).unwrap(),
        DiagnosticStationEvent::WorkExpired {
            id: work,
            admitted: false
        }
    );
}
#[test]
fn rejection_reports_keep_individual_deadlines_and_incapable_is_sticky() {
    let mut sta =
        DiagnosticStation::<128, 2, 2>::new(link(), NetworkIdentity(1), Duration::from_secs(10))
            .unwrap();
    let mut bytes = request(1, 1, DiagnosticType::MANUFACTURER, 1, &[]);
    bytes.extend_from_slice(&req_element(2, DiagnosticType::CONFIGURATION, 5, &[]));
    sta.receive(link(), &bytes, at(0), |_| DiagnosticAdmission::Incapable)
        .unwrap();
    let DiagnosticStationEvent::ReportExpired { id } = sta.poll(at(1)).unwrap() else {
        panic!("report expired")
    };
    assert_eq!(id.request(), sta.request().unwrap().0);
    assert_eq!(id.token(), 1);
    let delivered = ack(&mut sta, at(1));
    let frame = DiagnosticReportFrame::parse(&delivered).unwrap();
    assert_eq!(frame.reports().unwrap().next().unwrap().token, 2);
    let mut ap = DiagnosticRequester::<128, 2>::new(link(), Duration::from_secs(10)).unwrap();
    let id = ap
        .request(DiagnosticRequestFrame::parse(&bytes).unwrap(), at(0))
        .unwrap()
        .id;
    ap.admitted(id, at(0)).unwrap();
    ap.receive(link(), &delivered, at(1)).unwrap();
    assert!(ap.peer_incapable(DiagnosticType::CONFIGURATION));
    assert!(!ap.peer_incapable(DiagnosticType::MANUFACTURER));
    assert_eq!(
        ap.request(
            DiagnosticRequestFrame::parse(&request(2, 1, DiagnosticType::CONFIGURATION, 5, &[]))
                .unwrap(),
            at(1)
        ),
        Err(ReportError::UnsupportedRequest)
    );
}
#[test]
fn new_requests_and_remote_cancel_retire_work_and_ignore_late_callbacks() {
    let mut sta =
        DiagnosticStation::<128, 2, 2>::new(link(), NetworkIdentity(1), Duration::from_secs(10))
            .unwrap();
    let old = received(
        &mut sta,
        &request(1, 1, DiagnosticType::MANUFACTURER, 10, &[]),
        at(0),
    );
    let work = sta.works().next().unwrap().id;
    sta.cancel_work(work, at(0)).unwrap();
    let tx = sta.send_next(at(0)).unwrap().unwrap();
    sta.admitted(tx, at(0)).unwrap();
    let next = received(
        &mut sta,
        &request(2, 2, DiagnosticType::CONFIGURATION, 10, &[]),
        at(1),
    );
    assert_ne!(old, next);
    assert_eq!(
        sta.complete_work(work, Elements::EMPTY, at(1)),
        Err(ReportError::WrongWork)
    );
    assert_eq!(
        sta.tx_completed(tx, TxOutcome::Failed, at(0)).unwrap(),
        DiagnosticStationEvent::Ignored
    );
    let mut requester =
        DiagnosticRequester::<128, 2>::new(link(), Duration::from_secs(10)).unwrap();
    let cancel = requester.cancel_remote(3, at(1)).unwrap();
    let body = requester.transmission().unwrap().body.to_vec();
    let cancelled = received(&mut sta, &body, at(1));
    assert_eq!(sta.works().count(), 0);
    let bytes = ack(&mut sta, at(1));
    let frame = DiagnosticReportFrame::parse(&bytes).unwrap();
    let r = frame.reports().unwrap().next().unwrap();
    assert_eq!(
        (r.kind, r.status),
        (DiagnosticType::CANCEL, DiagnosticReportStatus::CANCELLED)
    );
    assert_eq!(sta.request().unwrap().0, cancelled);
    requester.admitted(cancel.id, at(1)).unwrap();
    requester.receive(link(), &bytes, at(1)).unwrap();
}
#[test]
fn diagnostic_caused_transition_survives_and_return_rebinds_the_generation() {
    let network = NetworkIdentity(1);
    let mut sta =
        DiagnosticStation::<256, 2, 2>::new(link(), network, Duration::from_secs(120)).unwrap();
    let mut bytes = request(
        1,
        1,
        DiagnosticType::ASSOCIATION,
        300,
        &active_request(DiagnosticType::ASSOCIATION),
    );
    bytes.extend_from_slice(&[141, 2, 1, b'x']);
    received(&mut sta, &bytes, at(0));
    let work = sta.works().next().unwrap().id;
    sta.admitted_work(work, at(0)).unwrap();
    assert_eq!(
        sta.association_changed(network, Some(work), at(1)).unwrap(),
        DiagnosticStationEvent::DiagnosticTransition { id: work }
    );
    assert_eq!(
        sta.receive(link(), &[], at(1), |_| DiagnosticAdmission::Accept)
            .unwrap(),
        DiagnosticStationEvent::Ignored
    );
    let reports = result(
        1,
        DiagnosticType::ASSOCIATION,
        0,
        &active_result(DiagnosticType::ASSOCIATION),
    );
    sta.complete_work(work, Elements::parse(&reports).unwrap(), at(10))
        .unwrap();
    let tx = sta.send_next(at(10)).unwrap().unwrap();
    assert_eq!(sta.delivery(reachable(), at(59)).unwrap(), None);
    assert_eq!(
        sta.delivery(reachable(), at(60)).unwrap().unwrap().route,
        ReportRoute::Uri { uri: b"x" }
    );
    sta.admitted(tx, at(60)).unwrap();
    assert_eq!(
        sta.tx_completed(tx, TxOutcome::Failed, at(60)).unwrap(),
        DiagnosticStationEvent::TxFailed { id: tx }
    );
    let new_link = LinkIdentity {
        generation: 2,
        ..link()
    };
    sta.reattached(new_link, network, at(61)).unwrap();
    assert_eq!(
        sta.receive(link(), &[], at(0), |_| DiagnosticAdmission::Accept)
            .unwrap(),
        DiagnosticStationEvent::Ignored
    );
    let next = sta.send_next(at(61)).unwrap().unwrap();
    assert_eq!(next.link, new_link);
    assert_eq!(
        sta.delivery(reachable(), at(61)).unwrap().unwrap().route,
        ReportRoute::Action {
            destination: link().peer
        }
    );
    assert_eq!(
        sta.tx_completed(tx, TxOutcome::Failed, at(0)).unwrap(),
        DiagnosticStationEvent::Ignored
    );
    assert!(matches!(
        sta.association_changed(network, None, at(62)).unwrap(),
        DiagnosticStationEvent::Cancelled { .. }
    ));
    assert!(sta.request().is_none());
}
#[test]
fn requester_rejects_conflicting_profiles_and_invalid_frames_atomically() {
    let mut ap = DiagnosticRequester::<128, 1>::new(link(), Duration::from_secs(10)).unwrap();
    let bytes = request(1, 1, DiagnosticType::CONFIGURATION, 10, &[]);
    let id = ap
        .request(DiagnosticRequestFrame::parse(&bytes).unwrap(), at(0))
        .unwrap()
        .id;
    ap.admitted(id, at(0)).unwrap();
    let mut reports = result(
        1,
        DiagnosticType::CONFIGURATION,
        0,
        &[15, 1, 1, 18, 1, b'a'],
    );
    reports.extend_from_slice(&result(
        1,
        DiagnosticType::CONFIGURATION,
        0,
        &[15, 1, 1, 18, 1, b'b'],
    ));
    assert_eq!(
        ap.receive(link(), &frame(1, &reports), at(0)),
        Err(ReportError::InvalidResult)
    );
    assert_eq!(ap.reports().count(), 0);
    let valid = frame(
        1,
        &result(
            1,
            DiagnosticType::CONFIGURATION,
            0,
            &[15, 1, 1, 18, 1, b'a'],
        ),
    );
    ap.receive(link(), &valid, at(0)).unwrap();
    assert_eq!(
        ap.receive(link(), &valid, at(0)).unwrap(),
        ReportWindowEvent::Ignored
    );
    let other = frame(
        1,
        &result(
            1,
            DiagnosticType::CONFIGURATION,
            0,
            &[15, 1, 1, 18, 1, b'b'],
        ),
    );
    assert_eq!(
        ap.receive(link(), &other, at(0)),
        Err(ReportError::InvalidResult)
    );
    let other = frame(1, &result(1, DiagnosticType::CONFIGURATION, 0, &[15, 1, 2]));
    assert_eq!(ap.receive(link(), &other, at(0)), Err(ReportError::Full));
    assert_eq!(ap.reports().count(), 1);
    assert_eq!(
        ap.receive(
            LinkIdentity {
                generation: 0,
                ..link()
            },
            &[],
            at(0)
        )
        .unwrap(),
        ReportWindowEvent::Ignored
    );
}
