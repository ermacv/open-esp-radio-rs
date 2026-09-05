use super::super::test_support::{BSSID, LOCAL, authentication_response, deauthentication};
use super::*;
use open_esp_radio_ieee80211::station::StaDisconnectKind;

#[test]
fn authentication_runtime_owns_attempt_sequence_deadline_and_timeout_limit() {
    let mut runtime = StaAuthenticationRuntime::new(LOCAL, BSSID);
    let mut sequence = StaSequenceCounter::new(0x0ffe);

    for ordinal in 1..=STA_AUTHENTICATION_ATTEMPT_LIMIT {
        let attempt = runtime.begin_attempt(&mut sequence).unwrap();
        assert_eq!(attempt.ordinal, ordinal);
        assert_eq!(attempt.sequence_number, (0x0ffd + ordinal) & 0x0fff);
        assert_eq!(attempt.response_timeout_ms, STA_RESPONSE_TIMEOUT_MS);
        runtime.observe_received_frame().unwrap();
        let event = runtime.response_timed_out().unwrap();
        if ordinal < STA_AUTHENTICATION_ATTEMPT_LIMIT {
            assert_eq!(
                event,
                StaAuthenticationEvent::Retry {
                    attempt: ordinal,
                    failure: StaAuthenticationFailure::Timeout,
                    received_frames: 1,
                    total_received_frames: u32::from(ordinal),
                }
            );
        } else {
            assert_eq!(
                event,
                StaAuthenticationEvent::Failed {
                    attempts: ordinal,
                    failure: StaAuthenticationFailure::Timeout,
                    total_received_frames: u32::from(ordinal),
                }
            );
        }
    }
    assert_eq!(
        runtime.begin_attempt(&mut sequence),
        Err(StaAuthenticationRuntimeError::Terminal)
    );
}

#[test]
fn authentication_runtime_ignores_other_management_and_accepts_selected_peer() {
    let mut runtime = StaAuthenticationRuntime::new(LOCAL, BSSID);
    let mut sequence = StaSequenceCounter::new(7);
    let attempt = runtime.begin_attempt(&mut sequence).unwrap();
    runtime.observe_received_frame().unwrap();
    assert_eq!(
        runtime.observe_management_frame(&[0; 30]).unwrap(),
        StaAuthenticationEvent::Irrelevant
    );
    runtime.observe_received_frame().unwrap();
    assert_eq!(
        runtime
            .observe_management_frame(&authentication_response(0))
            .unwrap(),
        StaAuthenticationEvent::Authenticated {
            attempt: attempt.ordinal,
            total_received_frames: 2,
        }
    );
    assert_eq!(runtime.total_received_frames(), 2);
}

#[test]
fn authentication_runtime_retries_disconnect_but_not_status_rejection() {
    let mut runtime = StaAuthenticationRuntime::new(LOCAL, BSSID);
    let mut sequence = StaSequenceCounter::new(0);
    runtime.begin_attempt(&mut sequence).unwrap();
    runtime.observe_received_frame().unwrap();
    assert_eq!(
        runtime
            .observe_management_frame(&deauthentication(1))
            .unwrap(),
        StaAuthenticationEvent::Retry {
            attempt: 1,
            failure: StaAuthenticationFailure::PeerDisconnect(StaDisconnect {
                kind: StaDisconnectKind::Deauthentication,
                reason_code: 1,
            }),
            received_frames: 1,
            total_received_frames: 1,
        }
    );

    runtime.begin_attempt(&mut sequence).unwrap();
    runtime.observe_received_frame().unwrap();
    assert_eq!(
        runtime
            .observe_management_frame(&authentication_response(17))
            .unwrap(),
        StaAuthenticationEvent::Failed {
            attempts: 2,
            failure: StaAuthenticationFailure::Rejected { status_code: 17 },
            total_received_frames: 2,
        }
    );
}
