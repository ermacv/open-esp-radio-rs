use super::*;

#[test]
fn remote_provenance_never_retains_https_credentials() {
    assert_eq!(
        sanitize_git_remote(String::from(
            "https://user:secret-token@github.com/owner/repository.git"
        )),
        "https://github.com/owner/repository.git"
    );
    assert_eq!(
        sanitize_git_remote(String::from("git@github.com:owner/repository.git")),
        "git@github.com:owner/repository.git"
    );
}

#[test]
fn source_stability_check_rejects_a_post_capture_change() {
    let root = std::env::temp_dir().join(format!(
        "open-radio-hil-source-stability-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir(&root).unwrap();
    for arguments in [
        &["init", "-q", "-b", "main"][..],
        &["config", "user.name", "HIL Test"],
        &["config", "user.email", "hil@example.invalid"],
        &[
            "remote",
            "add",
            "origin",
            "https://example.invalid/repository.git",
        ],
    ] {
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(arguments)
                .status()
                .unwrap()
                .success()
        );
    }
    fs::write(root.join("tracked.txt"), b"before\n").unwrap();
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(["add", "tracked.txt"])
            .status()
            .unwrap()
            .success()
    );
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(["commit", "-m", "base"])
            .status()
            .unwrap()
            .success()
    );
    let run = root.join("run");
    fs::create_dir(&run).unwrap();
    let source = capture_source_material(
        "repository",
        &root,
        &run,
        Path::new("source/repository.patch"),
    )
    .unwrap();
    assert_eq!(
        capture_git_source_state(&root)
            .unwrap()
            .unwrap()
            .workspace_sha256,
        source.workspace_sha256
    );
    fs::write(root.join("tracked.txt"), b"after\n").unwrap();
    assert_ne!(
        capture_git_source_state(&root)
            .unwrap()
            .unwrap()
            .workspace_sha256,
        source.workspace_sha256
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn collection_deletes_only_objects_no_archive_links_to() {
    let directory = tempfile::tempdir().unwrap();
    let target = directory.path().join("target");
    let source = directory.path().join("source");
    fs::create_dir_all(&source).unwrap();
    for name in ["kept", "dropped"] {
        fs::write(source.join(name), name).unwrap();
        archive_content_addressed(
            &source.join(name),
            &directory.path().join("run").join(name),
            &target,
        )
        .unwrap();
    }
    // The run that archived `dropped` is pruned.
    let dropped = directory.path().join("run/dropped");
    fs::remove_file(&dropped).unwrap();
    let prefix = target.join("objects/sha256/00");
    fs::create_dir_all(&prefix).unwrap();
    fs::write(prefix.join(".firmware-artifact.tmp-1-0"), "partial").unwrap();

    let collected = collect_objects(&target).unwrap();
    assert_eq!(collected.objects, 1);
    assert_eq!(collected.bytes, ("dropped".len() + "partial".len()) as u64);
    assert!(!prefix.join(".firmware-artifact.tmp-1-0").exists());
    assert_eq!(
        fs::read(directory.path().join("run/kept")).unwrap(),
        b"kept"
    );
    // The kept object is reused by a later identical archive.
    archive_content_addressed(
        &source.join("kept"),
        &directory.path().join("again/kept"),
        &target,
    )
    .unwrap();
    assert_eq!(
        collect_objects(&target).unwrap(),
        CollectedObjects::default()
    );
    // An empty store collects nothing.
    assert_eq!(
        collect_objects(&directory.path().join("none")).unwrap(),
        CollectedObjects::default()
    );
}
