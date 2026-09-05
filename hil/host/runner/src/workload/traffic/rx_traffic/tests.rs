use super::*;

#[test]
fn parses_rx_options_and_rates() {
    assert_eq!(parse_rate("20M").unwrap(), 20_000_000);
    let lab = test_lab_config();
    let options = parse_options(
        &["--rate".into(), "40M".into(), "--phy".into(), "ht40".into()],
        &lab,
    )
    .unwrap();
    assert_eq!(options.rate_bps, 40_000_000);
    assert_eq!(options.expected_rx_format, 2);
    let guarded = parse_options(
        &["--max-idle-channel-utilization-255".into(), "64".into()],
        &lab,
    )
    .unwrap();
    assert_eq!(guarded.maximum_idle_channel_utilization_255, Some(64));
    assert!(
        parse_options(
            &["--max-idle-channel-utilization-255".into(), "0".into(),],
            &lab,
        )
        .is_err()
    );
    let ht20 = parse_options(&["--phy".into(), "ht20".into()], &lab).unwrap();
    assert_eq!(ht20.expected_rx_format, 2);
    assert_eq!(ht20.phy, PhyExpectation::Ht20);
}

fn test_lab_config() -> LabConfig {
    LabConfig::for_test()
}
