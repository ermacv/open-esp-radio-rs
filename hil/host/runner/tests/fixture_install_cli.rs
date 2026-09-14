//! Offline fixture installation grammar and output through the actual binary.

use std::{fs, os::unix::fs::PermissionsExt as _, process::Command};

fn trap(directory: &std::path::Path, name: &str) -> std::path::PathBuf {
    let path = directory.join(name);
    fs::write(&path, "#!/bin/sh\nexit 97\n").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    path
}

fn dry_run(arguments: &[&str]) -> std::process::Output {
    let directory = tempfile::tempdir().unwrap();
    let cargo = trap(directory.path(), "cargo-must-not-run");
    let _sudo = trap(directory.path(), "sudo");
    let invalid_lab = directory.path().join("private-lab.toml");
    fs::write(&invalid_lab, "this is not TOML = [").unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_open-esp-radio-hil-runner"));
    command
        .args(["--lab-config", invalid_lab.to_str().unwrap(), "fixture"])
        .args(arguments)
        .env("CARGO", cargo)
        .env(
            "PATH",
            format!("{}:/usr/bin:/bin", directory.path().display()),
        );
    command.output().unwrap()
}

#[test]
fn both_provider_plans_are_valid_json_without_build_sudo_or_lab_access() {
    for provider in ["linux-net", "linux-bluetooth"] {
        let first = dry_run(&["install", "--provider", provider, "--dry-run"]);
        assert!(
            first.status.success(),
            "{}",
            String::from_utf8_lossy(&first.stderr)
        );
        let plan: serde_json::Value = serde_json::from_slice(&first.stdout).unwrap();
        assert_eq!(plan["provider"], provider);
        assert_eq!(plan["automatic_hardware_checks"], false);
        let second = dry_run(&["install", "--provider", provider, "--dry-run"]);
        assert_eq!(first.stdout, second.stdout);
    }
}

#[test]
fn install_host_dry_run_is_exactly_the_linux_net_plan() {
    let canonical = dry_run(&["install", "--provider", "linux-net", "--dry-run"]);
    let alias = dry_run(&["install-host", "--dry-run"]);
    assert!(canonical.status.success());
    assert!(alias.status.success());
    assert_eq!(canonical.stdout, alias.stdout);
}

#[test]
fn missing_or_invalid_provider_fails_in_cli_before_any_effect() {
    for arguments in [
        vec!["install", "--dry-run"],
        vec!["install", "--provider", "all", "--dry-run"],
    ] {
        let output = dry_run(&arguments);
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
    }
}

#[test]
fn legacy_shell_entries_delegate_unprivileged_and_reject_old_sudo_shape() {
    let repository = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .unwrap();
    for (script, provider) in [
        ("hil/host/linux-net/install.sh", "linux-net"),
        ("hil/host/linux-bluetooth/install.sh", "linux-bluetooth"),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let arguments = directory.path().join("cargo-arguments");
        let cargo = directory.path().join("cargo");
        fs::write(
            &cargo,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" >'{}'\nexit 23\n",
                arguments.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&cargo, fs::Permissions::from_mode(0o700)).unwrap();
        let id = directory.path().join("id");
        fs::write(&id, "#!/bin/sh\ntest \"$1\" = -u\necho 1000\n").unwrap();
        fs::set_permissions(&id, fs::Permissions::from_mode(0o700)).unwrap();
        let output = Command::new(repository.join(script))
            .arg("--dry-run")
            .env(
                "PATH",
                format!("{}:/usr/bin:/bin", directory.path().display()),
            )
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(23));
        assert_eq!(
            fs::read_to_string(&arguments).unwrap(),
            format!("hil fixture install --provider {provider} --dry-run\n")
        );

        fs::write(&id, "#!/bin/sh\ntest \"$1\" = -u\necho 0\n").unwrap();
        let output = Command::new(repository.join(script))
            .env(
                "PATH",
                format!("{}:/usr/bin:/bin", directory.path().display()),
            )
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(64));
        assert!(String::from_utf8_lossy(&output.stderr).contains("do not run"));
    }
}
