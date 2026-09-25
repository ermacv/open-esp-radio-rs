use super::*;

const COMPLETE: &str = r#"
schema = 4
target = "test-radio"
required-capabilities = ["channel-switch"]

[verification]
evidence-index = "evidence/vendor.json"

[hil]
target = "test-radio"
catalog = "hil/scenarios"
runs = "target/hil/test-radio/runs"

[[capabilities]]
id = "channel-switch"
title = "Channel switch"
scope = "One finite transition"
implementation = "complete"
host = "covered"
async = "bounded"
vendor-roots = [{ source = "archive", symbol = "set_channel" }]
vendor-evidence = [{ suite = "radio", source = "archive", symbol = "set_channel" }]
hil-requirements = [{ scenario = "channel-switch", minimum-repetitions = 3 }]
"#;

#[test]
fn parses_strict_v4_toml() {
    let manifest: ManifestDocument = toml_edit::de::from_str(COMPLETE).unwrap();
    assert_eq!(manifest.schema, 4);
    assert_eq!(
        manifest.capabilities[0].implementation,
        ImplementationProof::Complete
    );
    assert_eq!(manifest.capabilities[0].host, HostProof::Covered);
    assert_eq!(manifest.capabilities[0].async_proof, AsyncProof::Bounded);
    assert!(manifest.capabilities[0].source_contracts.is_empty());
    assert_eq!(
        manifest.capabilities[0].hil_requirements[0].minimum_repetitions,
        3
    );
}

#[test]
fn source_contracts_attach_to_existing_capability_without_adding_roots() {
    let input = format!(
        "{COMPLETE}\n{}",
        r#"
[[capabilities.source-contracts]]
id = "alternative-dma-path"
composition = "unimplemented"
scope = "Alternative peripheral backing"
limits = "Hardware reachability is unknown"
source-paths = ["Cargo.toml"]
"#
    );
    let manifest: ManifestDocument = toml_edit::de::from_str(&input).unwrap();
    assert_eq!(manifest.required_capabilities, ["channel-switch"]);
    assert_eq!(manifest.capabilities.len(), 1);
    assert_eq!(manifest.capabilities[0].source_contracts.len(), 1);
    assert_eq!(
        manifest.capabilities[0].implementation,
        ImplementationProof::Complete
    );
}

