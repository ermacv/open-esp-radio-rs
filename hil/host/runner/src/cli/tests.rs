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
        command:
            ImageCommand::Snapshot {
                source_include,
                include_untracked,
            },
    } = cli.command
    else {
        panic!("snapshot expected")
    };
    assert_eq!(source_include, ["new.rs", "esp-hal:src/new.rs"]);
    assert!(!include_untracked);
}

#[test]
fn include_untracked_is_a_run_flag_that_replaying_firmware_excludes() {
    let run =
        Cli::try_parse_from(["cargo-hil", "run", "boot-smoke", "--include-untracked"]).unwrap();
    assert!(matches!(
        run.command,
        CliCommand::Run {
            include_untracked: true,
            ..
        }
    ));
    assert!(
        Cli::try_parse_from([
            "cargo-hil",
            "run",
            "boot-smoke",
            "--include-untracked",
            "--firmware-from",
            "sealed-run-1",
        ])
        .is_err()
    );
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
            scenarios,
            firmware_from,
            ..
        } => {
            assert_eq!(scenarios, ["station-udp-rx-ceiling"]);
            assert_eq!(firmware_from.as_deref(), Some("sealed-run-1"));
        }
        _ => panic!("parsed the wrong HIL command"),
    }
}

#[test]
fn run_takes_scenarios_in_order_and_requires_one() {
    let cli = Cli::try_parse_from(["cargo-hil", "run", "b", "a"]).unwrap();
    match cli.command {
        CliCommand::Run { scenarios, .. } => assert_eq!(scenarios, ["b", "a"]),
        _ => panic!("parsed the wrong HIL command"),
    }
    assert!(Cli::try_parse_from(["cargo-hil", "run"]).is_err());
}

#[test]
fn removed_commands_and_flags_are_rejected() {
    for removed in [
        &["cargo-hil", "run", "a", "--network", "owned-xarxa"][..],
        &["cargo-hil", "run", "a", "--ap-scheduler", "rr"],
        &["cargo-hil", "run-plan", "plan.json"],
        &["cargo-hil", "image", "flash", "correctness"],
        &["cargo-hil", "image", "replay", "run", "correctness"],
        &["cargo-hil", "image", "mono", "correctness"],
        &["cargo-hil", "image", "verify-rebuild", "correctness"],
        &["cargo-hil", "device", "status"],
        &["cargo-hil", "report", "rebuild"],
        &["cargo-hil", "archive", "verify", "a.tar.gz"],
        &["cargo-hil", "fixture", "probe-plan"],
        &["cargo-hil", "plan", "--out", "plan.json"],
    ] {
        assert!(Cli::try_parse_from(removed).is_err(), "{removed:?}");
    }
}

#[test]
fn run_all_does_not_accept_one_ambiguous_firmware_origin() {
    assert!(
        Cli::try_parse_from(["cargo-hil", "run-all", "--firmware-from", "sealed-run-1",]).is_err()
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
                adapter: hil_bluetooth::fixture::bluetooth::model::Adapter(2),
                dtm_version: hil_bluetooth::fixture::bluetooth::model::DtmVersion::V2,
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
                dtm_version: hil_bluetooth::fixture::bluetooth::model::DtmVersion::V2,
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
                dtm_version: hil_bluetooth::fixture::bluetooth::model::DtmVersion::V1,
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
    use oer_hil_fixture_install::Provider;

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
fn run_all_runs_the_whole_catalog_only_when_asked() {
    assert!(Cli::try_parse_from(["cargo-hil", "run-all"]).is_err());
    assert!(Cli::try_parse_from(["cargo-hil", "run-all", "--tag", "wifi"]).is_ok());
    assert!(Cli::try_parse_from(["cargo-hil", "run-all", "--all"]).is_ok());
}

#[test]
fn run_repetitions_replace_the_scenarios_counts_within_the_evidence_range() {
    let cli = Cli::try_parse_from(["cargo-hil", "run", "a", "--repetitions", "2"]).unwrap();
    let CliCommand::Run { repetitions, .. } = cli.command else {
        panic!("expected run");
    };
    assert_eq!(repetitions, Some(2));
    for out_of_range in ["0", "21"] {
        assert!(
            Cli::try_parse_from(["cargo-hil", "run", "a", "--repetitions", out_of_range]).is_err()
        );
    }
}

#[test]
fn a_build_only_run_builds_its_own_sources_and_is_no_replay() {
    let cli = Cli::try_parse_from(["cargo-hil", "run", "a", "--build-only"]).unwrap();
    let CliCommand::Run { build_only, .. } = cli.command else {
        panic!("expected run");
    };
    assert!(build_only);
    assert!(
        Cli::try_parse_from([
            "cargo-hil",
            "run",
            "a",
            "--build-only",
            "--firmware-from",
            "r"
        ])
        .is_err()
    );
}

#[test]
fn run_all_takes_repeated_exclusions() {
    let cli = Cli::try_parse_from([
        "cargo-hil",
        "run-all",
        "--all",
        "--exclude",
        "a",
        "--exclude",
        "b",
    ])
    .unwrap();
    let CliCommand::RunAll { exclude, .. } = cli.command else {
        panic!("expected run-all");
    };
    assert_eq!(exclude, ["a", "b"]);
}
