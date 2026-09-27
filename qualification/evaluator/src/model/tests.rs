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
/// `(suite, source, symbol)` roots, one shard per suite.
pub(crate) fn native_evidence(root: &Path, roots: &[(&str, &str, &str)]) -> NativeEvidence {
    let suites: BTreeSet<&str> = roots.iter().map(|(suite, ..)| *suite).collect();
    let shards = suites
        .into_iter()
        .map(|suite| {
            let own: Vec<_> = roots.iter().filter(|r| r.0 == suite).copied().collect();
            let shard = native_shard(root, suite, &own);
            let current = shard.is_current(root);
            (shard, current)
        })
        .collect();
    NativeEvidence { shards }
}

/// A current shard of `suite` over `root/production` holding MATCH entries
/// for its `(suite, source, symbol)` roots.
pub(crate) fn native_shard(
    root: &Path,
    suite: &str,
    roots: &[(&str, &str, &str)],
) -> scenario_evidence::Index {
    let path = PathBuf::from("production");
    if !root.join(&path).exists() {
        fs::create_dir_all(root.join("production/src")).unwrap();
        fs::write(root.join("production/src/lib.rs"), b"production-input").unwrap();
    }
    let index = scenario_evidence::Index {
        schema: scenario_evidence::SCHEMA,
        command: scenario_evidence::COMMAND.into(),
        target: "test-radio".into(),
        scenario: suite.into(),
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
                    open: 0,
                    excluded: 0,
                    untriaged: 0,
                },
                observation: scenario_evidence::Observation {
                    executed: 1,
                    observed: 1,
                    reviewed: 0,
                    untriaged: 0,
                },
                state: scenario_evidence::State {
                    written: 1,
                    compared: 1,
                    reviewed: 0,
                    untriaged: 0,
                },
            })
            .collect(),
        untriaged: vec![],
        functions: vec![],
        unobserved: vec![],
        observed: vec![],
        unprojected: vec![],
    };
    index.validate("test-radio").unwrap();
    index
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
    assert!(evidence.supports(&reference));
    assert_eq!(evidence.current_entries(), 1);
    let other_suite = VendorEvidenceRef {
        suite: "unrelated".into(),
        ..reference.clone()
    };
    assert!(!evidence.supports(&other_suite));
    // A changed production source makes the shard stale.
    fs::write(
        root.join("production/src/lib.rs"),
        b"changed-production-input",
    )
    .unwrap();
    let shard = evidence.shards[0].0.clone();
    let stale = reloaded(root, &shard);
    assert!(!stale.supports(&reference));
    assert_eq!(stale.current_entries(), 0);
    assert_eq!(stale.entries(), 1);
    // A new file in a digested directory is also a change.
    fs::write(root.join("production/src/lib.rs"), b"production-input").unwrap();
    assert!(reloaded(root, &shard).supports(&reference));
    // Documentation establishes no verdict.
    fs::write(root.join("production/README.md"), b"notes").unwrap();
    assert!(reloaded(root, &shard).supports(&reference));
    fs::write(root.join("production/src/new.rs"), b"").unwrap();
    assert!(!reloaded(root, &shard).supports(&reference));
}

fn reloaded(root: &Path, shard: &scenario_evidence::Index) -> NativeEvidence {
    NativeEvidence {
        shards: vec![(shard.clone(), shard.is_current(root))],
    }
}

#[test]
fn a_stale_shard_leaves_other_scenarios_current() {
    let fixture = fixture_root("shards");
    let root = &fixture.0;
    fs::create_dir_all(root.join("other/src")).unwrap();
    fs::write(root.join("other/src/lib.rs"), b"other-input").unwrap();
    let mut other = native_shard(root, "other", &[("other", "archive", "other_root")]);
    other.sources = vec![scenario_evidence::SourceDigest {
        sha256: scenario_evidence::digest_directory(root, Path::new("other")).unwrap(),
        path: PathBuf::from("other"),
    }];
    let radio = native_shard(root, "radio", &[("radio", "archive", "set_channel")]);
    let directory = Path::new("shards");
    fs::create_dir_all(root.join(directory)).unwrap();
    for shard in [&radio, &other] {
        fs::write(
            root.join(directory).join(format!(
                "{}.{}",
                shard.scenario,
                scenario_evidence::SHARD_EXTENSION
            )),
            serde_json::to_vec(shard).unwrap(),
        )
        .unwrap();
    }
    fs::write(root.join("production/src/lib.rs"), b"changed").unwrap();
    let evidence = NativeEvidence::load(root, directory, "test-radio").unwrap();
    let supported = |suite: &str, symbol: &str| {
        evidence.supports(&VendorEvidenceRef {
            suite: suite.into(),
            source: "archive".into(),
            symbol: symbol.into(),
        })
    };
    assert!(!supported("radio", "set_channel"));
    assert!(supported("other", "other_root"));
    assert_eq!(evidence.current_entries(), 1);
}