#[test]
fn declarative_axis_status_is_required() {
    let input = COMPLETE.replace("implementation = \"complete\"\n", "");
    let error = match toml_edit::de::from_str::<ManifestDocument>(&input) {
        Ok(_) => panic!("missing implementation status was accepted"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("missing field"));
}

#[test]
fn declared_axis_status_must_match_gaps() {
    let gaps = vec![Gap {
        axis: Axis::Implementation,
        id: "implementation-missing".to_owned(),
    }];
    assert!(validate_declared_axis("radio", Axis::Implementation, false, &gaps).is_ok());
    assert!(validate_declared_axis("radio", Axis::Implementation, true, &gaps).is_err());
    assert!(validate_declared_axis("radio", Axis::Implementation, false, &[]).is_err());
    assert!(validate_declared_axis("radio", Axis::Implementation, true, &[]).is_ok());
}

#[test]
fn async_not_applicable_requires_a_reason_and_no_gap() {
    assert!(
        validate_async_declaration(
            "radio",
            AsyncProof::NotApplicable,
            Some("synchronous-operation"),
            &[],
        )
        .is_ok()
    );
    assert!(validate_async_declaration("radio", AsyncProof::NotApplicable, None, &[]).is_err());
    assert!(
        validate_async_declaration(
            "radio",
            AsyncProof::Bounded,
            Some("synchronous-operation"),
            &[],
        )
        .is_err()
    );
}

#[test]
fn rejects_parent_paths() {
    assert!(validate_relative_path(Path::new("../crates/src/lib.rs")).is_err());
    assert!(validate_relative_path(Path::new("crates/src/lib.rs")).is_ok());
}

#[test]
fn vendor_anchors_are_unique_regular_files() {
    let anchor = PathBuf::from("Cargo.toml");
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    assert!(validate_vendor_anchors("radio", std::slice::from_ref(&anchor), root).is_ok());
    assert!(validate_vendor_anchors("radio", &[anchor.clone(), anchor], root).is_err());
}

#[test]
fn dependency_cycles_fail_closed() {
    let capability = |id: &str, dependency: &str| Capability {
        id: id.to_owned(),
        title: id.to_owned(),
        scope: id.to_owned(),
        implementation: ImplementationProof::Complete,
        host: HostProof::Covered,
        vendor: VendorProof::NotApplicable,
        hil: HilProof::NotApplicable,
        async_proof: AsyncProof::NotApplicable,
        dependencies: vec![dependency.to_owned()],
        gaps: Vec::new(),
        evidence: Vec::new(),
        source_contracts: Vec::new(),
        vendor_evidence: Vec::new(),
        vendor_not_applicable: Some("not-applicable".to_owned()),
        hil_requirements: Vec::new(),
        hil_checks: Vec::new(),
        hil_decisions: Vec::new(),
        hil_not_applicable: Some("not-applicable".to_owned()),
        async_not_applicable: Some("not-applicable".to_owned()),
    };
    let capabilities = BTreeMap::from([
        ("a".to_owned(), capability("a", "b")),
        ("b".to_owned(), capability("b", "a")),
    ]);
    assert!(validate_dependencies(&capabilities).is_err());
}

#[test]
fn vendor_evidence_must_name_a_declared_root() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let scenarios = ScenarioCatalog::load(&root, Path::new("hil/scenarios")).unwrap();
    let context = StaticContext {
        root: &root,
        scenario_catalog: &scenarios,
    };
    let mut capability = toml_edit::de::from_str::<ManifestDocument>(COMPLETE)
        .unwrap()
        .capabilities
        .remove(0);
    capability.hil_requirements.clear();
    capability.hil_not_applicable = Some("vendor-evidence-fixture".into());
    validate_capability_declaration(&capability, &context).unwrap();
    let mut other = capability.clone();
    other.vendor_evidence[0].symbol = "undeclared_root".into();
    assert!(validate_capability_declaration(&other, &context).is_err());
    let mut repeated = capability.clone();
    repeated
        .vendor_evidence
        .push(repeated.vendor_evidence[0].clone());
    assert!(validate_capability_declaration(&repeated, &context).is_err());
}

/// A temporary repository root with one digested source directory.
pub(crate) struct Fixture(pub(crate) PathBuf);
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

