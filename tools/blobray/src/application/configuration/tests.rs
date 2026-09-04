use std::fs;

use super::*;

fn composition_fixture(label: &str) -> (PathBuf, PathBuf) {
    let root = std::env::temp_dir().join(format!("blobray-compose-{label}-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let manifest = root.join("vendor-project.toml");
    fs::write(
        &manifest,
        "schema = 4\nid = \"fixture\"\ntarget-spec = \"target.toml\"\n",
    )
    .unwrap();
    fs::write(root.join("target.toml"), "schema = 3\nid = \"fixture\"\narchitecture = \"riscv32\"\ncalling-convention = \"riscv-ilp32\"\nendianness = \"little\"\npointer-width = 32\nrust-target = \"riscv32imac-unknown-none-elf\"\n").unwrap();
    for id in ["first", "second"] {
        fs::write(
            root.join(format!("{id}.toml")),
            format!("schema = 3\nid = \"{id}\"\nknowledge-packs = []\n"),
        )
        .unwrap();
    }
    (root, manifest)
}

#[test]
fn composes_multiple_ecosystems_and_checks_without_rewriting() {
    let (root, manifest) = composition_fixture("multiple");
    let request = ProjectConfigureRequest {
        ecosystem_packs: Some(vec![root.join("second.toml"), root.join("first.toml")]),
        check: false,
    };
    let report = configure_project(&manifest, request.clone()).unwrap();
    assert_eq!(report.ecosystem_packs, ["second", "first"]);
    assert_eq!(report.status, "written");
    let before = fs::read(&manifest).unwrap();
    let report = configure_project(
        &manifest,
        ProjectConfigureRequest {
            check: true,
            ..request
        },
    )
    .unwrap();
    assert_eq!(report.status, "verified");
    assert_eq!(fs::read(&manifest).unwrap(), before);
    assert!(!temporary_manifest_path(&manifest).unwrap().exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn duplicate_pack_or_changed_check_preserves_manifest_and_leaves_no_stage() {
    let (root, manifest) = composition_fixture("reject");
    let before = fs::read(&manifest).unwrap();
    let error = configure_project(
        &manifest,
        ProjectConfigureRequest {
            ecosystem_packs: Some(vec![root.join("first.toml"), root.join("./first.toml")]),
            check: false,
        },
    )
    .unwrap_err();
    assert!(error.to_string().contains("more than once"));
    let error = configure_project(
        &manifest,
        ProjectConfigureRequest {
            ecosystem_packs: Some(vec![root.join("first.toml")]),
            check: true,
        },
    )
    .unwrap_err();
    assert!(error.to_string().contains("differs"));
    assert_eq!(fs::read(&manifest).unwrap(), before);
    assert!(!temporary_manifest_path(&manifest).unwrap().exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn invalid_pack_never_changes_the_manifest() {
    let root = std::env::temp_dir().join(format!(
        "blobray-project-configure-incompatible-{}",
        std::process::id()
    ));
    if root.exists() {
        fs::remove_dir_all(&root).unwrap();
    }
    fs::create_dir_all(&root).unwrap();
    let manifest = root.join("vendor-project.toml");
    let original = "schema = 4\nid = \"fixture\"\ntarget-spec = \"target.toml\"\n";
    fs::write(&manifest, original).unwrap();
    fs::write(
        root.join("target.toml"),
        "schema = 3\nid = \"fixture\"\narchitecture = \"riscv32\"\ncalling-convention = \"riscv-ilp32\"\nendianness = \"little\"\npointer-width = 32\nrust-target = \"riscv32imac-unknown-none-elf\"\n",
    )
    .unwrap();
    fs::write(
        root.join("wrong.toml"),
        "schema = 3\nid = \"wrong\"\narchitecture = \"xtensa\"\nknowledge-packs = []\n",
    )
    .unwrap();

    let error = configure(
        &manifest,
        ProjectConfigureRequest {
            ecosystem_packs: Some(vec![root.join("wrong.toml")]),
            ..Default::default()
        },
    )
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("unknown ecosystem pack key \"architecture\"")
    );
    assert_eq!(fs::read_to_string(&manifest).unwrap(), original);

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn invalid_composition_removes_only_its_own_staged_manifest() {
    let (root, manifest) = composition_fixture("staged-validation");
    // Each pack is valid individually. The incompatible target ABI is caught
    // only when the staged project is resolved as a complete composition.
    let target = root.join("target.toml");
    let target_input = fs::read_to_string(&target).unwrap();
    fs::write(&target, target_input.replace("riscv-ilp32", "xtensa-call0")).unwrap();
    let before = fs::read(&manifest).unwrap();
    assert!(
        configure_project(
            &manifest,
            ProjectConfigureRequest {
                ecosystem_packs: Some(vec![root.join("first.toml"), root.join("second.toml")]),
                check: false,
            }
        )
        .is_err()
    );
    let staging = temporary_manifest_path(&manifest).unwrap();
    assert!(!staging.exists());
    assert_eq!(fs::read(&manifest).unwrap(), before);

    fs::write(&target, target_input).unwrap();
    fs::write(&staging, b"another caller owns this file").unwrap();
    assert!(
        configure_project(
            &manifest,
            ProjectConfigureRequest {
                ecosystem_packs: Some(vec![root.join("first.toml")]),
                check: false,
            }
        )
        .is_err()
    );
    assert_eq!(
        fs::read(&staging).unwrap(),
        b"another caller owns this file"
    );
    assert_eq!(fs::read(&manifest).unwrap(), before);
    fs::remove_dir_all(root).unwrap();
}
