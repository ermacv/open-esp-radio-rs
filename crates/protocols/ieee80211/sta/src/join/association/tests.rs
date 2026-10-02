use super::super::test_support::{BSSID, LOCAL, association_response, deauthentication};
use super::*;
use oer_ieee80211_mac::station::StaDisconnectKind;
use oer_time::{Duration, Instant};

fn at(millis: u64) -> Instant {
    Instant::from_micros(millis * 1_000)
}

fn ms(millis: u64) -> Duration {
    Duration::from_micros(millis * 1_000)
}

#[test]
fn association_retry_schedule_is_finite_inside_vendor_deadline() {
    for ordinal in 1..=7 {
        assert_eq!(
            StaAssociationRetrySchedule::offset(ordinal),
            Some(ms(u64::from(ordinal - 1) * 160))
        );
    }
    assert_eq!(StaAssociationRetrySchedule::offset(0), None);
    assert_eq!(StaAssociationRetrySchedule::offset(8), None);
}

#[test]
fn association_runtime_owns_epoch_schedule_sequence_and_timeout() {
    let mut runtime = StaAssociationRuntime::new(LOCAL, BSSID, LinkProtection::Ccmp);
    let mut sequence = StaSequenceCounter::new(SequenceNumber::new(0x0ffc).unwrap());
    let mut attempts = std::vec::Vec::new();
    let mut timed_out_at = None;

    for millis in 0..=1_000 {
        match runtime.poll(at(millis), &mut sequence).unwrap() {
            StaAssociationPoll::Idle => {}
            StaAssociationPoll::Transmit(attempt) => attempts.push((millis, attempt)),
            StaAssociationPoll::Failed {
                failure,
                total_received_frames,
            } => {
                assert_eq!(failure, StaAssociationFailure::Timeout);
                assert_eq!(total_received_frames, 1_000);
                timed_out_at = Some(millis);
                break;
            }
        }
        runtime.observe_received_frame().unwrap();
    }

    assert_eq!(timed_out_at, Some(1_000));
    assert_eq!(attempts.len(), 7);
    for (index, (millis, attempt)) in attempts.into_iter().enumerate() {
        assert_eq!(millis, index as u64 * 160);
        assert_eq!(attempt.ordinal, index as u16 + 1);
        assert_eq!(attempt.offset, ms(index as u64 * 160));
        assert_eq!(
            attempt.sequence_number,
            SequenceNumber::new(0x0ffc)
                .unwrap()
                .wrapping_add(index as u16)
        );
    }
    assert_eq!(runtime.total_received_frames(), 1_000);
    assert_eq!(runtime.next_deadline(), None);
    assert_eq!(
        runtime.poll(at(1_001), &mut sequence),
        Err(StaAssociationRuntimeError::Terminal)
    );
}

#[test]
fn the_next_deadline_is_the_next_request_or_the_epoch_deadline() {
    let mut runtime = StaAssociationRuntime::new(LOCAL, BSSID, LinkProtection::Ccmp);
    let mut sequence = StaSequenceCounter::new(SequenceNumber::new(0).unwrap());
    assert_eq!(runtime.next_deadline(), None);
    assert!(matches!(
        runtime.poll(at(5), &mut sequence),
        Ok(StaAssociationPoll::Transmit(_))
    ));
    assert_eq!(runtime.next_deadline(), Some(at(165)));
    assert_eq!(
        runtime.poll(at(164), &mut sequence),
        Ok(StaAssociationPoll::Idle)
    );
    for request in 1..7 {
        assert!(matches!(
            runtime.poll(at(5 + request * 160), &mut sequence),
            Ok(StaAssociationPoll::Transmit(_))
        ));
    }
    // After the seventh request only the epoch's deadline remains.
    assert_eq!(runtime.next_deadline(), Some(at(1_005)));
}

#[test]
fn association_runtime_accepts_only_selected_peer_response() {
    let mut runtime = StaAssociationRuntime::new(LOCAL, BSSID, LinkProtection::Ccmp);
    let mut sequence = StaSequenceCounter::new(SequenceNumber::new(7).unwrap());
    let StaAssociationPoll::Transmit(first) = runtime.poll(at(0), &mut sequence).unwrap() else {
        panic!("the first poll requests");
    };
    assert_eq!(first.ordinal, 1);
    runtime.observe_received_frame().unwrap();

    let mut other_peer = association_response(0);
    other_peer[10] ^= 1;
    assert_eq!(
        runtime.observe_management_frame(&other_peer, at(1)),
        Ok(StaAssociationEvent::Irrelevant)
    );

    let response = AssociationResponse {
        capability_info: 0x0431,
        status_code: 0,
        association_id: 42,
        ht_capability: false,
        he_capability: false,
        he_operation: false,
        wmm: false,
        wmm_parameters: None,
        association_comeback_tu: None,
    };
    assert_eq!(
        runtime.observe_management_frame(&association_response(0), at(1)),
        Ok(StaAssociationEvent::Associated {
            response,
            total_received_frames: 1,
        })
    );
    assert_eq!(
        runtime.poll(at(2), &mut sequence),
        Err(StaAssociationRuntimeError::Terminal)
    );
}

