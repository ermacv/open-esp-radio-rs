use super::*;

const COMPLETE: &str = r#"
schema = 4
target = "test-radio"
required-capabilities = ["channel-switch"]

[verification]
project = "tools/verification-project.toml"
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
fn corrupt_and_invalid_vendor_indexes_fail_closed() {
    let path = std::env::temp_dir().join(format!(
        "open-radio-invalid-vendor-index-{}.json",
        std::process::id()
    ));
    std::fs::write(&path, "{not-json").unwrap();
    assert!(VendorEvidenceIndex::load(&path, "test").is_err());

    std::fs::write(
        &path,
        r#"{
  "schema_version": 1,
  "command": "wrong-command",
  "project": "test",
  "complete_project_run": true,
  "entries": []
}"#,
    )
    .unwrap();
    let error = VendorEvidenceIndex::load(&path, "test")
        .unwrap_err()
        .to_string();
    assert!(error.contains("unsupported or incomplete"), "{error}");
    std::fs::remove_file(path).unwrap();
}

#[test]
fn vendor_entries_keep_contract_identity_and_do_not_inherit_another_roots_result() {
    use serde_json::json;
    let root = std::env::temp_dir().join(format!("oer-vendor-contracts-{}", std::process::id()));
    fs::create_dir(&root).unwrap();
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }
    let _cleanup = Cleanup(root.clone());
    for name in ["bluetooth.rs", "wifi.rs"] {
        fs::write(root.join(name), b"production-input").unwrap();
    }
    let make_entry = |path: &str, symbol: &str| {
        json!({
            "suite":"radio","source":"archive","symbol":symbol,
            "evidence_class":"production-trace","status":"match","release_eligible":true,
            "rust_component":symbol,"evidence_digest":"ab".repeat(32),"baseline_passed":true,
            "artifact_hashes":[{"role":"vendor","sha256":"cd".repeat(32)},
                {"role":"production","sha256":"ef".repeat(32)}],
            "source_hashes":[{"path":path,"sha256":format!("{:x}",Sha256::digest(b"production-input"))}],
            "release_blockers":[]
        })
    };
    let mut document = json!({"schema_version":1,"command":"project verify vendor evidence index",
        "project":"test","complete_project_run":true,
        "entries":[make_entry("bluetooth.rs","ble-publish"),make_entry("wifi.rs","wifi-publish")]});
    let path = root.join("index.json");
    let write_index = |document: &serde_json::Value| {
        fs::write(&path, serde_json::to_vec(document).unwrap()).unwrap();
    };
    write_index(&document);
    let index = VendorEvidenceIndex::load(&path, "test").unwrap();
    assert_eq!(index.current_release_count(&root, true), 2);
    assert_eq!(index.current_release_count(&root, false), 0);
    let ble = VendorEvidenceRef {
        suite: "radio".into(),
        source: "archive".into(),
        symbol: "ble-publish".into(),
    };
    let wifi = VendorEvidenceRef {
        symbol: "wifi-publish".into(),
        ..ble.clone()
    };
    let absent = VendorEvidenceRef {
        symbol: "unverified-root".into(),
        ..ble.clone()
    };
    assert!(index.get(&absent).is_none());

    // A producer's release flag cannot override DIFF/INCOMPLETE, baseline
    // failure, supporting-only evidence, or unresolved comparison blockers.
    for (field, value) in [
        ("status", json!("diff")),
        ("status", json!("incomplete")),
        ("evidence_class", json!("shared-core")),
        ("baseline_passed", json!(false)),
        ("release_blockers", json!(["contract-not-closed"])),
    ] {
        let mut changed = document.clone();
        changed["entries"][0][field] = value;
        write_index(&changed);
        let index = VendorEvidenceIndex::load(&path, "test").unwrap();
        assert!(
            !index
                .get(&ble)
                .unwrap()
                .is_current_release_evidence(&root, true)
        );
        assert!(
            index
                .get(&wifi)
                .unwrap()
                .is_current_release_evidence(&root, true)
        );
    }
    fs::write(root.join("bluetooth.rs"), b"changed-production-input").unwrap();
    assert!(
        !index
            .get(&ble)
            .unwrap()
            .is_current_release_evidence(&root, true)
    );
    assert!(
        index
            .get(&wifi)
            .unwrap()
            .is_current_release_evidence(&root, true)
    );
    // This checks only the existing per-entry source binding. It does not
    // assert that this synthetic file list is a complete cross-image impact set.

    document["complete_project_run"] = json!(false);
    write_index(&document);
    assert!(VendorEvidenceIndex::load(&path, "test").is_err());
    document["complete_project_run"] = json!(true);
    let duplicate = document["entries"][0].clone();
    document["entries"].as_array_mut().unwrap().push(duplicate);
    write_index(&document);
    assert!(
        VendorEvidenceIndex::load(&path, "test")
            .unwrap_err()
            .to_string()
            .contains("repeats")
    );
}

// Called with independently sealed archived observations by the review tests.
pub(crate) fn assert_reviewed_hil(
    root: &Path,
    document: CapabilityDocument,
    index: &HilEvidenceIndex,
    catalog: &ScenarioCatalog,
) {
    let declarations = BTreeMap::from([(document.id.clone(), document.clone())]);
    let dispositions = DispositionIndex {
        entries: BTreeMap::new(),
        project_id: "test".into(),
        vendor_evidence_index: None,
    };
    let vendor = VendorEvidenceIndex {
        schema_version: 1,
        command: "project verify vendor evidence index".into(),
        project: "test".into(),
        complete_project_run: true,
        entries: vec![],
    };
    let context = EvaluationContext {
        root,
        dispositions: &dispositions,
        vendor_index: &vendor,
        scenario_catalog: catalog,
        hil_index: index,
        evaluator_clean: false,
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
