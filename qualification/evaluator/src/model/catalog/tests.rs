use super::*;
mod bluetooth;
mod maintenance;
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

    let derived_input = canonical_input.replace(
        "required-capabilities = [\"base-phy\", \"wifi-channel\"]",
        "required-capabilities-from = \"catalog-closure\"",
    );
    let derived = parse(&derived_input)
        .resolve_catalogs(&root.path, Path::new("derived.toml"), &derived_input)
        .unwrap();
    derived.validate_program_structure(&root.path).unwrap();
    assert_eq!(derived.capabilities, canonical.capabilities);
    assert_eq!(
        derived.required_capabilities,
        canonical.required_capabilities
    );

    let mut canonical_documents = canonical.capabilities.clone();
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
        suite_states: BTreeMap::new(),
        schema_version: 1,
        command: "project verify vendor evidence index".to_owned(),
        project: "test".to_owned(),
        complete_project_run: true,
        entries: Vec::new(),
    };
    let scenario_catalog = ScenarioCatalog::load(&root.path, Path::new("scenarios")).unwrap();
    let hil_index = HilEvidenceIndex::default();
    let context = EvaluationContext {
        declarations: &canonical_documents
            .iter()
            .map(|d| (d.id.clone(), d.clone()))
            .collect(),
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
    assert_equivalent_proofs(evaluated.clone(), legacy_evaluated);
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
        declarations: canonical
            .capabilities
            .iter()
            .map(|d| (d.id.clone(), d.clone()))
            .collect(),
        program_source: canonical.program_source.unwrap(),
        catalog_sources: canonical.catalog_sources,
        capability_origins: canonical.capability_origins,
        catalog_scopes: canonical.catalog_scopes,
        catalog: Default::default(),
        direct_catalog_capabilities: Default::default(),
    };
    assert_eq!(qualification.ready_count(), 0);
    let map =
        crate::engineering::ProjectMap::from_program(&qualification, Some("wifi-channel")).unwrap();
    assert_eq!(map.mode, "saved-evidence");
    assert_eq!(map.entries.len(), 2);
    assert!(
        map.entries
            .iter()
            .all(|entry| entry.implementation == "complete"
                && entry.evidence.as_ref().unwrap().hil == "missing")
    );
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
    let mut wrong_source_entry = vendor_entry("base-suite", "base_root");
    wrong_source_entry.source = "rom".to_owned();
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
            "wrong-vendor-source",
            vec![wrong_source_entry, vendor_entry("wifi-suite", "wifi_root")],
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
            2,
        ),
    ];
    for (name, entries, hil_entries, clean, ready) in cases {
        let vendor = VendorEvidenceIndex {
            suite_states: BTreeMap::new(),
            schema_version: 1,
            command: "project verify vendor evidence index".to_owned(),
            project: "test".to_owned(),
            complete_project_run: true,
            entries,
        };
        let hil = HilEvidenceIndex::synthetic(&hil_entries);
        let context = EvaluationContext {
            declarations: &inline.iter().map(|d| (d.id.clone(), d.clone())).collect(),
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
        assert_equivalent_proofs(catalog_result.clone(), inline_result.clone());
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
            declarations: inline.iter().map(|d| (d.id.clone(), d.clone())).collect(),
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
        suite_states: BTreeMap::new(),
        schema_version: 1,
        command: "project verify vendor evidence index".to_owned(),
        project: "test".to_owned(),
        complete_project_run: true,
        entries: vec![vendor_entry("wifi-suite", "wifi_root")],
    };
    let hil = HilEvidenceIndex::synthetic(&[("base-phy", 1), ("wifi-channel", 1)]);
    let context = EvaluationContext {
        declarations: &inline.iter().map(|d| (d.id.clone(), d.clone())).collect(),
        root: &root.path,
        dispositions: &dispositions,
        vendor_index: &only_wifi_vendor,
        scenario_catalog: &scenarios,
        hil_index: &hil,
        evaluator_clean: true,
    };
    let evaluated = inline
        .iter()
        .cloned()
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
        declarations: inline.iter().map(|d| (d.id.clone(), d.clone())).collect(),
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
        suite_states: BTreeMap::new(),
        schema_version: 1,
        command: "project verify vendor evidence index".to_owned(),
        project: "test".to_owned(),
        complete_project_run: true,
        entries: vec![vendor_entry("base-suite", "base_root")],
    };
    let inline = parse(&format!("{PROGRAM_PREFIX}{base}{wifi}")).capabilities;
    let context = EvaluationContext {
        declarations: &inline.iter().map(|d| (d.id.clone(), d.clone())).collect(),
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

const SHARED_FACT: &str = r#"
[[source-facts]]
id = "shared-handoff"
status = "implemented"
level = "lower-primitive"

[source-facts.source-contract]
id = "shared-handoff"
composition = "production"
scope = "One initial PHY handoff"
limits = "The broader PHY lifetime remains incomplete"
source-paths = ["Cargo.toml"]
"#;

const TEST_CATALOG_VALIDATION: &str = r#"
[validation]
verification-project = "verification.toml"
hil-catalog = "scenarios"
"#;

const FACT_SECTIONS: &str = r#"
[[sections]]
id = "shared-view"
domain = "phy"
title = "Shared view"
source-document = "Cargo.toml"

[[sections]]
id = "bluetooth-view"
domain = "bluetooth"
title = "Bluetooth view"
source-document = "Cargo.toml"

[[items]]
id = "shared-handoff-row"
section = "shared-view"
title = "Initial Bluetooth PHY handoff"
source-fact = "shared-handoff"

[[items]]
id = "bluetooth-handoff-row"
section = "bluetooth-view"
title = "Initial PHY handoff"
source-fact = "shared-handoff"
"#;

#[test]
fn source_fact_projects_one_edit_into_inventory_and_qualification_views() {
    let root = TestRoot::new("source-fact-projection");
    let capability = BASE_PHY.replace(
        "async = \"bounded\"",
        "async = \"bounded\"\nsource-fact-refs = [\"shared-handoff\"]",
    );
    root.write_catalog(&format!(
        "schema = 2\nid = \"shared\"\n{TEST_CATALOG_VALIDATION}{SHARED_FACT}{capability}{BASE_SCOPE}{FACT_SECTIONS}"
    ));
    let first = CatalogView::load(&root.path, &[PathBuf::from("catalog/wifi.toml")]).unwrap();
    assert_eq!(first.items.len(), 2);
    assert!(first.items.iter().all(|item| {
        item.source_fact.as_deref() == Some("shared-handoff")
            && item.status == SourceStatus::Implemented
            && item
                .scope_and_limitations
                .contains("One initial PHY handoff")
            && item
                .scope_and_limitations
                .contains("broader PHY lifetime remains incomplete")
    }));
    assert_eq!(first.capabilities["base-phy"].source_contracts.len(), 1);
    assert_eq!(
        first.capabilities["base-phy"].source_contracts[0],
        first.source_facts["shared-handoff"].source_contract
    );

    let changed = SHARED_FACT
        .replace("status = \"implemented\"", "status = \"partial\"")
        .replace("One initial PHY handoff", "One revised PHY handoff");
    root.write_catalog(&format!(
        "schema = 2\nid = \"shared\"\n{TEST_CATALOG_VALIDATION}{changed}{capability}{BASE_SCOPE}{FACT_SECTIONS}"
    ));
    let second = CatalogView::load(&root.path, &[PathBuf::from("catalog/wifi.toml")]).unwrap();
    assert!(second.items.iter().all(|item| {
        item.status == SourceStatus::Partial
            && item
                .scope_and_limitations
                .contains("One revised PHY handoff")
    }));
    assert_eq!(
        second.capabilities["base-phy"].source_contracts[0].scope,
        "One revised PHY handoff"
    );
}

#[test]
fn source_fact_references_reject_dangling_and_independent_overrides() {
    let root = TestRoot::new("source-fact-invalid");
    root.write_catalog(&format!(
        "schema = 2\nid = \"shared\"\n{TEST_CATALOG_VALIDATION}{SHARED_FACT}{}{}{}",
        BASE_PHY.replace(
            "async = \"bounded\"",
            "async = \"bounded\"\nsource-fact-refs = [\"missing-fact\"]"
        ),
        BASE_SCOPE,
        FACT_SECTIONS
    ));
    let error = CatalogView::load(&root.path, &[PathBuf::from("catalog/wifi.toml")])
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("references missing source fact missing-fact"),
        "{error}"
    );

    let overridden = FACT_SECTIONS.replace(
        "source-fact = \"shared-handoff\"",
        "source-fact = \"shared-handoff\"\nstatus = \"absent\"",
    );
    root.write_catalog(&format!(
        "schema = 2\nid = \"shared\"\n{TEST_CATALOG_VALIDATION}{SHARED_FACT}{BASE_PHY}{BASE_SCOPE}{overridden}"
    ));
    let error = CatalogView::load(&root.path, &[PathBuf::from("catalog/wifi.toml")])
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("cannot override source fact shared-handoff"),
        "{error}"
    );
}

