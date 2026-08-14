use std::fs;

use super::*;

#[test]
fn invalid_pack_never_changes_the_manifest() {
    let root = std::env::temp_dir().join(format!(
        "vendor-workbench-project-configure-incompatible-{}",
        std::process::id()
    ));
    if root.exists() {
        fs::remove_dir_all(&root).unwrap();
    }
    fs::create_dir_all(&root).unwrap();
    let manifest = root.join("vendor-project.toml");
    let original = "schema = 3\nid = \"fixture\"\ntarget-spec = \"target.toml\"\n";
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

    let error = run(
        ProjectConfigureArgs {
            ecosystem_pack: Some(root.join("wrong.toml")),
            ..Default::default()
        },
        &manifest,
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
