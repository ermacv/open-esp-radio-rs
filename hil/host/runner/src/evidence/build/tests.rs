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
    verify_source_material_unchanged(&source).unwrap();
    fs::write(root.join("tracked.txt"), b"after\n").unwrap();
    assert!(verify_source_material_unchanged(&source).is_err());
    fs::remove_dir_all(root).unwrap();
}
