mod support;
use oer_process::Checkout;
use oer_xtask::checks;
use std::fs;
use support::Fixture;

const POLICY: &str =
    "[lints]\nworkspace = true\n[workspace.lints.rust]\nunsafe_op_in_unsafe_fn = \"deny\"\n";

fn fixture() -> Fixture {
    let f = Fixture::new();
    f.write(
        "Cargo.toml",
        &format!(
            "[package]\nname = \"root-fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n[workspace]\n{POLICY}"
        ),
    );
    f.write("src/lib.rs", "");
    for path in ["adapter", "helper", "crates"] {
        fs::remove_dir_all(f.root().join(path)).unwrap();
    }
    f.package(
        "crates/old",
        "independent-fixture",
        &format!("[workspace]\n{POLICY}"),
    );
    f.write(".gitignore", "/_oracles/\n**/target/\n");
    f.git(&["init", "--quiet"]);
    f.git(&["add", "."]);
    for manifest in ["Cargo.toml", "crates/old/Cargo.toml"] {
        oer_process::capture(oer_toolchain::cargo_in(&f.context.root).args([
            "generate-lockfile",
            "--offline",
            "--manifest-path",
            manifest,
        ]))
        .unwrap();
    }
    fs::rename(f.root().join("crates/old"), f.root().join("crates/moved")).unwrap();
    f
}
#[test]
fn unstaged_move_is_checked_without_private_or_build_inputs() {
    let f = fixture();
    f.write("_oracles/private/Cargo.toml", "invalid private manifest");
    f.git(&["add", "--force", "_oracles/private/Cargo.toml"]);
    f.write(
        "crates/moved/target/local/Cargo.toml",
        "invalid build manifest",
    );
    let repo = oer_repo::Repo::from_git(&f.context.root).unwrap();
    let manifests = repo.files().filter(|file| file.ends_with("Cargo.toml"));
    assert_eq!(manifests.count(), 2);
    assert_eq!(checks::metadata(&f.context).unwrap(), 2);
}
#[test]
fn invalid_unstaged_workspace_fails_the_audit() {
    let f = fixture();
    f.write("crates/moved/Cargo.toml", "invalid new manifest");
    assert!(checks::metadata(&f.context).is_err());
}
#[test]
fn independent_workspace_without_the_root_lint_policy_fails_the_audit() {
    let f = fixture();
    f.package("crates/moved", "independent-fixture", "[workspace]\n");
    let error = checks::metadata(&f.context).unwrap_err().to_string();
    assert!(error.contains("crates/moved/Cargo.toml: [workspace.lints] differs"));
    assert!(error.contains("package independent-fixture must declare"));
}

#[test]
fn git_worktree_discovery_uses_its_own_source_root() {
    let f = fixture();
    f.git(&["add", "."]);
    f.git(&[
        "-c",
        "user.name=Fixture",
        "-c",
        "user.email=fixture@example.invalid",
        "commit",
        "-qm",
        "fixture",
    ]);
    let external = tempfile::tempdir().unwrap();
    let path = external.path().join("worktree с пробелами");
    oer_process::capture(
        f.context
            .command("git")
            .args(["worktree", "add", "--detach"])
            .arg(&path),
    )
    .unwrap();
    let context = Checkout::new(&path).unwrap();
    assert_eq!(checks::metadata(&context).unwrap(), 2);
    let repo = oer_repo::Repo::from_git(&context.root).unwrap();
    assert!(repo.files().all(|file| context.root.join(file).is_file()));
}
