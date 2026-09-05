use std::{fs, path::PathBuf};

use super::*;

fn fixture(name: &str) -> (PathBuf, crate::project::ProjectSpec) {
    let root = std::env::temp_dir().join(format!(
        "blobray-closed-pac-publication-{name}-{}",
        std::process::id(),
    ));
    fs::create_dir_all(&root).unwrap();
    fs::write(
        root.join("model.toml"),
        concat!(
            "schema = 3\nchip = \"fixture-chip\"\nfragments = [\"radio.toml\"]\n",
            "[device]\nname = \"FIXTURE\"\nversion = \"1\"\ndescription = \"fixture\"\n",
            "address-unit-bits = 8\nwidth = 32\n",
        ),
    )
    .unwrap();
    fs::write(
        root.join("radio.toml"),
        concat!(
            "schema = 2\n[[peripherals]]\nname = \"RADIO\"\nbaseAddress = 0x1000\n",
            "[[peripherals.registers]]\n[peripherals.registers.register]\n",
            "name = \"CONTROL\"\naddressOffset = 0\nsize = 32\naccess = \"read-write\"\n",
        ),
    )
    .unwrap();
    fs::write(root.join("api.toml"), "schema = 5\n[options]\n").unwrap();
    fs::write(
        root.join("ownership.toml"),
        "schema = 1\nowned-ranges = [\"radio\"]\n",
    )
    .unwrap();
    let manifest = root.join("vendor-project.toml");
    fs::write(
        &manifest,
        concat!(
            "schema = 4\nid = \"source-only\"\ntarget-spec = \"target.toml\"\n",
            "[registers]\nfacts = \"absent-mmio.json\"\nmodel = \"model.toml\"\n",
            "ownership-policy = \"ownership.toml\"\n",
            "[registers.api]\npack = \"api.toml\"\noutput = \"generated.rs\"\n",
        ),
    )
    .unwrap();
    let project = crate::project::ProjectSpec::load(&manifest).unwrap();
    (root, project)
}

#[test]
fn closed_pac_api_publication_checks_staleness_without_artifact_review() {
    let (root, project) = fixture("stale");
    assert!(project.review.is_none());
    assert!(project.analysis_provider.is_none());
    let paths = project.registers.as_ref().unwrap();
    let output = root.join("generated.rs");
    assert!(generate_pac_api(CheckArgs { check: true }, paths).is_err());
    assert!(!output.exists());
    assert!(generate_pac_api(CheckArgs::default(), paths).unwrap());
    let generated = fs::read(&output).unwrap();
    assert!(generate_pac_api(CheckArgs { check: true }, paths).unwrap());

    fs::write(&output, "stale output").unwrap();
    assert!(generate_pac_api(CheckArgs { check: true }, paths).is_err());
    assert_eq!(fs::read_to_string(&output).unwrap(), "stale output");
    assert!(generate_pac_api(CheckArgs::default(), paths).unwrap());
    assert_eq!(fs::read(&output).unwrap(), generated);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn closed_pac_api_publication_rejects_missing_or_unsupported_policy_before_writing() {
    let (root, mut project) = fixture("policy");
    let paths = project.registers.as_mut().unwrap();
    let original = paths.api_pack.take();
    assert!(generate_pac_api(CheckArgs::default(), paths).is_err());
    paths.api_pack = original;
    fs::write(root.join("api.toml"), "schema = 999\n[options]\n").unwrap();
    assert!(generate_pac_api(CheckArgs::default(), paths).is_err());
    fs::remove_file(root.join("api.toml")).unwrap();
    assert!(generate_pac_api(CheckArgs::default(), paths).is_err());
    assert!(!root.join("generated.rs").exists());
    fs::remove_dir_all(root).unwrap();
}
