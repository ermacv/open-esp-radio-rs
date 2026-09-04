//! Machine diagnostics, composable packs, and the process-launch boundary.

use std::{fs, path::PathBuf, process::Command};

fn blobray() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_blobray-generic"));
    command.env_remove("RUST_LOG");
    command
}

fn generic_project() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/generic-project/vendor-project.toml")
}

#[test]
fn runtime_diagnostics_are_json_without_a_false_result_on_stdout() {
    let missing = std::env::temp_dir().join(format!(
        "blobray-missing-machine-manifest-{}.toml",
        std::process::id()
    ));
    let output = blobray()
        .args(["project", "files", "--project"])
        .arg(missing)
        .args(["--format", "json", "--diagnostic-format", "json", "--quiet"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let report: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(report["schema_version"], 1);
    assert!(
        report["diagnostic"]["code"]
            .as_str()
            .unwrap()
            .starts_with("blobray::")
    );
    assert!(
        report["diagnostic"]["message"]
            .as_str()
            .unwrap()
            .contains("cannot read")
    );
}

#[test]
fn cli_can_compose_multiple_ecosystems_in_one_validated_change() {
    let root = std::env::temp_dir().join(format!("blobray-multiple-packs-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let fixture = generic_project();
    let manifest = root.join("vendor-project.toml");
    let target = fixture.parent().unwrap().join("target.toml");
    fs::write(
        &manifest,
        format!(
            "schema = 4\nid = \"fixture\"\ntarget-spec = {:?}\n",
            target.to_str().unwrap()
        ),
    )
    .unwrap();
    for id in ["first", "second"] {
        fs::write(
            root.join(format!("{id}.toml")),
            format!("schema = 3\nid = \"{id}\"\nknowledge-packs = []\n"),
        )
        .unwrap();
    }
    let output = blobray()
        .args(["project", "configure", "--project"])
        .arg(&manifest)
        .arg("--ecosystem-pack")
        .arg(root.join("first.toml"))
        .arg("--ecosystem-pack")
        .arg(root.join("second.toml"))
        .args(["--format", "json"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        report["ecosystem_packs"],
        serde_json::json!(["first", "second"])
    );
    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn a_closed_stdout_consumer_does_not_panic_in_either_output_format() {
    use std::{
        os::{fd::OwnedFd, unix::net::UnixStream},
        process::Stdio,
    };

    for format in ["human", "json"] {
        // Close the reader before spawning to exercise EPIPE deterministically,
        // even when the complete report would fit in a pipe buffer.
        let (reader, writer) = UnixStream::pair().unwrap();
        drop(reader);
        let output = blobray()
            .args(["project", "files", "--project"])
            .arg(generic_project())
            .args(["--format", format, "--quiet"])
            .stdout(Stdio::from(OwnedFd::from(writer)))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{format}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[cfg(target_os = "linux")]
#[test]
fn resource_launcher_preserves_the_selected_host_and_argument_boundaries() {
    use std::os::unix::fs::PermissionsExt;

    let root = std::env::temp_dir().join(format!("blobray-host-launcher-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let host = root.join("selected host");
    fs::write(&host, "#!/bin/sh\nprintf '%s\\n' \"$@\"\n").unwrap();
    fs::set_permissions(&host, fs::Permissions::from_mode(0o700)).unwrap();
    for selected in [
        host.as_path(),
        std::path::Path::new("./selected host"),
        std::path::Path::new("selected host"),
    ] {
        let output =
            Command::new(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("scripts/run-limited"))
                .current_dir(&root)
                .env("BLOBRAY_BINARY", selected)
                .env("BLOBRAY_LIMIT_BACKEND", "watchdog")
                .env("BLOBRAY_REPORT_USAGE", "0")
                .args(["--format", "json", "a path with spaces"])
                .output()
                .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(output.stdout, b"--format\njson\na path with spaces\n");
    }
    fs::remove_dir_all(root).unwrap();
}
