use super::*;
use crate::TxOutcome::*;
use oer_ieee80211_mac::roaming::BssTerminationDuration;

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
const TARGET: MacAddress = [2, 0, 0, 0, 0, 9];
const REPORT: &[u8] = &[52, 16, 2, 0, 0, 0, 0, 9, 0, 0, 0, 0, 81, 6, 7, 3, 1, 200];
fn timing(now: u64) -> BssTiming {
    BssTiming {
        beacon_interval_tu: 100,
        next_tbtt: time(now + 10),
        termination_deadline: None,
    }
}
fn station<const N: usize>() -> BtmStation<N> {
    BtmStation::new(ap(), Duration::from_micros(200), Duration::from_micros(100)).unwrap()
}
fn request(token: u8) -> BtmRequest<'static> {
    BtmRequest {
        dialog_token: token,
        mode: BtmRequestMode::CANDIDATES,
        disassociation_timer: 0,
        validity_interval: 1,
        termination: None,
        session_url: None,
        elements: Elements::parse(REPORT).unwrap(),
    }
}
fn bytes(request: BtmRequest<'_>) -> std::vec::Vec<u8> {
    let mut output = [0; 256];
    let len = request.encode(&mut output).unwrap();
    output[..len].to_vec()
}
fn proposal<const N: usize>(owner: &mut BtmStation<N>, request: &[u8], now: u64) -> OperationId {
    let StationEvent::Proposal { id, .. } = owner
        .receive(ap(), request, time(now), timing(now))
        .unwrap()
    else {
        panic!("missing proposal");
    };
    id
}
fn accept() -> BtmDecision<'static> {
    BtmDecision::Accept {
        target: TARGET,
        source: CandidateSource::Request,
        elements: Elements::EMPTY,
    }
}

#[test]
fn solicited_sta_ap_exchange_accepts_only_after_response_ack() {
    let mut owner = station::<128>();
    let mut access_point = BtmAccessPoint::<128>::new(sta(), Duration::from_micros(200)).unwrap();
    let query_id = owner
        .query(
            time(0),
            BtmQuery {
                dialog_token: 7,
                reason: 19,
                elements: Elements::parse(REPORT).unwrap(),
            },
        )
        .unwrap();
    let query_bytes = owner.transmission().unwrap().body.to_vec();
    owner.admitted(query_id, time(1)).unwrap();
    let query = access_point
        .receive_query(sta(), &query_bytes)
        .unwrap()
        .unwrap();
    assert_eq!(query.reason, 19);
    assert_eq!(query.elements.as_bytes(), REPORT);
    let request_id = access_point
        .respond_to_query(time(2), query, request(7))
        .unwrap();
    let request_bytes = access_point.transmission().unwrap().body.to_vec();
    access_point.admitted(request_id, time(3)).unwrap();
    let StationEvent::Proposal {
        id,
        solicited,
        superseded,
        ..
    } = owner
        .receive(ap(), &request_bytes, time(4), timing(4))
        .unwrap()
    else {
        panic!("missing proposal");
    };
    assert!(solicited);
    assert_eq!(superseded, Some(query_id));
    assert_eq!(
        owner.tx_completed(query_id, Failed, time(5)).unwrap(),
        StationEvent::Ignored
    );
    let response_id = owner.decide(id, accept(), time(6)).unwrap();
    let response_bytes = owner.transmission().unwrap().body.to_vec();
    assert_eq!(
        owner.tx_completed(response_id, Acknowledged, time(6)),
        Err(Error::NotAdmitted)
    );
    owner.admitted(response_id, time(7)).unwrap();
    assert_eq!(
        access_point
            .receive(sta(), &response_bytes, time(8))
            .unwrap(),
        AccessPointEvent::Response {
            status: BtmStatus::ACCEPT,
            termination_delay_minutes: 0,
            target_bssid: Some(TARGET)
        }
    );
    assert_eq!(
        access_point
            .tx_completed(request_id, Failed, time(9))
            .unwrap(),
        AccessPointEvent::Ignored
    );
    assert_eq!(
        owner
            .tx_completed(response_id, Acknowledged, time(9))
            .unwrap(),
        StationEvent::TransitionReady {
            id: response_id,
            target: TARGET
        }
    );
    assert_eq!(
        owner
            .tx_completed(response_id, Acknowledged, time(10))
            .unwrap(),
        StationEvent::Ignored
    );
}

