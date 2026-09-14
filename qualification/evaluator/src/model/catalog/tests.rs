use super::*;
use crate::{
    hil::{HilEvidenceIndex, RepositoryState, ScenarioCatalog},
    model::{
        DispositionEntry, DispositionIndex, EvaluationContext, EvidenceInputs, HilProof,
        Qualification, VendorEvidenceArtifactHash, VendorEvidenceIndex, VendorEvidenceIndexEntry,
        VendorEvidenceSourceHash, VendorProof, evaluate_capability, validate_dependencies,
    },
};

const PROGRAM_PREFIX: &str = r#"
schema = 4
target = "test-radio"
required-capabilities = ["base-phy", "wifi-channel"]

[verification]
project = "verification.toml"
evidence-index = "vendor.json"

[hil]
target = "test-radio"
catalog = "scenarios"
runs = "runs"
"#;

const BASE_PHY: &str = r#"
[[capabilities]]
id = "base-phy"
title = "Base PHY"
scope = "One base initialization"
implementation = "complete"
host = "covered"
async = "bounded"
vendor-anchors = ["Cargo.toml"]
hil-requirements = [{ scenario = "wifi-channel", minimum-repetitions = 1 }]
gaps = [{ axis = "vendor", id = "vendor-trace-missing" }]
"#;

const WIFI_CHANNEL: &str = r#"
[[capabilities]]
id = "wifi-channel"
title = "Wi-Fi channel"
scope = "One composed Wi-Fi channel switch"
implementation = "complete"
host = "covered"
async = "bounded"
depends-on = ["base-phy"]
vendor-anchors = ["Cargo.toml"]
hil-requirements = [{ scenario = "wifi-channel", minimum-repetitions = 1 }]
gaps = [{ axis = "vendor", id = "vendor-trace-missing" }]
"#;

const BASE_SCOPE: &str = r#"
[capabilities.catalog-scope]
chip = "test-chip"
role = "shared-phy"
phy = "wifi-2g4"
security = ["not-applicable"]
composition = "production-phy-initialization"
level = "lower-primitive"
activation-boundary = "Before Wi-Fi MAC activation"
limitations = "No RF qualification"
"#;

const WIFI_SCOPE: &str = r#"
[capabilities.catalog-scope]
chip = "test-chip"
role = "station"
phy = "wifi-2g4"
security = ["independent-of-link-security"]
composition = "connected-station"
level = "composed-product"
activation-boundary = "After MAC stop and before restart"
limitations = "No measured channel correctness"
"#;

struct TestRoot {
    path: PathBuf,
}

impl TestRoot {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "open-radio-capability-catalog-{}-{name}",
            std::process::id()
        ));
        if path.exists() {
            fs::remove_dir_all(&path).unwrap();
        }
        fs::create_dir_all(path.join("catalog")).unwrap();
        fs::create_dir_all(path.join("scenarios")).unwrap();
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
            path.join("scenarios/wifi-channel.toml"),
            "schema = 4\nid = \"wifi-channel\"\nrepetitions = 1\n",
        )
        .unwrap();
        fs::write(
            path.join("scenarios/base-phy.toml"),
            "schema = 4\nid = \"base-phy\"\nrepetitions = 1\n",
        )
        .unwrap();
        Self { path }
    }

    fn write_catalog(&self, body: &str) {
        fs::write(self.path.join("catalog/wifi.toml"), body).unwrap();
    }

    fn write_named_catalog(&self, name: &str, body: &str) {
        fs::write(self.path.join(format!("catalog/{name}.toml")), body).unwrap();
    }
}

impl Drop for TestRoot {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.path).unwrap();
    }
}

fn parse(input: &str) -> ManifestDocument {
    toml_edit::de::from_str(input).unwrap()
}

fn catalog_program(path: &str, capability: &str) -> String {
    PROGRAM_PREFIX.replace(
        "\n[verification]",
        &format!(
            "\ncatalogs = [\"{path}\"]\ncatalog-capabilities = [\"{capability}\"]\n\n[verification]"
        ),
    )
}

fn resolution_error(result: Result<ManifestDocument>) -> String {
    match result {
        Ok(_) => panic!("invalid catalog input was accepted"),
        Err(error) => error.to_string(),
    }
}