pub(crate) fn fixture_root(name: &str) -> Fixture {
    let root = std::env::temp_dir().join(format!(
        "oer-scenario-evidence-{name}-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("production/src")).unwrap();
    fs::write(root.join("production/src/lib.rs"), b"production-input").unwrap();
    Fixture(root)
}

/// A current native index over `root/production` holding MATCH entries for
/// `(suite, source, symbol)` roots.
pub(crate) fn native_evidence(root: &Path, roots: &[(&str, &str, &str)]) -> NativeEvidence {
    let path = PathBuf::from("production");
    if !root.join(&path).exists() {
        fs::create_dir_all(root.join("production/src")).unwrap();
        fs::write(root.join("production/src/lib.rs"), b"production-input").unwrap();
    }
    let index = scenario_evidence::Index {
        schema: scenario_evidence::SCHEMA,
        command: scenario_evidence::COMMAND.into(),
        target: "test-radio".into(),
        inputs: BTreeMap::new(),
        sources: vec![scenario_evidence::SourceDigest {
            sha256: scenario_evidence::digest_directory(root, &path).unwrap(),
            path,
        }],
        entries: roots
            .iter()
            .map(|(suite, source, symbol)| scenario_evidence::Entry {
                suite: (*suite).into(),
                source: (*source).into(),
                symbol: (*symbol).into(),
                production: format!("open_{symbol}"),
                verdict: scenario_evidence::MATCH.into(),
                cases: 1,
                reviews: vec!["ab".repeat(32)],
                coverage: scenario_evidence::Coverage {
                    blocks: scenario_evidence::Count {
                        reached: 1,
                        total: 1,
                    },
                    directions: scenario_evidence::Count {
                        reached: 0,
                        total: 0,
                    },
                    excluded: 0,
                    untriaged: 0,
                },
                observation: scenario_evidence::Observation {
                    executed: 1,
                    observed: 1,
                    reviewed: 0,
                    untriaged: 0,
                },
            })
            .collect(),
        untriaged: vec![],
        unobserved: vec![],
    };
    index.validate("test-radio").unwrap();
    let current = index.is_current(root);
    NativeEvidence { index, current }
}

#[test]
fn native_index_supports_only_current_match_entries_of_the_referenced_suite() {
    let fixture = fixture_root("supports");
    let root = &fixture.0;
    let evidence = native_evidence(root, &[("radio", "archive", "set_channel")]);
    let reference = VendorEvidenceRef {
        suite: "radio".into(),
        source: "archive".into(),
        symbol: "set_channel".into(),
    };
    assert!(evidence.current && evidence.supports(&reference));
    assert_eq!(evidence.current_entries(), 1);
    let other_suite = VendorEvidenceRef {
        suite: "unrelated".into(),
        ..reference.clone()
    };
    assert!(!evidence.supports(&other_suite));
    // A changed production source makes the whole index stale.
    fs::write(
        root.join("production/src/lib.rs"),
        b"changed-production-input",
    )
    .unwrap();
    let stale = native_evidence_reloaded(root, evidence.index);
    assert!(!stale.current && !stale.supports(&reference));
    assert_eq!(stale.current_entries(), 0);
    // A new file in a digested directory is also a change.
    fs::write(root.join("production/src/lib.rs"), b"production-input").unwrap();
    assert!(native_evidence_reloaded(root, stale.index.clone()).current);
    // Documentation establishes no verdict.
    fs::write(root.join("production/README.md"), b"notes").unwrap();
    assert!(native_evidence_reloaded(root, stale.index.clone()).current);
    fs::write(root.join("production/src/new.rs"), b"").unwrap();
    assert!(!native_evidence_reloaded(root, stale.index).current);
}

fn native_evidence_reloaded(root: &Path, index: scenario_evidence::Index) -> NativeEvidence {
    let current = index.is_current(root);
    NativeEvidence { index, current }
}

#[test]
fn corrupt_unsupported_and_non_match_native_indexes_fail_closed() {
    let fixture = fixture_root("invalid");
    let root = &fixture.0;
    let path = Path::new("index.json");
    // Absent evidence supports nothing, but is not an error.
    let absent = NativeEvidence::load(root, path, "test-radio").unwrap();
    assert!(!absent.current && absent.index.entries.is_empty());
    fs::write(root.join(path), "{not-json").unwrap();
    assert!(NativeEvidence::load(root, path, "test-radio").is_err());
    let valid = native_evidence(root, &[("radio", "archive", "set_channel")]).index;
    let write = |index: &scenario_evidence::Index| {
        fs::write(root.join(path), serde_json::to_vec(index).unwrap()).unwrap();
    };
    write(&valid);
    assert!(
        NativeEvidence::load(root, path, "test-radio")
            .unwrap()
            .current
    );
    assert!(NativeEvidence::load(root, path, "other-radio").is_err());
    let mut changed = valid.clone();
    changed.command = "project verify vendor evidence index".into();
    write(&changed);
    assert!(NativeEvidence::load(root, path, "test-radio").is_err());
    for verdict in ["diff", "incomplete"] {
        let mut changed = valid.clone();
        changed.entries[0].verdict = verdict.into();
        write(&changed);
        assert!(NativeEvidence::load(root, path, "test-radio").is_err());
    }
    let mut repeated = valid.clone();
    repeated.entries.push(repeated.entries[0].clone());
    write(&repeated);
    assert!(NativeEvidence::load(root, path, "test-radio").is_err());
    let mut escaping = valid.clone();
    escaping.sources[0].path = PathBuf::from("../outside");
    write(&escaping);
    assert!(NativeEvidence::load(root, path, "test-radio").is_err());
}

// Called with independently sealed archived observations by the review tests.
pub(crate) fn assert_reviewed_hil(
    root: &Path,
    document: CapabilityDocument,
    index: &HilEvidenceIndex,
    catalog: &ScenarioCatalog,
) {
    let declarations = BTreeMap::from([(document.id.clone(), document.clone())]);
    let evidence = NativeEvidence {
        index: scenario_evidence::Index {
            schema: scenario_evidence::SCHEMA,
            command: scenario_evidence::COMMAND.into(),
            target: "test-radio".into(),
            inputs: BTreeMap::new(),
            sources: vec![],
            entries: vec![],
            untriaged: vec![],
            unobserved: vec![],
        },
        current: false,
    };
    let context = EvaluationContext {
        root,
        evidence: &evidence,
        scenario_catalog: catalog,
        hil_index: index,
        declarations: &declarations,
    };
    let capability = evaluate_capability(document, &context).unwrap();
    assert_eq!(capability.hil, HilProof::Qualified);
    assert!(capability.proof_ready());
    assert_eq!(capability.hil_decisions[0].reviews[0].status, "applied");
    assert!(
        capability
            .evidence
            .iter()
            .any(|r| r.starts_with("hil:old/exchange"))
    );
}

#[test]
fn native_index_coverage_must_account_for_every_uncovered_location() {
    let fixture = fixture_root("coverage");
    let evidence = native_evidence(&fixture.0, &[("radio", "archive", "set_channel")]);
    let rejected = |mutate: &dyn Fn(&mut scenario_evidence::Index)| {
        let mut index = evidence.index.clone();
        mutate(&mut index);
        index.validate("test-radio").is_err()
    };
    // More reached than exist.
    assert!(rejected(&|i| i.entries[0].coverage.blocks.reached = 2));
    // An uncovered block neither excluded nor untriaged.
    assert!(rejected(&|i| i.entries[0].coverage.blocks.total = 2));
    // Untriaged locations the index does not list.
    assert!(rejected(&|i| {
        i.entries[0].coverage.blocks.total = 2;
        i.entries[0].coverage.untriaged = 1;
    }));
    let location = |offset| scenario_evidence::Location {
        function: "set_channel".into(),
        offset,
        kind: scenario_evidence::LocationKind::Block,
    };
    let mut listed = evidence.index.clone();
    listed.entries[0].coverage.blocks.total = 2;
    listed.entries[0].coverage.untriaged = 1;
    listed.untriaged = vec![location(4)];
    listed.validate("test-radio").unwrap();
    // Listed locations are ascending and unique.
    listed.untriaged = vec![location(4), location(4)];
    assert!(listed.validate("test-radio").is_err());
}

#[test]
fn native_index_observation_must_account_for_every_executed_line() {
    let fixture = fixture_root("observation");
    let evidence = native_evidence(&fixture.0, &[("radio", "archive", "set_channel")]);
    let rejected = |mutate: &dyn Fn(&mut scenario_evidence::Index)| {
        let mut index = evidence.index.clone();
        mutate(&mut index);
        index.validate("test-radio").is_err()
    };
    // An executed line neither observed, reviewed nor untriaged.
    assert!(rejected(&|i| i.entries[0].observation.executed = 2));
    let line = |line| scenario_evidence::SourceLine {
        path: "production/src/lib.rs".into(),
        line,
    };
    let mut listed = evidence.index.clone();
    listed.entries[0].observation.executed = 2;
    listed.entries[0].observation.untriaged = 1;
    listed.unobserved = vec![line(3)];
    listed.validate("test-radio").unwrap();
    // Listed lines are relative, ascending and unique.
    listed.unobserved = vec![line(3), line(3)];
    assert!(listed.validate("test-radio").is_err());
    listed.unobserved = vec![scenario_evidence::SourceLine {
        path: "/production/src/lib.rs".into(),
        line: 3,
    }];
    assert!(listed.validate("test-radio").is_err());
}
