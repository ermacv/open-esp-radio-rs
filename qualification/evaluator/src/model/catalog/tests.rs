use super::*;
use crate::{
    hil::{HilEvidenceIndex, RepositoryState, ScenarioCatalog},
    model::{
        DispositionIndex, EvaluationContext, EvidenceInputs, HilProof, Qualification,
        VendorEvidenceIndex, VendorProof, evaluate_capability, validate_dependencies,
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
            path.join("scenarios/wifi-channel.toml"),
            "schema = 4\nid = \"wifi-channel\"\nrepetitions = 1\n",
        )
        .unwrap();
        Self { path }
    }

    fn write_catalog(&self, body: &str) {
        fs::write(self.path.join("catalog/wifi.toml"), body).unwrap();
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
    root.write_catalog(&format!(
        "schema = 1\nid = \"test-wifi-phy\"\n{BASE_PHY}{BASE_SCOPE}{WIFI_CHANNEL}{WIFI_SCOPE}"
    ));
    let canonical_input = catalog_program("catalog/wifi.toml", "wifi-channel");
    let legacy_input = format!("{PROGRAM_PREFIX}{BASE_PHY}{WIFI_CHANNEL}");
    let canonical = parse(&canonical_input)
        .resolve_catalogs(&root.path, Path::new("canonical.toml"), &canonical_input)
        .unwrap();
    let legacy = parse(&legacy_input)
        .resolve_catalogs(&root.path, Path::new("legacy.toml"), &legacy_input)
        .unwrap();

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
        },
        capabilities: evaluated,
        program_source: canonical.program_source.unwrap(),
        catalog_sources: canonical.catalog_sources,
        capability_origins: canonical.capability_origins,
        catalog_scopes: canonical.catalog_scopes,
    };
    assert_eq!(qualification.ready_count(), 0);
}

#[test]
fn catalogs_reject_unknown_repeated_and_unsupported_inputs() {
    let root = TestRoot::new("invalid");
    root.write_catalog(&format!(
        "schema = 2\nid = \"test-wifi-phy\"\n{BASE_PHY}{BASE_SCOPE}"
    ));
    let unknown = catalog_program("catalog/wifi.toml", "missing");
    let error = resolution_error(parse(&unknown).resolve_catalogs(
        &root.path,
        Path::new("program.toml"),
        &unknown,
    ));
    assert!(error.contains("unsupported capability catalog schema"));

    root.write_catalog(&format!(
        "schema = 1\nid = \"test-wifi-phy\"\n{BASE_PHY}{BASE_SCOPE}{BASE_PHY}{BASE_SCOPE}"
    ));
    let error = resolution_error(parse(&unknown).resolve_catalogs(
        &root.path,
        Path::new("program.toml"),
        &unknown,
    ));
    assert!(error.contains("declared by both catalog"));

    root.write_catalog(&format!(
        "schema = 1\nid = \"test-wifi-phy\"\n{BASE_PHY}{BASE_SCOPE}"
    ));
    let error = resolution_error(parse(&unknown).resolve_catalogs(
        &root.path,
        Path::new("program.toml"),
        &unknown,
    ));
    assert!(error.contains("unknown catalog capability"));

    root.write_catalog(&format!("schema = 1\nid = \"test-wifi-phy\"\n{BASE_PHY}"));
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
        "schema = 1\nid = \"test-wifi-phy\"\n{BASE_PHY}{BASE_SCOPE}"
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