#[test]
fn resolution_preserves_evaluation_and_missing_evidence() {
    let root = TestRoot::new("equivalence");
    let unselected = BASE_PHY.replace("id = \"base-phy\"", "id = \"unselected-phy\"");
    root.write_catalog(&format!(
        "schema = 2\nid = \"test-wifi-phy\"\n{BASE_PHY}{BASE_SCOPE}{WIFI_CHANNEL}{WIFI_SCOPE}{unselected}{BASE_SCOPE}"
    ));
    let canonical_input = catalog_program("catalog/wifi.toml", "wifi-channel");
    let legacy_input = format!("{PROGRAM_PREFIX}{BASE_PHY}{WIFI_CHANNEL}");
    let canonical = parse(&canonical_input)
        .resolve_catalogs(&root.path, Path::new("canonical.toml"), &canonical_input)
        .unwrap();
    let legacy = parse(&legacy_input)
        .resolve_catalogs(&root.path, Path::new("legacy.toml"), &legacy_input)
        .unwrap();

    assert_eq!(canonical.catalog.capabilities.len(), 3);
    assert!(
        canonical
            .catalog
            .capabilities
            .contains_key("unselected-phy")
    );
    assert!(!canonical.capability_origins.contains_key("unselected-phy"));

    let mut canonical_documents = canonical.capabilities;
    let mut legacy_documents = legacy.capabilities;
    canonical_documents.sort_by(|left, right| left.id.cmp(&right.id));
    legacy_documents.sort_by(|left, right| left.id.cmp(&right.id));
    assert_eq!(canonical_documents.len(), 2);
    let canonical_without_scope = canonical_documents
        .iter()
        .cloned()
        .map(|mut capability| {
            capability.catalog_scope = None;
            capability
        })
        .collect::<Vec<_>>();
    assert_eq!(canonical_without_scope, legacy_documents);

    let dispositions = DispositionIndex {
        entries: BTreeMap::new(),
        project_id: "test".to_owned(),
        vendor_evidence_index: None,
    };
    let vendor_index = VendorEvidenceIndex {
        schema_version: 1,
        command: "project verify vendor evidence index".to_owned(),
        project: "test".to_owned(),
        complete_project_run: true,
        entries: Vec::new(),
    };
    let scenario_catalog = ScenarioCatalog::load(&root.path, Path::new("scenarios")).unwrap();
    let hil_index = HilEvidenceIndex::default();
    let context = EvaluationContext {
        root: &root.path,
        dispositions: &dispositions,
        vendor_index: &vendor_index,
        scenario_catalog: &scenario_catalog,
        hil_index: &hil_index,
        evaluator_clean: true,
    };
    let evaluate = |documents: Vec<CapabilityDocument>| {
        documents
            .into_iter()
            .map(|document| evaluate_capability(document, &context).unwrap())
            .map(|capability| (capability.id.clone(), capability))
            .collect::<BTreeMap<_, _>>()
    };
    let evaluated = evaluate(canonical_documents);
    let legacy_evaluated = evaluate(legacy_documents);
    assert_eq!(evaluated, legacy_evaluated);
    validate_dependencies(&evaluated).unwrap();
    assert!(evaluated.values().all(|capability| {
        capability.vendor == VendorProof::Mapped
            && capability.hil == HilProof::Missing
            && !capability.proof_ready()
    }));
    assert_eq!(evaluated["wifi-channel"].dependencies, ["base-phy"]);

    let qualification = Qualification {
        target: "test-radio".to_owned(),
        repository: RepositoryState {
            commit: "test".to_owned(),
            dirty: false,
        },
        evidence_inputs: EvidenceInputs {
            verification_entries: 0,
            verification_current_release_entries: 0,
            hil: Default::default(),
            verification_project: PathBuf::from("verification.toml"),
            vendor_evidence_index: PathBuf::from("vendor.json"),
            hil_catalog: PathBuf::from("scenarios"),
            hil_runs: PathBuf::from("runs"),
        },
        capabilities: evaluated,
        program_source: canonical.program_source.unwrap(),
        catalog_sources: canonical.catalog_sources,
        capability_origins: canonical.capability_origins,
        catalog_scopes: canonical.catalog_scopes,
        catalog: Default::default(),
        direct_catalog_capabilities: Default::default(),
    };
    assert_eq!(qualification.ready_count(), 0);
}

