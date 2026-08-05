//! Tests for linked-IR export argument handling.

use super::*;

#[test]
fn artifact_input_supports_legacy_paths_and_explicit_source_names() {
    assert_eq!(
        parse_artifact("vendor.a").unwrap(),
        IrArtifactInput {
            source: "primary".to_owned(),
            path: PathBuf::from("vendor.a"),
            explicitly_named: false,
        }
    );
    assert_eq!(
        parse_artifact("libphy=/tmp/vendor=archive.a").unwrap(),
        IrArtifactInput {
            source: "libphy".to_owned(),
            path: PathBuf::from("/tmp/vendor=archive.a"),
            explicitly_named: true,
        }
    );
    assert_eq!(
        parse_artifact("/tmp/vendor=archive.a").unwrap(),
        IrArtifactInput {
            source: "primary".to_owned(),
            path: PathBuf::from("/tmp/vendor=archive.a"),
            explicitly_named: false,
        }
    );
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
    assert!(validate_artifact_inputs(&[rom.clone(), libphy], &[]).unwrap());
    assert!(validate_artifact_inputs(&[rom.clone(), rom], &[]).is_err());
    assert!(
        validate_artifact_inputs(
            &[
                parse_artifact("rom.elf").unwrap(),
                parse_artifact("libphy.a").unwrap()
            ],
            &[],
        )
        .is_err()
    );
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
