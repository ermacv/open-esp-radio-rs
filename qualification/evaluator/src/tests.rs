use super::*;
use std::{
    fs,
    path::{Path, PathBuf},
};

struct StaticProgramRoot {
    path: PathBuf,
}

impl StaticProgramRoot {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "open-radio-static-program-{}-{name}",
            std::process::id()
        ));
        if path.exists() {
            fs::remove_dir_all(&path).unwrap();
        }
        fs::create_dir_all(path.join("catalog")).unwrap();
        fs::create_dir_all(path.join("scenarios")).unwrap();
        fs::write(
            path.join("scenarios/static.toml"),
            "schema = 4\nid = \"static\"\nrepetitions = 1\n",
        )
        .unwrap();
        fs::write(path.join("Cargo.toml"), "[workspace]\n").unwrap();
        fs::write(
            path.join("verification.toml"),
            "id = \"test\"\nverification-addon = \"verification-addon.toml\"\n",
        )
        .unwrap();
        fs::write(
            path.join("verification-addon.toml"),
            "evidence-index = \"vendor.json\"\n",
        )
        .unwrap();
        fs::write(
            path.join("catalog/source.toml"),
            r#"schema = 2
id = "test-catalog"

[validation]
verification-project = "verification.toml"
hil-catalog = "scenarios"

[[capabilities]]
id = "base"
title = "Base"
scope = "Static source scope"
implementation = "complete"
host = "covered"
async = "not-applicable"
vendor-not-applicable = "source-only-contract"
hil-not-applicable = "source-only-contract"
async-not-applicable = "no-asynchronous-boundary"

[capabilities.catalog-scope]
chip = "test-chip"
role = "test-role"
phy = "test-phy"
security = ["not-applicable"]
composition = "test-composition"
level = "lower-primitive"
activation-boundary = "One static call"
limitations = "No readiness claim"
"#,
        )
        .unwrap();
        Self { path }
    }

    fn write_program(&self, required: &str, selected: &str) {
        fs::write(
            self.path.join("program.toml"),
            format!(
                r#"schema = 4
target = "test-program"
required-capabilities = [{required}]
catalogs = ["catalog/source.toml"]
catalog-capabilities = [{selected}]

[verification]
project = "verification.toml"
evidence-index = "vendor.json"

[hil]
target = "test-target"
catalog = "scenarios"
runs = "missing-runs"
"#
            ),
        )
        .unwrap();
    }

    fn check(&self) -> Result<()> {
        execute(Arguments {
            command: Command::CatalogCheck,
            manifest: Some(PathBuf::from("program.toml")),
            catalogs: Vec::new(),
            root: self.path.clone(),
            json_report: None,
            output_directory: None,
        })
    }
}

impl Drop for StaticProgramRoot {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.path).unwrap();
    }
}

#[test]
fn command_requires_one_explicit_manifest() {
    let parsed = parse_arguments([
        "evaluate".to_owned(),
        "--manifest".to_owned(),
        "qualification/test.toml".to_owned(),
    ])
    .unwrap();
    assert_eq!(parsed.command, Command::Evaluate);
    assert_eq!(
        parsed.manifest.as_deref(),
        Some(Path::new("qualification/test.toml"))
    );
    assert!(parsed.json_report.is_none());
    assert!(parsed.output_directory.is_none());
}

#[test]
fn removed_check_command_is_rejected() {
    let error = parse_arguments([
        "check".to_owned(),
        "--manifest".to_owned(),
        "qualification/test.toml".to_owned(),
    ])
    .unwrap_err();
    assert!(error.to_string().contains("unknown qualification command"));
}

#[test]
fn command_rejects_silent_extra_options() {
    let error = parse_arguments([
        "gate".to_owned(),
        "--manifest".to_owned(),
        "test.toml".to_owned(),
        "--best-effort".to_owned(),
    ])
    .unwrap_err();
    assert!(error.to_string().contains("unknown option"));
}