#[test]
fn catalogs_reject_unknown_repeated_and_unsupported_inputs() {
    let root = TestRoot::new("invalid");
    root.write_catalog(&format!(
        "schema = 3\nid = \"test-wifi-phy\"\n{BASE_PHY}{BASE_SCOPE}"
    ));
    let unknown = catalog_program("catalog/wifi.toml", "missing");
    let error = resolution_error(parse(&unknown).resolve_catalogs(
        &root.path,
        Path::new("program.toml"),
        &unknown,
    ));
    assert!(error.contains("unsupported capability catalog schema"));

    root.write_catalog(&format!(
        "schema = 2\nid = \"test-wifi-phy\"\n{BASE_PHY}{BASE_SCOPE}{BASE_PHY}{BASE_SCOPE}"
    ));
    let error = resolution_error(parse(&unknown).resolve_catalogs(
        &root.path,
        Path::new("program.toml"),
        &unknown,
    ));
    assert!(error.contains("declared by both catalog"));

    root.write_catalog(&format!(
        "schema = 2\nid = \"test-wifi-phy\"\n{BASE_PHY}{BASE_SCOPE}"
    ));
    let error = resolution_error(parse(&unknown).resolve_catalogs(
        &root.path,
        Path::new("program.toml"),
        &unknown,
    ));
    assert!(error.contains("unknown catalog capability"));

    root.write_catalog(&format!("schema = 2\nid = \"test-wifi-phy\"\n{BASE_PHY}"));
    let selected = catalog_program("catalog/wifi.toml", "base-phy");
    let error = resolution_error(parse(&selected).resolve_catalogs(
        &root.path,
        Path::new("program.toml"),
        &selected,
    ));
    assert!(error.contains("requires catalog-scope metadata"));
}

#[cfg(unix)]
#[test]
fn catalog_paths_reject_symlink_components() {
    use std::os::unix::fs::symlink;

    let root = TestRoot::new("symlink");
    root.write_catalog(&format!(
        "schema = 2\nid = \"test-wifi-phy\"\n{BASE_PHY}{BASE_SCOPE}"
    ));
    symlink(root.path.join("catalog"), root.path.join("linked-catalog")).unwrap();
    let input = catalog_program("linked-catalog/wifi.toml", "base-phy");
    let error = resolution_error(parse(&input).resolve_catalogs(
        &root.path,
        Path::new("program.toml"),
        &input,
    ));
    assert!(error.contains("regular directories"));
}

#[test]
fn unselected_catalog_declarations_fail_static_validation() {
    let root = TestRoot::new("unselected-invalid");
    let selected = catalog_program("catalog/wifi.toml", "base-phy");
    let check = |body: String| {
        root.write_catalog(&body);
        resolution_error(parse(&selected).resolve_catalogs(
            &root.path,
            Path::new("program.toml"),
            &selected,
        ))
    };

    let missing_dependency = WIFI_CHANNEL.replace(
        "depends-on = [\"base-phy\"]",
        "depends-on = [\"missing-phy\"]",
    );
    let error = check(format!(
        "schema = 2\nid = \"test-wifi-phy\"\n{BASE_PHY}{BASE_SCOPE}{missing_dependency}{WIFI_SCOPE}"
    ));
    assert!(error.contains("depends on missing missing-phy"));

    let first = BASE_PHY
        .replace("id = \"base-phy\"", "id = \"cycle-a\"")
        .replace(
            "async = \"bounded\"",
            "async = \"bounded\"\ndepends-on = [\"cycle-b\"]",
        );
    let second = BASE_PHY
        .replace("id = \"base-phy\"", "id = \"cycle-b\"")
        .replace(
            "async = \"bounded\"",
            "async = \"bounded\"\ndepends-on = [\"cycle-a\"]",
        );
    let error = check(format!(
        "schema = 2\nid = \"test-wifi-phy\"\n{BASE_PHY}{BASE_SCOPE}{first}{BASE_SCOPE}{second}{BASE_SCOPE}"
    ));
    assert!(error.contains("dependency cycle"));

    let inconsistent = WIFI_CHANNEL.replace(
        "gaps = [{ axis = \"vendor\", id = \"vendor-trace-missing\" }]",
        "gaps = [{ axis = \"implementation\", id = \"impossible-gap\" }, { axis = \"vendor\", id = \"vendor-trace-missing\" }]",
    );
    let error = check(format!(
        "schema = 2\nid = \"test-wifi-phy\"\n{BASE_PHY}{BASE_SCOPE}{inconsistent}{WIFI_SCOPE}"
    ));
    assert!(error.contains("terminal implementation axis"));

    let bad_contract = format!(
        "{}\n[[capabilities.source-contracts]]\nid = \"missing-owner\"\ncomposition = \"production\"\nscope = \"One operation\"\nlimits = \"Bounded scope\"\nsource-paths = [\"missing.rs\"]\n{}",
        WIFI_CHANNEL, WIFI_SCOPE
    );
    let error = check(format!(
        "schema = 2\nid = \"test-wifi-phy\"\n{BASE_PHY}{BASE_SCOPE}{bad_contract}"
    ));
    assert!(error.contains("missing.rs"), "{error}");
}

