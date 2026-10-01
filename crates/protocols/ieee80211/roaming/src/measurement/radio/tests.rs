use super::*;
fn link() -> LinkIdentity {
    LinkIdentity {
        peer: [2, 0, 0, 0, 0, 1],
        generation: 1,
    }
}
fn at(us: u64) -> Instant {
    Instant::from_micros(us)
}
fn ttl(us: u64) -> Duration {
    Duration::from_micros(us)
}
fn request() -> RadioMeasurementRequest<'static> {
    RadioMeasurementRequest::parse(&[5, 0, 1, 0, 0, 38, 9, 1, 0, 3, 81, 6, 10, 0, 10, 0]).unwrap()
}
fn load(token: u8, value: u8, duration_tu: u16) -> std::vec::Vec<u8> {
    let mut wire = std::vec![39, 16, token, 0, 3, 81, 6, 1, 0, 0, 0, 0, 0, 0, 0];
    wire.extend_from_slice(&duration_tu.to_le_bytes());
    wire.push(value);
    wire
}
#[test]
fn result_window_keeps_multiple_frames_and_rejects_capacity_atomically() {
    let mut owner = RadioMeasurementRequester::<64, 2>::new(link(), ttl(100)).unwrap();
    let id = owner.request(at(0), request()).unwrap();
    owner.admitted(id, at(0)).unwrap();
    for (index, value) in [50, 80].into_iter().enumerate() {
        let bytes = [std::vec![5, 1, 1], load(1, value, 10), std::vec![221, 1, 6]].concat();
        assert_eq!(
            owner.receive(link(), &bytes, at(1)).unwrap(),
            MeasurementEvent::ReportsReceived { id, frame: index }
        );
        assert_eq!(
            owner.receive(link(), &bytes, at(2)).unwrap(),
            MeasurementEvent::Ignored
        );
    }
    let bytes = [std::vec![5, 1, 1], load(1, 90, 10)].concat();
    assert_eq!(
        owner.receive(link(), &bytes, at(3)),
        Err(MeasurementError::ReportStorageFull)
    );
    assert_eq!(owner.reports().count(), 2);
    assert_eq!(
        owner.tx_completed(id, TxOutcome::Failed, at(4)).unwrap(),
        MeasurementEvent::Ignored
    );
    assert_eq!(
        owner.poll(at(100)).unwrap(),
        MeasurementEvent::WindowClosed { id, frames: 2 }
    );
    assert_eq!(
        owner
            .reports()
            .next()
            .unwrap()
            .elements
            .unique(221)
            .unwrap(),
        Some(&[6][..])
    );
}
#[test]
fn incapable_is_sticky_for_the_association_and_disabled_request_is_explicit() {
    let mut owner = RadioMeasurementRequester::<64, 2>::new(link(), ttl(100)).unwrap();
    let id = owner.request(at(0), request()).unwrap();
    owner.admitted(id, at(0)).unwrap();
    owner
        .receive(link(), &[5, 1, 1, 39, 3, 1, 2, 3], at(1))
        .unwrap();
    owner.cancel();
    assert_eq!(
        owner.request(at(2), request()),
        Err(MeasurementError::PeerIncapable(
            MeasurementType::CHANNEL_LOAD
        ))
    );
    owner
        .peer_controls(link(), &[5, 0, 2, 0, 0, 38, 3, 1, 6, 3])
        .unwrap();
    assert!(
        !owner
            .permissions()
            .requests_allowed(MeasurementType::CHANNEL_LOAD)
    );
    let mut fresh = RadioMeasurementRequester::<64, 2>::new(
        LinkIdentity {
            generation: 2,
            ..link()
        },
        ttl(100),
    )
    .unwrap();
    assert!(fresh.request(at(2), request()).is_ok());
}
#[test]
fn sequential_parallel_groups_repetitions_and_old_work_identity_are_bound() {
    let mut owner =
        RadioMeasurementResponder::<128, 4>::new(link(), ttl(100_000), ttl(10_000)).unwrap();
    let bytes = [
        5, 0, 1, 1, 0, 38, 9, 1, 1, 3, 81, 6, 10, 0, 10, 0, 38, 9, 2, 0, 4, 81, 6, 10, 0, 10, 0,
        38, 13, 3, 0, 3, 81, 6, 10, 0, 10, 0, 1, 2, 1, 100,
    ];
    owner
        .receive(link(), &bytes, RequestAddressing::Individual, at(0))
        .unwrap();
    let work = owner.work(at(0)).unwrap().unwrap();
    let first = work.id;
    assert_eq!(work.requests.iter().flatten().count(), 2);
    assert_eq!(work.latest_start, at(10_240));
    assert_eq!(
        owner.complete_work(first, Elements::EMPTY, |_| ReportMetadata::default(), at(0)),
        Err(MeasurementError::WorkNotStarted)
    );
    owner.start_work(first, at(0)).unwrap();
    let result = owner
        .complete_work(
            first,
            Elements::parse(&[load(1, 50, 10), std::vec![39, 3, 2, 2, 4]].concat()).unwrap(),
            |_| ReportMetadata::default(),
            at(10_240),
        )
        .unwrap();
    let PlanEvent::GroupCompleted {
        more: true,
        response: Some(tx),
    } = result
    else {
        panic!()
    };
    owner.admitted(tx, at(10_240)).unwrap();
    owner
        .tx_completed(tx, TxOutcome::Acknowledged, at(10_240))
        .unwrap();
    let next = owner.work(at(10_240)).unwrap().unwrap().id;
    assert_ne!(first, next);
    assert_eq!(
        owner.start_work(first, at(10_240)),
        Err(MeasurementError::WrongWork)
    );
    owner.start_work(next, at(10_240)).unwrap();
    assert_eq!(
        owner
            .complete_work(
                next,
                Elements::parse(&load(3, 50, 10)).unwrap(),
                |_| ReportMetadata::default(),
                at(20_480)
            )
            .unwrap(),
        PlanEvent::GroupCompleted {
            more: true,
            response: None
        }
    );
    assert_eq!(owner.work(at(20_480)).unwrap().unwrap().id.round(), 1);
}
#[test]
fn supersession_precedence_and_lower_priority_controls_preserve_live_work() {
    let mut owner =
        RadioMeasurementResponder::<128, 4>::new(link(), ttl(100_000), ttl(1000)).unwrap();
    let bytes = [5, 0, 1, 0, 0, 38, 9, 1, 0, 3, 81, 6, 10, 0, 10, 0];
    owner
        .receive(link(), &bytes, RequestAddressing::Individual, at(0))
        .unwrap();
    let work = owner.work(at(0)).unwrap().unwrap().id;
    assert_eq!(
        owner
            .receive(
                link(),
                &[5, 0, 2, 0, 0, 38, 3, 2, 2, 3],
                RequestAddressing::Broadcast,
                at(1)
            )
            .unwrap(),
        PlanEvent::ControlsApplied
    );
    assert!(
        !owner
            .permissions()
            .requests_allowed(MeasurementType::CHANNEL_LOAD)
    );
    assert_eq!(owner.work(at(1)).unwrap().unwrap().id, work);
    assert_eq!(
        owner
            .receive(
                link(),
                &[5, 0, 3, 0, 0],
                RequestAddressing::Individual,
                at(2)
            )
            .unwrap(),
        PlanEvent::Cancelled { id: work.operation }
    );
    assert!(owner.work(at(2)).unwrap().is_none());
}
#[test]
fn reporting_conditions_duration_and_beacon_offsets_use_supplied_evidence() {
    let wire = [38, 13, 1, 16, 3, 81, 6, 0, 0, 10, 0, 1, 2, 1, 100];
    let request = MeasurementRequestElement::parse(&wire[2..]).unwrap();
    let report = load(1, 90, 10);
    assert!(
        !report_allowed(
            request,
            MeasurementReportElement::parse(&report[2..]).unwrap(),
            ReportMetadata::default()
        )
        .unwrap()
    );
    let short = load(1, 110, 9);
    assert_eq!(
        report_allowed(
            request,
            MeasurementReportElement::parse(&short[2..]).unwrap(),
            ReportMetadata::default()
        ),
        Err(MeasurementError::InvalidDuration)
    );
    assert!(
        beacon_condition(
            5,
            0xfc,
            80,
            255,
            ReportMetadata {
                serving_rcpi: Some(83),
                ..ReportMetadata::default()
            }
        )
        .unwrap()
    );
    assert!(
        !beacon_condition(
            5,
            0xfc,
            79,
            255,
            ReportMetadata {
                serving_rcpi: Some(83),
                ..ReportMetadata::default()
            }
        )
        .unwrap()
    );
    assert_eq!(
        beacon_condition(9, 5, 80, 255, ReportMetadata::default()),
        Err(MeasurementError::MissingReference)
    );
}
#[test]
fn malformed_or_excessive_request_does_not_replace_accepted_plan() {
    let mut owner =
        RadioMeasurementResponder::<64, 1>::new(link(), ttl(100_000), ttl(1000)).unwrap();
    owner
        .receive(
            link(),
            &[5, 0, 1, 0, 0, 38, 9, 1, 0, 3, 81, 6, 0, 0, 10, 0],
            RequestAddressing::Individual,
            at(0),
        )
        .unwrap();
    let id = owner.work(at(0)).unwrap().unwrap().id;
    assert!(
        owner
            .receive(
                link(),
                &[5, 0, 2, 0, 0, 38, 4, 1, 0, 3, 1],
                RequestAddressing::Individual,
                at(1)
            )
            .is_err()
    );
    assert_eq!(owner.work(at(1)).unwrap().unwrap().id, id);
    assert_eq!(
        owner.poll(at(100_000)).unwrap(),
        PlanEvent::TimedOut { id: id.operation }
    );
}