#[test]
fn reject_retains_delay_and_caller_supplied_candidate_elements() {
    let mut owner = station::<128>();
    let id = proposal(&mut owner, &bytes(request(3)), 0);
    let tx = owner
        .decide(
            id,
            BtmDecision::Reject {
                status: BtmStatus::REJECT_CANDIDATES_PROVIDED,
                termination_delay_minutes: 7,
                elements: Elements::parse(REPORT).unwrap(),
            },
            time(1),
        )
        .unwrap();
    let response = BtmResponse::parse(owner.transmission().unwrap().body).unwrap();
    assert_eq!(response.target_bssid, None);
    assert_eq!(response.termination_delay_minutes, 7);
    assert_eq!(response.elements.as_bytes(), REPORT);
    owner.admitted(tx, time(2)).unwrap();
    assert_eq!(
        owner.tx_completed(tx, Acknowledged, time(3)).unwrap(),
        StationEvent::ResponseSent {
            id: tx,
            status: BtmStatus::REJECT_CANDIDATES_PROVIDED
        }
    );
}

#[test]
fn duplicate_requests_do_not_extend_decisions_or_repeat_transitions() {
    let mut owner = station::<128>();
    let request = bytes(request(1));
    let id = proposal(&mut owner, &request, 0);
    let expires = owner.next_deadline();
    assert_eq!(
        owner.receive(ap(), &request, time(10), timing(10)).unwrap(),
        StationEvent::Ignored
    );
    assert_eq!(owner.next_deadline(), expires);
    let tx = owner.decide(id, accept(), time(11)).unwrap();
    let reply = owner.transmission().unwrap().body.to_vec();
    owner.admitted(tx, time(12)).unwrap();
    assert_eq!(
        owner.receive(ap(), &request, time(13), timing(13)).unwrap(),
        StationEvent::Ignored
    );
    assert_eq!(
        owner.tx_completed(tx, Acknowledged, time(14)).unwrap(),
        StationEvent::TransitionReady {
            id: tx,
            target: TARGET
        }
    );
    let replay_deadline = owner.next_deadline();
    assert_eq!(
        owner.receive(ap(), &request, time(15), timing(15)).unwrap(),
        StationEvent::Ignored
    );
    let replay = owner.transmission().unwrap();
    assert_ne!(replay.id, tx);
    assert_eq!(replay.body, reply);
    let replay_id = replay.id;
    assert_eq!(owner.next_deadline(), replay_deadline);
    owner.admitted(replay_id, time(16)).unwrap();
    assert_eq!(
        owner
            .tx_completed(replay_id, Acknowledged, time(17))
            .unwrap(),
        StationEvent::ResponseSent {
            id: replay_id,
            status: BtmStatus::ACCEPT
        }
    );
    assert_eq!(owner.next_deadline(), replay_deadline);
}

#[test]
fn distinct_request_supersedes_but_conflicting_token_keeps_original() {
    let mut owner = station::<128>();
    let original = bytes(request(1));
    let first = proposal(&mut owner, &original, 0);
    let mut conflict = original.clone();
    conflict[6] = 2;
    assert_eq!(
        owner.receive(ap(), &conflict, time(1), timing(1)),
        Err(Error::ConflictingDialog)
    );
    assert_eq!(owner.proposal().unwrap().0, first);
    let tx = owner.decide(first, accept(), time(2)).unwrap();
    owner.admitted(tx, time(3)).unwrap();
    let second = owner
        .receive(ap(), &bytes(request(2)), time(4), timing(4))
        .unwrap();
    let StationEvent::Proposal {
        id,
        superseded,
        solicited,
        ..
    } = second
    else {
        panic!("missing proposal");
    };
    assert_eq!(superseded, Some(tx));
    assert!(!solicited);
    assert_eq!(
        owner.decide(first, accept(), time(5)),
        Err(Error::WrongOperation)
    );
    assert_eq!(
        owner.tx_completed(tx, Acknowledged, time(500)).unwrap(),
        StationEvent::Ignored
    );
    assert_eq!(owner.proposal().unwrap().0, id);
}

