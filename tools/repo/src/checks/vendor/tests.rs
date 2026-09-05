use super::*;
use std::collections::BTreeSet;

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
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("Cargo.toml"), "[workspace]\n").unwrap();
    let context = Context::new(directory.path()).unwrap();
    let mut roles = BTreeSet::new();
    let mut packages = BTreeSet::new();
    let mut outputs = BTreeSet::new();
    for probe in &PROBES {
        assert!(roles.insert(probe.role));
        assert!(packages.insert(probe.package));
        assert!(outputs.insert(probe.target_directory));
        let command = command(&context, probe, None);
        let arguments: Vec<_> = command.get_args().collect();
        assert!(
            arguments
                .windows(2)
                .any(|pair| pair == ["--package", probe.package])
        );
        assert!(
            arguments
                .windows(2)
                .any(|pair| pair == ["--target", "riscv32imafc-unknown-none-elf"])
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
    let command = command(&context, &PROBES[0], NonZeroUsize::new(3));
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
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("Cargo.toml"), "[workspace]\n").unwrap();
    let mut context = Context::new(directory.path()).unwrap();
    context.cargo = directory
        .path()
        .join("cargo-does-not-exist")
        .into_os_string();
    run(&context, "esp32s31", true).unwrap();
    assert!(!directory.path().join("target").exists());
    assert!(run(&context, "unsupported", true).is_err());
}

#[test]
fn builder_rejects_invalid_jobs_before_execution_and_stops_at_failed_artifact() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("Cargo.toml"), "[workspace]\n").unwrap();
    let context = Context::new(directory.path()).unwrap();
    let mut calls = 0;
    assert!(
        build(&context, Some(OsStr::new("0")), |_| {
            calls += 1;
            Ok(())
        })
        .is_err()
    );
    assert_eq!(calls, 0);
    assert!(
        build(&context, None, |_| {
            calls += 1;
            Err("compiled artifact failed".into())
        })
        .is_err()
    );
    assert_eq!(calls, 1);
}

#[test]
fn every_requested_artifact_receives_the_explicit_job_override() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("Cargo.toml"), "[workspace]\n").unwrap();
    let context = Context::new(directory.path()).unwrap();
    let mut calls = 0;
    build(&context, Some(OsStr::new("4")), |command| {
        let arguments: Vec<_> = command.get_args().collect();
        assert!(arguments.windows(2).any(|pair| pair == ["--jobs", "4"]));
        assert!(arguments.contains(&OsStr::new("--locked")));
        calls += 1;
        Ok(())
    })
    .unwrap();
    assert_eq!(calls, PROBES.len());
}

#[test]
fn declared_artifact_roles_cover_the_project_verification_consumers() {
    let context = Context::discover().unwrap();
    let project_root = context.root.join(PROJECT);
    let project: toml::Value =
        toml::from_str(&std::fs::read_to_string(project_root.join("vendor-project.toml")).unwrap())
            .unwrap();
    let addon = project["verification-addon"].as_str().unwrap();
    let verification: toml::Value =
        toml::from_str(&std::fs::read_to_string(project_root.join(addon)).unwrap()).unwrap();
    let consumed: BTreeSet<_> = verification["suites"]
        .as_array()
        .unwrap()
        .iter()
        .map(|suite| suite["rust-artifact-role"].as_str().unwrap())
        .collect();
    let declared: BTreeSet<_> = PROBES.iter().map(|probe| probe.role).collect();
    assert!(!consumed.is_empty());
    assert_eq!(declared, consumed);
}

#[test]
fn cached_probe_build_scripts_resolve_the_runtime_manifest_after_package_relocation() {
    let context = Context::discover().unwrap();
    let scratch = tempfile::tempdir().unwrap();
    let original = scratch.path().join("original");
    let relocated = scratch.path().join("relocated with spaces");
    std::fs::create_dir(&original).unwrap();
    std::fs::create_dir(&relocated).unwrap();
    std::fs::write(relocated.join("link.x"), "SECTIONS {}\n").unwrap();
    for package in ["elf", "register-elf", "bluetooth-elf"] {
        let executable = scratch
            .path()
            .join(format!("{package}{}", std::env::consts::EXE_SUFFIX));
        let compiler = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
        // Compile once with the old directory; run the same executable after
        // relocation, as Cargo does when its build-script cache is reusable.
        process::run(
            context
                .command(compiler)
                .args(["--edition=2024", "--crate-name", "linker_fixture"])
                .arg(
                    context
                        .root
                        .join(PROJECT)
                        .join("probes")
                        .join(package)
                        .join("build.rs"),
                )
                .arg("-o")
                .arg(&executable)
                .env("CARGO_MANIFEST_DIR", &original),
        )
        .unwrap();
        let output = process::capture(
            context
                .command(&executable)
                .env("CARGO_MANIFEST_DIR", &relocated),
        )
        .unwrap();
        let output = String::from_utf8(output.stdout).unwrap();
        let expected = format!(
            "cargo:rustc-link-arg=-T{}",
            relocated.join("link.x").display()
        );
        assert!(
            output.lines().any(|line| line == expected),
            "{package}: {output}"
        );
        assert!(
            !output.contains(&original.display().to_string()),
            "{package}: {output}"
        );
    }
}
