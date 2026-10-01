use super::*;
use crate::database::{ReportContext, ReportSource};
use oer_ieee80211_mac::{channel::ChannelWidth, roaming::Elements, scan::ScanRecord};

fn link() -> LinkIdentity {
    LinkIdentity {
        peer: [2, 0, 0, 0, 0, 1],
        generation: 1,
    }
}
fn now(us: u64) -> Instant {
    Instant::from_micros(us)
}
fn ttl(us: u64) -> Duration {
    Duration::from_micros(us)
}
fn channel() -> Channel {
    Channel::ghz2_4(6, ChannelWidth::Mhz20).unwrap()
}
fn ssid() -> WifiSsid {
    WifiSsid::new(b"a\0b").unwrap()
}
fn network() -> NetworkProfile {
    NetworkProfile {
        ssid: ssid(),
        security: StaSecurityPolicy::Open,
    }
}
fn policy() -> SelectionPolicy {
    SelectionPolicy {
        ghz2_4: BandPolicy {
            minimum_rssi_dbm: -80,
            sufficient_rssi_dbm: -60,
        },
        ghz5: BandPolicy {
            minimum_rssi_dbm: -77,
            sufficient_rssi_dbm: -60,
        },
        weights: ScoreWeights {
            rssi: 10,
            available_channel: 30,
            ap_preference: 20,
            throughput: 30,
            throughput_limit_kbps: 100_000,
        },
        minimum_score_improvement: 30,
        confirmation_time: ttl(2),
        cooldown: ttl(10),
        attempt_timeout: ttl(10),
        failure_threshold: 1,
        failure_block: ttl(5),
        maximum_failure_block: ttl(20),
        failure_memory: ttl(100),
        imminent_btm_bypasses_improvement: false,
    }
}
fn observation(id: u8, rssi: i8, utilization: Option<u8>) -> Observation {
    let mut record = ScanRecord {
        bssid: [2, 0, 0, 0, 0, id],
        channel: 6,
        rssi,
        ssid_len: 3,
        ..ScanRecord::EMPTY
    };
    record.ssid[..3].copy_from_slice(b"a\0b");
    let load = [11, 5, 1, 0, utilization.unwrap_or(0), 0, 0];
    Observation::new(
        record,
        channel(),
        if utilization.is_some() {
            Elements::parse(&load).unwrap()
        } else {
            Elements::EMPTY
        },
    )
    .unwrap()
}
fn database() -> NeighborDatabase<8, 64> {
    let mut db = NeighborDatabase::new(link());
    db.observe(link(), observation(1, -75, None), now(0), ttl(100))
        .unwrap();
    db
}
fn evaluate<const N: usize>(
    owner: &mut RoamingSelector<N>,
    db: &NeighborDatabase<8, 64>,
    at: u64,
) -> SelectionEvent {
    owner
        .evaluate(
            db,
            &[channel()],
            SelectionTrigger::Autonomous,
            now(at),
            |_| CandidateAssessment {
                phy_supported: true,
                estimated_throughput_kbps: None,
            },
        )
        .unwrap()
}
fn immediate<const N: usize>() -> RoamingSelector<N> {
    RoamingSelector::new(
        link(),
        network(),
        SelectionPolicy {
            confirmation_time: Duration::ZERO,
            ..policy()
        },
    )
    .unwrap()
}
fn selected(event: SelectionEvent) -> Selection {
    let SelectionEvent::Selected(value) = event else {
        panic!("expected selected: {event:?}")
    };
    value
}
fn report(id: u8, preference: u8) -> std::vec::Vec<u8> {
    std::vec![
        52, 16, 2, 0, 0, 0, 0, id, 0, 0, 0, 0, 81, 6, 7, 3, 1, preference
    ]
}

