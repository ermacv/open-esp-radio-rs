use super::*;

fn git(root: &Path, args: &[&str]) -> oer_process::Result<Vec<u8>> {
    oer_process::git::output(root, args)
}

pub fn test_snapshot(inputs: &Path) -> (tempfile::TempDir, Snapshot) {
    let root = repository();
    fs::write(root.path().join(".gitignore"), "snapshots/\n").unwrap();
    for relative in [
        "Cargo.lock",
        "hil/targets/chip-a/Cargo.lock",
        "hil/targets/chip-a/Cargo.toml",
        "hil/targets/chip-a/stack.toml",
        "platform/chip-a/stack.toml",
        "platform/chip-a/partitions/applications.csv",
        "platform/chip-a/chip.toml",
    ] {
        let target = root.path().join(relative);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::copy(inputs.join(relative), target).unwrap();
    }
    git(root.path(), &["add", "."]).unwrap();
    git(root.path(), &["commit", "-qm", "build inputs"]).unwrap();
    let snapshot = capture_roots(
        &[("repository".into(), root.path().to_owned())],
        &[],
        &[],
        &root.path().join("snapshots"),
    )
    .unwrap();
    (root, snapshot)
}

fn repository() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for args in [
        vec!["init", "-q"],
        vec!["config", "user.name", "Snapshot Test"],
        vec!["config", "user.email", "snapshot@example.invalid"],
    ] {
        git(dir.path(), &args).unwrap();
    }
    fs::write(dir.path().join("Cargo.toml"), "initial\n").unwrap();
    fs::write(dir.path().join(".gitignore"), "secret.txt\n").unwrap();
    git(dir.path(), &["add", "."]).unwrap();
    git(dir.path(), &["commit", "-qm", "initial"]).unwrap();
    dir
}

#[test]
fn untracked_evidence_shards_neither_block_nor_enter_a_snapshot() {
    let root = repository();
    let output = tempfile::tempdir().unwrap();
    let target = output.path().join("snapshots");
    fs::create_dir_all(root.path().join("hil/evidence/chip-a")).unwrap();
    fs::write(root.path().join("hil/evidence/chip-a/station.json"), "{}").unwrap();
    fs::write(root.path().join("new.rs"), "pub fn new() {}\n").unwrap();
    let roots = vec![("repository".into(), root.path().to_owned())];
    let error = capture_roots(&roots, &[], &[], &target)
        .err()
        .unwrap()
        .to_string();
    assert!(error.contains("\n  --source-include new.rs\n"), "{error}");
    assert!(!error.contains("station.json"), "{error}");
    let snapshot = capture_roots(&roots, &["new.rs".into()], &[], &target).unwrap();
    let mut archive =
        tar::Archive::new(fs::File::open(snapshot.directory.join("sources.tar")).unwrap());
    assert!(!archive.entries().unwrap().any(|e| {
        e.unwrap()
            .path()
            .unwrap()
            .starts_with("repository/hil/evidence")
    }));
}

#[test]
fn untracked_selection_is_explicit_complete_and_does_not_archive_secrets() {
    let root = repository();
    let output = tempfile::tempdir().unwrap();
    let target = output.path().join("snapshots");
    fs::write(root.path().join("new.rs"), "pub fn new() {}\n").unwrap();
    fs::write(root.path().join("secret.txt"), "do not archive").unwrap();
    let roots = vec![("repository".into(), root.path().to_owned())];
    let error = capture_roots(&roots, &[], &[], &target)
        .err()
        .unwrap()
        .to_string();
    assert!(error.contains("repository:new.rs"));
    assert!(!error.contains("secret.txt"));
    assert!(!target.exists());
    let snapshot = capture_roots(&roots, &["new.rs".into()], &[], &target).unwrap();
    let mut archive =
        tar::Archive::new(fs::File::open(snapshot.directory.join("sources.tar")).unwrap());
    let entries = archive
        .entries()
        .unwrap()
        .map(|e| e.unwrap().path().unwrap().into_owned())
        .collect::<Vec<_>>();
    assert!(entries.contains(&PathBuf::from("repository/new.rs")));
    assert!(
        !entries
            .iter()
            .any(|p| p.ends_with("secret.txt") || p.starts_with(".git"))
    );
    for invalid in [
        "../new.rs",
        "unknown:new.rs",
        "Cargo.toml",
        "secret.txt",
        "new.rs/child",
    ] {
        assert!(
            capture_roots(&roots, &[invalid.into()], &[], &target).is_err(),
            "{invalid}"
        );
    }
}

