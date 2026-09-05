use super::*;

#[test]
fn tcp_tx_defaults_separate_offer_from_acceptance_floor() {
    let options = parse_options(&[], &LabConfig::for_test(), Direction::Tx).unwrap();

    assert_eq!(options.chunk_bytes, 32_768);
    assert_eq!(options.tx_rate_bps, Some(60_000_000));
    assert_eq!(options.tx_floor_bps, Some(45_000_000));
    assert_eq!(options.rx_rate_bps, None);
}

#[test]
fn direction_rejects_an_inapplicable_flow_option() {
    assert!(
        parse_options(
            &["--tx-rate".into(), "1000000".into(),],
            &LabConfig::for_test(),
            Direction::Rx,
        )
        .is_err()
    );
}
