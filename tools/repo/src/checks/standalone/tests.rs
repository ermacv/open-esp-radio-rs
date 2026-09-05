use super::*;

fn write(path: &Path, contents: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

#[test]
fn extraction_uses_source_inventory_and_includes_new_binary_targets() {
    let repository = tempfile::tempdir().unwrap();
    let destination = tempfile::tempdir().unwrap();
    write(&repository.path().join("Cargo.toml"), "[workspace]\n");
    let source = repository.path().join("tools/blobray");
    write(
        &source.join("Cargo.toml"),
        "[package]\nname = 'extracted'\nversion = '0.1.0'\n",
    );
    write(&source.join("src/main.rs"), "fn main() {}\n");
    write(&source.join("src/bin/new-launcher.rs"), "fn main() {}\n");
    write(&source.join(".gitignore"), "private-input\n");
    write(&source.join("private-input"), "not source");
    write(&source.join("target/cached-output"), "not source");
    write(&source.join("_oracles/private-input"), "not source");
    write(&repository.path().join("driver/source.rs"), "not Blobray");
    let context = Context::new(repository.path()).unwrap();
    process::run(context.command("git").args(["init", "--quiet"])).unwrap();
    let files = paths::source_files(&context).unwrap();
    extract(&source, destination.path(), files).unwrap();
    assert!(destination.path().join("src/main.rs").is_file());
    assert!(destination.path().join("src/bin/new-launcher.rs").is_file());
    for excluded in ["private-input", "target", "_oracles", "driver"] {
        assert!(
            !destination.path().join(excluded).exists(),
            "copied {excluded}"
        );
    }
}

#[test]
fn dependency_containment_rejects_sibling_prefix_and_accepts_nested_crate() {
    let parent = tempfile::tempdir().unwrap();
    let root = parent.path().join("extracted");
    let inside = root.join("crates/model");
    let outside = parent.path().join("extracted-other");
    fs::create_dir_all(&inside).unwrap();
    fs::create_dir_all(&outside).unwrap();
    require_contained_dependencies(&root, [&inside].map(PathBuf::as_path)).unwrap();
    assert!(require_contained_dependencies(&root, [&outside].map(PathBuf::as_path)).is_err());
}

#[cfg(unix)]
#[test]
fn dependency_containment_rejects_symlink_to_repository() {
    let parent = tempfile::tempdir().unwrap();
    let root = parent.path().join("extracted");
    let outside = parent.path().join("repository");
    fs::create_dir_all(&root).unwrap();
    fs::create_dir_all(&outside).unwrap();
    let link = root.join("model");
    std::os::unix::fs::symlink(outside, &link).unwrap();
    assert!(require_contained_dependencies(&root, [link.as_path()]).is_err());
}

#[cfg(unix)]
#[test]
fn extraction_rejects_source_symlink_outside_blobray() {
    let repository = tempfile::tempdir().unwrap();
    let destination = tempfile::tempdir().unwrap();
    let source = repository.path().join("blobray");
    write(&source.join("Cargo.toml"), "[package]\n");
    let outside = repository.path().join("driver.rs");
    write(&outside, "not Blobray");
    let link = source.join("borrowed.rs");
    std::os::unix::fs::symlink(outside, &link).unwrap();
    assert!(extract(&source, destination.path(), vec![link]).is_err());
}

#[test]
fn extracted_cargo_commands_own_their_config_and_output_directory() {
    let repository = tempfile::tempdir().unwrap();
    write(&repository.path().join("Cargo.toml"), "[workspace]\n");
    let context = Context::new(repository.path()).unwrap();
    let extraction = tempfile::tempdir().unwrap();
    let command = command(
        &context,
        extraction.path(),
        OsStr::new("selected-toolchain"),
    );
    assert_eq!(command.get_current_dir(), Some(extraction.path()));
    assert!(
        command
            .get_envs()
            .any(|(key, value)| key == "CARGO_TARGET_DIR"
                && value == Some(extraction.path().join("target").as_os_str()))
    );
}

#[test]
fn extraction_preserves_caller_toolchain_or_uses_the_repository_channel() {
    let repository = tempfile::tempdir().unwrap();
    write(&repository.path().join("Cargo.toml"), "[workspace]\n");
    write(
        &repository.path().join("rust-toolchain.toml"),
        "[toolchain]\nchannel = 'repository-pin'\n",
    );
    let context = Context::new(repository.path()).unwrap();
    assert_eq!(
        selected_toolchain(&context, None).unwrap(),
        OsStr::new("repository-pin")
    );
    assert_eq!(
        selected_toolchain(&context, Some("caller-override".into())).unwrap(),
        OsStr::new("caller-override")
    );
}