#[test]
fn implemented_source_fact_does_not_promote_an_incomplete_parent() {
    let root = TestRoot::new("source-fact-parent");
    let capability = BASE_PHY
        .replace(
            "implementation = \"complete\"",
            "implementation = \"incomplete\"",
        )
        .replace(
            "gaps = [",
            "gaps = [{ axis = \"implementation\", id = \"broader-lifetime-incomplete\" },",
        )
        .replace(
            "async = \"bounded\"",
            "async = \"bounded\"\nsource-fact-refs = [\"shared-handoff\"]",
        );
    root.write_catalog(&format!(
        "schema = 2\nid = \"shared\"\n{TEST_CATALOG_VALIDATION}{SHARED_FACT}{capability}{BASE_SCOPE}{FACT_SECTIONS}"
    ));
    let view = CatalogView::load(&root.path, &[PathBuf::from("catalog/wifi.toml")]).unwrap();
    assert_eq!(
        view.source_facts["shared-handoff"].status,
        SourceStatus::Implemented
    );
    assert_eq!(
        view.capabilities["base-phy"].implementation,
        crate::model::ImplementationProof::Incomplete
    );
}

#[test]
fn bluetooth_catalog_migration_preserves_program_and_full_source_inventory() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let manifest = root.join("qualification/targets/esp32s31/bluetooth-le.toml");
    let validated = ManifestDocument::load_and_validate(&manifest, &root).unwrap();
    let document = &validated.document;
    assert_eq!(document.capabilities.len(), 68);
    assert_eq!(document.required_capabilities.len(), 68);
    assert_eq!(
        document
            .capabilities
            .iter()
            .map(|capability| capability.id.as_str())
            .collect::<BTreeSet<_>>(),
        document
            .required_capabilities
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>()
    );
    assert_eq!(document.direct_catalog_capabilities.len(), 17);

    let catalog = &document.catalog;
    let bluetooth_document = Path::new("crates/hardware/esp32s31/driver/bluetooth/FEATURES.md");
    let bluetooth_sections = catalog
        .sections
        .iter()
        .filter(|section| section.source_document == bluetooth_document)
        .map(|section| section.id.as_str())
        .collect::<BTreeSet<_>>();
    let source_rows = catalog
        .items
        .iter()
        .filter(|item| {
            bluetooth_sections.contains(item.section.as_str()) && item.source_fact.is_none()
        })
        .count();
    let bluetooth_projections = catalog
        .items
        .iter()
        .filter(|item| {
            bluetooth_sections.contains(item.section.as_str()) && item.source_fact.is_some()
        })
        .count();
    assert_eq!((source_rows, bluetooth_projections), (136, 5));
    assert_eq!(
        catalog
            .references
            .iter()
            .filter(|reference| { reference.kind == InventoryReferenceKind::QualificationMapping })
            .count(),
        22
    );
    assert_eq!(
        catalog
            .references
            .iter()
            .filter(|reference| {
                bluetooth_sections.contains(reference.section.as_str())
                    && reference.kind == InventoryReferenceKind::SourceReference
            })
            .count(),
        4
    );
    let classic_rows = catalog
        .items
        .iter()
        .filter(|item| item.section.starts_with("bluetooth-classic-"))
        .count();
    let host_only_rows = catalog
        .items
        .iter()
        .filter(|item| item.status == SourceStatus::HostOnly)
        .count();
    assert_eq!((classic_rows, host_only_rows), (33, 7));

    let handoff = &catalog.source_facts["bluetooth-initial-phy-handoff"];
    let parent = &catalog.capabilities["common-phy-baseband"];
    assert_eq!(handoff.status, SourceStatus::Implemented);
    assert_eq!(
        parent.implementation,
        crate::model::ImplementationProof::Incomplete
    );
    assert!(parent.source_fact_refs.iter().any(|id| id == &handoff.id));
    assert_eq!(parent.source_contracts[0], handoff.source_contract);
}

