#![cfg(unix)]
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::Path,
    process::Command,
    time::{Duration, Instant},
};

fn executable(path: &Path, text: &str) {
    fs::write(path, text).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}
fn fixture() -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    fs::create_dir_all(root.join("hil/schema")).unwrap();
    fs::write(root.join("Cargo.toml"), "[workspace]\n").unwrap();
    fs::write(
        root.join("hil/schema/observer-inputs.json"),
        r#"{"build":{"profile":"debug"}}"#,
    )
    .unwrap();
    executable(
        &root.join("cargo"),
        r#"#!/bin/sh
printf '%s\n' "$@" > "$FIXTURE_ROOT/cargo-args"
printf '%s' "${CARGO_NET_OFFLINE-unset}" > "$FIXTURE_ROOT/offline"
printf '{"reason":"compiler-artifact","target":{"name":"open-esp-radio-hil-runner"},"executable":"%s/runner"}\n' "$FIXTURE_ROOT"
"#,
    );
    executable(
        &root.join("runner"),
        r#"#!/bin/sh
if [ "$1" = --observer-build ]; then
    printf '{"schema":2,"resolved":{"nodes":[]}}\n'
    exit 0
fi
if [ "$1" = exit ]; then exit 37; fi
trap 'sleep 2; echo cleaned > "$FIXTURE_ROOT/cleaned"; exit 42' TERM
echo $$ > "$FIXTURE_ROOT/pid"
while :; do sleep 0.1; done
"#,
    );
    directory
}
fn wrapper(root: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_oer-xtask"));
    command
        .arg("--root")
        .arg(root)
        .arg("hil")
        .env("CARGO", root.join("cargo"))
        .env("FIXTURE_ROOT", root);
    command
}
#[test]
fn forwards_runner_exit_and_uses_locked_cargo_with_explicit_offline_only() {
    let directory = fixture();
    let root = directory.path();
    for offline in [false, true] {
        let mut command = wrapper(root);
        command.arg("exit").env_remove("CARGO_NET_OFFLINE");
        if offline {
            command.env("CARGO_NET_OFFLINE", "true");
        }
        assert_eq!(command.status().unwrap().code(), Some(37));
        let args = fs::read_to_string(root.join("cargo-args")).unwrap();
        assert!(args.lines().any(|a| a == "--locked"));
        assert!(!args.lines().any(|a| a == "--offline"));
        assert_eq!(
            fs::read_to_string(root.join("offline")).unwrap(),
            if offline { "true" } else { "unset" }
        );
        assert!(root.join("target/hil/current-observer.json").is_file());
    }
}
#[test]
fn cancellation_of_wrapper_pid_reaches_runner_and_waits_for_cleanup_status() {
    let directory = fixture();
    let root = directory.path();
    let mut child = wrapper(root).arg("wait").spawn().unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    while !root.join("pid").is_file() {
        assert!(Instant::now() < deadline, "runner never started");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        Command::new("kill")
            .args(["-TERM", &child.id().to_string()])
            .status()
            .unwrap()
            .success()
    );
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            panic!("wrapper did not propagate cancellation");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(status.code(), Some(42));
    assert_eq!(
        fs::read_to_string(root.join("cleaned")).unwrap(),
        "cleaned\n"
    );
}
