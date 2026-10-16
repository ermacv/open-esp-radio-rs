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
        fs::create_dir_all(path.join("qualification/targets")).unwrap();
        fs::write(
            path.join("scenarios/static.toml"),
            "schema = 5\nid = \"static\"\ndescription = \"static\"\nrole = \"investigation\"\nrepetitions = 1\n[system]\nkind = \"boot-smoke\"\n",
        )
        .unwrap();
        fs::write(path.join("Cargo.toml"), "[workspace]\n").unwrap();
        fs::write(
            path.join("catalog/source.toml"),
            r#"schema = 4
id = "test-catalog"

[validation]
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
                r#"schema = 5
target = "test-program"
required-capabilities = [{required}]
catalogs = ["catalog/source.toml"]
catalog-capabilities = [{selected}]

[hil]
target = "test-target"
catalog = "scenarios"
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
            archive: None,
            run: None,
            candidate: None,
            changed: Vec::new(),
            base: None,
            drops: None,
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
        parse_arguments(
            ["evaluate", "--manifest", "a.toml", "--capability", "ble"].map(str::to_owned)
        )
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
        // The program now references `static`: a qualification scenario.
        let scenario = root.path.join("scenarios/static.toml");
        let qualified = fs::read_to_string(&scenario)
            .unwrap()
            .replace("role = \"investigation\"", "role = \"qualification\"");
        fs::write(&scenario, qualified).unwrap();
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
            oer_process::git::output(&root.path, args).unwrap();
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
        assert!(
            !root
                .path
                .join("target/verification/test-target/evidence")
                .exists()
        );
        fs::create_dir_all(root.path.join("target/verification/test-target")).unwrap();
        fs::write(
            root.path.join("target/verification/test-target/evidence"),
            "{broken",
        )
        .unwrap();
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
    fs::create_dir_all(root.path.join("target/verification/test-target")).unwrap();
    fs::write(
        root.path.join("target/verification/test-target/evidence"),
        "invalid evidence",
    )
    .unwrap();
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
fn removed_commands_are_rejected() {
    for removed in ["gate", "hil-evidence"] {
        let error = parse_arguments([
            removed.to_owned(),
            "--manifest".to_owned(),
            "qualification/test.toml".to_owned(),
        ])
        .unwrap_err();
        assert!(error.to_string().contains("unknown qualification command"));
    }
    for words in [
        vec!["check"],
        vec!["check", "refs"],
        vec!["check", "--manifest", "m.toml"],
    ] {
        assert_eq!(
            usage_status(
                &words
                    .iter()
                    .map(|word| word.to_string())
                    .collect::<Vec<_>>()
            ),
            2,
            "{words:?}"
        );
    }
    assert_eq!(usage_status(&["verdict".to_owned()]), 2);
    assert_eq!(usage_status(&["gate".to_owned()]), 1);
    // `check` is a group: only `check references` remains of it.
    let error = parse_arguments(["check", "--manifest", "m.toml"].map(str::to_owned)).unwrap_err();
    assert!(
        error.to_string().contains("unknown check operation"),
        "{error}"
    );
}

#[test]
fn command_rejects_silent_extra_options() {
    let error = parse_arguments([
        "evaluate".to_owned(),
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
    assert!(
        !root
            .path
            .join("target/verification/test-target/evidence")
            .exists()
    );
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
    assert!(
        !root
            .path
            .join("target/verification/test-target/evidence")
            .exists()
    );
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
            "catalogs and catalog-capabilities are required",
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

#[test]
fn readiness_requires_its_output_directory() {
    let parse = |words: &[&str]| parse_arguments(words.iter().map(|w| w.to_string()));
    let parsed = parse(&["readiness", "--out", "o", "--base", "b", "--drops", "d.md"]).unwrap();
    assert_eq!(parsed.base, Some(PathBuf::from("b")));
    assert_eq!(parsed.drops, Some(PathBuf::from("d.md")));
    assert!(parse(&["readiness", "--base", "b"]).is_err());
}

#[test]
fn each_command_accepts_exactly_the_options_its_tree_lists() {
    let every = Command::ALL
        .iter()
        .flat_map(|command| command.options())
        .copied()
        .collect::<std::collections::BTreeSet<_>>();
    for command in Command::ALL {
        let required: &[&str] = match command {
            Command::CatalogAnchors => &["--catalog", "c.toml"],
            Command::CatalogRender => &["--manifest", "m.toml", "--out", "out"],
            Command::Readiness => &["--out", "out"],
            Command::CheckReferences => &[],
            Command::Verdict => &["--run", "run"],
            _ => &["--manifest", "m.toml"],
        };
        let base = command
            .words()
            .iter()
            .chain(required)
            .map(|word| word.to_string())
            .collect::<Vec<_>>();
        parse_arguments(base.clone()).unwrap();
        for option in every.iter().filter(|option| !required.contains(option)) {
            let mut arguments = base.clone();
            // The alternatives to a manifest replace it.
            if *option == "--catalog"
                && let Some(at) = arguments.iter().position(|word| word == "--manifest")
            {
                arguments.drain(at..at + 2);
            }
            arguments.push(option.to_string());
            if *option != "--details" {
                arguments.push(String::from("value"));
            }
            assert_eq!(
                parse_arguments(arguments).is_ok(),
                command.options().contains(option),
                "{command:?} {option}"
            );
        }
    }
    let tree = command_tree();
    assert_eq!(
        tree[0].subcommands,
        [
            "status",
            "next",
            "plan",
            "validate",
            "evaluate",
            "check",
            "verdict",
            "readiness",
            "catalog"
        ]
    );
    let catalog = tree
        .iter()
        .find(|node| node.path == ["qualification", "catalog"])
        .unwrap();
    assert_eq!(catalog.subcommands, ["check", "render", "anchors"]);
    let check = tree
        .iter()
        .find(|node| node.path == ["qualification", "check"])
        .unwrap();
    assert_eq!(check.subcommands, ["references"]);
    let render = tree
        .iter()
        .find(|node| node.path == ["qualification", "catalog", "render"])
        .unwrap();
    assert!(render.flags.iter().any(|flag| flag == "--out"));
}

#[test]
fn an_archive_copy_is_read_only_against_the_pin() {
    let root = tempfile::tempdir().unwrap();
    assert!(archive_copy(root.path(), None).unwrap().is_none());
    // Without a pin, no copy is any version's.
    let error = archive_copy(root.path(), Some(Path::new("copy"))).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("pins no evidence archive version"),
        "{error}"
    );
    // A copy that is not the pinned version's is refused.
    let manifest = oer_hil_evidence_archive::Manifest::new(
        oer_hil_evidence_archive::Payload {
            sha256: oer_hil_evidence_archive::Sha256::of(b"payload"),
            bytes: 7,
        },
        vec![oer_hil_evidence_archive::RunRef {
            run: oer_hil_evidence_archive::RunId::parse("1-a").unwrap(),
            sha256: oer_hil_evidence_archive::Sha256::of(b"seal"),
        }],
    )
    .unwrap();
    oer_hil_evidence_archive::Pin::new(
        oer_hil_evidence_archive::Repository::parse("o/evidence").unwrap(),
        manifest,
    )
    .write(&root.path().join(oer_hil_evidence_archive::PIN))
    .unwrap();
    fs::create_dir(root.path().join("copy")).unwrap();
    fs::write(root.path().join("copy/manifest.json"), "{}").unwrap();
    let error = archive_copy(root.path(), Some(Path::new("copy"))).unwrap_err();
    assert!(
        error.to_string().contains("is not the manifest of version"),
        "{error}"
    );
}

fn judged(status: &'static str) -> verdict::Judged {
    verdict::Judged {
        program: String::from("qualification/targets/p.toml"),
        capability: String::from("base"),
        scenario: String::from("static"),
        status,
        passed: usize::from(status == "satisfied"),
        failed: usize::from(status != "satisfied"),
        excluded: 0,
    }
}

#[test]
fn a_verdict_is_never_evidence_and_fails_on_an_unmet_requirement() {
    let run = Path::new("runs/1-a");
    let (lines, outcome) = verdict::report(run, &[judged("satisfied")]);
    assert!(outcome.is_ok());
    assert_eq!(lines.len(), 3);
    assert!(
        lines.iter().all(|line| line.contains("not-evidence")),
        "{lines:?}"
    );
    assert!(lines[0].contains("not-published") && lines[2].contains("not-published"));
    let (_, outcome) = verdict::report(run, &[judged("satisfied"), judged("unresolved-failure")]);
    assert!(outcome.unwrap_err().is::<verdict::Unmet>());
    // A run that observed nothing a program requires gives no verdict.
    let (_, outcome) = verdict::report(run, &[]);
    let error = outcome.unwrap_err();
    assert!(!error.is::<verdict::Unmet>());
    assert!(
        error.to_string().contains("observed no scenario"),
        "{error}"
    );
}

#[test]
fn a_verdict_reads_a_run_of_another_chip_as_none_and_refuses_a_missing_one() {
    let root = StaticProgramRoot::new("verdict-other-chip");
    root.write_program("\"base\"", "\"base\"");
    fs::create_dir_all(root.path.join(model::PROGRAMS)).unwrap();
    fs::rename(
        root.path.join("program.toml"),
        root.path.join(model::PROGRAMS).join("test.toml"),
    )
    .unwrap();
    let run = root.path.join("runs/1-a");
    fs::create_dir_all(&run).unwrap();
    fs::write(
        run.join("manifest.json"),
        r#"{"schema": 0, "run_id": "1-a", "target": "other-chip"}"#,
    )
    .unwrap();
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
        oer_process::git::output(&root.path, args).unwrap();
    }
    assert_eq!(verdict::judge(&root.path, &run).unwrap(), []);
    // A bundle of the program's chip that fails validation names why.
    let broken = root.path.join("runs/2-b");
    fs::create_dir_all(&broken).unwrap();
    fs::write(
        broken.join("manifest.json"),
        format!(
            r#"{{"schema": {}, "run_id": "2-b", "target": "test-target"}}"#,
            oer_hil_run_bundle_format::run::RUN_SCHEMA
        ),
    )
    .unwrap();
    let error = verdict::judge(&root.path, &broken).unwrap_err();
    assert!(error.is::<verdict::Invalid>(), "{error}");
    assert!(error.to_string().contains("not valid: "), "{error}");
    assert_eq!(exit_status(&*error, false), 1);
    let error = verdict::judge(&root.path, &root.path.join("runs/9-z")).unwrap_err();
    assert!(error.is::<verdict::Unjudged>(), "{error}");
    assert!(error.to_string().contains("no run bundle"), "{error}");
    assert_eq!(exit_status(&*error, false), 1);
}

/// No catalog record takes a field its reader does not know: a run
/// reference cannot be written past `check references` (owner decision
/// 6226361520 on #761, the citing field comes with its first record).
#[test]
fn catalogs_refuse_fields_they_do_not_read() {
    for (index, (anchor, field)) in [
        ("scope = \"Static source scope\"", "\ncited-runs = []"),
        ("limitations = \"No readiness claim\"", "\nruns = []"),
    ]
    .into_iter()
    .enumerate()
    {
        let root = StaticProgramRoot::new(&format!("unknown-field-{index}"));
        root.write_program("\"base\"", "\"base\"");
        let catalog = root.path.join("catalog/source.toml");
        let text = fs::read_to_string(&catalog).unwrap();
        assert!(text.contains(anchor), "{anchor}");
        fs::write(
            &catalog,
            text.replacen(anchor, &format!("{anchor}{field}"), 1),
        )
        .unwrap();
        let error = root.check().unwrap_err().to_string();
        assert!(error.contains("unknown field"), "{field}: {error}");
    }
}

#[test]
fn check_references_and_verdict_exit_by_what_failed() {
    assert_eq!(exit_status(&oer_process::Cancelled, false), 130);
    let boxed: Box<dyn Error> = boxed(Box::new(oer_process::Cancelled));
    assert_eq!(exit_status(&*boxed, false), 130);
    assert_eq!(
        exit_status(
            &oer_hil_evidence_archive::Refused(String::from("rule")),
            false
        ),
        1
    );
    assert_eq!(exit_status(&verdict::Unmet(1), false), 1);
    assert_eq!(
        exit_status(&verdict::Unjudged(String::from("runs/1-a")), false),
        1
    );
    assert_eq!(
        exit_status(&verdict::Invalid(String::from("seal")), false),
        1
    );
    let failed: Box<dyn Error> = "read failed".into();
    assert_eq!(exit_status(&*failed, false), 3);
    // A failure reported only as text, such as an interrupted git query,
    // is the cancellation's once Ctrl-C was requested.
    assert_eq!(exit_status(&*failed, true), 130);
}

#[test]
fn status_next_and_plan_say_which_archive_they_read() {
    assert_eq!(archive_input(None), "INPUT\thil-archive=none");
    assert_eq!(
        archive_input(Some(String::from("evidence-0123"))),
        "INPUT\thil-archive=evidence-0123"
    );
    // `plan`'s stdout stays its JSON.
    assert!(input_on_stderr(Command::Plan));
    assert!(!input_on_stderr(Command::Status));
    assert!(!input_on_stderr(Command::Next));
}

#[test]
fn only_a_bundle_failure_makes_a_verdict_invalid() {
    let io: Box<dyn Error> = Box::new(std::io::Error::other("store unreadable"));
    assert!(!hil::bundle_error(io).is::<verdict::Invalid>());
    let cancelled: Box<dyn Error> = Box::new(oer_process::Cancelled);
    assert!(!hil::bundle_error(cancelled).is::<verdict::Invalid>());
    let broken: Box<dyn Error> = "a missing observer build".into();
    assert!(hil::bundle_error(broken).is::<verdict::Invalid>());
}
