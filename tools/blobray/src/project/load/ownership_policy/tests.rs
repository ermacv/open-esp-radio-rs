use std::{fs, path::PathBuf};

use crate::project::ProjectSpec;

fn fixture(name: &str, policy: &str, extra: &str) -> (PathBuf, PathBuf) {
    let root = std::env::temp_dir().join(format!(
        "blobray-register-ownership-{name}-{}",
        std::process::id(),
    ));
    fs::create_dir_all(root.join("project")).unwrap();
    fs::create_dir_all(root.join("policy")).unwrap();
    fs::write(root.join("policy/ownership.toml"), policy).unwrap();
    let manifest = root.join("project/vendor-project.toml");
    fs::write(
        &manifest,
        format!(
            "schema = 4\nid = \"fixture\"\ntarget-spec = \"target.toml\"\n\
         [registers]\nfacts = \"generated/mmio.json\"\nmodel = \"model.toml\"\n\
         ownership-policy = \"../policy/ownership.toml\"\n{extra}",
        ),
    )
    .unwrap();
    (root, manifest)
}

#[test]
fn shared_ownership_policy_resolves_from_each_manifest_without_context_inheritance() {
    let (root, manifest) = fixture(
        "relative",
        "schema = 1\nowned-ranges = [\"radio\", \"coex\"]\n",
        "",
    );
    let project = ProjectSpec::load(&manifest).unwrap();
    let registers = project.registers.unwrap();
    assert_eq!(registers.owned_ranges, ["radio", "coex"]);
    assert_eq!(
        registers.ownership_policy.unwrap().canonicalize().unwrap(),
        root.join("policy/ownership.toml")
    );
    assert_eq!(registers.model, root.join("project/model.toml"));
    assert!(registers.evidence_catalogs.is_empty());
    assert!(registers.lint_pack.is_none());
    assert!(project.analysis_provider.is_none());

    fs::create_dir_all(root.join("other/nested")).unwrap();
    let other = root.join("other/nested/publication.toml");
    let input = fs::read_to_string(&manifest)
        .unwrap()
        .replace("../policy/ownership.toml", "../../policy/ownership.toml");
    fs::write(&other, input).unwrap();
    assert_eq!(
        ProjectSpec::load(&other)
            .unwrap()
            .registers
            .unwrap()
            .owned_ranges,
        ["radio", "coex"]
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn ownership_policy_cannot_be_overridden_or_extended_inline() {
    let (root, manifest) = fixture(
        "ambiguous",
        "schema = 1\nowned-ranges = [\"radio\"]\n",
        "owned-ranges = [\"coex\"]\n",
    );
    let error = ProjectSpec::load(&manifest).unwrap_err();
    assert!(error.to_string().contains("not both"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn ownership_policy_rejects_missing_and_malformed_inputs() {
    for (name, input, expected) in [
        (
            "schema",
            "schema = 2\nowned-ranges = [\"radio\"]",
            "schema = 1",
        ),
        ("empty", "schema = 1\nowned-ranges = []", "at least one"),
        (
            "duplicate",
            "schema = 1\nowned-ranges = [\"radio\", \"radio\"]",
            "duplicate",
        ),
        (
            "unknown",
            "schema = 1\nowned-ranges = [\"radio\"]\nanalysis-provider = \"unsafe-overlay\"",
            "unknown register ownership policy key",
        ),
        ("invalid", "schema = 1\nowned-ranges = [", "unclosed array"),
    ] {
        let (root, manifest) = fixture(name, input, "");
        let error = ProjectSpec::load(&manifest).unwrap_err();
        assert!(error.to_string().contains(expected), "{name}: {error}");
        fs::remove_dir_all(root).unwrap();
    }
    let (root, manifest) = fixture("missing", "", "");
    fs::remove_file(root.join("policy/ownership.toml")).unwrap();
    assert!(
        ProjectSpec::load(&manifest)
            .unwrap_err()
            .to_string()
            .contains("cannot read register ownership policy")
    );
    fs::remove_dir_all(root).unwrap();
}