#[test]
fn snapshot_is_deterministic_and_tracks_actual_bytes_even_when_git_ignores_changes() {
    let root = repository();
    let output = tempfile::tempdir().unwrap();
    let roots = vec![("repository".into(), root.path().to_owned())];
    let first = capture_roots(&roots, &[], &[], output.path()).unwrap();
    let second = capture_roots(&roots, &[], &[], output.path()).unwrap();
    assert_eq!(first.snapshot_id, second.snapshot_id);
    assert_eq!(first.archive_sha256, second.archive_sha256);
    git(
        root.path(),
        &["update-index", "--assume-unchanged", "Cargo.toml"],
    )
    .unwrap();
    fs::write(root.path().join("Cargo.toml"), "different\n").unwrap();
    let changed = capture_roots(&roots, &[], &[], output.path()).unwrap();
    assert_ne!(first.snapshot_id, changed.snapshot_id);
    assert_ne!(first.archive_sha256, changed.archive_sha256);
    let first_bytes = fs::read(first.directory.join("sources.tar")).unwrap();
    assert_eq!(digest(&first_bytes), first.archive_sha256);
}

#[test]
fn override_roles_require_their_own_explicit_files() {
    let root = repository();
    let other = repository();
    let output = tempfile::tempdir().unwrap();
    fs::write(other.path().join("new.rs"), "override\n").unwrap();
    let roots = vec![
        ("repository".into(), root.path().into()),
        ("esp-hal".into(), other.path().into()),
    ];
    assert!(capture_roots(&roots, &["new.rs".into()], &[], output.path()).is_err());
    assert!(capture_roots(&roots, &["esp-hal:new.rs".into()], &[], output.path()).is_ok());
}

#[test]
fn persisted_archive_corruption_is_not_overwritten_on_reuse() {
    let root = repository();
    let output = tempfile::tempdir().unwrap();
    let roots = vec![("repository".into(), root.path().to_owned())];
    let snapshot = capture_roots(&roots, &[], &[], output.path()).unwrap();
    fs::write(snapshot.directory.join("sources.tar"), b"corrupt").unwrap();
    assert!(capture_roots(&roots, &[], &[], output.path()).is_err());
    assert_eq!(
        fs::read(snapshot.directory.join("sources.tar")).unwrap(),
        b"corrupt"
    );
}

#[cfg(unix)]
#[test]
fn links_cannot_capture_bytes_outside_the_selected_checkout() {
    let root = repository();
    let output = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink("/etc/passwd", root.path().join("external")).unwrap();
    let roots = vec![("repository".into(), root.path().to_owned())];
    assert!(capture_roots(&roots, &["external".into()], &[], output.path()).is_err());
}

#[test]
fn materialized_inputs_do_not_follow_later_changes_to_the_live_tree() {
    let root = repository();
    fs::write(
        root.path().join("Cargo.toml"),
        "[package]\nname = \"snapshot-input-test\"\nversion = \"0.0.0\"\nedition = \"2024\"\n",
    )
    .unwrap();
    fs::create_dir(root.path().join("src")).unwrap();
    fs::write(
        root.path().join("src/lib.rs"),
        "pub const INPUT: u32 = 42;\n",
    )
    .unwrap();
    let output = tempfile::tempdir().unwrap();
    let roots = vec![("repository".into(), root.path().to_owned())];
    let snapshot = capture_roots(&roots, &["src/lib.rs".into()], &[], output.path()).unwrap();
    fs::write(
        root.path().join("src/lib.rs"),
        "compile_error!(\"live tree is not the snapshot\");\n",
    )
    .unwrap();
    let build = tempfile::tempdir().unwrap();
    materialize(&snapshot.directory, build.path()).unwrap();
    assert_eq!(
        fs::read_to_string(build.path().join("repository/src/lib.rs")).unwrap(),
        "pub const INPUT: u32 = 42;\n"
    );
    use oer_process::CommandExt as _;
    let result = std::process::Command::new(oer_toolchain::cargo_program())
        .current_dir(build.path().join("repository"))
        .args(["check", "--offline", "--quiet"])
        .env("CARGO_TARGET_DIR", build.path().join("target"))
        .supervised_output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}