#[test]
fn bluetooth_lifecycle_facts_reach_all_views_without_promoting_products() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let catalog = CatalogView::load(
        &root,
        &[
            PathBuf::from("qualification/catalog/esp32s31/bluetooth-products.toml"),
            PathBuf::from("qualification/catalog/esp32s31/whole-radio.toml"),
        ],
    )
    .unwrap();
    let domains = BTreeSet::from(["bluetooth", "phy", "whole-radio"]);
    for (fact_id, expected) in [
        ("bluetooth-idle-phy-maintenance", SourceStatus::Implemented),
        ("bluetooth-periodic-phy-maintenance", SourceStatus::Partial),
        ("bluetooth-idle-powered-release", SourceStatus::Implemented),
        (
            "bluetooth-same-storage-powered-restart",
            SourceStatus::Implemented,
        ),
    ] {
        let fact = &catalog.source_facts[fact_id];
        assert_eq!(fact.status, expected);
        let projected_domains = catalog
            .items
            .iter()
            .filter(|item| item.source_fact.as_deref() == Some(fact_id))
            .map(|item| {
                assert_eq!(item.status, expected);
                assert_eq!(item.source_paths, fact.source_contract.source_paths);
                catalog
                    .sections
                    .iter()
                    .find(|section| section.id == item.section)
                    .unwrap()
                    .domain
                    .as_str()
            })
            .collect::<BTreeSet<_>>();
        assert_eq!(projected_domains, domains, "{fact_id}");
        for parent_id in ["common-phy-baseband", "peripheral-acl"] {
            let parent = &catalog.capabilities[parent_id];
            assert!(parent.source_contracts.contains(&fact.source_contract));
            assert_eq!(
                parent.implementation,
                crate::model::ImplementationProof::Incomplete
            );
        }
    }
    let teardown = &catalog.capabilities["powered-teardown"];
    assert_eq!(
        teardown.implementation,
        crate::model::ImplementationProof::Incomplete
    );
    for fact_id in [
        "bluetooth-idle-powered-release",
        "bluetooth-same-storage-powered-restart",
    ] {
        assert!(
            teardown
                .source_contracts
                .contains(&catalog.source_facts[fact_id].source_contract)
        );
    }
}

