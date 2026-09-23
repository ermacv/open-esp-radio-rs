use super::super::tests::snapshot;
use super::*;
use crate::application::{ProjectSessionOptions, register_inventory};
use open_radio_vendor_contracts::register_inventory::{
    CoverageGap, KnowledgeProperty, SourceState,
};
use register_inventory::{BitCoverage, InventoryField, InventoryRegister, RegisterEvidence};

fn unknown_inventory() -> RegisterInventory {
    let subject = RegisterSubject {
        chip: "fixture".to_owned(),
        address_space: "cpu".to_owned(),
        route: "mmio".to_owned(),
        bank: None,
        address: 0x1_0000_1000,
    };
    let evidence = "evidence:fixture".to_owned();
    let register = InventoryRegister {
        id: subject.id(),
        subject,
        names: KnowledgeProperty::Unknown,
        physical_width: KnowledgeProperty::Unknown,
        semantics: KnowledgeProperty::Unknown,
        access_widths: BTreeSet::from([8, 32]),
        functions: BTreeSet::new(),
        fields: BTreeMap::new(),
        evidence: BTreeSet::from([evidence.clone()]),
        coverage: BitCoverage::default(),
    };
    let mut inventory = RegisterInventory::default();
    inventory.registers.insert(register.id.clone(), register);
    inventory.evidence.insert(
        evidence.clone(),
        RegisterEvidence {
            id: evidence,
            kind: "opaque-observation".to_owned(),
            identity: serde_json::json!("fixture"),
            sources: BTreeSet::from(["source:fixture".to_owned()]),
            payload: serde_json::json!({"uninterpreted": [255, 128, 66]}),
        },
    );
    inventory
}

#[test]
fn unknown_subjects_alternative_fields_and_evidence_survive_revision() {
    let mut inventory = unknown_inventory();
    let id = inventory.registers.keys().next().unwrap().clone();
    let register = inventory.registers.get_mut(&id).unwrap();
    for width in [1, 3] {
        let field_id = format!("{id}/field/0/{width}");
        register.fields.insert(
            field_id.clone(),
            InventoryField {
                id: field_id,
                offset: 0,
                width,
                mask: Some((1 << width) - 1),
                names: KnowledgeProperty::Unknown,
                semantics: KnowledgeProperty::Unknown,
                kind: "opaque".to_owned(),
                evidence: register.evidence.clone(),
            },
        );
    }
    let captured = RevisionRegisters::capture(inventory.clone()).unwrap();
    assert_eq!(captured.inventory.registers, inventory.registers);
    assert_eq!(
        captured.inventory.registers.len(),
        1,
        "access widths do not split subjects"
    );
    let json = serde_json::to_vec(&captured).unwrap();
    let decoded: RevisionRegisters = serde_json::from_slice(&json).unwrap();
    decoded.validate().unwrap();
    assert_eq!(decoded, captured);
    assert!(!String::from_utf8(json).unwrap().contains("uninterpreted"));
    let mut forged = captured.clone();
    forged
        .inventory
        .registers
        .get_mut(&id)
        .unwrap()
        .fields
        .values_mut()
        .next()
        .unwrap()
        .id = "other".to_owned();
    assert!(
        forged
            .validate()
            .unwrap_err()
            .to_string()
            .contains("field identity mismatch")
    );

    let before = snapshot("empty", Vec::new());
    let mut after = snapshot("unknown", Vec::new());
    after.registers = captured;
    let change = diff(&before, &after)
        .changes
        .into_iter()
        .find(|c| c.domain == "register")
        .unwrap();
    assert_eq!(change.classification, RevisionChangeClass::Added);
    assert_eq!(change.after.as_slice(), std::slice::from_ref(&id));

    let before = after.clone();
    inventory
        .registers
        .get_mut(&id)
        .unwrap()
        .fields
        .values_mut()
        .next()
        .unwrap()
        .names
        .insert("candidate".to_owned(), "evidence:fixture".to_owned());
    after.registers = RevisionRegisters::capture(inventory.clone()).unwrap();
    assert!(
        diff(&before, &after)
            .changes
            .iter()
            .any(|c| c.domain == "register"
                && c.classification == RevisionChangeClass::Modified
                && c.before == [id.clone()])
    );

    // Even a payload change behind the same evidence reference is visible.
    let before = after.clone();
    inventory.evidence.values_mut().next().unwrap().payload =
        serde_json::json!({"uninterpreted": [0]});
    after.registers = RevisionRegisters::capture(inventory).unwrap();
    assert!(
        diff(&before, &after)
            .changes
            .iter()
            .any(|c| c.domain == "register" && c.classification == RevisionChangeClass::Modified)
    );
}

