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
fn hardware_sources_are_bound_to_the_selected_project_and_suite() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let dispositions = DispositionIndex::load_project(
        &root.join("verification/vendor/projects/esp32s31/vendor-project.toml"),
    )
    .unwrap();
    let scenarios = ScenarioCatalog::load(&root, Path::new("hil/scenarios")).unwrap();
    let context = StaticContext {
        root: &root,
        dispositions: &dispositions,
        scenario_catalog: &scenarios,
    };
    let mut capability = toml_edit::de::from_str::<ManifestDocument>(COMPLETE)
        .unwrap()
        .capabilities
        .remove(0);
    capability.hil_requirements.clear();
    capability.hil_not_applicable = Some("hardware-comparison-fixture".into());
    capability.vendor_roots[0].source = "libpp".into();
    capability.vendor_roots[0].symbol = "hal_mac_txq_enable".into();
    capability.vendor_evidence[0] = VendorEvidenceRef {
        suite: "ordinary-tx-ownership".into(),
        source: "libpp".into(),
        symbol: "hal_mac_txq_enable".into(),
    };
    validate_capability_declaration(&capability, &context).unwrap();

    for (source, symbol, suite) in [
        (
            "unknown-source",
            "hal_mac_txq_enable",
            "ordinary-tx-ownership",
        ),
        ("libpp", "unknown_symbol", "ordinary-tx-ownership"),
        ("libpp", "hal_mac_txq_enable", "unknown-suite"),
        ("libpp", "hal_mac_txq_enable", "tx-protection-control"),
        ("archive", "hal_mac_txq_enable", "ordinary-tx-ownership"),
    ] {
        let mut invalid = capability.clone();
        invalid.vendor_roots[0].source = source.into();
        invalid.vendor_roots[0].symbol = symbol.into();
        invalid.vendor_evidence[0] = VendorEvidenceRef {
            suite: suite.into(),
            source: source.into(),
            symbol: symbol.into(),
        };
        assert!(validate_capability_declaration(&invalid, &context).is_err());
        invalid.vendor_evidence.clear();
        if source != "libpp" || symbol != "hal_mac_txq_enable" {
            assert!(validate_capability_declaration(&invalid, &context).is_err());
        }
    }
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
            "comparison":comparison_fixture(&root, "radio", symbol, symbol),
            "suite":"radio","source":"archive","symbol":symbol,
            "evidence_class":"production-trace","status":"match","release_eligible":true,
            "rust_component":symbol,"evidence_digest":"ab".repeat(32),"baseline_passed":true,
            "artifact_hashes":[{"role":"source:archive:artifact","sha256":"cd".repeat(32)},
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
    assert_eq!(
        index.current_release_count(&root, Path::new("comparison-project.toml")),
        2
    );
    assert_eq!(
        index.current_release_count(&root, Path::new("comparison-project.toml")),
        2
    );
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
                .is_current_release_evidence(&root, Path::new("comparison-project.toml"))
        );
        assert!(
            index
                .get(&wifi)
                .unwrap()
                .is_current_release_evidence(&root, Path::new("comparison-project.toml"))
        );
    }
    fs::write(root.join("bluetooth.rs"), b"changed-production-input").unwrap();
    assert!(
        !index
            .get(&ble)
            .unwrap()
            .is_current_release_evidence(&root, Path::new("comparison-project.toml"))
    );
    assert!(
        index
            .get(&wifi)
            .unwrap()
            .is_current_release_evidence(&root, Path::new("comparison-project.toml"))
    );
    // This checks only the existing per-entry source binding. It does not
    // assert that this synthetic file list is a complete cross-image impact set.

    document["complete_project_run"] = json!(false);
    write_index(&document);
    assert!(VendorEvidenceIndex::load(&path, "test").is_err());
    document["schema_version"] = json!(2);
    document["suite_states"] = json!({"radio":"complete","unrelated":"incomplete"});
    write_index(&document);
    let partial = VendorEvidenceIndex::load(&path, "test").unwrap();
    assert!(
        partial
            .get(&wifi)
            .unwrap()
            .is_current_release_evidence(&root, Path::new("comparison-project.toml"))
    );
    document["suite_states"]["radio"] = json!("incomplete");
    write_index(&document);
    assert!(VendorEvidenceIndex::load(&path, "test").is_err());
    document["suite_states"]["radio"] = json!("complete");
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
        project_manifest: PathBuf::from("comparison-project.toml"),
        entries: BTreeMap::new(),
        suite_entries: BTreeSet::new(),
        project_id: "test".into(),
        vendor_evidence_index: None,
    };
    let vendor = VendorEvidenceIndex {
        schema_version: 1,
        command: "project verify vendor evidence index".into(),
        project: "test".into(),
        complete_project_run: true,
        suite_states: BTreeMap::new(),
        entries: vec![],
    };
    let context = EvaluationContext {
        root,
        dispositions: &dispositions,
        vendor_index: &vendor,
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

pub(super) fn comparison_fixture(
    root: &Path,
    suite: &str,
    symbol: &str,
    component: &str,
) -> comparison::Binding {
    comparison::models::fixture(root);
    let project = root.join("comparison-project.toml");
    fs::write(
        &project,
        "id = 'test'\ntarget-spec = 'target.toml'\nchip-pack = 'chip.toml'\nverification-addon = 'comparison-addon.toml'\n",
    )
    .unwrap();
    let addon = root.join("comparison-addon.toml");
    let mut document = fs::read_to_string(&addon)
        .unwrap_or_else(|_| "model-inputs = 'model-inputs.json'\n".into());
    let marker = format!("id = '{suite}'");
    if !document.contains(&marker) {
        document.push_str(&format!("\n[[suites]]\n{marker}\nmodel-mechanisms = ['abi']\nprofiles = ['{suite}-profiles.toml']\ndispositions = ['{suite}-functions.toml']\nbaselines = ['{suite}-baselines.toml']\n[[suites.vendor]]\nsource = 'archive'\nall = true\nartifact-sha256 = '{}'\n", "cd".repeat(32)));
        fs::write(&addon, document).unwrap();
    }
    for (suffix, row) in [
        (
            "profiles",
            format!(
                "[[profiles]]\nvendor-source = 'archive'\nvendor-symbol = '{symbol}'\nname = '{symbol}'\ncompare-return = true\n"
            ),
        ),
        (
            "functions",
            format!(
                "[[functions]]\nsource = 'archive'\nsymbol = '{symbol}'\nrust-component = '{component}'\neffect-contract = 'exact'\n"
            ),
        ),
        (
            "baselines",
            format!(
                "[[evidence]]\nsource = 'archive'\nsymbol = '{symbol}'\ndigest = '{}'\n",
                "ab".repeat(32)
            ),
        ),
    ] {
        let path = root.join(format!("{suite}-{suffix}.toml"));
        let mut text = fs::read_to_string(&path).unwrap_or_default();
        if !text.contains(&format!("symbol = '{symbol}'")) {
            text.push_str(&row);
            fs::write(path, text).unwrap();
        }
    }
    let artifacts = [("source:archive:artifact".into(), "cd".repeat(32))]
        .into_iter()
        .collect();
    let current = comparison::current(
        root,
        Path::new("comparison-project.toml"),
        suite,
        "archive",
        symbol,
        &artifacts,
    )
    .unwrap();
    comparison::Binding {
        project_manifest: "comparison-project.toml".into(),
        sha256: current.sha256,
    }
}

#[test]
fn comparison_inputs_invalidate_only_their_own_evidence_and_ignore_annotations() {
    let root = std::env::temp_dir().join(format!("oer-comparison-inputs-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let first = comparison_fixture(&root, "first", "first", "driver::first");
    let second = comparison_fixture(&root, "second", "second", "driver::second");
    let artifacts = [("source:archive:artifact".into(), "cd".repeat(32))]
        .into_iter()
        .collect();
    let current = |suite: &str| {
        comparison::current(
            &root,
            Path::new("comparison-project.toml"),
            suite,
            "archive",
            suite,
            &artifacts,
        )
        .unwrap()
    };
    for (path, from, to) in [
        (
            "first-profiles.toml",
            "compare-return = true",
            "compare-return = false",
        ),
        (
            "first-functions.toml",
            "effect-contract = 'exact'",
            "effect-contract = 'bounded'",
        ),
        ("first-functions.toml", "driver::first", "driver::other"),
        ("first-baselines.toml", &"ab".repeat(32), &"ef".repeat(32)),
        (
            "comparison-addon.toml",
            &format!("artifact-sha256 = '{}'", "cd".repeat(32)),
            &format!("artifact-sha256 = '{}'", "ef".repeat(32)),
        ),
    ] {
        let path = root.join(path);
        let before = fs::read_to_string(&path).unwrap();
        fs::write(&path, before.replacen(from, to, 1)).unwrap();
        assert_ne!(current("first").sha256, first.sha256);
        assert_eq!(current("second").sha256, second.sha256);
        fs::write(path, before).unwrap();
    }
    let path = root.join("first-profiles.toml");
    let text = fs::read_to_string(&path).unwrap();
    fs::write(
        path,
        format!("{text}\ndescription = 'clarified text'\n# spelling fix\n"),
    )
    .unwrap();
    assert_eq!(current("first").sha256, first.sha256);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn public_artifact_bindings_cover_primary_companion_and_auxiliary_images() {
    let root = std::env::temp_dir().join(format!("oer-public-artifacts-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    comparison_fixture(&root, "radio", "entry", "driver::entry");
    let mut artifacts = BTreeMap::from([("source:archive:artifact".into(), "cd".repeat(32))]);
    let check = |artifacts: &BTreeMap<String, String>| {
        comparison::current(
            &root,
            Path::new("comparison-project.toml"),
            "radio",
            "archive",
            "entry",
            artifacts,
        )
        .unwrap()
        .public_artifacts_bound
    };
    assert!(check(&artifacts));
    artifacts.insert("source:archive:companion".into(), "ef".repeat(32));
    artifacts.insert("auxiliary:linked-image".into(), "12".repeat(32));
    assert!(!check(&artifacts));
    let addon = root.join("comparison-addon.toml");
    let before = fs::read_to_string(&addon).unwrap();
    fs::write(&addon, format!("{before}\n[suites.artifact-bindings]\n'source:archive:companion' = '{}'\n'auxiliary:linked-image' = '{}'\n", "ef".repeat(32), "12".repeat(32))).unwrap();
    assert!(check(&artifacts));
    artifacts.insert("auxiliary:linked-image".into(), "34".repeat(32));
    assert!(!check(&artifacts));
    let current = fs::read_to_string(&addon).unwrap();
    fs::write(
        &addon,
        current.replace(&format!("artifact-sha256 = '{}'", "cd".repeat(32)), ""),
    )
    .unwrap();
    artifacts.insert("auxiliary:linked-image".into(), "12".repeat(32));
    assert!(!check(&artifacts));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn suite_model_identity_tracks_used_mechanisms_and_target_without_report_or_ble_leakage() {
    let root = std::env::temp_dir().join(format!("oer-suite-model-scope-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    comparison_fixture(&root, "wifi", "wifi", "driver::wifi");
    comparison_fixture(&root, "ble", "ble", "driver::ble");
    fs::write(root.join("ble-model.rs"), "BLE A").unwrap();
    fs::write(root.join("model-inputs.json"), r#"{"schema":1,"mechanisms":{"abi":{"implementation":["model.rs"],"contracts":[]},"ble":{"implementation":["ble-model.rs"],"contracts":[]}}}"#).unwrap();
    let addon = fs::read_to_string(root.join("comparison-addon.toml"))
        .unwrap()
        .replace(
            "id = 'ble'\nmodel-mechanisms = ['abi']",
            "id = 'ble'\nmodel-mechanisms = ['abi', 'ble']",
        );
    fs::write(root.join("comparison-addon.toml"), addon).unwrap();
    let identity = |suite| {
        comparison::current(
            &root,
            Path::new("comparison-project.toml"),
            suite,
            "archive",
            suite,
            &BTreeMap::new(),
        )
        .unwrap()
        .sha256
    };
    let wifi = identity("wifi");
    let ble = identity("ble");
    fs::write(root.join("ble-model.rs"), "BLE B").unwrap();
    fs::write(root.join("report.rs"), "new renderer").unwrap();
    assert_eq!(wifi, identity("wifi"));
    assert_ne!(ble, identity("ble"));
    let target = fs::read_to_string(root.join("target.toml"))
        .unwrap()
        .replace(
            "riscv32imac-unknown-none-elf",
            "riscv32imafc-unknown-none-elf",
        );
    fs::write(root.join("target.toml"), target).unwrap();
    assert_ne!(wifi, identity("wifi"));
    fs::remove_dir_all(root).unwrap();
}