#[test]
fn association_runtime_reports_peer_disconnect_and_rejection() {
    let mut sequence = StaSequenceCounter::new(SequenceNumber::new(0).unwrap());
    let mut disconnected = StaAssociationRuntime::new(LOCAL, BSSID, LinkProtection::Ccmp);
    disconnected.poll(at(0), &mut sequence).unwrap();
    disconnected.observe_received_frame().unwrap();
    assert_eq!(
        disconnected.observe_management_frame(&deauthentication(7), at(0)),
        Ok(StaAssociationEvent::Failed {
            failure: StaAssociationFailure::PeerDisconnect(StaDisconnect {
                kind: StaDisconnectKind::Deauthentication,
                reason_code: 7,
            }),
            total_received_frames: 1,
        })
    );

    let mut rejected = StaAssociationRuntime::new(LOCAL, BSSID, LinkProtection::Ccmp);
    rejected.poll(at(0), &mut sequence).unwrap();
    assert_eq!(
        rejected.observe_management_frame(&association_response(17), at(0)),
        Ok(StaAssociationEvent::Failed {
            failure: StaAssociationFailure::Rejected { status_code: 17 },
            total_received_frames: 0,
        })
    );
}

fn refused_temporarily(comeback_tu: u32) -> std::vec::Vec<u8> {
    let mut frame = association_response(30).to_vec();
    frame.extend_from_slice(&[56, 5, 3]);
    frame.extend_from_slice(&comeback_tu.to_le_bytes());
    frame
}

/// Poll every millisecond from `from` until the runtime asks to transmit,
/// returning the milliseconds that passed.
fn millis_until_attempt(
    runtime: &mut StaAssociationRuntime,
    sequence: &mut StaSequenceCounter,
    from: u64,
) -> (u64, StaAssociationAttempt) {
    for millis in 0..10_000 {
        match runtime.poll(at(from + millis), sequence).unwrap() {
            StaAssociationPoll::Transmit(attempt) => return (millis, attempt),
            StaAssociationPoll::Idle => {}
            failed => panic!("unexpected association poll: {failed:?}"),
        }
    }
    panic!("no attempt within ten seconds");
}

#[test]
fn a_temporary_refusal_comes_back_once_after_its_comeback_time() {
    let mut sequence = StaSequenceCounter::new(SequenceNumber::new(0).unwrap());
    let mut runtime = StaAssociationRuntime::new(LOCAL, BSSID, LinkProtection::Ccmp);
    let (_, first) = millis_until_attempt(&mut runtime, &mut sequence, 0);
    assert_eq!(first.ordinal, 1);
    // 1000 TUs, as hostapd names while it confirms the old association.
    assert_eq!(
        runtime.observe_management_frame(&refused_temporarily(1_000), at(0)),
        Ok(StaAssociationEvent::Irrelevant)
    );
    // (1000 + 100) TUs of 1024 us: 1126.4 ms, met by the first millisecond
    // poll at or after it.
    assert_eq!(
        runtime.next_deadline(),
        Some(Instant::from_micros(1_126_400))
    );
    let (waited, again) = millis_until_attempt(&mut runtime, &mut sequence, 1);
    assert_eq!(waited + 1, 1_127);
    assert_eq!(again.ordinal, 1);
    assert_eq!(again.offset, Duration::ZERO);
    // A second refusal ends the association.
    assert_eq!(
        runtime.observe_management_frame(&refused_temporarily(1_000), at(1_127)),
        Ok(StaAssociationEvent::Failed {
            failure: StaAssociationFailure::ComebackRefused { comeback_tu: 1_000 },
            total_received_frames: 0,
        })
    );
}

#[test]
fn a_long_or_unnamed_temporary_refusal_ends_the_association() {
    let mut sequence = StaSequenceCounter::new(SequenceNumber::new(0).unwrap());
    let mut long = StaAssociationRuntime::new(LOCAL, BSSID, LinkProtection::Ccmp);
    long.poll(at(0), &mut sequence).unwrap();
    assert_eq!(
        long.observe_management_frame(&refused_temporarily(5_001), at(0)),
        Ok(StaAssociationEvent::Failed {
            failure: StaAssociationFailure::ComebackRefused { comeback_tu: 5_001 },
            total_received_frames: 0,
        })
    );
    let mut unnamed = StaAssociationRuntime::new(LOCAL, BSSID, LinkProtection::Ccmp);
    unnamed.poll(at(0), &mut sequence).unwrap();
    assert_eq!(
        unnamed.observe_management_frame(&association_response(30), at(0)),
        Ok(StaAssociationEvent::Failed {
            failure: StaAssociationFailure::Rejected { status_code: 30 },
            total_received_frames: 0,
        })
    );
}