#[test]
fn report_only_stale_incompatible_and_disallowed_candidates_are_not_selected() {
    let mut db = database();
    db.update_reports(
        link(),
        Elements::parse(&report(2, 255)).unwrap(),
        ReportContext {
            source: ReportSource::NeighborResponse { token: 1 },
            ssid: Some(ssid()),
        },
        now(0),
        ttl(100),
    )
    .unwrap();
    let mut owner = immediate::<4>();
    assert_eq!(
        evaluate(&mut owner, &db, 0),
        SelectionEvent::ScanNeeded(ScanReason::AdvertisedCandidateUnobserved)
    );
    db.observe(link(), observation(2, -40, None), now(1), ttl(2))
        .unwrap();
    assert_eq!(
        owner
            .evaluate(&db, &[], SelectionTrigger::Autonomous, now(1), |_| {
                CandidateAssessment {
                    phy_supported: true,
                    estimated_throughput_kbps: None,
                }
            })
            .unwrap(),
        SelectionEvent::Stay(StayReason::NoEligibleCandidate)
    );
    assert_eq!(
        RoamingSelector::<4>::new(
            link(),
            NetworkProfile {
                security: StaSecurityPolicy::Wpa3Personal,
                ..network()
            },
            policy()
        )
        .unwrap()
        .evaluate(
            &db,
            &[channel()],
            SelectionTrigger::Autonomous,
            now(1),
            |_| CandidateAssessment {
                phy_supported: true,
                estimated_throughput_kbps: None
            }
        )
        .unwrap(),
        SelectionEvent::Stay(StayReason::NoEligibleCandidate)
    );
    assert_eq!(
        owner
            .evaluate(
                &db,
                &[channel()],
                SelectionTrigger::Autonomous,
                now(1),
                |_| CandidateAssessment {
                    phy_supported: false,
                    estimated_throughput_kbps: None
                }
            )
            .unwrap(),
        SelectionEvent::Stay(StayReason::NoEligibleCandidate)
    );
    assert_eq!(
        evaluate(&mut owner, &db, 3),
        SelectionEvent::ScanNeeded(ScanReason::AdvertisedCandidateUnobserved)
    );
}

#[test]
fn weighted_score_caps_signal_and_uses_real_load_and_optional_throughput() {
    let mut db = database();
    db.observe(link(), observation(2, -40, Some(255)), now(0), ttl(100))
        .unwrap();
    db.observe(link(), observation(3, -55, Some(0)), now(0), ttl(100))
        .unwrap();
    assert_eq!(
        selected(evaluate(&mut immediate::<4>(), &db, 0)).target[5],
        3
    );
    let decision = immediate::<4>()
        .evaluate(
            &db,
            &[channel()],
            SelectionTrigger::Autonomous,
            now(0),
            |observed| CandidateAssessment {
                phy_supported: true,
                estimated_throughput_kbps: (observed.record().bssid[5] == 2).then_some(100_000),
            },
        )
        .unwrap();
    assert_eq!(selected(decision).target[5], 2); // Equal score has a stable BSSID tie break.
}

#[test]
fn confirmation_needs_new_observation_and_resets_when_winner_changes() {
    let mut db = database();
    db.observe(link(), observation(2, -60, None), now(0), ttl(100))
        .unwrap();
    let mut owner = RoamingSelector::<4>::new(link(), network(), policy()).unwrap();
    assert_eq!(
        evaluate(&mut owner, &db, 0),
        SelectionEvent::Confirming {
            target: [2, 0, 0, 0, 0, 2],
            earliest: now(2)
        }
    );
    assert!(matches!(
        evaluate(&mut owner, &db, 2),
        SelectionEvent::Confirming { .. }
    ));
    db.observe(link(), observation(3, -60, Some(0)), now(3), ttl(100))
        .unwrap();
    assert_eq!(
        evaluate(&mut owner, &db, 3),
        SelectionEvent::Confirming {
            target: [2, 0, 0, 0, 0, 3],
            earliest: now(5)
        }
    );
    db.observe(link(), observation(3, -60, Some(0)), now(5), ttl(100))
        .unwrap();
    let choice = selected(evaluate(&mut owner, &db, 5));
    assert_eq!(choice.target[5], 3);
    assert_eq!(
        evaluate(&mut owner, &db, 6),
        SelectionEvent::InProgress { id: choice.id }
    );
}