#[test]
fn coex_and_whole_radio_catalogs_preserve_facets_without_program_promotion() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let paths = [
        "qualification/catalog/esp32s31/wifi-phy.toml",
        "qualification/catalog/esp32s31/coex.toml",
        "qualification/catalog/esp32s31/bluetooth.toml",
        "qualification/catalog/esp32s31/whole-radio.toml",
    ]
    .map(PathBuf::from);
    let catalog = CatalogView::load(&root, &paths).unwrap();
    let coex_document = Path::new("crates/hardware/esp32s31/driver/coex/FEATURES.md");
    let whole_radio_document = Path::new("crates/hardware/esp32s31/driver/FEATURES.md");
    let section_ids = |document: &Path| {
        catalog
            .sections
            .iter()
            .filter(|section| section.source_document == document)
            .map(|section| section.id.as_str())
            .collect::<BTreeSet<_>>()
    };
    let coex_sections = section_ids(coex_document);
    let whole_radio_sections = section_ids(whole_radio_document);
    let item_counts = |sections: &BTreeSet<&str>| {
        catalog
            .items
            .iter()
            .filter(|item| sections.contains(item.section.as_str()))
            .fold((0, 0), |(rows, projections), item| {
                if item.source_fact.is_some() {
                    (rows, projections + 1)
                } else {
                    (rows + 1, projections)
                }
            })
    };
    assert_eq!(item_counts(&coex_sections), (42, 1));
    assert_eq!(item_counts(&whole_radio_sections), (28, 6));
    assert_eq!(
        catalog
            .references
            .iter()
            .filter(|reference| coex_sections.contains(reference.section.as_str()))
            .count(),
        8
    );
    assert!(
        catalog
            .references
            .iter()
            .filter(|reference| { coex_sections.contains(reference.section.as_str()) })
            .all(|reference| reference.kind == InventoryReferenceKind::SourceReference)
    );
    assert_eq!(
        catalog
            .references
            .iter()
            .filter(|reference| whole_radio_sections.contains(reference.section.as_str()))
            .count(),
        6
    );

    let timer_fact = &catalog.source_facts["coex-timer-validation-bridge"];
    assert_eq!(timer_fact.status, SourceStatus::Diagnostic);
    let coexistence = &catalog.capabilities["coexistence"];
    assert_eq!(
        coexistence.implementation,
        crate::model::ImplementationProof::Incomplete
    );
    assert_eq!(
        coexistence.source_fact_refs,
        ["coex-timer-validation-bridge"]
    );
    assert_eq!(coexistence.source_contracts[0], timer_fact.source_contract);
    assert_eq!(
        coexistence.source_contracts[1].id,
        "wifi-bluetooth-coex-runtime"
    );
    assert!(catalog.items.iter().any(|item| {
        item.id == "whole-radio-diagnostic-coexistence-timer-bridge"
            && item.source_fact.as_deref() == Some("coex-timer-validation-bridge")
    }));
    assert!(catalog.items.iter().any(|item| {
        item.id == "whole-radio-bounded-bluetooth-initial-phy-handoff"
            && item.source_fact.as_deref() == Some("bluetooth-initial-phy-handoff")
    }));
    assert_eq!(
        catalog
            .items
            .iter()
            .find(|item| item.title == "Protocol-specific PHY handoff")
            .unwrap()
            .status,
        SourceStatus::Partial
    );
    assert_eq!(
        catalog
            .items
            .iter()
            .find(|item| item.title == "Wi-Fi + Bluetooth concurrent composition")
            .unwrap()
            .status,
        SourceStatus::Absent
    );
    assert!(
        catalog
            .capability_owners
            .values()
            .all(|(owner, _)| { owner != "esp32s31-coex" && owner != "esp32s31-whole-radio" })
    );

    let error = CatalogView::load(
        &root,
        &[PathBuf::from(
            "qualification/catalog/esp32s31/whole-radio.toml",
        )],
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("references missing source fact"), "{error}");
}

