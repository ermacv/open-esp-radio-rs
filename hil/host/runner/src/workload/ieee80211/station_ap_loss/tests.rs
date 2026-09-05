use super::*;

#[test]
fn parser_keeps_one_bounded_edge_deadline() {
    let options = parse_options(
        &["--timeout-seconds".into(), "75".into()],
        &LabConfig::for_test(),
    )
    .unwrap();
    assert_eq!(options.serial, PathBuf::from("/dev/ttyACM0"));
    assert_eq!(options.timeout, Duration::from_secs(75));
}

#[test]
fn link_policy_disconnect_cannot_qualify_beacon_loss() {
    let error = validate_event(
        StationLifecycleEvent::Disconnected {
            generation: 0,
            reason: StationDisconnectReason::LinkPolicy,
        },
        StationLifecycleEvent::Disconnected {
            generation: 0,
            reason: StationDisconnectReason::BeaconLoss,
        },
        "beacon loss",
    )
    .unwrap_err();
    assert!(error.to_string().contains("LinkPolicy"));
}
