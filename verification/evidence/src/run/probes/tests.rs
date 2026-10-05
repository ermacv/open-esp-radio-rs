use super::*;
use std::collections::BTreeSet;

/// A temporary repository with the real chip profiles.
fn repository() -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("Cargo.toml"), "[workspace]\n").unwrap();
    let real = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../platform");
    for chip in ["esp32s31", "esp32c5"] {
        let profile = directory.path().join("platform").join(chip);
        std::fs::create_dir_all(&profile).unwrap();
        std::fs::copy(real.join(chip).join("chip.toml"), profile.join("chip.toml")).unwrap();
    }
    directory
}

#[test]
fn build_job_override_is_optional_and_rejects_nonpositive_or_nondecimal_values() {
    assert_eq!(parse_jobs(None).unwrap(), None);
    assert_eq!(
        parse_jobs(Some(OsStr::new("12"))).unwrap().unwrap().get(),
        12
    );
    for invalid in [
        "",
        "0",
        "01",
        "-1",
        "+1",
        "1.0",
        " 2",
        "2\n",
        "999999999999999999999999999999999",
    ] {
        assert!(
            parse_jobs(Some(OsStr::new(invalid))).is_err(),
            "accepted {invalid:?}"
        );
    }
}

#[test]
fn every_declared_role_builds_a_distinct_locked_embedded_package_and_target_directory() {
    let directory = repository();
    let context = Checkout::new(directory.path()).unwrap();
    let mut roles = BTreeSet::new();
    let mut packages = BTreeSet::new();
    let mut outputs = BTreeSet::new();
    for probe in &PROBES {
        assert!(!probe.chip.is_empty());
        if probe.chip == "esp32s31" {
            assert!(roles.insert(probe.role));
        }
        assert!(packages.insert(probe.package));
        assert!(outputs.insert(probe.target_directory));
        let target = oer_chip_profile::rust_target(&context.root, probe.chip).unwrap();
        let command = command(&context, probe, &target, None);
        let arguments: Vec<_> = command.get_args().collect();
        assert!(
            arguments
                .windows(2)
                .any(|pair| pair == ["--package", probe.package])
        );
        assert!(
            arguments
                .windows(2)
                .any(|pair| pair == ["--target", target.as_str()])
        );
        assert!(arguments.contains(&OsStr::new("--locked")));
        assert!(arguments.contains(&OsStr::new("--release")));
        assert!(!arguments.contains(&OsStr::new("--jobs")));
        assert!(
            command
                .get_envs()
                .any(|(key, value)| key == "CARGO_TARGET_DIR"
                    && value == Some(context.root.join(probe.target_directory).as_os_str()))
        );
    }
    assert_eq!(
        roles,
        BTreeSet::from([
            "rust-artifact",
            "rust-artifact:wifi-registers",
            "rust-artifact:bluetooth"
        ])
    );
    let command = command(&context, &PROBES[0], "t", NonZeroUsize::new(3));
    assert!(
        command
            .get_args()
            .collect::<Vec<_>>()
            .windows(2)
            .any(|pair| pair == ["--jobs", "3"])
    );
}

#[test]
fn role_listing_does_not_require_cargo_or_build_outputs() {
    let directory = repository();
    let context = Checkout::new(directory.path()).unwrap();
    run(&context, "esp32s31", true).unwrap();
    assert!(!directory.path().join("target").exists());
    assert!(run(&context, "unsupported", true).is_err());
}

#[test]
fn builder_rejects_invalid_jobs_before_execution_and_stops_at_failed_artifact() {
    let directory = repository();
    let context = Checkout::new(directory.path()).unwrap();
    let mut calls = 0;
    assert!(
        build(&context, "esp32s31", Some(OsStr::new("0")), |_| {
            calls += 1;
            Ok(())
        })
        .is_err()
    );
    assert_eq!(calls, 0);
    assert!(
        build(&context, "esp32s31", None, |_| {
            calls += 1;
            Err("compiled artifact failed".into())
        })
        .is_err()
    );
    assert_eq!(calls, 1);
}

#[test]
fn every_requested_artifact_receives_the_explicit_job_override() {
    let directory = repository();
    let context = Checkout::new(directory.path()).unwrap();
    let mut calls = 0;
    build(&context, "esp32s31", Some(OsStr::new("4")), |command| {
        let arguments: Vec<_> = command.get_args().collect();
        assert!(arguments.windows(2).any(|pair| pair == ["--jobs", "4"]));
        assert!(arguments.contains(&OsStr::new("--locked")));
        calls += 1;
        Ok(())
    })
    .unwrap();
    assert_eq!(calls, probes("esp32s31").count());
}

#[test]
fn each_chip_builds_its_own_workspace_for_its_instruction_set() {
    let directory = repository();
    let context = Checkout::new(directory.path()).unwrap();
    let mut seen = Vec::new();
    build(&context, "esp32c5", None, |command| {
        let arguments: Vec<_> = command.get_args().collect();
        assert!(
            arguments
                .windows(2)
                .any(|pair| pair == ["--target", "riscv32imac-unknown-none-elf"])
        );
        let manifest = arguments
            .windows(2)
            .find(|pair| pair[0] == "--manifest-path")
            .map(|pair| pair[1].to_owned())
            .unwrap();
        assert!(
            std::path::Path::new(&manifest).ends_with("verification/esp32c5/probes/Cargo.toml")
        );
        seen.push(());
        Ok(())
    })
    .unwrap();
    assert_eq!(seen.len(), 1);
    assert!(
        elf(&context, "oer-esp32c5-probe-radio-elf")
            .unwrap()
            .ends_with(
                "esp32c5-probes/riscv32imac-unknown-none-elf/release/oer-esp32c5-probe-radio-elf"
            )
    );
}