#[test]
fn ieee802154_catalog_migration_preserves_program_and_full_source_inventory() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let manifest = root.join("qualification/targets/esp32s31/ieee802154.toml");
    let validated = ManifestDocument::load_and_validate(&manifest, &root).unwrap();
    let document = &validated.document;
    assert_eq!(document.capabilities.len(), 6);
    assert_eq!(document.required_capabilities.len(), 6);
    assert_eq!(
        document
            .capabilities
            .iter()
            .map(|capability| capability.id.as_str())
            .collect::<BTreeSet<_>>(),
        document
            .required_capabilities
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>()
    );
    assert_eq!(document.direct_catalog_capabilities.len(), 2);

    let catalog = &document.catalog;
    assert_eq!(catalog.sources.len(), 1);
    let source_document = Path::new("crates/hardware/esp32s31/driver/ieee802154/FEATURES.md");
    let sections = catalog
        .sections
        .iter()
        .filter(|section| section.source_document == source_document)
        .map(|section| section.id.as_str())
        .collect::<BTreeSet<_>>();
    assert_eq!(sections.len(), 11);
    let source_rows = catalog
        .items
        .iter()
        .filter(|item| sections.contains(item.section.as_str()) && item.source_fact.is_none())
        .count();
    let projections = catalog
        .items
        .iter()
        .filter(|item| sections.contains(item.section.as_str()) && item.source_fact.is_some())
        .count();
    assert_eq!((source_rows, projections), (42, 2));
    assert_eq!(
        catalog
            .items
            .iter()
            .filter(|item| item.status == SourceStatus::HostOnly)
            .count(),
        3
    );
    assert_eq!(
        catalog
            .references
            .iter()
            .filter(|reference| reference.kind == InventoryReferenceKind::QualificationMapping)
            .count(),
        6
    );
    assert_eq!(
        catalog
            .references
            .iter()
            .filter(|reference| reference.kind == InventoryReferenceKind::SourceReference)
            .count(),
        11
    );

    for (fact_id, parent_id) in [
        ("ieee802154-registered-timing-entry", "rf-channel-readiness"),
        ("ieee802154-mac-operation-subset", "rx-tx-dataplane"),
    ] {
        let fact = &catalog.source_facts[fact_id];
        let parent = &catalog.capabilities[parent_id];
        assert_eq!(fact.status, SourceStatus::Implemented);
        assert_eq!(
            parent.implementation,
            crate::model::ImplementationProof::Incomplete
        );
        assert_eq!(parent.source_fact_refs, [fact_id]);
        assert_eq!(
            parent.source_contracts.as_slice(),
            std::slice::from_ref(&fact.source_contract),
        );
    }
}

