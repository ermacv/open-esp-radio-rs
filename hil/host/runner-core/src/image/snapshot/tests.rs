use super::*;

pub fn test_snapshot(inputs: &Path) -> (tempfile::TempDir, Snapshot) {
    let root = repository();
    fs::write(root.path().join(".gitignore"), "snapshots/\n").unwrap();
    for relative in [
        "Cargo.lock",
        "hil/targets/esp32s31/Cargo.lock",
        "hil/targets/esp32s31/Cargo.toml",
        "hil/targets/esp32s31/stack.toml",
        "platform/esp32s31/partitions/applications.csv",
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
fn untracked_selection_is_explicit_complete_and_does_not_archive_secrets() {
    let root = repository();
    let output = tempfile::tempdir().unwrap();
    let target = output.path().join("snapshots");
    fs::write(root.path().join("new.rs"), "pub fn new() {}\n").unwrap();
    fs::write(root.path().join("secret.txt"), "do not archive").unwrap();
    let roots = vec![("repository".into(), root.path().to_owned())];
    let error = capture_roots(&roots, &[], &target)
        .err()
        .unwrap()
        .to_string();
    assert!(error.contains("repository:new.rs"));
    assert!(!error.contains("secret.txt"));
    assert!(!target.exists());
    let snapshot = capture_roots(&roots, &["new.rs".into()], &target).unwrap();
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
            capture_roots(&roots, &[invalid.into()], &target).is_err(),
            "{invalid}"
        );
    }
}

#[test]
fn snapshot_is_deterministic_and_tracks_actual_bytes_even_when_git_ignores_changes() {
    let root = repository();
    let output = tempfile::tempdir().unwrap();
    let roots = vec![("repository".into(), root.path().to_owned())];
    let first = capture_roots(&roots, &[], output.path()).unwrap();
    let second = capture_roots(&roots, &[], output.path()).unwrap();
    assert_eq!(first.snapshot_id, second.snapshot_id);
    assert_eq!(first.archive_sha256, second.archive_sha256);
    git(
        root.path(),
        &["update-index", "--assume-unchanged", "Cargo.toml"],
    )
    .unwrap();
    fs::write(root.path().join("Cargo.toml"), "different\n").unwrap();
    let changed = capture_roots(&roots, &[], output.path()).unwrap();
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
    assert!(capture_roots(&roots, &["new.rs".into()], output.path()).is_err());
    assert!(capture_roots(&roots, &["esp-hal:new.rs".into()], output.path()).is_ok());
}

#[test]
fn persisted_archive_corruption_is_not_overwritten_on_reuse() {
    let root = repository();
    let output = tempfile::tempdir().unwrap();
    let roots = vec![("repository".into(), root.path().to_owned())];
    let snapshot = capture_roots(&roots, &[], output.path()).unwrap();
    fs::write(snapshot.directory.join("sources.tar"), b"corrupt").unwrap();
    assert!(capture_roots(&roots, &[], output.path()).is_err());
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
    assert!(capture_roots(&roots, &["external".into()], output.path()).is_err());
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
    let snapshot = capture_roots(&roots, &["src/lib.rs".into()], output.path()).unwrap();
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
    let result = Command::new(super::super::program_from_env("CARGO", "cargo"))
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
        let snapshot = capture_roots(&roots, &[], output.path()).unwrap();
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
    let first = capture_roots(&roots, &[], output.path()).unwrap();
    fs::write(root.path().join("Cargo.toml"), "changed\n").unwrap();
    git(root.path(), &["commit", "-qam", "changed"]).unwrap();
    let second = capture_roots(&roots, &[], &output.path().join("second")).unwrap();
    let build = tempfile::tempdir().unwrap();
    let workspace = build.path().join("source-build");
    let opened = FrozenSources::open_in_workspace(&first.directory, &workspace).unwrap();
    assert_eq!(opened.repository(), workspace.join("repository"));
    fs::write(workspace.join("stale.txt"), "left behind").unwrap();
    let unchanged = workspace.join("repository/.gitignore");
    let unchanged_time = fs::metadata(&unchanged).unwrap().modified().unwrap();
    // A second build waits for the lock; the holder releases it on drop.
    let lock = fs::File::open(workspace.with_extension("lock")).unwrap();
    assert!(fs2::FileExt::try_lock_exclusive(&lock).is_err());
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
