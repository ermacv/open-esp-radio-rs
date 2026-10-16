//! `cargo qualification check references` exits 130 when it is sent SIGINT
//! (#783 exit codes).

#![cfg(unix)]

use std::{fs, io::Write as _, process::Command};

use oer_hil_evidence_archive::{Manifest, Payload, Pin, Repository, RunId, RunRef, Sha256};

#[test]
fn check_references_exits_130_on_sigint() {
    let root = tempfile::tempdir().unwrap();
    let programs = root.path().join("qualification/targets");
    fs::create_dir_all(&programs).unwrap();
    // Never read: the check stops before its first program.
    fs::write(programs.join("a.toml"), "").unwrap();
    let pin = root.path().join(oer_hil_evidence_archive::PIN);
    fs::create_dir_all(pin.parent().unwrap()).unwrap();
    assert!(Command::new("mkfifo").arg(&pin).status().unwrap().success());
    let mut child = Command::new(env!("CARGO_BIN_EXE_oer-qualification"))
        .args(["check", "references", "--root"])
        .arg(root.path())
        .spawn()
        .unwrap();
    // Opening the pin to write waits until the check opens it to read: its
    // signal handlers are installed by then, and it blocks reading the pin.
    let mut writer = fs::OpenOptions::new().write(true).open(&pin).unwrap();
    assert!(
        Command::new("kill")
            .args(["-INT", &child.id().to_string()])
            .status()
            .unwrap()
            .success()
    );
    let manifest = Manifest::new(
        Payload {
            sha256: Sha256::of(b"payload"),
            bytes: 1,
        },
        vec![RunRef {
            run: RunId::parse("1-a").unwrap(),
            sha256: Sha256::of(b"seal"),
        }],
    )
    .unwrap();
    let text = Pin::new(Repository::parse("o/evidence").unwrap(), manifest).to_toml();
    writer.write_all(text.as_bytes()).unwrap();
    drop(writer);
    assert_eq!(child.wait().unwrap().code(), Some(130));
}
