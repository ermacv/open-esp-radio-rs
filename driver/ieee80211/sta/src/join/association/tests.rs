use super::super::test_support::{BSSID, LOCAL, association_response, deauthentication};
use super::*;
use open_esp_radio_ieee80211::station::StaDisconnectKind;

#[test]
fn association_retry_schedule_is_finite_inside_vendor_deadline() {
    let mut attempts = [0_u16; 7];
    let mut count = 0;
    for elapsed_ms in 0..=STA_RESPONSE_TIMEOUT_MS {
        if let Some(attempt) = StaAssociationRetrySchedule::attempt_at(elapsed_ms) {
            attempts[count] = attempt;
            count += 1;
        }
    }
    assert_eq!(count, attempts.len());
    assert_eq!(attempts, [1, 2, 3, 4, 5, 6, 7]);
    assert_eq!(StaAssociationRetrySchedule::attempt_at(159), None);
    assert_eq!(StaAssociationRetrySchedule::attempt_at(1_000), None);
}

#[test]
fn association_runtime_owns_epoch_schedule_sequence_and_timeout() {
    let mut runtime = StaAssociationRuntime::new(LOCAL, BSSID, WifiSecurityMode::Wpa2Personal);
    let mut sequence = StaSequenceCounter::new(0x0ffc);
    let mut attempts = [StaAssociationAttempt {
        ordinal: 0,
        sequence_number: 0,
        elapsed_ms: 0,
    }; 7];
    let mut attempt_count = 0;

    loop {
        if let Some(attempt) = runtime.begin_tick(&mut sequence).unwrap() {
            attempts[attempt_count] = attempt;
            attempt_count += 1;
        }
        runtime.observe_received_frame().unwrap();
        match runtime.finish_tick().unwrap() {
            StaAssociationEvent::Irrelevant => {}
            StaAssociationEvent::Failed {
                failure,
                total_received_frames,
            } => {
                assert_eq!(failure, StaAssociationFailure::Timeout);
                assert_eq!(total_received_frames, STA_RESPONSE_TIMEOUT_MS);
                break;
            }
            event => panic!("unexpected association event: {event:?}"),
        }
    }

    assert_eq!(attempt_count, attempts.len());
    for (index, attempt) in attempts.into_iter().enumerate() {
        assert_eq!(attempt.ordinal, index as u16 + 1);
        assert_eq!(attempt.elapsed_ms, index as u32 * 160);
        assert_eq!(attempt.sequence_number, (0x0ffc + index as u16) & 0x0fff);
    }
    assert_eq!(runtime.elapsed_ms(), STA_RESPONSE_TIMEOUT_MS);
    assert_eq!(runtime.total_received_frames(), STA_RESPONSE_TIMEOUT_MS);
    assert_eq!(
        runtime.begin_tick(&mut sequence),
        Err(StaAssociationRuntimeError::Terminal)
    );
}

#[test]
fn association_runtime_accepts_only_selected_peer_response() {
    let mut runtime = StaAssociationRuntime::new(LOCAL, BSSID, WifiSecurityMode::Wpa2Personal);
    let mut sequence = StaSequenceCounter::new(7);
    assert_eq!(
        runtime.begin_tick(&mut sequence).unwrap().unwrap().ordinal,
        1
    );
    assert_eq!(
        runtime.begin_tick(&mut sequence),
        Err(StaAssociationRuntimeError::TickAlreadyActive)
    );
    runtime.observe_received_frame().unwrap();

    let mut other_peer = association_response(0);
    other_peer[10] ^= 1;
    assert_eq!(
        runtime.observe_management_frame(&other_peer),
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
    };
    assert_eq!(
        runtime.observe_management_frame(&association_response(0)),
        Ok(StaAssociationEvent::Associated {
            response,
            total_received_frames: 1,
        })
    );
    assert_eq!(
        runtime.finish_tick(),
        Err(StaAssociationRuntimeError::Terminal)
    );
}

#[test]
fn association_runtime_reports_peer_disconnect_and_rejection() {
    let mut sequence = StaSequenceCounter::new(0);
    let mut disconnected = StaAssociationRuntime::new(LOCAL, BSSID, WifiSecurityMode::Wpa2Personal);
    disconnected.begin_tick(&mut sequence).unwrap();
    disconnected.observe_received_frame().unwrap();
    assert_eq!(
        disconnected.observe_management_frame(&deauthentication(7)),
        Ok(StaAssociationEvent::Failed {
            failure: StaAssociationFailure::PeerDisconnect(StaDisconnect {
                kind: StaDisconnectKind::Deauthentication,
                reason_code: 7,
            }),
            total_received_frames: 1,
        })
    );

    let mut rejected = StaAssociationRuntime::new(LOCAL, BSSID, WifiSecurityMode::Wpa2Personal);
    rejected.begin_tick(&mut sequence).unwrap();
    assert_eq!(
        rejected.observe_management_frame(&association_response(17)),
        Ok(StaAssociationEvent::Failed {
            failure: StaAssociationFailure::Rejected { status_code: 17 },
            total_received_frames: 0,
        })
    );
}