#[test]
fn hysteresis_and_success_cooldown_prevent_repeated_transitions() {
    let mut db = database();
    db.observe(link(), observation(2, -73, None), now(0), ttl(100))
        .unwrap();
    let mut owner = immediate::<4>();
    assert_eq!(
        evaluate(&mut owner, &db, 0),
        SelectionEvent::Stay(StayReason::InsufficientImprovement)
    );
    db.observe(link(), observation(2, -65, None), now(1), ttl(100))
        .unwrap();
    let choice = selected(evaluate(&mut owner, &db, 1));
    owner.admit(choice.id, now(1)).unwrap();
    let replacement = LinkIdentity {
        peer: choice.target,
        generation: 2,
    };
    assert_eq!(
        owner
            .finish(
                choice.id,
                AssociationOutcome::Connected(replacement),
                now(2)
            )
            .unwrap(),
        SelectionEvent::Connected {
            id: choice.id,
            link: replacement
        }
    );
    assert_eq!(evaluate(&mut owner, &db, 3), SelectionEvent::Ignored);
    let mut db = NeighborDatabase::<8, 64>::new(replacement);
    db.observe(replacement, observation(2, -65, None), now(3), ttl(100))
        .unwrap();
    db.observe(replacement, observation(3, -55, None), now(3), ttl(100))
        .unwrap();
    assert_eq!(
        evaluate(&mut owner, &db, 3),
        SelectionEvent::Stay(StayReason::Cooldown)
    );
    assert!(matches!(
        evaluate(&mut owner, &db, 12),
        SelectionEvent::Selected(_)
    ));
    assert_eq!(
        owner
            .finish(choice.id, AssociationOutcome::Failed, now(13))
            .unwrap(),
        SelectionEvent::Ignored
    );
}

#[test]
fn failures_block_then_back_off_and_timeout_counts_only_admitted_attempts() {
    let mut db = database();
    db.observe(link(), observation(2, -60, None), now(0), ttl(100))
        .unwrap();
    let mut owner = immediate::<2>();
    let first = selected(evaluate(&mut owner, &db, 0));
    assert_eq!(
        owner.finish(first.id, AssociationOutcome::Failed, now(0)),
        Err(SelectionError::Protocol(Error::NotAdmitted))
    );
    owner.admit(first.id, now(0)).unwrap();
    owner
        .finish(first.id, AssociationOutcome::Failed, now(1))
        .unwrap();
    assert!(owner.blocked(first.target, now(5)).unwrap());
    assert!(!owner.blocked(first.target, now(6)).unwrap());
    let second = selected(evaluate(&mut owner, &db, 6));
    owner.admit(second.id, now(6)).unwrap();
    assert_eq!(
        owner
            .finish(first.id, AssociationOutcome::Failed, now(7))
            .unwrap(),
        SelectionEvent::Ignored
    );
    assert_eq!(
        owner.poll(now(16)).unwrap(),
        SelectionEvent::Expired { id: second.id }
    );
    assert!(owner.blocked(first.target, now(25)).unwrap());
    assert!(!owner.blocked(first.target, now(26)).unwrap());
    let third = selected(evaluate(&mut owner, &db, 26));
    assert_eq!(
        owner.poll(now(100)).unwrap(),
        SelectionEvent::Expired { id: third.id }
    );
    assert!(!owner.blocked(first.target, now(100)).unwrap());
}

#[test]
fn failure_capacity_and_clock_overflow_fail_before_retiring_pending_choice() {
    let mut db = database();
    db.observe(link(), observation(2, -60, None), now(0), ttl(100))
        .unwrap();
    let mut owner = immediate::<1>();
    let first = selected(evaluate(&mut owner, &db, 0));
    owner.admit(first.id, now(0)).unwrap();
    owner
        .finish(first.id, AssociationOutcome::Failed, now(1))
        .unwrap();
    db.observe(link(), observation(3, -60, Some(0)), now(2), ttl(100))
        .unwrap();
    let second = selected(evaluate(&mut owner, &db, 2));
    owner.admit(second.id, now(2)).unwrap();
    assert_eq!(
        owner.finish(second.id, AssociationOutcome::Failed, now(3)),
        Err(SelectionError::FailureTableFull)
    );
    assert_eq!(owner.pending().unwrap().id, second.id);
    assert_eq!(
        owner.cancel(second.id, now(1)),
        Err(SelectionError::Protocol(Error::TimeBeforeOperation))
    );
    owner.cancel(second.id, now(3)).unwrap();
    assert_eq!(owner.next_deadline(), Some(now(6)));
    let mut bad = policy();
    bad.maximum_failure_block = ttl(1);
    assert!(matches!(
        RoamingSelector::<1>::new(link(), network(), bad),
        Err(SelectionError::InvalidPolicy)
    ));
}

