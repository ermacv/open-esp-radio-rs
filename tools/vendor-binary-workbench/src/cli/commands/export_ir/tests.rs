//! Tests for linked-IR export argument handling.

use std::path::PathBuf;

use super::*;

#[test]
fn artifact_input_requires_explicit_source_names() {
    assert!(parse_artifact("vendor.a").is_err());
    assert_eq!(
        parse_artifact("libphy=/tmp/vendor=archive.a").unwrap(),
        IrArtifactInput {
            source: "libphy".to_owned(),
            path: PathBuf::from("/tmp/vendor=archive.a"),
        }
    );
    assert!(parse_artifact("/tmp/vendor=archive.a").is_err());
}

#[test]
fn artifact_source_ids_are_stable_machine_keys() {
    assert!(named_artifact("wifi-rom.v1", "rom.elf").is_ok());
    assert!(named_artifact("wifi/rom", "rom.elf").is_err());
    assert!(named_artifact("", "rom.elf").is_err());
}

#[test]
fn project_inputs_require_unique_explicit_sources_and_no_companions() {
    let rom = named_artifact("rom", "rom.elf").unwrap();
    let libphy = named_artifact("libphy", "libphy.a").unwrap();
    validate_artifact_inputs(&[rom.clone(), libphy], &[]).unwrap();
    assert!(validate_artifact_inputs(&[rom.clone(), rom], &[]).is_err());
    assert!(
        validate_artifact_inputs(
            &[
                named_artifact("rom", "rom.elf").unwrap(),
                named_artifact("libphy", "libphy.a").unwrap()
            ],
            &[PathBuf::from("rom-companion.elf")],
        )
        .is_err()
    );
}
