use super::*;
use std::fs;

#[test]
fn ignored_driver_workspace_member_remains_in_compiled_audit_inventory() {
    let repository = tempfile::tempdir().unwrap();
    fs::write(
        repository.path().join("Cargo.toml"),
        "[workspace]\nresolver = '3'\nmembers = ['driver/visible', 'driver/ignored']\n",
    )
    .unwrap();
    fs::write(repository.path().join(".gitignore"), "/driver/ignored/\n").unwrap();
    for name in ["visible", "ignored"] {
        let directory = repository.path().join("driver").join(name);
        fs::create_dir_all(directory.join("src")).unwrap();
        fs::write(
            directory.join("Cargo.toml"),
            format!("[package]\nname = '{name}'\nversion = '0.1.0'\nedition = '2024'\n"),
        )
        .unwrap();
        fs::write(directory.join("src/lib.rs"), "").unwrap();
    }
    let context = Context::new(repository.path()).unwrap();
    crate::process::run(context.command("git").args(["init", "--quiet"])).unwrap();
    assert!(
        !paths::source_manifests(&context)
            .unwrap()
            .iter()
            .any(|path| path.ends_with("driver/ignored/Cargo.toml"))
    );
    // Metadata only; this test never builds a nested Cargo workspace.
    let packages = driver_packages(&context).unwrap();
    assert_eq!(
        packages
            .iter()
            .map(|package| package.package.name.as_str())
            .collect::<BTreeSet<_>>(),
        BTreeSet::from(["ignored", "visible"])
    );
    assert!(packages.iter().all(|package| package.workspace_member));
}
