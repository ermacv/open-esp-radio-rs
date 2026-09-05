use super::*;

#[test]
fn parser_requires_output_and_bounds_capture() {
    let lab = LabConfig::for_test();
    assert!(parse_options(&[], &lab).is_err());
    let options = parse_options(
        &[
            "--output".into(),
            "capture.pcapng".into(),
            "--seconds".into(),
            "5".into(),
            "--channel".into(),
            "11".into(),
            "--snapshot-length".into(),
            "512".into(),
        ],
        &lab,
    )
    .unwrap();
    assert_eq!(options.output, PathBuf::from("capture.pcapng"));
    assert_eq!(options.duration, Duration::from_secs(5));
    assert_eq!(options.channel, Some(11));
    assert_eq!(options.snapshot_length, 512);
}

#[test]
fn parser_rejects_unbounded_values() {
    assert!(
        parse_options(
            &[
                "--output".into(),
                "capture.pcapng".into(),
                "--seconds".into(),
                "31".into(),
            ],
            &LabConfig::for_test()
        )
        .is_err()
    );
}
