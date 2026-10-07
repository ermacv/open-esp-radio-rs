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
    write(&repository.path().join("crates/source.rs"), "not Blobray");
    let context = Checkout::new(repository.path()).unwrap();
    process::run(oer_process::git::command(&context.root).args(["init", "--quiet"])).unwrap();
    let repo = oer_repo::Repo::from_git(&context.root).unwrap();
    let files: Vec<_> = repo.files().map(|file| context.root.join(file)).collect();
    extract(&source, destination.path(), &files).unwrap();
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
    assert!(extract(&source, destination.path(), &[link]).is_err());
}

#[test]
fn extraction_follows_transitive_and_all_declared_path_dependencies() {
    let repository = tempfile::tempdir().unwrap();
    let destination = tempfile::tempdir().unwrap();
    write(
        &repository.path().join("Cargo.toml"),
        "[workspace]\nmembers = []\nexclude = ['tools/blobray']\n",
    );
    write(
        &repository.path().join("tools/blobray/Cargo.toml"),
        "[workspace]\nmembers = ['cli', 'crates/*']\n",
    );
    let dependencies = "\
        [dependencies]\n\
        foundation = { path = '../../foundation' }\n\
        optional = { path = '../../optional', optional = true }\n\
        [build-dependencies]\n\
        generator = { path = '../../generator' }\n\
        [dev-dependencies]\n\
        testing = { path = '../../testing' }\n\
        [target.'cfg(windows)'.dependencies]\n\
        platform = { path = '../../platform' }\n";
    for (directory, name, dependencies) in [
        ("tools/blobray/cli", "cli", dependencies),
        ("tools/blobray/crates/new-member", "new-member", ""),
        (
            "tools/foundation",
            "foundation",
            "[dependencies]\ndurable = { path = '../durable' }\n",
        ),
        ("tools/durable", "durable", ""),
        ("tools/optional", "optional", ""),
        ("tools/generator", "generator", ""),
        ("tools/testing", "testing", ""),
        ("tools/platform", "platform", ""),
        ("tools/unrelated", "unrelated", ""),
    ] {
        write(
            &repository.path().join(directory).join("Cargo.toml"),
            &format!("[package]\nname = '{name}'\nversion = '0.1.0'\n{dependencies}"),
        );
        write(
            &repository.path().join(directory).join("src/lib.rs"),
            "pub fn available() {}\n",
        );
    }
    let context = Checkout::new(repository.path()).unwrap();
    process::run(oer_process::git::command(&context.root).args(["init", "--quiet"])).unwrap();
    extract_workspace(&context, destination.path()).unwrap();

    // Cargo resolves the extracted graph after its original source is gone.
    fs::remove_dir_all(repository.path()).unwrap();
    let output = process::capture(oer_toolchain::cargo_in(destination.path()).args([
        "metadata",
        "--no-deps",
        "--format-version",
        "1",
        "--offline",
    ]))
    .unwrap();
    let metadata: cargo_metadata::Metadata = serde_json::from_slice(&output.stdout).unwrap();
    let packages: std::collections::BTreeSet<_> = metadata
        .packages
        .iter()
        .map(|package| package.name.as_str())
        .collect();
    assert_eq!(
        packages,
        [
            "cli",
            "durable",
            "foundation",
            "generator",
            "new-member",
            "optional",
            "platform",
            "testing",
        ]
        .into_iter()
        .collect()
    );
    assert!(!destination.path().join("tools/unrelated").exists());
    require_contained_dependencies(
        destination.path(),
        metadata.packages.iter().flat_map(|package| {
            package
                .dependencies
                .iter()
                .filter_map(|dependency| dependency.path.as_ref().map(|path| path.as_std_path()))
        }),
    )
    .unwrap();
}

#[test]
fn extracted_cargo_commands_own_their_config_and_output_directory() {
    let repository = tempfile::tempdir().unwrap();
    write(&repository.path().join("Cargo.toml"), "[workspace]\n");
    let context = Checkout::new(repository.path()).unwrap();
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
    let context = Checkout::new(repository.path()).unwrap();
    assert_eq!(
        selected_toolchain(&context, None).unwrap(),
        OsStr::new("repository-pin")
    );
    assert_eq!(
        selected_toolchain(&context, Some("caller-override".into())).unwrap(),
        OsStr::new("caller-override")
    );
}
