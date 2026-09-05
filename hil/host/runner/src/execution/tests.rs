use super::*;

#[test]
fn io_failure_is_broken_but_a_scenario_assertion_is_failed() {
    let io = std::io::Error::from(std::io::ErrorKind::NotConnected);
    let evidence = ExecutionEvidence {
        measurements: Vec::new(),
        interrupted: false,
        failure: Some(classify(&io)),
    };
    assert_eq!(evidence.outcome(), Outcome::Broken);
    assert_eq!(evidence.failure.unwrap().kind, FailureKind::Infrastructure);
    let assertion: Box<dyn std::error::Error + Send + Sync> =
        "target failed the throughput criterion".into();
    let evidence = ExecutionEvidence {
        measurements: Vec::new(),
        interrupted: false,
        failure: Some(classify(&*assertion)),
    };
    assert_eq!(evidence.outcome(), Outcome::Failed);
    assert_eq!(evidence.failure.unwrap().kind, FailureKind::Scenario);
}

#[test]
fn operation_context_preserves_the_infrastructure_classification() {
    let source: Box<dyn std::error::Error + Send + Sync> =
        std::io::Error::from(std::io::ErrorKind::BrokenPipe).into();
    let error = crate::error::context("session configuration", source);
    let failure = classify(&*error);
    assert_eq!(failure.kind, FailureKind::Infrastructure);
    assert!(failure.message.starts_with("session configuration:"));
}

#[test]
fn fixture_setup_failure_is_broken_even_through_operation_context() {
    let error = crate::error::context(
        "prepare AP client",
        crate::fixture::Error::new("OpenWrt radio does not match the laboratory configuration")
            .into(),
    );
    let evidence = ExecutionEvidence {
        failure: Some(classify(&*error)),
        ..Default::default()
    };
    assert_eq!(evidence.outcome(), Outcome::Broken);
    assert_eq!(evidence.failure.unwrap().kind, FailureKind::Infrastructure);
}