#[test]
fn catalog_render_requires_an_output_directory() {
    let error = parse_arguments([
        "catalog".to_owned(),
        "render".to_owned(),
        "--manifest".to_owned(),
        "qualification/test.toml".to_owned(),
    ])
    .unwrap_err();
    assert!(error.to_string().contains("requires --out"));

    let parsed = parse_arguments([
        "catalog".to_owned(),
        "render".to_owned(),
        "--manifest".to_owned(),
        "qualification/test.toml".to_owned(),
        "--out".to_owned(),
        "target/qualification/catalog".to_owned(),
    ])
    .unwrap();
    assert_eq!(parsed.command, Command::CatalogRender);
    assert_eq!(
        parsed.output_directory.as_deref(),
        Some(Path::new("target/qualification/catalog"))
    );
}

#[test]
fn static_catalog_form_is_distinct_from_manifest_form() {
    let parsed = parse_arguments([
        "catalog".to_owned(),
        "check".to_owned(),
        "--catalog".to_owned(),
        "qualification/catalog/a.toml".to_owned(),
        "--catalog".to_owned(),
        "qualification/catalog/b.toml".to_owned(),
    ])
    .unwrap();
    assert!(parsed.manifest.is_none());
    assert_eq!(parsed.catalogs.len(), 2);

    let error = parse_arguments([
        "catalog".to_owned(),
        "check".to_owned(),
        "--manifest".to_owned(),
        "qualification/program.toml".to_owned(),
        "--catalog".to_owned(),
        "qualification/catalog/a.toml".to_owned(),
    ])
    .unwrap_err();
    assert!(error.to_string().contains("exactly one"));
}

#[test]
fn help_forms_are_explicit_and_finite() {
    assert!(is_help(&["--help".to_owned()]));
    assert!(is_help(&["catalog".to_owned(), "--help".to_owned()]));
    assert!(!is_help(&["evaluate".to_owned(), "--help".to_owned(),]));
}

#[test]
fn manifest_catalog_check_rejects_unknown_selection_without_evidence_outputs() {
    let root = StaticProgramRoot::new("unknown-selection");
    root.write_program("\"base\"", "\"missing\"");
    assert!(!root.path.join("vendor.json").exists());
    assert!(!root.path.join("missing-runs").exists());
    let error = root.check().unwrap_err().to_string();
    assert!(
        error.contains("unknown catalog capability missing"),
        "{error}"
    );
}

#[test]
fn manifest_catalog_check_rejects_non_exact_required_set_without_evidence_outputs() {
    let root = StaticProgramRoot::new("required-set");
    root.write_program("\"other\"", "\"base\"");
    assert!(!root.path.join("vendor.json").exists());
    assert!(!root.path.join("missing-runs").exists());
    let error = root.check().unwrap_err().to_string();
    assert!(error.contains("qualification root mismatch"), "{error}");
    assert!(error.contains("missing=[other]"), "{error}");
    assert!(error.contains("undeclared=[base]"), "{error}");
}

#[test]
fn manifest_catalog_check_rejects_repeated_invalid_and_incompatible_ids() {
    let root = StaticProgramRoot::new("id-validation");
    let cases = [
        (
            "\"base\"",
            "\"base\", \"base\"",
            "repeats catalog capability base",
        ),
        (
            "\"base\"",
            "\"Bad\"",
            "invalid catalog capability reference",
        ),
        (
            "\"base\", \"base\"",
            "\"base\"",
            "duplicate required capability base",
        ),
        ("\"Bad\"", "\"base\"", "invalid required capability"),
        ("", "\"base\"", "has no required capabilities"),
        (
            "\"base\"",
            "",
            "catalogs and catalog-capabilities must either both be present or both be absent",
        ),
    ];
    for (required, selected, expected) in cases {
        root.write_program(required, selected);
        let error = root.check().unwrap_err().to_string();
        assert!(
            error.contains(expected),
            "expected {expected:?}, got {error:?}"
        );
    }
}
