use super::*;

const COMPLETE: &str = r#"
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

/// Capability declarations as a catalog carries them.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Declarations {
    capabilities: Vec<CapabilityDocument>,
}

fn declarations(input: &str) -> std::result::Result<Declarations, toml_edit::de::Error> {
    toml_edit::de::from_str(input)
}

#[test]
fn parses_a_strict_capability_declaration() {
    let manifest = declarations(COMPLETE).unwrap();
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
    let manifest = declarations(&input).unwrap();
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
    let error = match declarations(&input) {
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
    let mut capability = declarations(COMPLETE).unwrap().capabilities.remove(0);
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
    NativeEvidence {
        shards,
        stale: Vec::new(),
    }
}

/// A current shard of `suite` over `root/production` holding MATCH entries
/// for its `(suite, source, symbol)` roots.
pub(crate) fn native_shard(
    root: &Path,
    suite: &str,
    roots: &[(&str, &str, &str)],
) -> oer_vendor_evidence_shard::Index {
    let path = PathBuf::from("production");
    if !root.join(&path).exists() {
        fs::create_dir_all(root.join("production/src")).unwrap();
        fs::write(root.join("production/src/lib.rs"), b"production-input").unwrap();
    }
    let index = oer_vendor_evidence_shard::Index {
        schema: oer_vendor_evidence_shard::SCHEMA,
        command: oer_vendor_evidence_shard::BLOBRAY.into(),
        target: "test-radio".into(),
        scenario: suite.into(),
        inputs: BTreeMap::new(),
        sources: vec![oer_vendor_evidence_shard::SourceDigest {
            sha256: oer_vendor_evidence_shard::digest_directory(root, &path).unwrap(),
            path,
        }],
        dependence: oer_vendor_evidence_shard::Dependence::whole_closure("test"),
        entries: roots
            .iter()
            .map(|(suite, source, symbol)| oer_vendor_evidence_shard::Entry {
                suite: (*suite).into(),
                source: (*source).into(),
                symbol: (*symbol).into(),
                production: format!("open_{symbol}"),
                verdict: oer_vendor_evidence_shard::MATCH.into(),
                cases: 1,
                reviews: vec!["ab".repeat(32)],
                coverage: Some(oer_vendor_evidence_shard::Coverage {
                    blocks: oer_vendor_evidence_shard::Count {
                        reached: 1,
                        total: 1,
                    },
                    directions: oer_vendor_evidence_shard::Count {
                        reached: 0,
                        total: 0,
                    },
                    open: 0,
                    excluded: 0,
                    untriaged: 0,
                }),
                observation: Some(oer_vendor_evidence_shard::Observation {
                    executed: 1,
                    observed: 1,
                    reviewed: 0,
                    untriaged: 0,
                }),
                state: Some(oer_vendor_evidence_shard::State {
                    written: 1,
                    compared: 1,
                    reviewed: 0,
                    untriaged: 0,
                }),
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

fn reloaded(root: &Path, shard: &oer_vendor_evidence_shard::Index) -> NativeEvidence {
    NativeEvidence {
        shards: vec![(shard.clone(), shard.is_current(root))],
        stale: Vec::new(),
    }
}

#[test]
fn a_stale_shard_voids_the_whole_derived_index() {
    let fixture = fixture_root("shards");
    let root = &fixture.0;
    fs::create_dir_all(root.join("other/src")).unwrap();
    fs::write(root.join("other/src/lib.rs"), b"other-input").unwrap();
    let mut other = native_shard(root, "other", &[("other", "archive", "other_root")]);
    other.sources = vec![oer_vendor_evidence_shard::SourceDigest {
        sha256: oer_vendor_evidence_shard::digest_directory(root, Path::new("other")).unwrap(),
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
                oer_vendor_evidence_shard::SHARD_EXTENSION
            )),
            serde_json::to_vec(shard).unwrap(),
        )
        .unwrap();
    }
    let evidence = NativeEvidence::load(root, directory, "test-radio").unwrap();
    assert_eq!(evidence.current_entries(), 2);
    // A source of `radio` changed since the index was computed: the index is
    // recomputed whole, never partly trusted, so it holds no evidence until
    // then and names the stale scenario.
    fs::write(root.join("production/src/lib.rs"), b"changed").unwrap();
    let evidence = NativeEvidence::load(root, directory, "test-radio").unwrap();
    assert_eq!(evidence.stale, ["radio"]);
    assert_eq!(evidence.entries(), 0);
    assert!(!evidence.supports(&VendorEvidenceRef {
        suite: "other".into(),
        source: "archive".into(),
        symbol: "other_root".into(),
    }));
}

#[test]
fn cross_scenario_views_follow_every_shard() {
    let fixture = fixture_root("views");
    let root = &fixture.0;
    let location = |function: &str| oer_vendor_evidence_shard::Location {
        function: function.into(),
        offset: 4,
        kind: oer_vendor_evidence_shard::LocationKind::Block,
    };
    let line = |line| oer_vendor_evidence_shard::SourceLine {
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
    let evidence = oer_vendor_evidence_shard::Evidence {
        shards: vec![(first.clone(), true), (second.clone(), true)],
        other_schema: Vec::new(),
    };
    // The second scenario's closure covers the shared location.
    assert_eq!(evidence.untriaged(), vec![location("solo")]);
    // The second scenario observes line 2.
    assert_eq!(evidence.unobserved(), vec![line(1)]);
    second.untriaged = vec![location("shared")];
    let evidence = oer_vendor_evidence_shard::Evidence {
        shards: vec![(first, true), (second, true)],
        other_schema: Vec::new(),
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
    let write = |index: &oer_vendor_evidence_shard::Index| {
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
    fs::remove_file(root.join(directory).join("notes.txt")).unwrap();
    // What another checkout explains is staleness of the derived index, not
    // an error: a recorded source deleted since, or a shard another schema
    // of the format wrote, leaves the index holding no evidence.
    let mut missing = valid.clone();
    missing.sources[0].path = PathBuf::from("deleted/source");
    write(&missing);
    let evidence = NativeEvidence::load(root, directory, "test-radio").unwrap();
    assert_eq!(
        (evidence.stale.as_slice(), evidence.entries()),
        (&["radio".to_owned()][..], 0)
    );
    let mut older = valid.clone();
    older.schema -= 1;
    write(&older);
    let evidence = NativeEvidence::load(root, directory, "test-radio").unwrap();
    assert_eq!(
        (evidence.stale.as_slice(), evidence.entries()),
        (&["radio".to_owned()][..], 0)
    );
}

#[test]
fn native_index_coverage_must_account_for_every_uncovered_location() {
    let fixture = fixture_root("coverage");
    let evidence = native_shard(&fixture.0, "radio", &[("radio", "archive", "set_channel")]);
    let rejected = |mutate: &dyn Fn(&mut oer_vendor_evidence_shard::Index)| {
        let mut index = evidence.clone();
        mutate(&mut index);
        index.validate("test-radio").is_err()
    };
    // More reached than exist.
    assert!(rejected(&|i| i.entries[0]
        .coverage
        .as_mut()
        .unwrap()
        .blocks
        .reached = 2));
    // An uncovered block neither excluded nor untriaged.
    assert!(rejected(&|i| i.entries[0]
        .coverage
        .as_mut()
        .unwrap()
        .blocks
        .total = 2));
    // A claim's untriaged location another claim covers is not listed.
    let mut covered = evidence.clone();
    covered.entries[0].coverage.as_mut().unwrap().blocks.total = 2;
    covered.entries[0].coverage.as_mut().unwrap().untriaged = 1;
    covered.validate("test-radio").unwrap();
    let location = |offset| oer_vendor_evidence_shard::Location {
        function: "set_channel".into(),
        offset,
        kind: oer_vendor_evidence_shard::LocationKind::Block,
    };
    let mut listed = evidence.clone();
    listed.entries[0].coverage.as_mut().unwrap().blocks.total = 2;
    listed.entries[0].coverage.as_mut().unwrap().untriaged = 1;
    listed.untriaged = vec![location(4)];
    listed.validate("test-radio").unwrap();
    // Listed locations are ascending and unique.
    listed.untriaged = vec![location(4), location(4)];
    assert!(listed.validate("test-radio").is_err());
    // An open transfer site must be excluded or untriaged like a block.
    assert!(rejected(&|i| i.entries[0]
        .coverage
        .as_mut()
        .unwrap()
        .open = 1));
    let mut open = evidence.clone();
    open.entries[0].coverage.as_mut().unwrap().open = 1;
    open.entries[0].coverage.as_mut().unwrap().untriaged = 1;
    open.untriaged = vec![oer_vendor_evidence_shard::Location {
        function: "set_channel".into(),
        offset: 8,
        kind: oer_vendor_evidence_shard::LocationKind::Followed,
    }];
    open.validate("test-radio").unwrap();
}

#[test]
fn native_index_state_must_account_for_every_written_byte() {
    let fixture = fixture_root("state");
    let evidence = native_shard(&fixture.0, "radio", &[("radio", "archive", "set_channel")]);
    // A written byte neither compared, reviewed nor untriaged.
    let mut index = evidence.clone();
    index.entries[0].state.as_mut().unwrap().written = 2;
    assert!(index.validate("test-radio").is_err());
    let range = |offset, length| oer_vendor_evidence_shard::StateRange {
        symbol: "phy_param".into(),
        offset,
        length,
    };
    let mut listed = evidence.clone();
    listed.entries[0].state.as_mut().unwrap().written = 2;
    listed.entries[0].state.as_mut().unwrap().untriaged = 1;
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
    let rejected = |mutate: &dyn Fn(&mut oer_vendor_evidence_shard::Index)| {
        let mut index = evidence.clone();
        mutate(&mut index);
        index.validate("test-radio").is_err()
    };
    // An executed line neither observed, reviewed nor untriaged.
    assert!(rejected(&|i| i.entries[0]
        .observation
        .as_mut()
        .unwrap()
        .executed = 2));
    let line = |line| oer_vendor_evidence_shard::SourceLine {
        path: "production/src/lib.rs".into(),
        line,
    };
    let mut listed = evidence.clone();
    listed.entries[0].observation.as_mut().unwrap().executed = 2;
    listed.entries[0].observation.as_mut().unwrap().untriaged = 1;
    listed.unobserved = vec![line(3)];
    listed.validate("test-radio").unwrap();
    // Listed lines are relative, ascending and unique.
    listed.unobserved = vec![line(3), line(3)];
    assert!(listed.validate("test-radio").is_err());
    listed.unobserved = vec![oer_vendor_evidence_shard::SourceLine {
        path: "/production/src/lib.rs".into(),
        line: 3,
    }];
    assert!(listed.validate("test-radio").is_err());
}

#[test]
fn source_compiled_entries_carry_no_blobray_metrics() {
    let fixture = fixture_root("stand");
    let mut shard = native_shard(&fixture.0, "stand", &[("stand", "esp-idf", "transmit")]);
    let entry = &mut shard.entries[0];
    (entry.coverage, entry.observation, entry.state) = (None, None, None);
    shard.validate("test-radio").unwrap();
    let text = serde_json::to_string(&shard).unwrap();
    assert!(!text.contains("\"coverage\""));
    let reloaded: oer_vendor_evidence_shard::Index = serde_json::from_str(&text).unwrap();
    assert_eq!(reloaded, shard);
}

#[test]
fn an_absent_evidence_directory_is_reported_and_supports_nothing() {
    let fixture = fixture_root("absent-directories");
    let root = &fixture.0;
    fs::create_dir_all(root.join("verification/chip/evidence")).unwrap();
    fs::create_dir_all(root.join("target/hil/chip")).unwrap();
    // A checkout's runs link before its first run points nowhere.
    std::os::unix::fs::symlink(
        root.join("store/never-created"),
        root.join("target/hil/chip/runs"),
    )
    .unwrap();
    let absent = absent_directories(
        root,
        &[
            ("vendor-evidence", Path::new("verification/chip/evidence")),
            ("hil-evidence", Path::new("hil/evidence/chip")),
            ("hil-runs", Path::new("target/hil/chip/runs")),
            ("hil-evidence", Path::new("")),
        ],
    );
    assert_eq!(
        absent,
        [
            AbsentDirectory {
                kind: "hil-evidence",
                path: PathBuf::from("hil/evidence/chip"),
                state: "absent",
            },
            AbsentDirectory {
                kind: "hil-runs",
                path: PathBuf::from("target/hil/chip/runs"),
                state: "absent",
            },
        ]
    );
    assert_eq!(
        crate::report::absent_lines(&absent),
        [
            "EVIDENCE-DIR\tabsent\tkind=hil-evidence\tpath=hil/evidence/chip\tshards=0",
            "EVIDENCE-DIR\tabsent\tkind=hil-runs\tpath=target/hil/chip/runs\tbundles=0",
        ]
    );
    // A stale derived vendor index is reported as such and holds nothing.
    assert_eq!(
        crate::report::absent_lines(&[AbsentDirectory {
            kind: "vendor-evidence",
            path: PathBuf::from("target/verification/chip/evidence"),
            state: "stale",
        }]),
        [
            "EVIDENCE-DIR\tstale\tkind=vendor-evidence\tpath=target/verification/chip/evidence\tshards=0"
        ]
    );
    // What an absent vendor index holds supports no obligation.
    let evidence = NativeEvidence::load(root, Path::new("verification/gone"), "chip").unwrap();
    assert_eq!(evidence.entries(), 0);
    assert!(!evidence.supports(&VendorEvidenceRef {
        suite: "suite".into(),
        source: "source".into(),
        symbol: "symbol".into(),
    }));
}