#[test]
fn expired_candidates_need_an_independent_scan_and_expire_once() {
    let mut owner = station::<128>();
    let mut request = request(1);
    request.validity_interval = 0;
    let id = proposal(&mut owner, &bytes(request), 0);
    assert_eq!(owner.next_deadline(), Some(time(0)));
    assert_eq!(
        owner.poll(time(0)).unwrap(),
        StationEvent::CandidatesExpired { id }
    );
    assert_eq!(owner.next_deadline(), Some(time(100)));
    assert_eq!(owner.poll(time(1)).unwrap(), StationEvent::Ignored);
    assert_eq!(
        owner.decide(id, accept(), time(1)),
        Err(Error::CandidateExpired)
    );
    let tx = owner
        .decide(
            id,
            BtmDecision::Accept {
                target: TARGET,
                source: CandidateSource::Scan,
                elements: Elements::EMPTY,
            },
            time(2),
        )
        .unwrap();
    owner.admitted(tx, time(3)).unwrap();
    assert_eq!(
        owner.tx_completed(tx, Failed, time(4)).unwrap(),
        StationEvent::TxFailed { id: tx }
    );
    assert!(owner.proposal().is_none());
}

#[test]
fn invalid_excluded_ambiguous_and_unoffered_targets_keep_proposal() {
    let mut owner = station::<128>();
    let mut reports = REPORT.to_vec();
    reports[17] = 0;
    let mut request = request(1);
    request.elements = Elements::parse(&reports).unwrap();
    let id = proposal(&mut owner, &bytes(request), 0);
    assert_eq!(
        owner.decide(id, accept(), time(1)),
        Err(Error::CandidateExcluded)
    );
    for target in [[0; 6], [1, 0, 0, 0, 0, 9], ap().peer] {
        assert_eq!(
            owner.decide(
                id,
                BtmDecision::Accept {
                    target,
                    source: CandidateSource::Scan,
                    elements: Elements::EMPTY
                },
                time(1)
            ),
            Err(Error::InvalidTarget)
        );
    }
    assert_eq!(
        owner.decide(
            id,
            BtmDecision::Accept {
                target: [2, 0, 0, 0, 0, 8],
                source: CandidateSource::Request,
                elements: Elements::EMPTY
            },
            time(1)
        ),
        Err(Error::CandidateNotOffered)
    );
    assert_eq!(owner.proposal().unwrap().0, id);
    owner.cancel();
    let reports = [REPORT, REPORT].concat();
    request.elements = Elements::parse(&reports).unwrap();
    request.dialog_token = 2;
    let id = proposal(&mut owner, &bytes(request), 2);
    assert_eq!(
        owner.decide(id, accept(), time(3)),
        Err(Error::ConflictingCandidates)
    );
}

#[test]
fn decision_timeout_queues_rejection_with_a_separate_tx_identity() {
    let mut owner = station::<128>();
    let id = proposal(&mut owner, &bytes(request(1)), 0);
    assert_eq!(
        owner.poll(time(100)).unwrap(),
        StationEvent::DecisionTimedOut { id }
    );
    let transmission = owner.transmission().unwrap();
    assert_ne!(transmission.id, id);
    let tx = transmission.id;
    assert_eq!(
        BtmResponse::parse(transmission.body).unwrap().status,
        BtmStatus::REJECT_NO_SUITABLE_CANDIDATES
    );
    owner.admitted(tx, time(101)).unwrap();
    assert_eq!(
        owner.tx_completed(tx, Acknowledged, time(102)).unwrap(),
        StationEvent::ResponseSent {
            id: tx,
            status: BtmStatus::REJECT_NO_SUITABLE_CANDIDATES
        }
    );
}

