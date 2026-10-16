//! Hardware obligations stay visible as vendor gaps.
use super::*;

#[test]
fn hardware_obligations_keep_their_vendor_gaps() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let bluetooth = ManifestDocument::load_and_validate(
        &program(&root, "bluetooth-peripheral-acl.toml"),
        &root,
    )
    .unwrap()
    .document;
    for id in ["peripheral-acl", "peripheral-link-security"] {
        let hardware = bluetooth.catalog.capabilities.get(id).unwrap();
        assert!(hardware.vendor_not_applicable.is_none());
        assert!(
            hardware
                .gaps
                .iter()
                .any(|g| g.axis == crate::model::Axis::Vendor)
        );
    }
}

/// The chip program `name` of this checkout: `qualification/targets/<chip>/<name>`.
fn program(root: &Path, name: &str) -> std::path::PathBuf {
    std::fs::read_dir(root.join("qualification/targets"))
        .unwrap()
        .map(|chip| chip.unwrap().path().join(name))
        .find(|program| program.is_file())
        .unwrap_or_else(|| panic!("no chip has the program {name}"))
}
