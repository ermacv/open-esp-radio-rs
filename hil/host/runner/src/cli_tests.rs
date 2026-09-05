use super::*;

#[test]
fn run_firmware_from_is_an_explicit_single_scenario_input() {
    let cli = Cli::try_parse_from([
        "cargo-hil",
        "run",
        "station-udp-rx-ceiling",
        "--firmware-from",
        "sealed-run-1",
    ])
    .unwrap();
    match cli.command {
        CliCommand::Run {
            scenario,
            firmware_from,
        } => {
            assert_eq!(scenario, "station-udp-rx-ceiling");
            assert_eq!(firmware_from.as_deref(), Some("sealed-run-1"));
        }
        _ => panic!("parsed the wrong HIL command"),
    }
}

#[test]
fn run_all_does_not_accept_one_ambiguous_firmware_origin() {
    assert!(
        Cli::try_parse_from(["cargo-hil", "run-all", "--firmware-from", "sealed-run-1",]).is_err()
    );
}

#[test]
fn reproducible_rebuild_is_an_explicit_image_operation() {
    let cli = Cli::try_parse_from(["cargo-hil", "image", "verify-rebuild", "performance"]).unwrap();
    match cli.command {
        CliCommand::Image {
            command: ImageCommand::VerifyRebuild { class, trim_paths },
        } => {
            assert_eq!(class, crate::image::ImageClass::Performance);
            assert!(!trim_paths);
        }
        _ => panic!("parsed the wrong HIL command"),
    }
}

#[test]
fn path_trimming_is_explicit_and_diagnostic() {
    let cli = Cli::try_parse_from([
        "cargo-hil",
        "image",
        "verify-rebuild",
        "performance",
        "--trim-paths",
    ])
    .unwrap();
    match cli.command {
        CliCommand::Image {
            command: ImageCommand::VerifyRebuild { trim_paths, .. },
        } => assert!(trim_paths),
        _ => panic!("parsed the wrong HIL command"),
    }
}
