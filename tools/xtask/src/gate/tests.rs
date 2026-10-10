use super::*;

fn package(name: &str, directory: &str, workspace: &str, host: bool, inputs: &[&str]) -> Package {
    Package {
        name: name.into(),
        directory: directory.into(),
        workspace: workspace.into(),
        host,
        chip: !host,
        inputs: inputs.iter().map(|input| (*input).to_owned()).collect(),
    }
}

/// A fixture checkout: root-workspace packages (one nested in another, one
/// of a chip), a chip workspace and Blobray's, as manifests on disk.
fn fixture() -> (tempfile::TempDir, Tree) {
    let host = "platform = \"host\"\nhost-layer = \"build\"";
    let files = [
        (
            "Cargo.toml",
            "[workspace]\nmembers = [\"crates/hal\", \"crates/hal/nested\", \"qualification/evaluator\", \"tools/xtask\", \"crates/driver\"]\nexclude = [\"hil/targets\", \"tools/blobray\"]\n".to_owned(),
        ),
        ("crates/hal/Cargo.toml", manifest("hal", host, "", "")),
        ("crates/hal/nested/Cargo.toml", manifest("hal-nested", host, "", "")),
        (
            "crates/driver/Cargo.toml",
            manifest("driver", host, "", "hal-nested = { path = \"../hal/nested\" }"),
        ),
        (
            "qualification/evaluator/Cargo.toml",
            manifest(
                "evaluator",
                host,
                "inputs = [\"qualification\", \"hil/targets/**/Cargo.toml\"]",
                "",
            ),
        ),
        (
            "tools/xtask/Cargo.toml",
            manifest("xtask", host, "inputs = [\".github/workflows\"]", ""),
        ),
        (
            "hil/targets/chip/Cargo.toml",
            "[workspace]\nmembers = [\"agent\"]\n".to_owned(),
        ),
        (
            "hil/targets/chip/agent/Cargo.toml",
            manifest("agent", "platform = \"chip\"\nchip = \"chip\"", "", ""),
        ),
        (
            "tools/blobray/Cargo.toml",
            "[workspace]\nmembers = [\"cli\"]\n".to_owned(),
        ),
        ("tools/blobray/cli/Cargo.toml", manifest("blobray", host, "", "")),
    ];
    let dir = tempfile::tempdir().unwrap();
    for (path, text) in &files {
        let path = dir.path().join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    let model = Model::load(&oer_repo::Repo::from_dir(dir.path()).unwrap()).unwrap();
    (dir, Tree::of(model))
}

fn manifest(name: &str, platform: &str, extra: &str, dependencies: &str) -> String {
    format!(
        "[package]\nname = \"{name}\"\n[dependencies]\n{dependencies}\n[package.metadata.open-radio]\nlayer = \"tool\"\n{platform}\n{extra}\n"
    )
}

fn tree() -> Tree {
    fixture().1
}

fn run(changed: &[&str]) -> Selection {
    select(
        &tree(),
        &changed
            .iter()
            .map(|path| (*path).to_owned())
            .collect::<Vec<_>>(),
    )
}

fn key(workspace: &str, name: &str) -> Key {
    (workspace.into(), name.into())
}

#[test]
fn a_source_selects_its_innermost_package_and_formats_its_workspace() {
    let selection = run(&["crates/hal/nested/src/lib.rs"]);
    assert_eq!(
        selection.packages,
        BTreeSet::from([key("Cargo.toml", "hal-nested")])
    );
    assert_eq!(selection.format, BTreeSet::from(["Cargo.toml".to_owned()]));
    assert!(selection.locks.is_empty() && !selection.docs);
    assert_eq!(
        selection.capabilities,
        Some(BTreeSet::from(["crates/hal/nested/src/lib.rs".to_owned()]))
    );
}

#[test]
fn the_fast_gate_tests_only_what_changed() {
    let selection = run(&["crates/hal/nested/src/lib.rs"]);
    let affected = BTreeSet::from([key("Cargo.toml", "hal-nested"), key("Cargo.toml", "driver")]);
    assert_eq!(
        tested(Tier::Fast, &selection, &affected),
        BTreeSet::from([&key("Cargo.toml", "hal-nested")])
    );
    assert_eq!(
        tested(Tier::Full, &selection, &affected),
        affected.iter().collect()
    );
}

#[test]
fn chip_code_is_in_chip_only_and_chip_platform_packages() {
    let affected = BTreeSet::from([
        key("Cargo.toml", "hal"),
        key("hil/targets/chip/Cargo.toml", "agent"),
    ]);
    assert_eq!(
        chip_code(&tree(), &affected),
        BTreeSet::from([&key("hil/targets/chip/Cargo.toml", "agent")])
    );
    let host_only = BTreeSet::from([key("Cargo.toml", "hal")]);
    assert!(chip_code(&tree(), &host_only).is_empty());

    // A chip composition the host builds for its ownership tests still has
    // code under the chip target's `cfg` only an image type-check compiles.
    let mut tree = tree();
    tree.packages.push(Package {
        chip: true,
        ..package("composition", "crates/composition", "Cargo.toml", true, &[])
    });
    let composition = BTreeSet::from([key("Cargo.toml", "composition")]);
    assert_eq!(
        chip_code(&tree, &composition),
        BTreeSet::from([&key("Cargo.toml", "composition")])
    );
}

#[test]
fn every_input_of_the_guides_selects_the_documentation() {
    for path in [
        "docs/book.toml",
        "docs/theme/head.hbs",
        "crates/a/README.md",
    ] {
        assert!(run(&[path]).docs, "{path}");
    }
    assert!(!run(&["tools/tool/src/main.rs"]).docs);
}

#[test]
fn declared_inputs_select_the_packages_that_read_them() {
    let selection = run(&["qualification/catalog/chip-a/coex.toml"]);
    assert!(selection.docs);
    assert_eq!(
        selection.packages,
        BTreeSet::from([key("Cargo.toml", "evaluator")])
    );
    assert_eq!(selection.capabilities, Some(BTreeSet::new()));
    let selection = run(&["hil/targets/chip/agent/Cargo.toml"]);
    assert_eq!(
        selection.packages,
        BTreeSet::from([
            key("Cargo.toml", "evaluator"),
            key("hil/targets/chip/Cargo.toml", "agent")
        ])
    );
    assert_eq!(
        selection.locks,
        tree().workspaces.into_iter().collect::<BTreeSet<_>>()
    );
    // A workflow change tests the package whose tests read the workflows.
    let selection = run(&[".github/workflows/ci.yml"]);
    assert_eq!(
        selection.packages,
        BTreeSet::from([key("Cargo.toml", "xtask")])
    );
}

#[test]
fn shared_build_inputs_select_every_package_of_their_workspaces() {
    let selection = run(&["Cargo.toml"]);
    assert_eq!(
        selection
            .packages
            .iter()
            .filter(|(w, _)| w == "Cargo.toml")
            .count(),
        5
    );
    assert!(selection.packages.iter().all(|(w, _)| w == "Cargo.toml"));
    assert!(selection.locks.contains("Cargo.toml"));
    let selection = run(&["rust-toolchain.toml"]);
    assert_eq!(selection.packages.len(), 7);
    assert_eq!(selection.format.len(), 3);
}

#[test]
fn a_package_manifest_checks_the_lock_of_every_workspace() {
    // A root package's new dependency also changes the lock of every
    // firmware workspace that builds it through a path dependency.
    let selection = run(&["crates/hal/Cargo.toml"]);
    assert_eq!(
        selection.locks,
        tree().workspaces.into_iter().collect::<BTreeSet<_>>()
    );
}

#[test]
fn a_lock_change_checks_the_lock_and_prose_checks_only_documents() {
    let selection = run(&["tools/blobray/Cargo.lock"]);
    assert_eq!(
        selection.locks,
        BTreeSet::from(["tools/blobray/Cargo.toml".to_owned()])
    );
    assert!(selection.packages.is_empty());
    let selection = run(&["docs/architecture.md"]);
    assert!(selection.docs);
    assert!(selection.packages.is_empty() && selection.format.is_empty());
    assert!(run(&[]).is_empty());
}

#[test]
fn dependents_of_a_selection_stay_in_its_workspace() {
    let (_dir, tree) = fixture();
    let selection = run(&["crates/hal/nested/src/lib.rs"]);
    assert_eq!(
        affected(&tree, &selection),
        BTreeSet::from([key("Cargo.toml", "driver"), key("Cargo.toml", "hal-nested")])
    );
}

#[test]
fn a_lock_change_selects_the_members_whose_resolution_changed() {
    let old = r#"
[[package]]
name = "a"
version = "0.1.0"
dependencies = ["serde"]

[[package]]
name = "b"
version = "0.1.0"
dependencies = ["log", "a"]

[[package]]
name = "c"
version = "0.1.0"

[[package]]
name = "serde"
version = "1.0.0"
source = "registry+https://github.com/rust-lang/crates.io-index"

[[package]]
name = "log"
version = "0.4.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
"#;
    assert!(lock_dependents(old, old).is_empty());
    let bumped = old.replace("version = \"0.4.0\"", "version = \"0.4.1\"");
    assert_eq!(
        lock_dependents(old, &bumped),
        BTreeSet::from(["b".to_owned()])
    );
    let bumped = old.replace("version = \"1.0.0\"", "version = \"1.0.1\"");
    assert_eq!(
        lock_dependents(old, &bumped),
        BTreeSet::from(["a".to_owned(), "b".to_owned()])
    );
    let added = old.replace(
        "name = \"c\"\nversion = \"0.1.0\"\n",
        "name = \"c\"\nversion = \"0.1.0\"\ndependencies = [\"log\"]\n",
    );
    assert_eq!(
        lock_dependents(old, &added),
        BTreeSet::from(["c".to_owned()])
    );
}