#[test]
fn cross_scenario_views_follow_every_shard() {
    let fixture = fixture_root("views");
    let root = &fixture.0;
    let location = |function: &str| scenario_evidence::Location {
        function: function.into(),
        offset: 4,
        kind: scenario_evidence::LocationKind::Block,
    };
    let line = |line| scenario_evidence::SourceLine {
        path: PathBuf::from("production/src/lib.rs"),
        line,
    };
    let mut first = native_shard(root, "first", &[]);
    first.untriaged = vec![location("shared"), location("solo")];
    first.functions = vec!["shared".into(), "solo".into()];
    first.unobserved = vec![line(1), line(2)];
    let mut second = native_shard(root, "second", &[]);
    second.functions = vec!["shared".into()];
    second.observed = vec![line(2)];
    let evidence = scenario_evidence::Evidence {
        shards: vec![(first.clone(), true), (second.clone(), true)],
    };
    // The second scenario's closure covers the shared location.
    assert_eq!(evidence.untriaged(), vec![location("solo")]);
    // The second scenario observes line 2.
    assert_eq!(evidence.unobserved(), vec![line(1)]);
    second.untriaged = vec![location("shared")];
    let evidence = scenario_evidence::Evidence {
        shards: vec![(first, true), (second, true)],
    };
    assert_eq!(
        evidence.untriaged(),
        vec![location("shared"), location("solo")]
    );
}

#[test]
fn corrupt_unsupported_and_non_match_native_indexes_fail_closed() {
    let fixture = fixture_root("invalid");
    let root = &fixture.0;
    let directory = Path::new("index");
    // Absent evidence supports nothing, but is not an error.
    let absent = NativeEvidence::load(root, directory, "test-radio").unwrap();
    assert!(absent.shards.is_empty());
    fs::create_dir_all(root.join(directory)).unwrap();
    let path = directory.join("radio.json");
    fs::write(root.join(&path), "{not-json").unwrap();
    assert!(NativeEvidence::load(root, directory, "test-radio").is_err());
    let valid = native_shard(root, "radio", &[("radio", "archive", "set_channel")]);
    let write = |index: &scenario_evidence::Index| {
        fs::write(root.join(&path), serde_json::to_vec(index).unwrap()).unwrap();
    };
    write(&valid);
    assert_eq!(
        NativeEvidence::load(root, directory, "test-radio")
            .unwrap()
            .current_entries(),
        1
    );
    assert!(NativeEvidence::load(root, directory, "other-radio").is_err());
    let mut changed = valid.clone();
    changed.command = "project verify vendor evidence index".into();
    write(&changed);
    assert!(NativeEvidence::load(root, directory, "test-radio").is_err());
    for verdict in ["diff", "incomplete"] {
        let mut changed = valid.clone();
        changed.entries[0].verdict = verdict.into();
        write(&changed);
        assert!(NativeEvidence::load(root, directory, "test-radio").is_err());
    }
    let mut repeated = valid.clone();
    repeated.entries.push(repeated.entries[0].clone());
    write(&repeated);
    assert!(NativeEvidence::load(root, directory, "test-radio").is_err());
    let mut escaping = valid.clone();
    escaping.sources[0].path = PathBuf::from("../outside");
    write(&escaping);
    assert!(NativeEvidence::load(root, directory, "test-radio").is_err());
    // A shard holds only its own scenario's entries, under its own name.
    let mut foreign = valid.clone();
    foreign.entries[0].suite = "other".into();
    write(&foreign);
    assert!(NativeEvidence::load(root, directory, "test-radio").is_err());
    let mut renamed = valid.clone();
    renamed.scenario = "other".into();
    renamed.entries.clear();
    write(&renamed);
    assert!(NativeEvidence::load(root, directory, "test-radio").is_err());
    write(&valid);
    fs::write(root.join(directory).join("notes.txt"), "").unwrap();
    assert!(NativeEvidence::load(root, directory, "test-radio").is_err());
}