#[test]
fn tbtt_count_and_supplied_termination_mapping_bound_the_exchange() {
    let mut owner =
        BtmStation::<128>::new(ap(), Duration::from_secs(1), Duration::from_secs(1)).unwrap();
    let mut request = request(1);
    request.mode = request.mode.union(BtmRequestMode::DISASSOCIATION_IMMINENT);
    request.disassociation_timer = 3;
    let StationEvent::Proposal {
        id,
        timing: proposal_timing,
        ..
    } = owner
        .receive(
            ap(),
            &bytes(request),
            time(100),
            BssTiming {
                beacon_interval_tu: 10,
                next_tbtt: time(200),
                termination_deadline: None,
            },
        )
        .unwrap()
    else {
        panic!("missing proposal");
    };
    assert_eq!(proposal_timing.disassociation_deadline, Some(time(20_680)));
    assert_eq!(proposal_timing.candidates_valid_until, time(10_340));
    assert_eq!(proposal_timing.decision_deadline, time(20_680));
    assert_eq!(
        owner.poll(time(20_680)).unwrap(),
        StationEvent::TimedOut { id }
    );
    request.mode = request.mode.union(BtmRequestMode::TERMINATION);
    request.termination = Some(BssTerminationDuration {
        tsf: 123,
        duration_minutes: 2,
    });
    assert_eq!(
        owner.receive(ap(), &bytes(request), time(100), timing(100)),
        Err(Error::MissingTerminationDeadline)
    );
    let StationEvent::Proposal {
        timing: proposal_timing,
        ..
    } = owner
        .receive(
            ap(),
            &bytes(request),
            time(100),
            BssTiming {
                termination_deadline: Some(time(300)),
                ..timing(100)
            },
        )
        .unwrap()
    else {
        panic!("missing proposal");
    };
    assert_eq!(proposal_timing.termination_deadline, Some(time(300)));
    assert_eq!(proposal_timing.decision_deadline, time(300));
}

#[test]
fn capacity_clock_and_unknown_mode_errors_preserve_existing_proposal() {
    let mut owner = station::<32>();
    let id = proposal(&mut owner, &bytes(request(1)), 0);
    let original = bytes(owner.proposal().unwrap().1);
    let mut extended = request(2);
    let reports = [REPORT, &[221, 20][..], &[1; 20][..]].concat();
    extended.elements = Elements::parse(&reports).unwrap();
    assert!(matches!(
        owner.receive(ap(), &bytes(extended), time(1), timing(1)),
        Err(Error::FrameTooLarge { .. })
    ));
    extended = request(2);
    extended.mode = BtmRequestMode(0x81);
    assert_eq!(
        owner.receive(ap(), &bytes(extended), time(1), timing(1)),
        Err(Error::UnsupportedRequestMode(0x81))
    );
    assert_eq!(
        owner.receive(
            ap(),
            &bytes(request(2)),
            time(1),
            BssTiming {
                next_tbtt: time(0),
                ..timing(1)
            }
        ),
        Err(Error::InvalidBeaconTiming)
    );
    assert_eq!(bytes(owner.proposal().unwrap().1), original);
    assert_eq!(owner.proposal().unwrap().0, id);
    let too_large = BtmDecision::Reject {
        status: BtmStatus::REJECT_UNSPECIFIED,
        termination_delay_minutes: 0,
        elements: Elements::parse(&reports).unwrap(),
    };
    assert!(matches!(
        owner.decide(id, too_large, time(1)),
        Err(Error::FrameTooLarge { .. })
    ));
    assert_eq!(owner.proposal().unwrap().0, id);
    owner.cancel();
    assert_eq!(
        owner.query(
            time(u64::MAX),
            BtmQuery {
                dialog_token: 1,
                reason: 0,
                elements: Elements::EMPTY
            }
        ),
        Err(Error::TimeOverflow)
    );
    assert!(owner.transmission().is_none());
}

