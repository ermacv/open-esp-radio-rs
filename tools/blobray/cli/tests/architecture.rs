use std::collections::{BTreeMap, BTreeSet};

#[test]
fn core_obeys_crate_boundaries() {
    let metadata = cargo_metadata::MetadataCommand::new()
        .manifest_path(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"))
        .no_deps()
        .exec()
        .unwrap();
    const MODEL: &str = "oer-riscv-model";
    let allowed: BTreeMap<&str, &[&str]> = BTreeMap::from([
        ("blobray-domain", [MODEL].as_slice()),
        (
            "blobray-artifacts",
            ["blobray-domain", MODEL, "oer-riscv-program"].as_slice(),
        ),
        (
            "blobray-analysis",
            ["blobray-domain", "oer-riscv-analysis", MODEL].as_slice(),
        ),
        ("blobray-verification", ["blobray-domain", MODEL].as_slice()),
        (
            "blobray-backend-riscv",
            [
                "blobray-domain",
                "oer-riscv-decode",
                "oer-riscv-lift",
                MODEL,
            ]
            .as_slice(),
        ),
        (
            "blobray-application",
            [
                "blobray-domain",
                "blobray-artifacts",
                "blobray-analysis",
                "blobray-verification",
                "oer-riscv-analysis",
                MODEL,
                "oer-riscv-program",
            ]
            .as_slice(),
        ),
        (
            "blobray-linker",
            ["blobray-domain", "blobray-application", MODEL].as_slice(),
        ),
        (
            "blobray-cli",
            [
                "blobray-domain",
                "blobray-application",
                "blobray-backend-riscv",
                "oer-riscv-lift",
                MODEL,
            ]
            .as_slice(),
        ),
    ]);
    for (name, expected) in allowed {
        let package = metadata
            .packages
            .iter()
            .find(|p| p.name.as_str() == name)
            .unwrap();
        let actual: BTreeSet<_> = package
            .dependencies
            .iter()
            .filter(|d| d.path.is_some())
            .map(|d| d.name.as_str())
            .collect();
        assert_eq!(
            actual,
            expected.iter().copied().collect(),
            "dependency boundary for {name}"
        );
    }
}
