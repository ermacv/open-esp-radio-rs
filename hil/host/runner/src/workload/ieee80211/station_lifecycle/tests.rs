use super::*;

#[test]
fn parses_bounded_station_reconnect_options() {
    let options = parse_options(
        &[
            "--timeout-seconds".into(),
            "120".into(),
            "--cycles".into(),
            "3".into(),
        ],
        &LabConfig::for_test(),
    )
    .unwrap();
    assert_eq!(options.serial, PathBuf::from("/dev/ttyACM0"));
    assert_eq!(options.timeout, Duration::from_secs(120));
    assert_eq!(options.cycles, 3);
    assert_eq!(options.boots, 1);
    assert_eq!(options.initial_hold, Duration::ZERO);
}

#[test]
fn rejects_unbounded_station_reconnect_timeout() {
    assert!(
        parse_options(
            &["--timeout-seconds".into(), "301".into()],
            &LabConfig::for_test()
        )
        .is_err()
    );
}

#[test]
fn rejects_unbounded_station_reconnect_cycles() {
    assert!(parse_options(&["--cycles".into(), "0".into()], &LabConfig::for_test()).is_err());
    assert!(parse_options(&["--cycles".into(), "9".into()], &LabConfig::for_test()).is_err());
}

#[test]
fn parses_and_bounds_reset_separated_boots() {
    let options = parse_options(&["--boots".into(), "30".into()], &LabConfig::for_test()).unwrap();
    assert_eq!(options.boots, 30);
    assert!(parse_options(&["--boots".into(), "0".into()], &LabConfig::for_test()).is_err());
    assert!(parse_options(&["--boots".into(), "101".into()], &LabConfig::for_test()).is_err());
}

#[test]
fn parses_and_bounds_initial_connected_hold() {
    let options = parse_options(
        &["--initial-hold-seconds".into(), "10".into()],
        &LabConfig::for_test(),
    )
    .unwrap();
    assert_eq!(options.initial_hold, Duration::from_secs(10));
    assert!(
        parse_options(
            &["--initial-hold-seconds".into(), "31".into()],
            &LabConfig::for_test()
        )
        .is_err()
    );
}

#[test]
fn missing_typed_completion_cannot_qualify_a_cycle() {
    assert!(!validate_cycle_completion(None, 2).unwrap());
}

#[test]
fn typed_completion_requires_every_owner_edge() {
    let mut incomplete = StationEpochEvidence::COMPLETE;
    incomplete.scan_owners_returned = false;
    assert!(validate_cycle_completion(Some(incomplete), 2).is_err());
    assert!(validate_cycle_completion(Some(StationEpochEvidence::COMPLETE), 2).unwrap());
}

#[test]
fn retryable_peer_failure_does_not_preempt_lifecycle_policy() {
    let retrying = StationLifecycleEvent::AttemptFailed {
        generation: 1,
        attempt: 1,
        stage: open_esp_radio_hil_protocol::StationFailureStage::Association,
        reason: StationAttemptFailureReason::PeerProtocol,
    };
    assert!(validate_cycle_event(retrying, 2).is_ok());
}

#[test]
fn fatal_or_exhausted_lifecycle_fails_immediately() {
    let fatal = StationLifecycleEvent::AttemptFailed {
        generation: 1,
        attempt: 1,
        stage: open_esp_radio_hil_protocol::StationFailureStage::Hardware,
        reason: StationAttemptFailureReason::ContractViolation,
    };
    assert!(validate_cycle_event(fatal, 2).is_err());
    let exhausted = StationLifecycleEvent::RetryExhausted {
        generation: 1,
        attempts: 3,
        stage: open_esp_radio_hil_protocol::StationFailureStage::CandidateSelection,
        reason: StationAttemptFailureReason::NoCandidate,
    };
    assert!(validate_cycle_event(exhausted, 2).is_err());
}

#[test]
fn cycle_requires_ordered_disconnect_and_next_generation_connect() {
    let mut progress = CycleProgress::default();
    assert!(
        progress
            .observe_lifecycle(StationLifecycleEvent::Connected { generation: 2 }, 1, 1,)
            .is_err()
    );

    progress
        .observe_lifecycle(
            StationLifecycleEvent::Disconnected {
                generation: 1,
                reason: open_esp_radio_hil_protocol::StationDisconnectReason::ReconnectRequested,
            },
            1,
            1,
        )
        .unwrap();
    progress
        .observe_lifecycle(StationLifecycleEvent::Connected { generation: 2 }, 1, 1)
        .unwrap();
    assert!(!progress.complete());
    progress.owners_complete = true;
    assert!(progress.complete());
}