#[test]
fn multi_catalog_closure_is_order_independent_and_duplicates_fail() {
    let root = TestRoot::new("multi-catalog");
    let base = format!("schema = 2\nid = \"base\"\n{BASE_PHY}{BASE_SCOPE}");
    let wifi = format!("schema = 2\nid = \"wifi\"\n{WIFI_CHANNEL}{WIFI_SCOPE}");
    root.write_named_catalog("a", &base);
    root.write_named_catalog("b", &wifi);
    let program = parse(PROGRAM_PREFIX);
    let empty = BTreeSet::new();
    let first = CatalogView::load_with_program(
        &root.path,
        &[
            PathBuf::from("catalog/a.toml"),
            PathBuf::from("catalog/b.toml"),
        ],
        Some((&program.verification, &program.hil)),
        &empty,
    )
    .unwrap();
    let second = CatalogView::load_with_program(
        &root.path,
        &[
            PathBuf::from("catalog/b.toml"),
            PathBuf::from("catalog/a.toml"),
        ],
        Some((&program.verification, &program.hil)),
        &empty,
    )
    .unwrap();
    assert_eq!(
        first.capabilities.keys().collect::<Vec<_>>(),
        second.capabilities.keys().collect::<Vec<_>>()
    );
    assert_eq!(
        first
            .sources
            .iter()
            .map(|source| &source.id)
            .collect::<Vec<_>>(),
        second
            .sources
            .iter()
            .map(|source| &source.id)
            .collect::<Vec<_>>()
    );

    root.write_named_catalog("b", &base.replace("id = \"base\"", "id = \"other\""));
    let error = CatalogView::load_with_program(
        &root.path,
        &[
            PathBuf::from("catalog/a.toml"),
            PathBuf::from("catalog/b.toml"),
        ],
        Some((&program.verification, &program.hil)),
        &empty,
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("declared by both catalog"));
}

