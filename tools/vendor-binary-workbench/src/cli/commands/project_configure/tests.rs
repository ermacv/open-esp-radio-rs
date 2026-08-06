use std::fs;

use super::*;

#[test]
fn configuration_is_validated_written_checked_and_cleared() {
    let root = std::env::temp_dir().join(format!(
        "vendor-workbench-project-configure-{}",
        std::process::id()
    ));
    if root.exists() {
        fs::remove_dir_all(&root).unwrap();
    }
    let project = root.join("project");
    let packs = root.join("packs");
    fs::create_dir_all(&project).unwrap();
    fs::create_dir_all(&packs).unwrap();
    fs::write(
        project.join("vendor-project.toml"),
        "schema = 1\nid = \"fixture\"\ntarget-spec = \"target.spec\"\n",
    )
    .unwrap();
    fs::write(
        project.join("target.spec"),
        "schema 1\ntarget fixture\narchitecture riscv32\ncalling-convention riscv-ilp32\nendianness little\npointer-width 32\nrust-target riscv32imac-unknown-none-elf\n",
    )
    .unwrap();
    fs::write(
        packs.join("platform.toml"),
        "schema = 1\nid = \"fixture-platform\"\narchitecture = \"riscv32\"\ncalling-convention = \"riscv-ilp32\"\nharness = \"esp32s31-radio-v1\"\nsemantic-catalogs = []\n",
    )
    .unwrap();
    let manifest = project.join("vendor-project.toml");

    run(
        vec![
            "--platform-pack".to_owned(),
            packs.join("platform.toml").display().to_string(),
        ],
        &manifest,
    )
    .unwrap();
    let contents = fs::read_to_string(&manifest).unwrap();
    assert!(contents.contains("platform-pack = \"../packs/platform.toml\""));
    run(
        vec![
            "--platform-pack".to_owned(),
            packs.join("platform.toml").display().to_string(),
            "--check".to_owned(),
        ],
        &manifest,
    )
    .unwrap();
    run(vec!["--no-platform-pack".to_owned()], &manifest).unwrap();
    assert!(
        !fs::read_to_string(&manifest)
            .unwrap()
            .contains("platform-pack")
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn incompatible_pack_never_changes_the_manifest() {
    let root = std::env::temp_dir().join(format!(
        "vendor-workbench-project-configure-incompatible-{}",
        std::process::id()
    ));
    if root.exists() {
        fs::remove_dir_all(&root).unwrap();
    }
    fs::create_dir_all(&root).unwrap();
    let manifest = root.join("vendor-project.toml");
    let original = "schema = 1\nid = \"fixture\"\ntarget-spec = \"target.spec\"\n";
    fs::write(&manifest, original).unwrap();
    fs::write(
        root.join("target.spec"),
        "schema 1\ntarget fixture\narchitecture riscv32\ncalling-convention riscv-ilp32\nendianness little\npointer-width 32\nrust-target riscv32imac-unknown-none-elf\n",
    )
    .unwrap();
    fs::write(
        root.join("wrong.toml"),
        "schema = 1\nid = \"wrong\"\narchitecture = \"xtensa\"\ncalling-convention = \"xtensa-call0\"\nsemantic-catalogs = []\n",
    )
    .unwrap();

    let error = run(
        vec![
            "--platform-pack".to_owned(),
            root.join("wrong.toml").display().to_string(),
        ],
        &manifest,
    )
    .unwrap_err();
    assert!(error.to_string().contains("requires xtensa/xtensa-call0"));
    assert_eq!(fs::read_to_string(&manifest).unwrap(), original);

    fs::remove_dir_all(root).unwrap();
}
