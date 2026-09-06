use std::{fs, io::Write, path::Path};

use clap::Parser;
use flate2::{Compression, write::GzEncoder};
use serde_json::json;

use super::*;

fn fixture(root: &Path, id: &str) {
    let run = root.join("target/hil/esp32s31/runs").join(id);
    fs::create_dir_all(&run).unwrap();
    let manifest = json!({
        "schema": 2, "run_id": id, "target": "esp32s31", "state": "interrupted",
        "started_unix_millis": 1, "finished_unix_millis": 2, "duration_millis": 1,
        "invocation": ["cargo hil run fixture"],
        "repository": {"commit": "fixture", "dirty": true, "workspace_sha256": "fixture"},
        "runner": {"package": "fixture", "version": "1", "protocol_version": 80,
            "host_os": "test", "host_arch": "test", "tools": []},
        "cell": {"cell_id": "fixture", "device_id": "fixture", "serial_device": "unused"},
        "firmware": []
    });
    fs::write(
        run.join("manifest.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    fs::write(run.join("uart.log"), b"partial observation\n").unwrap();
    seal(&run, id);
}

fn seal(run: &Path, id: &str) {
    let files: Vec<_> = package::inventory(run, Path::new(""))
        .unwrap()
        .into_iter()
        .filter(|f| f.path != "integrity.json")
        .collect();
    fs::write(
        run.join("integrity.json"),
        serde_json::to_vec(&json!({
            "schema": 2, "run_id": id, "files": files
        }))
        .unwrap(),
    )
    .unwrap();
}

#[test]
fn export_import_preserves_interrupted_evidence_and_analysis_without_origin() {
    let source = tempfile::tempdir().unwrap();
    let output = tempfile::tempdir().unwrap();
    fixture(source.path(), "run-1");
    let notes = source.path().join("analysis");
    fs::create_dir(&notes).unwrap();
    fs::write(notes.join("method.json"), b"{\"qualification\": false}").unwrap();
    let path = output.path().join("evidence.tar.gz");
    let hash = package::export(
        source.path(),
        "experiment-1",
        vec!["run-1".into()],
        Some(&notes),
        &path,
    )
    .unwrap();
    source.close().unwrap();
    let verified = package::open(&path, Some(&hash)).unwrap();
    let destination = tempfile::tempdir().unwrap();
    let installed = install::install(destination.path(), &verified).unwrap();
    assert_eq!(
        fs::read(installed.join("supplement/method.json")).unwrap(),
        b"{\"qualification\": false}"
    );
    crate::evidence::verify::verify(destination.path(), TARGET, Some("run-1")).unwrap();
    assert_eq!(
        install::install(destination.path(), &verified).unwrap(),
        installed
    );
    assert!(package::open(&path, Some(&"0".repeat(64))).is_err());
}

#[test]
fn export_is_deterministic_and_does_not_replace_existing_archive() {
    let source = tempfile::tempdir().unwrap();
    fixture(source.path(), "run-1");
    let a = source.path().join("a.tar.gz");
    let b = source.path().join("b.tar.gz");
    let one = package::export(source.path(), "experiment", vec!["run-1".into()], None, &a).unwrap();
    let two = package::export(source.path(), "experiment", vec!["run-1".into()], None, &b).unwrap();
    assert_eq!(one, two);
    assert_eq!(fs::read(&a).unwrap(), fs::read(&b).unwrap());
    assert!(package::export(source.path(), "experiment", vec!["run-1".into()], None, &a).is_err());
    fs::write(
        source
            .path()
            .join("target/hil/esp32s31/runs/run-1/uart.log"),
        b"changed",
    )
    .unwrap();
    assert!(
        package::export(
            source.path(),
            "changed",
            vec!["run-1".into()],
            None,
            &source.path().join("c.tar.gz")
        )
        .is_err()
    );
}

#[test]
fn conflicting_import_leaves_all_existing_evidence_untouched() {
    let source = tempfile::tempdir().unwrap();
    fixture(source.path(), "a-new");
    fixture(source.path(), "z-conflict");
    let path = source.path().join("test.tar.gz");
    package::export(
        source.path(),
        "experiment",
        vec!["a-new".into(), "z-conflict".into()],
        None,
        &path,
    )
    .unwrap();
    let verified = package::open(&path, None).unwrap();
    let destination = tempfile::tempdir().unwrap();
    fixture(destination.path(), "z-conflict");
    let runs = destination.path().join("target/hil/esp32s31/runs");
    fs::write(
        runs.join("z-conflict/uart.log"),
        b"different retained observation",
    )
    .unwrap();
    seal(&runs.join("z-conflict"), "z-conflict");
    assert!(install::install(destination.path(), &verified).is_err());
    assert!(!runs.join("a-new").exists());
    assert_eq!(
        fs::read(runs.join("z-conflict/uart.log")).unwrap(),
        b"different retained observation"
    );
}

fn write_tar(path: &Path, members: &[(&str, &[u8], tar::EntryType)]) {
    let gzip = GzEncoder::new(fs::File::create(path).unwrap(), Compression::default());
    let mut tar = tar::Builder::new(gzip);
    for (name, data, kind) in members {
        let mut header = tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(0o600);
        header.set_entry_type(*kind);
        if kind.is_symlink() {
            header.set_link_name("outside").unwrap();
        }
        header.set_cksum();
        tar.append_data(&mut header, name, *data).unwrap();
    }
    tar.into_inner().unwrap().finish().unwrap();
}

#[test]
fn extraction_rejects_links_duplicates_and_uninventoried_files() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("unsafe.tar.gz");
    for members in [
        vec![("link", &b""[..], tar::EntryType::Symlink)],
        vec![
            ("x", &b"a"[..], tar::EntryType::Regular),
            ("x", &b"b"[..], tar::EntryType::Regular),
        ],
        vec![("unexpected", &b"data"[..], tar::EntryType::Regular)],
    ] {
        write_tar(&file, &members);
        assert!(package::open(&file, None).is_err());
    }
    for name in [
        "../outside",
        "/absolute",
        "a/../../b",
        "a\\b",
        "C:/file",
        "a/./b",
        "a//b",
    ] {
        assert!(package::validate_path(Path::new(name)).is_err(), "{name}");
    }
}

#[test]
fn archive_detects_tampered_payload_even_with_a_correct_outer_digest() {
    let source = tempfile::tempdir().unwrap();
    fixture(source.path(), "run-1");
    let path = source.path().join("original.tar.gz");
    package::export(
        source.path(),
        "experiment",
        vec!["run-1".into()],
        None,
        &path,
    )
    .unwrap();
    let verified = package::open(&path, None).unwrap();
    fs::write(
        verified.directory.path().join("runs/run-1/uart.log"),
        b"tampered",
    )
    .unwrap();
    let files = package::inventory(verified.directory.path(), Path::new("")).unwrap();
    let contents: Vec<_> = files
        .iter()
        .map(|f| fs::read(verified.directory.path().join(&f.path)).unwrap())
        .collect();
    let members: Vec<_> = files
        .iter()
        .zip(&contents)
        .map(|(f, b)| (f.path.as_str(), b.as_slice(), tar::EntryType::Regular))
        .collect();
    let bad = source.path().join("tampered.tar.gz");
    write_tar(&bad, &members);
    assert!(package::open(&bad, Some(&package::digest(&bad).unwrap())).is_err());
    let mut bytes = fs::read(&path).unwrap();
    let mut appended = bytes.clone();
    appended.extend_from_slice(b"unlisted trailing data");
    fs::write(&bad, appended).unwrap();
    assert!(package::open(&bad, None).is_err());
    bytes.truncate(bytes.len() - 5);
    fs::File::create(&bad).unwrap().write_all(&bytes).unwrap();
    assert!(package::open(&bad, None).is_err());
}

#[cfg(unix)]
#[test]
fn export_does_not_follow_supplement_symlinks() {
    let source = tempfile::tempdir().unwrap();
    fixture(source.path(), "run-1");
    let supplement = source.path().join("notes");
    fs::create_dir(&supplement).unwrap();
    std::os::unix::fs::symlink("/etc/passwd", supplement.join("link")).unwrap();
    assert!(
        package::export(
            source.path(),
            "experiment",
            vec!["run-1".into()],
            Some(&supplement),
            &source.path().join("bad.tar.gz")
        )
        .is_err()
    );
}

#[test]
fn archive_cli_does_not_require_lab_or_network_for_offline_commands() {
    for args in [
        vec![
            "hil",
            "archive",
            "export",
            "comparison",
            "--run",
            "run-1",
            "--run",
            "run-2",
        ],
        vec!["hil", "archive", "verify", "comparison.tar.gz"],
        vec!["hil", "archive", "import", "comparison.tar.gz"],
        vec![
            "hil",
            "archive",
            "fetch",
            "comparison",
            "--repo",
            "ermacv/evidence",
        ],
    ] {
        crate::cli::Cli::try_parse_from(args).unwrap();
    }
}
