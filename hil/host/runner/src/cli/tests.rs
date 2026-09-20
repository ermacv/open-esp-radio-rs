use super::*;

#[test]
fn source_snapshot_accepts_only_explicit_repeated_file_arguments() {
    let cli = Cli::try_parse_from([
        "cargo-hil",
        "image",
        "snapshot",
        "--source-include",
        "new.rs",
        "--source-include",
        "esp-hal:src/new.rs",
    ])
    .unwrap();
    let CliCommand::Image {
        command: ImageCommand::Snapshot { source_include },
    } = cli.command
    else {
        panic!("snapshot expected")
    };
    assert_eq!(source_include, ["new.rs", "esp-hal:src/new.rs"]);
}

#[test]
fn ap_scheduler_is_a_runtime_choice_and_can_reuse_the_same_firmware() {
    for (name, expected) in [
        (
            "rr",
            open_esp_radio_hil_protocol::WifiApScheduler::RrHtResponse24,
        ),
        (
            "deficit",
            open_esp_radio_hil_protocol::WifiApScheduler::DeficitHtResponse24,
        ),
    ] {
        let cli = Cli::try_parse_from([
            "cargo-hil",
            "run",
            "diagnostic-ap-mixed-tx-work",
            "--firmware-from",
            "sealed-run",
            "--ap-scheduler",
            name,
        ])
        .unwrap();
        let CliCommand::Run {
            ap_scheduler: Some(policy),
            ..
        } = cli.command
        else {
            panic!("explicit policy expected")
        };
        assert_eq!(
            open_esp_radio_hil_protocol::WifiApScheduler::from(policy),
            expected
        );
    }
    assert!(
        Cli::try_parse_from([
            "cargo-hil",
            "run",
            "diagnostic-ap-mixed-tx-work",
            "--ap-scheduler",
            "unknown"
        ])
        .is_err()
    );
}

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
fn campaign_validation_is_an_explicit_offline_operation() {
    let cli =
        Cli::try_parse_from(["cargo-hil", "run-plan", "target/hil/plan.json", "--check"]).unwrap();
    assert!(matches!(
        cli.command,
        CliCommand::RunPlan { check: true, .. }
    ));
    assert!(
        Cli::try_parse_from([
            "cargo-hil",
            "run-plan",
            "target/hil/plan.json",
            "--network",
            "owned-xarxa"
        ])
        .is_err()
    );
    assert!(
        Cli::try_parse_from([
            "cargo-hil",
            "plan",
            "boot-smoke",
            "--out",
            "target/hil/plan.json"
        ])
        .is_ok()
    );
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

#[test]
fn bluetooth_and_wifi_fixture_commands_share_one_namespace() {
    let cli = Cli::try_parse_from([
        "cargo-hil",
        "fixture",
        "bluetooth-check",
        "--adapter",
        "hci2",
    ])
    .unwrap();
    assert!(matches!(
        cli.command,
        CliCommand::Fixture {
            command: FixtureCommand::BluetoothCheck {
                adapter: crate::fixture::bluetooth::model::Adapter(2),
                dtm_version: crate::fixture::bluetooth::model::DtmVersion::V2,
            }
        }
    ));
    assert!(Cli::try_parse_from(["cargo-hil", "fixture", "install-host"]).is_err());
    let cli =
        Cli::try_parse_from(["cargo-hil", "fixture", "check", "station-udp-tx-he20"]).unwrap();
    assert!(matches!(
        cli.command,
        CliCommand::Fixture { command: FixtureCommand::Check { scenario } }
            if scenario == "station-udp-tx-he20"
    ));
}

#[test]
fn bluetooth_dtm_v1_requires_explicit_selection() {
    let args = ["cargo-hil", "fixture", "bluetooth-check"];
    assert!(matches!(
        Cli::try_parse_from(args).unwrap().command,
        CliCommand::Fixture {
            command: FixtureCommand::BluetoothCheck {
                dtm_version: crate::fixture::bluetooth::model::DtmVersion::V2,
                ..
            }
        }
    ));
    assert!(matches!(
        Cli::try_parse_from(args.into_iter().chain(["--dtm-version", "v1"]))
            .unwrap()
            .command,
        CliCommand::Fixture {
            command: FixtureCommand::BluetoothCheck {
                dtm_version: crate::fixture::bluetooth::model::DtmVersion::V1,
                ..
            }
        }
    ));
    for version in ["auto", "v0", "v3"] {
        assert!(Cli::try_parse_from(args.into_iter().chain(["--dtm-version", version])).is_err());
    }
}

#[test]
fn fixture_install_requires_a_finite_provider_and_preserves_the_net_alias() {
    use open_esp_radio_hil_runner::fixture_install::Provider;

    for (name, expected) in [
        ("linux-net", Provider::LinuxNet),
        ("linux-bluetooth", Provider::LinuxBluetooth),
    ] {
        let cli = Cli::try_parse_from([
            "cargo-hil",
            "fixture",
            "install",
            "--provider",
            name,
            "--dry-run",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            CliCommand::Fixture {
                command: FixtureCommand::Install {
                    provider,
                    dry_run: true,
                    ..
                }
            } if provider == expected
        ));
    }
    assert!(Cli::try_parse_from(["cargo-hil", "fixture", "install", "--dry-run"]).is_err());
    assert!(
        Cli::try_parse_from(["cargo-hil", "fixture", "install", "--provider", "auto"]).is_err()
    );
    assert!(
        Cli::try_parse_from([
            "cargo-hil",
            "fixture",
            "install-host",
            "--provider",
            "linux-bluetooth"
        ])
        .is_err()
    );
}

#[test]
fn connection_reset_fixture_has_a_bounded_hold_and_requires_a_peer() {
    let command = [
        "hil",
        "fixture",
        "bluetooth-connect-reset",
        "--peer",
        "30:ED:A0:F3:F6:D1",
    ];
    assert!(Cli::try_parse_from(command).is_ok());
    assert!(Cli::try_parse_from(command.into_iter().chain(["--hold-ms", "5000"])).is_ok());
    assert!(Cli::try_parse_from(command.into_iter().chain(["--hold-ms", "5001"])).is_err());
    assert!(Cli::try_parse_from(["hil", "fixture", "bluetooth-connect-reset"]).is_err());
}

#[test]
fn qualification_planning_has_an_explicit_scope_and_excludes_manual_selectors() {
    assert!(
        Cli::try_parse_from([
            "cargo-hil",
            "plan",
            "--qualification",
            "program.toml",
            "--capability",
            "wifi"
        ])
        .is_ok()
    );
    assert!(
        Cli::try_parse_from([
            "cargo-hil",
            "plan",
            "boot-smoke",
            "--qualification",
            "program.toml"
        ])
        .is_err()
    );
    for selector in [
        ["--tag", "smoke"],
        ["--proof", "wifi.maintenance.same-link"],
    ] {
        assert!(
            Cli::try_parse_from([
                "cargo-hil",
                "plan",
                "--qualification",
                "program.toml",
                selector[0],
                selector[1]
            ])
            .is_err()
        );
    }
    assert!(Cli::try_parse_from(["cargo-hil", "plan", "--capability", "wifi"]).is_err());
}