#[test]
fn materialization_rejects_modified_manifest_and_archive_before_building() {
    for changed in ["manifest.json", "sources.tar"] {
        let root = repository();
        let output = tempfile::tempdir().unwrap();
        let roots = vec![("repository".into(), root.path().to_owned())];
        let snapshot = capture_roots(&roots, &[], &[], output.path()).unwrap();
        fs::write(snapshot.directory.join(changed), "corrupt").unwrap();
        let build = tempfile::tempdir().unwrap();
        assert!(materialize(&snapshot.directory, build.path()).is_err());
        assert_eq!(fs::read_dir(build.path()).unwrap().count(), 0);
    }
}

#[test]
fn a_build_workspace_is_stable_exclusive_and_replaced_on_reuse() {
    let root = repository();
    let output = tempfile::tempdir().unwrap();
    let roots = vec![("repository".into(), root.path().to_owned())];
    let first = capture_roots(&roots, &[], &[], output.path()).unwrap();
    fs::write(root.path().join("Cargo.toml"), "changed\n").unwrap();
    git(root.path(), &["commit", "-qam", "changed"]).unwrap();
    let second = capture_roots(&roots, &[], &[], &output.path().join("second")).unwrap();
    let build = tempfile::tempdir().unwrap();
    let workspace = build.path().join("source-build");
    let opened = FrozenSources::open_in_workspace(&first.directory, &workspace).unwrap();
    assert_eq!(opened.repository(), workspace.join("repository"));
    fs::write(workspace.join("stale.txt"), "left behind").unwrap();
    let unchanged = workspace.join("repository/.gitignore");
    let unchanged_time = fs::metadata(&unchanged).unwrap().modified().unwrap();
    // A second build waits for the lock; the holder releases it on drop.
    assert!(
        oer_process::lock::FileLock::try_acquire(
            &workspace.with_extension("lock"),
            oer_process::lock::Mode::Exclusive
        )
        .unwrap()
        .is_none()
    );
    drop(opened);
    let reopened = FrozenSources::open_in_workspace(&second.directory, &workspace).unwrap();
    assert_eq!(reopened.repository(), workspace.join("repository"));
    assert!(!workspace.join("stale.txt").exists());
    // Unchanged bytes keep their modification time, so Cargo keeps them fresh.
    assert_eq!(
        fs::metadata(&unchanged).unwrap().modified().unwrap(),
        unchanged_time
    );
    assert_eq!(
        fs::read_to_string(workspace.join("repository/Cargo.toml")).unwrap(),
        "changed\n"
    );
    reopened.verify_unchanged().unwrap();
}

#[test]
fn concurrent_builds_take_free_workspace_slots_and_wait_when_all_are_busy() {
    let root = repository();
    let output = tempfile::tempdir().unwrap();
    let roots = vec![("repository".into(), root.path().to_owned())];
    let snapshot = capture_roots(&roots, &[], &[], output.path()).unwrap();
    let build = tempfile::tempdir().unwrap();
    let base = build.path().join("source-build");
    let held = (0..super::WORKSPACE_SLOTS)
        .map(|slot| {
            let sources =
                FrozenSources::open_in_free_workspace(&snapshot.directory, &base).unwrap();
            assert_eq!(
                sources.repository(),
                build.path().join(format!("source-build-{slot}/repository"))
            );
            sources
        })
        .collect::<Vec<_>>();
    // Every slot is held: the next build waits until one is released.
    let (sender, receiver) = std::sync::mpsc::channel();
    let waiting = {
        let directory = snapshot.directory.clone();
        let base = base.clone();
        std::thread::spawn(move || {
            let sources = FrozenSources::open_in_free_workspace(&directory, &base).unwrap();
            sender.send(sources.repository()).unwrap();
        })
    };
    assert!(
        receiver
            .recv_timeout(std::time::Duration::from_millis(300))
            .is_err()
    );
    let mut held = held;
    drop(held.remove(0));
    let reused = receiver
        .recv_timeout(std::time::Duration::from_secs(30))
        .unwrap();
    assert_eq!(reused, build.path().join("source-build-0/repository"));
    waiting.join().unwrap();
    drop(held);
}