#[test]
fn inline_and_catalog_forms_match_across_the_evidence_matrix() {
    let root = TestRoot::new("differential-matrix");
    let base = BASE_PHY
        .replace("vendor-anchors = [\"Cargo.toml\"]", "vendor-roots = [{ source = \"archive\", symbol = \"base_root\" }]\nvendor-evidence = [{ suite = \"base-suite\", source = \"archive\", symbol = \"base_root\" }]")
        .replace("hil-requirements = [{ scenario = \"wifi-channel\", minimum-repetitions = 1 }]", "hil-requirements = [{ scenario = \"base-phy\", minimum-repetitions = 1 }]")
        .replace("gaps = [{ axis = \"vendor\", id = \"vendor-trace-missing\" }]", "");
    let wifi = WIFI_CHANNEL
        .replace("vendor-anchors = [\"Cargo.toml\"]", "vendor-roots = [{ source = \"archive\", symbol = \"wifi_root\" }]\nvendor-evidence = [{ suite = \"wifi-suite\", source = \"archive\", symbol = \"wifi_root\" }]")
        .replace("gaps = [{ axis = \"vendor\", id = \"vendor-trace-missing\" }]", "");
    let inline = parse(&format!("{PROGRAM_PREFIX}{base}{wifi}")).capabilities;
    let canonical: CatalogDocument = toml_edit::de::from_str(&format!(
        "schema = 2\nid = \"matrix\"\n{base}{BASE_SCOPE}{wifi}{WIFI_SCOPE}"
    ))
    .unwrap();

    let dispositions = DispositionIndex {
        entries: ["base_root", "wifi_root"]
            .into_iter()
            .map(|symbol| {
                (
                    ("archive".to_owned(), symbol.to_owned()),
                    DispositionEntry {
                        has_rust_component: true,
                        has_contract: true,
                    },
                )
            })
            .collect(),
        project_id: "test".to_owned(),
        vendor_evidence_index: None,
    };
    let scenarios = ScenarioCatalog::load(&root.path, Path::new("scenarios")).unwrap();
    let source_sha = format!(
        "{:x}",
        Sha256::digest(fs::read(root.path.join("Cargo.toml")).unwrap())
    );
    let vendor_entry = |suite: &str, symbol: &str| VendorEvidenceIndexEntry {
        suite: suite.to_owned(),
        source: "archive".to_owned(),
        symbol: symbol.to_owned(),
        evidence_class: "production-trace".to_owned(),
        status: "match".to_owned(),
        release_eligible: true,
        rust_component: Some("crate::component".to_owned()),
        evidence_digest: Some("ab".repeat(32)),
        baseline_passed: true,
        artifact_hashes: vec![VendorEvidenceArtifactHash {
            role: "trace".to_owned(),
            sha256: "cd".repeat(32),
        }],
        source_hashes: vec![VendorEvidenceSourceHash {
            path: PathBuf::from("Cargo.toml"),
            sha256: source_sha.clone(),
        }],
        release_blockers: Vec::new(),
    };
    let mut stale_entry = vendor_entry("base-suite", "base_root");
    stale_entry.source_hashes[0].sha256 = "ef".repeat(32);
    let cases = [
        ("no-evidence", Vec::new(), Vec::new(), true, 0usize),
        (
            "partial-vendor",
            vec![
                vendor_entry("base-suite", "base_root"),
                vendor_entry("wifi-suite", "wifi_root"),
            ],
            Vec::new(),
            true,
            0,
        ),
        (
            "partial-hil",
            Vec::new(),
            vec![("base-phy", 1usize), ("wifi-channel", 1usize)],
            true,
            0,
        ),
        (
            "stale-vendor-source",
            vec![stale_entry, vendor_entry("wifi-suite", "wifi_root")],
            vec![("base-phy", 1usize), ("wifi-channel", 1usize)],
            true,
            0,
        ),
        (
            "sufficient-positive",
            vec![
                vendor_entry("base-suite", "base_root"),
                vendor_entry("wifi-suite", "wifi_root"),
            ],
            vec![("base-phy", 1usize), ("wifi-channel", 1usize)],
            true,
            2,
        ),
        (
            "dirty-evaluator",
            vec![
                vendor_entry("base-suite", "base_root"),
                vendor_entry("wifi-suite", "wifi_root"),
            ],
            vec![("base-phy", 1usize), ("wifi-channel", 1usize)],
            false,
            0,
        ),
    ];
    for (name, entries, hil_entries, clean, ready) in cases {
        let vendor = VendorEvidenceIndex {
            schema_version: 1,
            command: "project verify vendor evidence index".to_owned(),
            project: "test".to_owned(),
            complete_project_run: true,
            entries,
        };
        let hil = HilEvidenceIndex::synthetic(&hil_entries);
        let context = EvaluationContext {
            root: &root.path,
            dispositions: &dispositions,
            vendor_index: &vendor,
            scenario_catalog: &scenarios,
            hil_index: &hil,
            evaluator_clean: clean,
        };
        let evaluate = |documents: Vec<CapabilityDocument>| {
            documents
                .into_iter()
                .map(|document| evaluate_capability(document, &context).unwrap())
                .map(|capability| (capability.id.clone(), capability))
                .collect::<BTreeMap<_, _>>()
        };
        let inline_result = evaluate(inline.clone());
        let catalog_result = evaluate(canonical.capabilities.clone());
        assert_eq!(catalog_result, inline_result, "case {name}");
        let qualification = Qualification {
            target: name.to_owned(),
            repository: RepositoryState {
                commit: "test".to_owned(),
                dirty: !clean,
            },
            evidence_inputs: EvidenceInputs {
                verification_entries: 0,
                verification_current_release_entries: 0,
                hil: Default::default(),
                verification_project: PathBuf::from("verification.toml"),
                vendor_evidence_index: PathBuf::from("vendor.json"),
                hil_catalog: PathBuf::from("scenarios"),
                hil_runs: PathBuf::from("runs"),
            },
            capabilities: inline_result,
            program_source: SourceIdentity {
                id: name.to_owned(),
                schema: 4,
                path: PathBuf::from("program.toml"),
                sha256: "00".repeat(32),
            },
            catalog_sources: Vec::new(),
            capability_origins: BTreeMap::new(),
            catalog_scopes: BTreeMap::new(),
            catalog: Default::default(),
            direct_catalog_capabilities: Default::default(),
        };
        assert_eq!(qualification.ready_count(), ready, "case {name}");
    }

    let only_wifi_vendor = VendorEvidenceIndex {
        schema_version: 1,
        command: "project verify vendor evidence index".to_owned(),
        project: "test".to_owned(),
        complete_project_run: true,
        entries: vec![vendor_entry("wifi-suite", "wifi_root")],
    };
    let hil = HilEvidenceIndex::synthetic(&[("base-phy", 1), ("wifi-channel", 1)]);
    let context = EvaluationContext {
        root: &root.path,
        dispositions: &dispositions,
        vendor_index: &only_wifi_vendor,
        scenario_catalog: &scenarios,
        hil_index: &hil,
        evaluator_clean: true,
    };
    let evaluated = inline
        .into_iter()
        .map(|document| evaluate_capability(document, &context).unwrap())
        .map(|capability| (capability.id.clone(), capability))
        .collect::<BTreeMap<_, _>>();
    assert!(!evaluated["base-phy"].proof_ready());
    assert!(evaluated["wifi-channel"].proof_ready());
    let qualification = Qualification {
        target: "dependency-not-ready".to_owned(),
        repository: RepositoryState {
            commit: "test".to_owned(),
            dirty: false,
        },
        evidence_inputs: EvidenceInputs {
            verification_entries: 1,
            verification_current_release_entries: 1,
            hil: Default::default(),
            verification_project: PathBuf::from("verification.toml"),
            vendor_evidence_index: PathBuf::from("vendor.json"),
            hil_catalog: PathBuf::from("scenarios"),
            hil_runs: PathBuf::from("runs"),
        },
        capabilities: evaluated,
        program_source: SourceIdentity {
            id: "test".to_owned(),
            schema: 4,
            path: PathBuf::from("program.toml"),
            sha256: "00".repeat(32),
        },
        catalog_sources: Vec::new(),
        capability_origins: BTreeMap::new(),
        catalog_scopes: BTreeMap::new(),
        catalog: Default::default(),
        direct_catalog_capabilities: Default::default(),
    };
    assert!(!qualification.is_ready("wifi-channel"));

    let only_base_vendor = VendorEvidenceIndex {
        schema_version: 1,
        command: "project verify vendor evidence index".to_owned(),
        project: "test".to_owned(),
        complete_project_run: true,
        entries: vec![vendor_entry("base-suite", "base_root")],
    };
    let inline = parse(&format!("{PROGRAM_PREFIX}{base}{wifi}")).capabilities;
    let context = EvaluationContext {
        root: &root.path,
        dispositions: &dispositions,
        vendor_index: &only_base_vendor,
        scenario_catalog: &scenarios,
        hil_index: &hil,
        evaluator_clean: true,
    };
    let evaluated = inline
        .into_iter()
        .map(|document| evaluate_capability(document, &context).unwrap())
        .map(|capability| (capability.id.clone(), capability))
        .collect::<BTreeMap<_, _>>();
    assert!(evaluated["base-phy"].proof_ready());
    assert!(!evaluated["wifi-channel"].proof_ready());
}