// Called with independently sealed archived observations by the review tests.
pub(crate) fn assert_reviewed_hil(
    root: &Path,
    document: CapabilityDocument,
    index: &HilEvidenceIndex,
    catalog: &ScenarioCatalog,
) {
    let declarations = BTreeMap::from([(document.id.clone(), document.clone())]);
    let evidence = NativeEvidence { shards: vec![] };
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
    let evidence = native_shard(&fixture.0, "radio", &[("radio", "archive", "set_channel")]);
    let rejected = |mutate: &dyn Fn(&mut scenario_evidence::Index)| {
        let mut index = evidence.clone();
        mutate(&mut index);
        index.validate("test-radio").is_err()
    };
    // More reached than exist.
    assert!(rejected(&|i| i.entries[0].coverage.blocks.reached = 2));
    // An uncovered block neither excluded nor untriaged.
    assert!(rejected(&|i| i.entries[0].coverage.blocks.total = 2));
    // A claim's untriaged location another claim covers is not listed.
    let mut covered = evidence.clone();
    covered.entries[0].coverage.blocks.total = 2;
    covered.entries[0].coverage.untriaged = 1;
    covered.validate("test-radio").unwrap();
    let location = |offset| scenario_evidence::Location {
        function: "set_channel".into(),
        offset,
        kind: scenario_evidence::LocationKind::Block,
    };
    let mut listed = evidence.clone();
    listed.entries[0].coverage.blocks.total = 2;
    listed.entries[0].coverage.untriaged = 1;
    listed.untriaged = vec![location(4)];
    listed.validate("test-radio").unwrap();
    // Listed locations are ascending and unique.
    listed.untriaged = vec![location(4), location(4)];
    assert!(listed.validate("test-radio").is_err());
    // An open transfer site must be excluded or untriaged like a block.
    assert!(rejected(&|i| i.entries[0].coverage.open = 1));
    let mut open = evidence.clone();
    open.entries[0].coverage.open = 1;
    open.entries[0].coverage.untriaged = 1;
    open.untriaged = vec![scenario_evidence::Location {
        function: "set_channel".into(),
        offset: 8,
        kind: scenario_evidence::LocationKind::Followed,
    }];
    open.validate("test-radio").unwrap();
}

#[test]
fn native_index_state_must_account_for_every_written_byte() {
    let fixture = fixture_root("state");
    let evidence = native_shard(&fixture.0, "radio", &[("radio", "archive", "set_channel")]);
    // A written byte neither compared, reviewed nor untriaged.
    let mut index = evidence.clone();
    index.entries[0].state.written = 2;
    assert!(index.validate("test-radio").is_err());
    let range = |offset, length| scenario_evidence::StateRange {
        symbol: "phy_param".into(),
        offset,
        length,
    };
    let mut listed = evidence.clone();
    listed.entries[0].state.written = 2;
    listed.entries[0].state.untriaged = 1;
    listed.unprojected = vec![range(0x16, 1), range(0x11e, 1)];
    listed.validate("test-radio").unwrap();
    // Listed ranges are nonempty, ascending and coalesced.
    for ranges in [
        vec![range(0x16, 0)],
        vec![range(0x11e, 1), range(0x16, 1)],
        vec![range(0x16, 1), range(0x17, 1)],
    ] {
        listed.unprojected = ranges;
        assert!(listed.validate("test-radio").is_err());
    }
}

#[test]
fn native_index_observation_must_account_for_every_executed_line() {
    let fixture = fixture_root("observation");
    let evidence = native_shard(&fixture.0, "radio", &[("radio", "archive", "set_channel")]);
    let rejected = |mutate: &dyn Fn(&mut scenario_evidence::Index)| {
        let mut index = evidence.clone();
        mutate(&mut index);
        index.validate("test-radio").is_err()
    };
    // An executed line neither observed, reviewed nor untriaged.
    assert!(rejected(&|i| i.entries[0].observation.executed = 2));
    let line = |line| scenario_evidence::SourceLine {
        path: "production/src/lib.rs".into(),
        line,
    };
    let mut listed = evidence.clone();
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