#[test]
fn ap_ignores_unadmitted_stale_and_wrong_token_responses() {
    let mut owner = BtmAccessPoint::<128>::new(sta(), Duration::from_micros(100)).unwrap();
    let id = owner.request(time(0), request(1)).unwrap();
    let response = [10, 8, 1, 1, 0];
    assert_eq!(
        owner.receive(sta(), &response, time(1)).unwrap(),
        AccessPointEvent::Ignored
    );
    owner.admitted(id, time(1)).unwrap();
    assert_eq!(
        owner
            .receive(
                LinkIdentity {
                    generation: 3,
                    ..sta()
                },
                &response,
                time(2)
            )
            .unwrap(),
        AccessPointEvent::Ignored
    );
    assert_eq!(
        owner.receive(ap(), &response, time(2)).unwrap(),
        AccessPointEvent::Ignored
    );
    assert_eq!(
        owner.receive(sta(), &[10, 8, 2, 1, 0], time(2)).unwrap(),
        AccessPointEvent::Ignored
    );
    assert!(owner.receive(sta(), &[10, 8, 1, 0, 0], time(2)).is_err());
    assert_eq!(owner.next_deadline(), Some(time(100)));
    assert_eq!(owner.poll(time(100)).unwrap(), AccessPointEvent::TimedOut);
}

#[test]
fn sta_ignores_foreign_epochs_and_zero_timer_has_no_scheduled_disassociation() {
    let mut owner = station::<128>();
    let request = bytes(request(1));
    assert_eq!(
        owner.receive(sta(), &request, time(0), timing(0)).unwrap(),
        StationEvent::Ignored
    );
    assert_eq!(
        owner
            .receive(
                LinkIdentity {
                    generation: 3,
                    ..ap()
                },
                &request,
                time(0),
                timing(0)
            )
            .unwrap(),
        StationEvent::Ignored
    );
    assert!(owner.proposal().is_none());
    let mut request = BtmRequest::parse(&request).unwrap();
    request.mode = request.mode.union(BtmRequestMode::DISASSOCIATION_IMMINENT);
    let id = proposal(&mut owner, &bytes(request), 1);
    assert_eq!(owner.proposal().unwrap().2.disassociation_deadline, None);
    assert!(owner.decide(id, accept(), time(2)).is_ok());
}

#[test]
fn scans_respect_live_exclusions_and_abridged_candidate_lists() {
    let mut owner = station::<128>();
    let mut excluded = REPORT.to_vec();
    excluded[17] = 0;
    let mut request = request(1);
    request.elements = Elements::parse(&excluded).unwrap();
    let id = proposal(&mut owner, &bytes(request), 0);
    let scanned = |target| BtmDecision::Accept {
        target,
        source: CandidateSource::Scan,
        elements: Elements::EMPTY,
    };
    assert_eq!(
        owner.decide(id, scanned(TARGET), time(1)),
        Err(Error::CandidateExcluded)
    );
    owner.cancel();
    request.mode = request.mode.union(BtmRequestMode::ABRIDGED);
    request.elements = Elements::parse(REPORT).unwrap();
    request.dialog_token = 2;
    let id = proposal(&mut owner, &bytes(request), 2);
    assert_eq!(
        owner.decide(id, scanned([2, 0, 0, 0, 0, 8]), time(3)),
        Err(Error::CandidateNotOffered)
    );
    assert!(owner.decide(id, scanned(TARGET), time(3)).is_ok());
    owner.cancel();
    request.validity_interval = 0;
    request.dialog_token = 3;
    let id = proposal(&mut owner, &bytes(request), 4);
    assert_eq!(
        owner.decide(id, scanned(TARGET), time(5)),
        Err(Error::CandidateExpired)
    );
}

#[test]
fn requests_without_a_candidate_list_do_not_emit_candidate_expiry() {
    let mut owner = station::<128>();
    let request = BtmRequest {
        dialog_token: 1,
        mode: BtmRequestMode::default(),
        disassociation_timer: 0,
        validity_interval: 0,
        termination: None,
        session_url: None,
        elements: Elements::EMPTY,
    };
    let id = proposal(&mut owner, &bytes(request), 0);
    assert_eq!(owner.next_deadline(), Some(time(100)));
    assert_eq!(owner.poll(time(1)).unwrap(), StationEvent::Ignored);
    assert!(
        owner
            .decide(
                id,
                BtmDecision::Accept {
                    target: TARGET,
                    source: CandidateSource::Scan,
                    elements: Elements::EMPTY
                },
                time(2)
            )
            .is_ok()
    );
}
