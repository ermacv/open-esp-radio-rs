use super::*;

fn package(name: &str, directory: &str, workspace: &str, host: bool, inputs: &[&str]) -> Package {
    Package {
        name: name.into(),
        directory: directory.into(),
        workspace: workspace.into(),
        host,
        inputs: inputs.iter().map(|input| (*input).to_owned()).collect(),
    }
}

fn tree() -> Tree {
    Tree {
        packages: vec![
            package("hal", "crates/hal", "Cargo.toml", true, &[]),
            package("hal-nested", "crates/hal/nested", "Cargo.toml", true, &[]),
            package(
                "evaluator",
                "qualification/evaluator",
                "Cargo.toml",
                true,
                &["qualification", "hil/targets/**/Cargo.toml"],
            ),
            package(
                "xtask",
                "tools/xtask",
                "Cargo.toml",
                true,
                &[".github/workflows"],
            ),
            package(
                "agent",
                "hil/targets/chip/agent",
                "hil/targets/chip/Cargo.toml",
                false,
                &[],
            ),
            package(
                "blobray",
                "tools/blobray/cli",
                "tools/blobray/Cargo.toml",
                true,
                &[],
            ),
        ],
        workspaces: vec![
            "Cargo.toml".into(),
            "hil/targets/chip/Cargo.toml".into(),
            "tools/blobray/Cargo.toml".into(),
        ],
    }
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
        tested(Depth::Fast, &selection, &affected),
        BTreeSet::from([&key("Cargo.toml", "hal-nested")])
    );
    assert_eq!(
        tested(Depth::Full, &selection, &affected),
        affected.iter().collect()
    );
}

#[test]
fn chip_only_packages_are_those_the_host_never_builds() {
    let affected = BTreeSet::from([
        key("Cargo.toml", "hal"),
        key("hil/targets/chip/Cargo.toml", "agent"),
    ]);
    assert_eq!(
        chip_only(&tree(), &affected),
        BTreeSet::from([&key("hil/targets/chip/Cargo.toml", "agent")])
    );
    let host_only = BTreeSet::from([key("Cargo.toml", "hal")]);
    assert!(chip_only(&tree(), &host_only).is_empty());
}

#[test]
fn declared_inputs_select_the_packages_that_read_them() {
    let selection = run(&["qualification/catalog/esp32s31/coex.toml"]);
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
        4
    );
    assert!(selection.packages.iter().all(|(w, _)| w == "Cargo.toml"));
    assert!(selection.locks.contains("Cargo.toml"));
    let selection = run(&["rust-toolchain.toml"]);
    assert_eq!(selection.packages.len(), 6);
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
fn dependents_follow_every_edge_transitively() {
    let edges = Edges::from([
        ("a".into(), BTreeSet::new()),
        ("b".into(), BTreeSet::from(["a".into()])),
        ("c".into(), BTreeSet::from(["b".into()])),
        ("d".into(), BTreeSet::new()),
    ]);
    assert_eq!(
        dependents(&edges, &BTreeSet::from(["a".into()])),
        BTreeSet::from(["a".into(), "b".into(), "c".into()])
    );
    assert_eq!(
        dependents(&edges, &BTreeSet::from(["d".into()])),
        BTreeSet::from(["d".into()])
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
