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
    assert!(validate_relative_path(Path::new("../driver/src/lib.rs")).is_err());
    assert!(validate_relative_path(Path::new("driver/src/lib.rs")).is_ok());
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
    };
    let capabilities = BTreeMap::from([
        ("a".to_owned(), capability("a", "b")),
        ("b".to_owned(), capability("b", "a")),
    ]);
    assert!(validate_dependencies(&capabilities).is_err());
}