#[test]
fn include_untracked_archives_only_files_inside_its_scopes_and_records_why() {
    let root = repository();
    let output = tempfile::tempdir().unwrap();
    let target = output.path().join("snapshots");
    for file in [
        "crates/driver/src/new.rs",
        "hil/host/runner/src/new.rs",
        "hil/scenarios/new.toml",
    ] {
        let path = root.path().join(file);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "new\n").unwrap();
    }
    fs::write(root.path().join("notes.txt"), "scratch\n").unwrap();
    let roots = vec![("repository".into(), root.path().to_owned())];
    let scopes = untracked_scopes(vec![PathBuf::from("crates/driver")]);
    // A file outside every scope still has to be named.
    let error = capture_roots(&roots, &[], &scopes, &target)
        .err()
        .unwrap()
        .to_string();
    assert!(error.contains("repository:notes.txt"), "{error}");
    assert!(!error.contains("new."), "{error}");
    let snapshot = capture_roots(&roots, &["notes.txt".into()], &scopes, &target).unwrap();
    let manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(snapshot.directory.join("manifest.json")).unwrap())
            .unwrap();
    assert_eq!(
        manifest["sources"][0]["untracked"],
        serde_json::json!([
            {"path": "crates/driver/src/new.rs", "by": "image-package"},
            {"path": "hil/host/runner/src/new.rs", "by": "hil-host"},
            {"path": "hil/scenarios/new.toml", "by": "hil-host"},
            {"path": "notes.txt", "by": "source-include"},
        ])
    );
}

#[test]
fn the_blocked_snapshot_error_names_arguments_ready_to_paste() {
    assert_eq!(
        source_include_arguments(&[
            ("repository", Path::new("a/new.rs")),
            ("esp-hal", Path::new("b.rs")),
        ]),
        "--source-include a/new.rs --source-include esp-hal:b.rs"
    );
}

#[test]
fn a_captured_snapshot_loads_again_from_its_directory() {
    let root = repository();
    let output = tempfile::tempdir().unwrap();
    let roots = vec![("repository".into(), root.path().to_owned())];
    let snapshot = capture_roots(&roots, &[], &[], output.path()).unwrap();
    let loaded = Snapshot::load(snapshot.directory()).unwrap();
    assert_eq!(loaded.directory(), snapshot.directory());
    assert!(Snapshot::load(output.path()).is_err());
}

#[test]
fn a_capture_is_taken_again_while_a_source_changes_under_it() {
    let pause = std::time::Duration::ZERO;
    let mut calls = 0;
    let captured = until_unchanged(5, pause, || {
        calls += 1;
        if calls < 3 {
            Err(ChangedDuringCapture("repository".into()).into())
        } else {
            Ok(calls)
        }
    })
    .unwrap();
    assert_eq!(captured, 3);
    // A source that never settles fails after the last attempt.
    let mut calls = 0;
    let error = until_unchanged(2, pause, || -> crate::Result<()> {
        calls += 1;
        Err(ChangedDuringCapture("repository".into()).into())
    })
    .unwrap_err();
    assert_eq!(calls, 2);
    assert!(error.is::<ChangedDuringCapture>());
    // Any other failure is not retried.
    let mut calls = 0;
    assert!(
        until_unchanged(5, pause, || -> crate::Result<()> {
            calls += 1;
            Err("untracked files".into())
        })
        .is_err()
    );
    assert_eq!(calls, 1);
}
