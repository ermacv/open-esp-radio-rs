use super::*;

#[test]
fn parses_monitor_controls() {
    let options = parse_options(
        &[
            "--channel".into(),
            "11".into(),
            "--monitor-seconds".into(),
            "5".into(),
            "--snapshot-length".into(),
            "512".into(),
        ],
        &LabConfig::for_test(),
    )
    .unwrap();
    assert_eq!(options.monitor_channel, Some(11));
    assert_eq!(options.monitor_duration, Duration::from_secs(5));
    assert_eq!(options.snapshot_length, 512);
}

#[test]
fn rejects_invalid_monitor_channel() {
    assert!(parse_options(&["--channel".into(), "14".into()], &LabConfig::for_test()).is_err());
}

#[test]
fn role_transition_is_exact() {
    let evidence = WifiRoleTransitionEvidence {
        previous: WifiRole::Station,
        current: WifiRole::Idle,
        generation: 3,
    };
    assert!(require_transition(evidence, WifiRole::Station, WifiRole::Idle).is_ok());
    assert!(require_transition(evidence, WifiRole::Idle, WifiRole::Station).is_err());
}
