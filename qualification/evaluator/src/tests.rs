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
            "schema = 5\nid = \"static\"\nrepetitions = 1\n[system]\nkind = \"boot-smoke\"\n",
        )
        .unwrap();
        fs::write(path.join("Cargo.toml"), "[workspace]\n").unwrap();
        fs::write(
            path.join("catalog/source.toml"),
            r#"schema = 3
id = "test-catalog"

[validation]
evidence-index = "vendor.json"
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
            capability: None,
            details: false,
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
fn engineering_commands_require_explicit_inputs_and_limit_focus_to_views() {
    for command in ["status", "next"] {
        let parsed = parse_arguments(
            [
                command,
                "--catalog",
                "catalog.toml",
                "--capability",
                "ble",
                "--json-report",
                "map.json",
            ]
            .map(str::to_owned),
        )
        .unwrap();
        assert_eq!(parsed.capability.as_deref(), Some("ble"));
        assert!(parse_arguments([command].map(str::to_owned)).is_err());
        assert!(
            parse_arguments(
                [command, "--manifest", "a.toml", "--catalog", "b.toml"].map(str::to_owned)
            )
            .is_err()
        );
    }
    assert!(
        parse_arguments(["gate", "--manifest", "a.toml", "--capability", "ble"].map(str::to_owned))
            .is_err()
    );
}

#[test]
fn absent_vendor_index_allows_status_and_hil_planning_without_qualifying_hardware() {
    for hardware in [false, true] {
        let root = StaticProgramRoot::new(if hardware {
            "missing-vendor-hardware"
        } else {
            "missing-vendor-owned"
        });
        root.write_program("\"base\"", "\"base\"");
        let catalog = root.path.join("catalog/source.toml");
        let mut document = fs::read_to_string(&catalog).unwrap().replace(
            "hil-not-applicable = \"source-only-contract\"",
            "hil-requirements = [{ scenario = \"static\", minimum-repetitions = 1 }]",
        );
        if hardware {
            document = document.replace(
                "vendor-not-applicable = \"source-only-contract\"",
                "vendor-roots = [{ source = \"libpp\", symbol = \"hardware_publish\" }]\nvendor-evidence = [{ suite = \"hardware\", source = \"libpp\", symbol = \"hardware_publish\" }]",
            );
        }
        fs::write(&catalog, document).unwrap();
        for args in [
            vec!["init", "-q"],
            vec!["add", "."],
            vec![
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "core.hooksPath=/dev/null",
                "commit",
                "-qm",
                "fixture",
            ],
        ] {
            assert!(
                std::process::Command::new("git")
                    .arg("-C")
                    .arg(&root.path)
                    .args(args)
                    .status()
                    .unwrap()
                    .success()
            );
        }
        for command in ["status", "plan"] {
            let report = format!("output/{command}.json");
            execute(
                parse_arguments(
                    [
                        command,
                        "--manifest",
                        "program.toml",
                        "--root",
                        root.path.to_str().unwrap(),
                        "--json-report",
                        &report,
                    ]
                    .map(str::to_owned),
                )
                .unwrap(),
            )
            .unwrap();
        }
        let status: serde_json::Value =
            serde_json::from_slice(&fs::read(root.path.join("output/status.json")).unwrap())
                .unwrap();
        let plan: serde_json::Value =
            serde_json::from_slice(&fs::read(root.path.join("output/plan.json")).unwrap()).unwrap();
        assert_eq!(
            status["entries"][0]["evidence"]["vendor"],
            if hardware { "mapped" } else { "not-applicable" }
        );
        assert_eq!(plan["obligations"].as_array().unwrap().len(), 1);
        assert_eq!(plan["obligations"][0]["action"], "run");
        assert!(!root.path.join("vendor.json").exists());
        fs::write(root.path.join("vendor.json"), "{broken").unwrap();
        assert!(
            execute(
                parse_arguments(
                    [
                        "plan",
                        "--manifest",
                        "program.toml",
                        "--root",
                        root.path.to_str().unwrap(),
                    ]
                    .map(str::to_owned)
                )
                .unwrap()
            )
            .is_err()
        );
    }
}

#[test]
fn status_reads_catalog_without_git_or_evidence_and_writes_the_same_map() {
    let root = StaticProgramRoot::new("engineering-map");
    // Invalid evidence must not be loaded in declarations-only mode.
    fs::write(root.path.join("vendor.json"), "invalid evidence").unwrap();
    let arguments = parse_arguments(
        [
            "status",
            "--catalog",
            "catalog/source.toml",
            "--root",
            root.path.to_str().unwrap(),
            "--json-report",
            "output/map.json",
        ]
        .map(str::to_owned),
    )
    .unwrap();
    execute(arguments).unwrap();
    let map: serde_json::Value =
        serde_json::from_slice(&fs::read(root.path.join("output/map.json")).unwrap()).unwrap();
    assert_eq!(map["mode"], "declarations-only");
    assert_eq!(map["entries"][0]["implementation"], "complete");
    assert!(map["entries"][0]["evidence"].is_null());
    assert!(!root.path.join("missing-runs").exists());
}

#[test]
fn development_links_are_validated_without_turning_them_into_proof() {
    let root = StaticProgramRoot::new("development-links");
    root.write_program("\"base\"", "\"base\"");
    fs::create_dir_all(root.path.join("crate/src")).unwrap();
    fs::write(
        root.path.join("crate/Cargo.toml"),
        "[package]\nname = \"test-package\"\n",
    )
    .unwrap();
    fs::write(
        root.path.join("crate/src/tests.rs"),
        "// reviewed test source\n",
    )
    .unwrap();
    let path = root.path.join("catalog/source.toml");
    let original = fs::read_to_string(&path).unwrap();
    let valid = r#"
[capabilities.development]
knowledge = ["Cargo.toml"]
host-tests = [{manifest = "crate/Cargo.toml", filter = "owner::tests", source = "crate/src/tests.rs"}]
"#;
    fs::write(&path, format!("{original}{valid}")).unwrap();
    root.check().unwrap();
    for invalid in [
        valid.replace(
            "knowledge = [\"Cargo.toml\"]",
            "knowledge = [\"../outside\"]",
        ),
        valid.replace("crate/src/tests.rs", "missing.rs"),
        valid.replace("filter = \"owner::tests\"", "filter = \" \""),
        format!(
            "{valid}\ngap-work = [{{gap = \"not-declared\", kind = \"research\", reason = \"Unknown behavior\"}}]\n"
        ),
    ] {
        fs::write(&path, format!("{original}{invalid}")).unwrap();
        assert!(root.check().is_err());
    }
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

#[test]
fn execution_plan_requires_an_evaluated_program() {
    assert!(
        parse_arguments(
            ["plan", "--manifest", "program.toml", "--capability", "wifi"].map(str::to_owned)
        )
        .is_ok()
    );
    assert!(parse_arguments(["plan", "--catalog", "catalog.toml"].map(str::to_owned)).is_err());
}