#[test]
fn gap_only_inventory_changes_are_visible_without_any_registers() {
    let before = snapshot("before", Vec::new());
    let mut inventory = RegisterInventory::default();
    inventory.gaps.insert(CoverageGap {
        source: "input".to_owned(),
        scope: "indexed-domain".to_owned(),
        reason: "upper bound unknown".to_owned(),
    });
    let mut after = snapshot("after", Vec::new());
    after.registers = RevisionRegisters::capture(inventory).unwrap();
    let report = diff(&before, &after);
    assert!(
        report
            .changes
            .iter()
            .any(|c| c.domain == "register-coverage"
                && c.classification == RevisionChangeClass::Added)
    );
    assert!(
        report
            .invalidated_research
            .iter()
            .any(|area| area.area == "register-model")
    );
}

#[test]
fn captured_mmio_artifacts_remain_authenticated_against_live_inputs() {
    let mut inventory = unknown_inventory();
    let digest = "a".repeat(64);
    let id = "run".to_owned();
    inventory.evidence.insert(
        id.clone(),
        RegisterEvidence {
            id: id.clone(),
            kind: "analysis-run".to_owned(),
            identity: serde_json::json!("run"),
            sources: BTreeSet::new(),
            payload: serde_json::json!({"artifacts": [{
                "source": "vendor", "artifact": {"path": "vendor.o", "sha256": digest},
                "functions": 1, "reviewed_boundaries": 0, "functions_with_mmio": 1,
                "functions_with_diagnostics": 0, "explored_states": 1, "terminal_paths": 1,
                "branch_sites": 0
            }]}),
        },
    );
    let live = BTreeSet::from([RevisionArtifact {
        role: Some("source-artifact:vendor".to_owned()),
        source: "vendor".to_owned(),
        sha256: digest,
    }]);
    let sources = BTreeSet::from(["vendor".to_owned()]);
    validate_register_artifacts(&inventory, &live, &sources).unwrap();
    inventory.evidence.get_mut(&id).unwrap().payload["artifacts"][0]["artifact"]["sha256"] =
        serde_json::json!("b".repeat(64));
    assert!(
        validate_register_artifacts(&inventory, &live, &sources)
            .unwrap_err()
            .to_string()
            .contains("stale vendor artifact")
    );
    inventory.evidence.get_mut(&id).unwrap().payload["artifacts"][0]["source"] =
        serde_json::json!("unbound");
    assert!(
        validate_register_artifacts(&inventory, &live, &sources)
            .unwrap_err()
            .to_string()
            .contains("non-vendor source")
    );
}

#[test]
fn differing_address_domains_do_not_collapse_and_unknown_geometry_cannot_authorize_rebase() {
    let mut inventory = unknown_inventory();
    let mut alias = inventory.registers.values().next().unwrap().clone();
    alias.subject.address_space = "other".to_owned();
    alias.id = alias.subject.id();
    inventory.registers.insert(alias.id.clone(), alias);
    let captured = RevisionRegisters::capture(inventory.clone()).unwrap();
    assert_eq!(captured.entities().count(), 2);
    let semantic = SemanticEntityId::register("fixture", "cpu", 0x1_0000_1000, 32).unwrap();
    assert!(!captured.supports_semantic(&semantic));
    let id = rebase_subject_base(&semantic);
    let width = &mut inventory.registers.get_mut(&id).unwrap().physical_width;
    width.insert(32, "evidence:fixture".to_owned());
    assert!(
        RevisionRegisters::capture(inventory.clone())
            .unwrap()
            .supports_semantic(&semantic)
    );
    inventory
        .registers
        .get_mut(&id)
        .unwrap()
        .physical_width
        .insert(16, "evidence:fixture".to_owned());
    assert!(
        !RevisionRegisters::capture(inventory)
            .unwrap()
            .supports_semantic(&semantic)
    );
}