#[test]
fn imported_catalogs_resolve_transitively_once_and_preserve_provenance() {
    let root = TestRoot::new("imports");
    let base = format!("schema = 2\nid = \"base\"\n{BASE_PHY}{BASE_SCOPE}");
    let wifi = format!(
        "schema = 2\nid = \"wifi\"\nimports = [\"catalog/a.toml\"]\n{WIFI_CHANNEL}{WIFI_SCOPE}"
    );
    root.write_named_catalog("a", &base);
    root.write_named_catalog("b", &wifi);
    let extra = BASE_PHY.replace("id = \"base-phy\"", "id = \"other-phy\"");
    root.write_named_catalog("c", &format!("schema = 2\nid = \"extra\"\nimports = [\"catalog/a.toml\", \"catalog/b.toml\"]\n{extra}{BASE_SCOPE}"));
    let program = parse(PROGRAM_PREFIX);
    let load = |paths: &[PathBuf]| {
        CatalogView::load_with_program(
            &root.path,
            paths,
            Some((&program.verification, &program.hil)),
            &BTreeSet::new(),
        )
        .unwrap()
    };
    let imported = load(&["catalog/c.toml".into()]);
    let explicit = load(&[
        "catalog/c.toml".into(),
        "catalog/b.toml".into(),
        "catalog/a.toml".into(),
    ]);
    assert_eq!(imported.capabilities.len(), 3);
    assert_eq!(
        imported.capabilities.keys().collect::<Vec<_>>(),
        explicit.capabilities.keys().collect::<Vec<_>>()
    );
    let identities = |view: &CatalogView| {
        view.sources
            .iter()
            .map(|s| (s.path.clone(), s.sha256.clone()))
            .collect::<Vec<_>>()
    };
    assert_eq!(identities(&imported), identities(&explicit));
    assert_eq!(imported.sources.len(), 3);
    root.write_named_catalog("a", &(base + "\n# reviewed source input changed\n"));
    assert_ne!(
        identities(&imported),
        identities(&load(&["catalog/c.toml".into()]))
    );
}

#[test]
fn invalid_catalog_imports_fail_before_any_selection() {
    let root = TestRoot::new("invalid-imports");
    let base = format!("schema = 2\nid = \"base\"\n{BASE_PHY}{BASE_SCOPE}");
    for (imports, expected) in [
        ("[\"catalog/missing.toml\"]", "missing"),
        ("[\"../outside.toml\"]", "relative"),
        ("[\"catalog/b.toml\"]", "cycle"),
        ("[\"catalog/a.toml\", \"catalog/a.toml\"]", "repeats import"),
    ] {
        root.write_named_catalog("a", &base);
        root.write_named_catalog(
            "b",
            &format!("schema = 2\nid = \"wifi\"\nimports = {imports}\n{WIFI_CHANNEL}{WIFI_SCOPE}"),
        );
        let error = imports::load(&root.path, &["catalog/b.toml".into()])
            .err()
            .unwrap()
            .to_string();
        assert!(error.contains(expected), "{error}");
    }
}

