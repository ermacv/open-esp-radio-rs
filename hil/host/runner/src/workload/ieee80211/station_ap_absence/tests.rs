use super::*;

#[test]
fn another_attempt_cannot_qualify_retry_exhaustion() {
    let error = validate_event(
        StationLifecycleEvent::RetryExhausted {
            generation: 1,
            attempts: 2,
            stage: StationFailureStage::CandidateSelection,
            reason: StationAttemptFailureReason::NoCandidate,
        },
        StationLifecycleEvent::RetryExhausted {
            generation: 1,
            attempts: QUALIFIED_ATTEMPTS,
            stage: StationFailureStage::CandidateSelection,
            reason: StationAttemptFailureReason::NoCandidate,
        },
        "retry exhaustion",
    )
    .unwrap_err();
    assert!(error.to_string().contains("attempts: 2"));
}

#[test]
fn typed_configuration_preserves_workload_bounds() {
    assert!(
        Config {
            timeout: Duration::from_secs(301),
        }
        .validate()
        .is_err()
    );
}
