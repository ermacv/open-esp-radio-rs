use super::*;
use crate::model::{AsyncProof, Capability, HilProof, HostProof, ImplementationProof, VendorProof};

fn contract() -> SourceContract {
    SourceContract {
        id: "dma-path".to_owned(),
        composition: SourceComposition::Diagnostic,
        scope: "One diagnostic DMA composition".to_owned(),
        limits: "Peripheral reachability remains unqualified".to_owned(),
        source_paths: vec![PathBuf::from("Cargo.toml")],
    }
}

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn source_contracts_require_unique_ids_and_repository_files() {
    let valid = contract();
    assert!(validate(std::slice::from_ref(&valid), root()).is_ok());
    assert!(validate(&[valid.clone(), valid], root()).is_err());
    for paths in [
        vec![],
        vec!["Cargo.toml", "Cargo.toml"],
        vec!["../Cargo.toml"],
        vec!["/Cargo.toml"],
        vec!["src"],
        vec!["missing-source.rs"],
    ] {
        let mut invalid = contract();
        invalid.source_paths = paths.into_iter().map(PathBuf::from).collect();
        assert!(validate(&[invalid], root()).is_err());
    }
}

#[test]
fn source_contracts_require_scope_and_explicit_limits() {
    let mut missing_scope = contract();
    missing_scope.scope = " ".to_owned();
    assert!(validate(&[missing_scope], root()).is_err());
    let mut missing_limits = contract();
    missing_limits.limits.clear();
    assert!(validate(&[missing_limits], root()).is_err());
}

#[test]
fn composition_is_strict_and_preserved_in_json() {
    let input = r#"
id = "dma-path"
composition = "diagnostic"
scope = "One diagnostic DMA composition"
limits = "Peripheral reachability remains unqualified"
source-paths = ["Cargo.toml"]
"#;
    let parsed: SourceContract = toml_edit::de::from_str(input).unwrap();
    let json = serde_json::to_value(parsed).unwrap();
    assert_eq!(json["composition"], "diagnostic");
    assert_eq!(json["source-paths"][0], "Cargo.toml");
    assert!(
        toml_edit::de::from_str::<SourceContract>(&input.replace("diagnostic", "ready")).is_err()
    );
    assert!(
        toml_edit::de::from_str::<SourceContract>(&input.replace("limits =", "limts =")).is_err()
    );
}

#[test]
fn source_composition_cannot_promote_or_block_readiness() {
    let mut capability = Capability {
        id: "dma".to_owned(),
        title: "DMA".to_owned(),
        scope: "SRAM DMA".to_owned(),
        implementation: ImplementationProof::Complete,
        host: HostProof::Covered,
        vendor: VendorProof::NotApplicable,
        hil: HilProof::NotApplicable,
        async_proof: AsyncProof::Bounded,
        dependencies: Vec::new(),
        gaps: Vec::new(),
        evidence: Vec::new(),
        source_contracts: Vec::new(),
    };
    for composition in [
        SourceComposition::Production,
        SourceComposition::Diagnostic,
        SourceComposition::Unimplemented,
    ] {
        let mut source = contract();
        source.composition = composition;
        capability.source_contracts = vec![source];
        capability.implementation = ImplementationProof::Complete;
        assert!(capability.proof_ready());
        capability.implementation = ImplementationProof::Incomplete;
        assert!(!capability.proof_ready());
    }
}
