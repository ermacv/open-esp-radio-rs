mod support;
use oer_xtask::{Context, checks, paths};
use std::fs;
use support::Fixture;

fn fixture() -> Fixture {
    let f = Fixture::new();
    f.write("Cargo.toml","[package]\nname = \"root-fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n[workspace]\n");
    f.write("src/lib.rs", "");
    for path in ["adapter", "helper", "driver"] {
        fs::remove_dir_all(f.root().join(path)).unwrap();
    }
    f.package("driver/old", "independent-fixture", "[workspace]\n");
    f.write(".gitignore", "/_oracles/\n**/target/\n");
    f.git(&["init", "--quiet"]);
    f.git(&["add", "."]);
    for manifest in ["Cargo.toml", "driver/old/Cargo.toml"] {
        oer_xtask::process::capture(f.context.cargo().args([
            "generate-lockfile",
            "--offline",
            "--manifest-path",
            manifest,
        ]))
        .unwrap();
    }
    fs::rename(f.root().join("driver/old"), f.root().join("driver/moved")).unwrap();
    f
}
#[test]
fn unstaged_move_is_checked_without_private_or_build_inputs() {
    let f = fixture();
    f.write("_oracles/private/Cargo.toml", "invalid private manifest");
    f.git(&["add", "--force", "_oracles/private/Cargo.toml"]);
    f.write(
        "driver/moved/target/local/Cargo.toml",
        "invalid build manifest",
    );
    let manifests = paths::source_manifests(&f.context).unwrap();
    assert_eq!(manifests.len(), 2);
    assert_eq!(checks::metadata(&f.context).unwrap(), 2);
}
#[test]
fn invalid_unstaged_workspace_fails_the_audit() {
    let f = fixture();
    f.write("driver/moved/Cargo.toml", "invalid new manifest");
    assert!(checks::metadata(&f.context).is_err());
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
    oer_xtask::process::capture(
        f.context
            .command("git")
            .args(["worktree", "add", "--detach"])
            .arg(&path),
    )
    .unwrap();
    let context = Context::new(&path).unwrap();
    assert_eq!(checks::metadata(&context).unwrap(), 2);
    assert!(
        paths::source_files(&context)
            .unwrap()
            .iter()
            .all(|p| p.starts_with(&context.root))
    );
}
