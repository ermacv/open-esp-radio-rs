use super::*;
use std::path::Path;

#[test]
fn command_requires_one_explicit_manifest() {
    let parsed = parse_arguments([
        "evaluate".to_owned(),
        "--manifest".to_owned(),
        "qualification/test.toml".to_owned(),
    ])
    .unwrap();
    assert_eq!(parsed.command, Command::Evaluate);
    assert_eq!(parsed.manifest, Path::new("qualification/test.toml"));
    assert!(parsed.json_report.is_none());
}

#[test]
fn removed_check_command_is_rejected() {
    let error = parse_arguments([
        "check".to_owned(),
        "--manifest".to_owned(),
        "qualification/test.toml".to_owned(),
    ])
    .unwrap_err();
    assert!(error.to_string().contains("unknown qualification command"));
}

#[test]
fn command_rejects_silent_extra_options() {
    let error = parse_arguments([
        "gate".to_owned(),
        "--manifest".to_owned(),
        "test.toml".to_owned(),
        "--best-effort".to_owned(),
    ])
    .unwrap_err();
    assert!(error.to_string().contains("unknown option"));
}