#[test]
fn revision_uses_selected_svd_and_retains_failed_source_state() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let target =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/generic-project/target.toml");
    let manifest = root.join("project.toml");
    fs::write(&manifest, format!("schema = 4\nid = \"revision-registers\"\ntarget-spec = {:?}\nchip-pack = \"chip.toml\"\n", target.display().to_string())).unwrap();
    fs::write(
        root.join("chip.toml"),
        "schema = 3\nid = \"fixture\"\nsvd = [\"base.svd\"]\nknowledge-packs = []\n",
    )
    .unwrap();
    let svd = |address| {
        format!(
            r#"<device schemaVersion="1.3"><name>Test</name><version>1</version><description>fixture</description><addressUnitBits>8</addressUnitBits><width>32</width><peripherals><peripheral><name>DEV</name><baseAddress>{address}</baseAddress><registers><register><name>CONTROL</name><description>unknown behavior</description><addressOffset>0</addressOffset><size>32</size></register></registers></peripheral></peripherals></device>"#
        )
    };
    fs::write(root.join("base.svd"), svd(0x1000)).unwrap();
    fs::write(root.join("selected.svd"), svd(0x2000)).unwrap();
    let session = ProjectSession::open_with(
        &manifest,
        ProjectSessionOptions {
            svd_paths: vec![root.join("selected.svd")],
            load_register_catalog: false,
            ..Default::default()
        },
    )
    .unwrap();
    let inventory = register_inventory::load(&session).unwrap();
    let revision = snapshot_registers(&session, &BTreeSet::new(), &BTreeSet::new()).unwrap();
    assert_eq!(revision.inventory.registers, inventory.registers);
    assert_eq!(revision.inventory.sources, inventory.sources);
    assert_eq!(revision.inventory.regions, inventory.regions);
    assert_eq!(revision.inventory.gaps, inventory.gaps);
    assert_eq!(
        revision
            .inventory
            .registers
            .values()
            .next()
            .unwrap()
            .subject
            .address,
        0x2000
    );
    assert!(revision.inventory.evidence.values().all(|reference| {
        let evidence = &inventory.evidence[&reference.id];
        reference.sha256
            == format!(
                "{:x}",
                Sha256::digest(serde_json::to_vec(evidence).unwrap())
            )
    }));

    fs::remove_file(root.join("selected.svd")).unwrap();
    let retained = snapshot_registers(&session, &BTreeSet::new(), &BTreeSet::new()).unwrap();
    assert_eq!(
        retained, revision,
        "revision must use the same captured graph after input removal"
    );
    fs::write(root.join("base.svd"), [255, 128, 66]).unwrap();
    let base = ProjectSession::open_with(
        &manifest,
        ProjectSessionOptions {
            load_register_catalog: false,
            ..Default::default()
        },
    )
    .unwrap();
    let failed = snapshot_registers(&base, &BTreeSet::new(), &BTreeSet::new()).unwrap();
    assert!(
        failed
            .inventory
            .sources
            .iter()
            .any(|source| matches!(source.state, SourceState::Failed { .. }))
    );
    assert!(
        failed
            .inventory
            .evidence
            .values()
            .any(|e| e.kind == "opaque-source")
    );
    assert!(failed.context_fingerprint.is_some());
    let mut serialized = serde_json::to_value(failed).unwrap();
    serialized["inventory"]["unsupported"] = serde_json::json!(true);
    assert!(serde_json::from_value::<RevisionRegisters>(serialized).is_err());
}
