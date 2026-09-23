use std::collections::{BTreeMap, BTreeSet};

#[test]
fn new_core_obeys_crate_boundaries_and_does_not_depend_on_legacy() {
    let metadata = cargo_metadata::MetadataCommand::new()
        .manifest_path(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"))
        .no_deps()
        .exec()
        .unwrap();
    let allowed: BTreeMap<&str, &[&str]> = BTreeMap::from([
        ("blobray-domain", [].as_slice()),
        ("blobray-artifacts", ["blobray-domain"].as_slice()),
        ("blobray-store", ["blobray-domain"].as_slice()),
        ("blobray-analysis", ["blobray-domain"].as_slice()),
        ("blobray-verification", ["blobray-domain"].as_slice()),
        ("blobray-knowledge", ["blobray-domain"].as_slice()),
        ("blobray-backend-riscv", ["blobray-domain"].as_slice()),
        (
            "blobray-application",
            [
                "blobray-domain",
                "blobray-artifacts",
                "blobray-store",
                "blobray-analysis",
                "blobray-knowledge",
                "blobray-verification",
            ]
            .as_slice(),
        ),
        (
            "blobray-next",
            [
                "blobray-domain",
                "blobray-application",
                "blobray-backend-riscv",
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