#[test]
fn refusal_needs_no_scan_admission_and_measurement_completion_cannot_skip_work() {
    let mut owner = RadioMeasurementResponder::<64, 1>::new(link(), ttl(10), ttl(1000)).unwrap();
    let wire = [5, 0, 1, 0, 0, 38, 9, 1, 0, 3, 81, 6, 0, 0, 10, 0];
    owner
        .receive(link(), &wire, RequestAddressing::Individual, at(0))
        .unwrap();
    let id = owner.work(at(0)).unwrap().unwrap().id;
    let result = owner
        .complete_work(
            id,
            Elements::parse(&[39, 3, 1, 4, 3]).unwrap(),
            |_| ReportMetadata::default(),
            at(1),
        )
        .unwrap();
    assert!(matches!(
        result,
        PlanEvent::GroupCompleted {
            more: false,
            response: Some(_)
        }
    ));
    assert_eq!(owner.next_deadline(), Some(at(10)));
    let mut fresh =
        RadioMeasurementResponder::<64, 1>::new(link(), ttl(100_000), ttl(1000)).unwrap();
    fresh
        .receive(link(), &wire, RequestAddressing::Individual, at(0))
        .unwrap();
    let id = fresh.work(at(0)).unwrap().unwrap().id;
    fresh.start_work(id, at(0)).unwrap();
    assert!(matches!(
        fresh.complete_work(id, Elements::EMPTY, |_| ReportMetadata::default(), at(1)),
        Err(MeasurementError::MissingMeasurement { token: 1, .. })
    ));
    assert_eq!(
        fresh.complete_work(
            id,
            Elements::parse(&load(1, 50, 10)).unwrap(),
            |_| ReportMetadata::default(),
            at(1)
        ),
        Err(MeasurementError::InvalidDuration)
    );
    assert!(
        fresh
            .complete_work(
                id,
                Elements::parse(&load(1, 50, 10)).unwrap(),
                |_| ReportMetadata::default(),
                at(10_240)
            )
            .is_ok()
    );
}
