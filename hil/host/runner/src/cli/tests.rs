use super::*;

#[test]
fn network_defaults_and_aliases_match_across_firmware_commands() {
    use crate::image::Integration;

    for command in [
        vec!["cargo-hil", "run", "station-udp-tx-ceiling"],
        vec!["cargo-hil", "run-all"],
        vec!["cargo-hil", "image", "build", "performance"],
        vec!["cargo-hil", "image", "flash", "performance"],
    ] {
        for (argument, expected) in [
            (None, Integration::UpstreamXarxa),
            (Some("upstream"), Integration::UpstreamXarxa),
            (Some("upstream-xarxa"), Integration::UpstreamXarxa),
            (Some("udp-backpressure"), Integration::PatchedXarxa),
            (Some("patched-xarxa"), Integration::PatchedXarxa),
            (Some("upstream-smoltcp"), Integration::UpstreamSmoltcp),
            (Some("owned-xarxa"), Integration::OwnedXarxa),
        ] {
            let mut args = command.clone();
            if let Some(argument) = argument {
                args.extend(["--network", argument]);
            }
            let cli = Cli::try_parse_from(args).unwrap();
            let network = match cli.command {
                CliCommand::Run { network, .. } | CliCommand::RunAll { network, .. } => network,
                CliCommand::Image {
                    command:
                        ImageCommand::Build { network, .. } | ImageCommand::Flash { network, .. },
                } => network,
                _ => panic!("parsed the wrong firmware command"),
            };
            assert_eq!(network, expected);
        }
    }
}

#[test]
fn preflight_selection_is_unambiguous() {
    for command in ["plan", "doctor"] {
        assert!(Cli::try_parse_from(["cargo-hil", command, "timebase"]).is_ok());
        assert!(Cli::try_parse_from(["cargo-hil", command, "--tag", "system"]).is_ok());
        assert!(
            Cli::try_parse_from(["cargo-hil", command, "timebase", "--tag", "system"]).is_err()
        );
    }
}

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
            ..
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

#[test]
fn network_selection_is_explicit_and_cannot_relabel_replayed_firmware() {
    for args in [
        vec![
            "cargo-hil",
            "run",
            "station-udp-tx-ceiling",
            "--network",
            "patched-xarxa",
        ],
        vec![
            "cargo-hil",
            "image",
            "build",
            "performance",
            "--network",
            "upstream-xarxa",
        ],
        vec!["cargo-hil", "run-all", "--network", "patched-xarxa"],
    ] {
        assert!(Cli::try_parse_from(args).is_ok());
    }
    assert!(
        Cli::try_parse_from([
            "cargo-hil",
            "run",
            "station-udp-tx-ceiling",
            "--network",
            "patched-xarxa",
            "--firmware-from",
            "earlier-run"
        ])
        .is_err()
    );
    assert!(
        Cli::try_parse_from([
            "cargo-hil",
            "image",
            "build",
            "performance",
            "--network",
            "unknown"
        ])
        .is_err()
    );
}