fn btm(elements: Elements<'_>, mode: BtmRequestMode) -> BtmRequest<'_> {
    BtmRequest {
        dialog_token: 1,
        mode,
        disassociation_timer: 0,
        validity_interval: 10,
        termination: None,
        session_url: None,
        elements,
    }
}
fn trigger(request: BtmRequest<'_>, valid_until: u64) -> SelectionTrigger<'_> {
    SelectionTrigger::Btm {
        proposal: OperationId {
            link: link(),
            serial: 12,
        },
        request,
        timing: ProposalTiming {
            candidates_valid_until: now(valid_until),
            decision_deadline: now(50),
            disassociation_deadline: None,
            termination_deadline: None,
        },
    }
}

#[test]
fn btm_exclusion_abridgement_expiry_and_duplicate_conflicts_are_enforced() {
    let mut db = database();
    db.observe(link(), observation(2, -50, None), now(0), ttl(100))
        .unwrap();
    db.observe(link(), observation(3, -60, None), now(0), ttl(100))
        .unwrap();
    let reports = [report(2, 0), report(3, 20)].concat();
    let request = btm(
        Elements::parse(&reports).unwrap(),
        BtmRequestMode::CANDIDATES.union(BtmRequestMode::ABRIDGED),
    );
    let run = |owner: &mut RoamingSelector<4>, trigger, at| {
        owner.evaluate(&db, &[channel()], trigger, now(at), |_| {
            CandidateAssessment {
                phy_supported: true,
                estimated_throughput_kbps: None,
            }
        })
    };
    let choice = selected(run(&mut immediate(), trigger(request, 10), 0).unwrap());
    assert_eq!(choice.target[5], 3);
    assert_eq!(choice.valid_until, now(10));
    assert!(choice.proposal.is_some());
    assert_eq!(
        run(&mut immediate(), trigger(request, 10), 10).unwrap(),
        SelectionEvent::Stay(StayReason::CandidateListExpired)
    );
    let duplicates = [report(2, 10), report(2, 20)].concat();
    assert_eq!(
        run(
            &mut immediate(),
            trigger(
                btm(
                    Elements::parse(&duplicates).unwrap(),
                    BtmRequestMode::CANDIDATES
                ),
                10
            ),
            0
        ),
        Err(SelectionError::Protocol(Error::ConflictingCandidates))
    );
    let request = BtmRequest {
        mode: BtmRequestMode::CANDIDATES,
        ..request
    };
    assert_eq!(
        selected(run(&mut immediate(), trigger(request, 10), 10).unwrap()).target[5],
        2
    );
}

#[test]
fn queued_choice_expires_without_a_failure_and_invalid_success_keeps_it_pending() {
    let mut db = database();
    db.observe(link(), observation(2, -60, None), now(0), ttl(3))
        .unwrap();
    let mut owner = immediate::<2>();
    let choice = selected(evaluate(&mut owner, &db, 0));
    assert_eq!(
        owner.admit(choice.id, now(3)),
        Err(SelectionError::Protocol(Error::CandidateExpired))
    );
    assert_eq!(
        owner.poll(now(3)).unwrap(),
        SelectionEvent::Expired { id: choice.id }
    );
    assert!(!owner.blocked(choice.target, now(3)).unwrap());
    db.observe(link(), observation(2, -60, None), now(4), ttl(100))
        .unwrap();
    let choice = selected(evaluate(&mut owner, &db, 4));
    owner.admit(choice.id, now(4)).unwrap();
    assert_eq!(
        owner.finish(choice.id, AssociationOutcome::Connected(link()), now(5)),
        Err(SelectionError::InvalidNewLink)
    );
    assert_eq!(owner.pending().unwrap().id, choice.id);
}