#[test]
fn explicit_catalog_closure_follows_new_dependencies_without_relaxing_exact_programs() {
    let root = TestRoot::new("derived-required-set");
    let exact_input = catalog_program("catalog/wifi.toml", "wifi-channel");
    let derived_input = exact_input.replace(
        "required-capabilities = [\"base-phy\", \"wifi-channel\"]",
        "required-capabilities-from = \"catalog-closure\"",
    );
    let additional = BASE_PHY.replace("id = \"base-phy\"", "id = \"clock-owner\"");
    let dependent = BASE_PHY.replace(
        "scope = \"One base initialization\"",
        "scope = \"One base initialization\"\ndepends-on = [\"clock-owner\"]",
    );
    root.write_catalog(&format!("schema = 2\nid = \"test-wifi-phy\"\n{additional}{BASE_SCOPE}{dependent}{BASE_SCOPE}{WIFI_CHANNEL}{WIFI_SCOPE}"));
    let load =
        |input: &str| parse(input).resolve_catalogs(&root.path, Path::new("program.toml"), input);
    let derived = load(&derived_input).unwrap();
    derived.validate_program_structure(&root.path).unwrap();
    assert!(
        derived
            .required_capabilities
            .iter()
            .any(|id| id == "clock-owner")
    );
    assert_eq!(derived.required_capabilities.len(), 3);
    assert!(
        load(&exact_input)
            .unwrap()
            .validate_program_structure(&root.path)
            .unwrap_err()
            .to_string()
            .contains("root mismatch")
    );
    let mixed = format!("required-capabilities-from = \"catalog-closure\"\n{exact_input}");
    assert!(
        load(&mixed)
            .err()
            .unwrap()
            .to_string()
            .contains("without explicit required IDs")
    );
    let inline = format!("{derived_input}{BASE_PHY}");
    assert!(
        load(&inline)
            .err()
            .unwrap()
            .to_string()
            .contains("inline capabilities")
    );
    let no_policy = exact_input.replace(
        "required-capabilities = [\"base-phy\", \"wifi-channel\"]",
        "",
    );
    assert!(
        load(&no_policy)
            .unwrap()
            .validate_program_structure(&root.path)
            .unwrap_err()
            .to_string()
            .contains("no required capabilities")
    );
    assert!(
        toml_edit::de::from_str::<ManifestDocument>(
            &derived_input.replace("catalog-closure", "unknown-policy")
        )
        .is_err()
    );
}

#[test]
fn peripheral_products_retain_lifecycle_and_security_without_other_radio_roles() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let load = |name: &str| {
        ManifestDocument::load_and_validate(
            &root.join(format!("qualification/targets/esp32s31/{name}.toml")),
            &root,
        )
        .unwrap()
        .document
    };
    let acl = load("bluetooth-peripheral-acl");
    let secure = load("bluetooth-secure-gatt");
    let ids = |program: &ManifestDocument| {
        program
            .required_capabilities
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>()
    };
    let acl_ids = ids(&acl);
    let secure_ids = ids(&secure);
    assert!(acl_ids.is_subset(&secure_ids));
    for required in [
        "cold-ownership",
        "common-phy-baseband",
        "ble-phy-engine",
        "hci-controller-endpoints",
        "peripheral-acl",
    ] {
        assert!(acl_ids.contains(required), "missing {required}");
    }
    for required in ["peripheral-link-security", "secure-peripheral-gatt"] {
        assert!(secure_ids.contains(required), "missing {required}");
    }
    for unrelated in [
        "central-initiator",
        "coexistence",
        "host-eatt-and-gatt",
        "iso-dataplane",
        "le-audio-profiles",
    ] {
        assert!(!secure_ids.contains(unrelated), "unexpected {unrelated}");
    }
    let product = acl
        .capabilities
        .iter()
        .find(|c| c.id == "peripheral-acl")
        .unwrap();
    assert_eq!(
        product.implementation,
        crate::model::ImplementationProof::Incomplete
    );
    assert!(
        product
            .gaps
            .iter()
            .any(|g| g.axis == crate::model::Axis::Hil)
    );
    for scenario in [
        "bluetooth-peripheral-recovery",
        "bluetooth-peripheral-local-disconnect",
        "bluetooth-peripheral-local-reset",
        "bluetooth-peripheral-rf-loss",
        "bluetooth-peripheral-soak",
    ] {
        assert!(
            product
                .hil_requirements
                .iter()
                .any(|r| r.scenario == scenario),
            "missing {scenario}"
        );
    }
}

// Catalog forms carry explicit scope metadata absent in legacy inline records.
// Reviews must bind that metadata, while the unreviewed proof result is unchanged.
fn assert_equivalent_proofs(
    mut a: BTreeMap<String, crate::model::Capability>,
    mut b: BTreeMap<String, crate::model::Capability>,
) {
    for map in [&mut a, &mut b] {
        for capability in map.values_mut() {
            for decision in &mut capability.hil_decisions {
                decision.property = None;
            }
        }
    }
    assert_eq!(a, b);
}
