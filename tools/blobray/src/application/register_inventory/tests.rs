use super::*;

fn query_project(root: &Path) -> std::path::PathBuf {
    let target =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/generic-project/target.toml");
    let manifest = root.join("project.toml");
    std::fs::write(
        &manifest,
        format!(
            "schema = 4\nid = \"query-fixture\"\ntarget-spec = {:?}\nchip-pack = \"chip.toml\"\n",
            target.display().to_string()
        ),
    )
    .unwrap();
    std::fs::write(
        root.join("chip.toml"),
        "schema = 3\nid = \"fixture\"\nsvd = [\"base.svd\"]\nknowledge-packs = []\n",
    )
    .unwrap();
    manifest
}

fn wide_svd(address: u64) -> String {
    format!(
        r#"<device schemaVersion="1.3"><name>Test</name><version>1</version><description>fixture</description><addressUnitBits>8</addressUnitBits><width>32</width><peripherals><peripheral><name>DEV</name><baseAddress>{address}</baseAddress><registers><register><name>WIDE</name><addressOffset>0</addressOffset><size>512</size><fields><field><name>HIGH</name><bitOffset>300</bitOffset><bitWidth>64</bitWidth></field></fields></register></registers></peripheral></peripherals></device>"#
    )
}

#[test]
fn public_application_preserves_wide_geometry_without_preparing_backend_models() {
    use crate::{AnalyzeRequest, BlobrayApplication};
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let manifest = query_project(root);
    let address = 0x1_0000_1000;
    std::fs::write(
        root.join("chip.toml"),
        r#"schema = 3
id = "fixture"
svd = ["base.svd"]
knowledge-packs = []
memory-map = "memory.toml"
"#,
    )
    .unwrap();
    std::fs::write(
        root.join("memory.toml"),
        r#"schema = 1
default-address-space = "cpu"
[[address-spaces]]
id = "cpu"
address-width = 64
endianness = "little"
[[regions]]
name = "wide-peripheral"
address-space = "cpu"
kind = "mmio"
start = 0x100001000
end-exclusive = 0x100002000
permissions = "rw"
"#,
    )
    .unwrap();
    std::fs::write(root.join("base.svd"), wide_svd(address)).unwrap();
    let mut app = BlobrayApplication::open(&manifest).unwrap();
    assert!(app.analysis_session.is_none());
    assert!(app.resolved.memory_map.is_some());
    assert!(app.resolved.mmio.regions.is_empty());
    let inventory = app.register_inventory().unwrap().inventory().clone();
    let register = inventory.at_address(address)[0];
    let selector = RegisterSelector::Subject(register.id.clone());
    let detail = app.register_detail(&selector).unwrap().unwrap();
    assert_eq!(detail.subjects, vec![register.clone()]);
    assert_eq!(detail.width, Some(512));
    assert_eq!(detail.address, address);
    let high = detail
        .fields
        .iter()
        .find(|field| field.field.offset == 300)
        .unwrap();
    assert_eq!(high.field.width, 64);
    assert_eq!(high.field.mask, None);
    assert!(high.field.semantics.is_unknown());
    assert_eq!(detail.fields.len(), register.fields.len());
    let snapshot = app.snapshot().unwrap();
    assert_eq!(snapshot.registers.fields, register.fields.len());
    assert_eq!(snapshot.registers.ranges, inventory.regions.len());
    assert!(snapshot.registers.publication.is_none());
    assert_eq!(
        snapshot.registers.registers().cloned().collect::<Vec<_>>(),
        inventory.registers.values().cloned().collect::<Vec<_>>()
    );
    assert_eq!(
        app.register_detail(&RegisterSelector::Address(address + 40))
            .unwrap()
            .unwrap()
            .subjects,
        detail.subjects
    );
    assert!(
        app.analysis_session.is_none(),
        "queries cannot prepare backend models"
    );
    let error = app
        .analyze(AnalyzeRequest {
            artifact: root.join("absent.o"),
            member: None,
            symbol: "probe".into(),
        })
        .unwrap_err()
        .to_string();
    assert!(error.contains("32-bit analysis backend"), "{error}");
    assert!(
        app.analysis_session.is_none(),
        "failed preparation cannot install a fallback"
    );
    assert_eq!(app.register_detail(&selector).unwrap().unwrap(), detail);
    assert_eq!(
        app.reload()
            .unwrap()
            .registers
            .registers()
            .cloned()
            .collect::<Vec<_>>(),
        vec![register.clone()]
    );
}

#[test]
fn selectors_keep_domains_and_property_evidence_without_address_merging() {
    let directory = tempfile::tempdir().unwrap();
    let project = crate::ProjectSpec::load(&query_project(directory.path())).unwrap();
    let mut inventory = modeled();
    let original = inventory.registers.values().next().unwrap().clone();
    for (space, route, bank) in [("debug", "mmio", None), ("cpu", "indexed", Some("bank1"))] {
        let mut sibling = original.clone();
        sibling.subject.address_space = space.into();
        sibling.subject.route = route.into();
        sibling.subject.bank = bank.map(str::to_owned);
        sibling.id = sibling.subject.id();
        sibling.fields.clear();
        sibling.names = KnowledgeProperty::Unknown;
        inventory.registers.insert(sibling.id.clone(), sibling);
    }
    // Property claims can own evidence that is not repeated on the register.
    let evidence_id = inventory.record(
        "hint",
        "hint",
        &json!("high-field"),
        json!({"note":"specific hint"}),
    );
    let register = inventory.registers.get_mut(&original.id).unwrap();
    let field = register.fields.values_mut().next().unwrap();
    field
        .semantics
        .insert("opaque operation".into(), evidence_id.clone());
    let expected_field = field.clone();
    for query in [
        InventoryQuery {
            source: Some("hint".into()),
            ..Default::default()
        },
        InventoryQuery {
            text: Some("specific hint".into()),
            ..Default::default()
        },
    ] {
        assert_eq!(
            inventory
                .select(&query)
                .iter()
                .map(|r| &r.id)
                .collect::<Vec<_>>(),
            vec![&original.id]
        );
    }
    let exact = RegisterSelector::Subject(original.id.clone());
    assert_eq!(
        inventory
            .resolve_selector(&RegisterSelector::Address(0x1000))
            .len(),
        3
    );
    assert_eq!(inventory.resolve_selector(&exact).len(), 1);
    let detail =
        crate::application::register_detail_from_inventory(&project, inventory.clone(), &exact)
            .unwrap()
            .unwrap();
    assert_eq!(
        detail
            .fields
            .iter()
            .find(|f| f.field.id == expected_field.id)
            .unwrap()
            .field,
        expected_field
    );
    assert!(detail.evidence.iter().any(|e| e.id == evidence_id));
    assert_eq!(
        detail.semantic_subject().unwrap().unwrap().to_string(),
        "register:chip/cpu/0x1000/32"
    );
    let all = crate::application::register_detail_from_inventory(
        &project,
        inventory.clone(),
        &RegisterSelector::Address(0x1000),
    )
    .unwrap()
    .unwrap();
    assert_eq!(all.subjects.len(), 3);
    assert_eq!(all.width, None);
    assert_eq!(all.semantic_subject().unwrap(), None);
    let bank = inventory
        .registers
        .values()
        .find(|r| r.subject.bank.is_some())
        .unwrap();
    let detail = crate::application::register_detail_from_inventory(
        &project,
        inventory.clone(),
        &RegisterSelector::Subject(bank.id.clone()),
    )
    .unwrap()
    .unwrap();
    assert!(
        detail
            .semantic_subject()
            .unwrap_err()
            .to_string()
            .contains("route or bank")
    );
    assert_eq!(detail.subjects, vec![bank.clone()]);
    assert!(detail.fields.is_empty());
    assert!(detail.regions.is_empty());
    assert_eq!(detail.publication_debt, None);
    assert_eq!(
        "0x100000001".parse::<RegisterSelector>().unwrap(),
        RegisterSelector::Address(0x1_0000_0001)
    );
    assert_eq!(original.id.parse::<RegisterSelector>().unwrap(), exact);
    assert!("0x10000000000000000".parse::<RegisterSelector>().is_err());
}

#[test]
fn resolved_svd_selection_reaches_inventory_snapshot_and_detail() {
    use crate::application::{BlobrayApplication, ProjectSession, ProjectSessionOptions};
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let target =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/generic-project/target.toml");
    let manifest = root.join("project.toml");
    std::fs::write(
        &manifest,
        format!(
            "schema = 4\nid = \"svd-context\"\ntarget-spec = {:?}\nchip-pack = \"chip.toml\"\n",
            target.display().to_string()
        ),
    )
    .unwrap();
    std::fs::write(
        root.join("chip.toml"),
        "schema = 3\nid = \"fixture\"\nsvd = [\"base.svd\"]\nknowledge-packs = []\n",
    )
    .unwrap();
    let svd = |address: u32| {
        format!(
            r#"<device schemaVersion="1.3"><name>Test</name><version>1</version><description>fixture</description><addressUnitBits>8</addressUnitBits><width>32</width><peripherals><peripheral><name>DEV</name><baseAddress>{address}</baseAddress><registers><register><name>CONTROL</name><description>unknown behavior</description><addressOffset>0</addressOffset><size>32</size></register></registers></peripheral></peripherals></device>"#
        )
    };
    std::fs::write(root.join("base.svd"), svd(0x1000)).unwrap();
    let selected = root.join("selected.svd");
    std::fs::write(&selected, svd(0x2000)).unwrap();
    let base = ProjectSession::open_with(&manifest, Default::default()).unwrap();
    assert!(!load(&base).unwrap().at_address(0x1000).is_empty());
    let session = ProjectSession::open_with(
        &manifest,
        ProjectSessionOptions {
            svd_paths: vec![selected.clone()],
            // Queries must work without the lossy name catalog used by binary analysis.
            load_register_catalog: false,
            ..Default::default()
        },
    )
    .unwrap();
    let mut app = BlobrayApplication {
        resolved: session,
        generation: 1,
        analysis_session: None,
        analysis_cache: BTreeMap::new(),
    };
    let inventory = app.register_inventory().unwrap().inventory().clone();
    assert!(inventory.at_address(0x1000).is_empty());
    assert_eq!(inventory.at_address(0x2000).len(), 1);
    assert_eq!(
        inventory,
        app.register_inventory().unwrap().inventory().clone()
    );
    let writer =
        crate::application::query_store::QueryStore::open_analysis_epoch(&manifest).unwrap();
    assert_eq!(
        inventory,
        app.register_inventory().unwrap().inventory().clone(),
        "retained query must remain readable while another analysis owns the writer"
    );
    drop(writer);
    assert_eq!(
        app.snapshot()
            .unwrap()
            .registers
            .register_at(0)
            .unwrap()
            .subject
            .address,
        0x2000
    );
    assert!(
        app.register_detail(&RegisterSelector::Address(0x1000))
            .unwrap()
            .is_none()
    );
    let detail = app
        .register_detail(&RegisterSelector::Address(0x2000))
        .unwrap()
        .unwrap();
    assert_eq!(
        app.analysis_session().unwrap().svd_paths,
        vec![selected.clone()]
    );
    assert!(
        app.analysis_session()
            .unwrap()
            .mmio
            .registers
            .iter()
            .any(|r| r.address == 0x2000)
    );
    let manifest_bytes = std::fs::read(&manifest).unwrap();
    std::fs::write(&manifest, "invalid manifest").unwrap();
    assert!(app.reload().is_err());
    assert_eq!(app.generation, 1);
    assert!(
        app.analysis_session.is_some(),
        "failed reload preserves the active generation"
    );
    std::fs::write(&manifest, manifest_bytes).unwrap();
    assert_eq!(detail.sources, inventory.sources);
    assert!(
        inventory
            .sources
            .iter()
            .all(|source| source.path != root.join("base.svd").display().to_string())
    );

    std::fs::write(&selected, svd(0x3000)).unwrap();
    // Prepared models belong to the old generation until explicit reload.
    assert!(
        app.analysis_session()
            .unwrap()
            .mmio
            .registers
            .iter()
            .any(|r| r.address == 0x2000)
    );
    assert_eq!(app.reload().unwrap().generation, 2);
    assert!(app.analysis_session.is_none());
    assert_eq!(
        app.analysis_session().unwrap().svd_paths,
        vec![selected.clone()]
    );
    assert!(
        app.analysis_session()
            .unwrap()
            .mmio
            .registers
            .iter()
            .any(|r| r.address == 0x3000)
    );
    let changed = app.register_inventory().unwrap().inventory().clone();
    assert!(changed.at_address(0x2000).is_empty());
    assert_eq!(changed.at_address(0x3000).len(), 1);
    assert_eq!(load(&base).unwrap().at_address(0x1000).len(), 1);

    std::fs::remove_file(&selected).unwrap();
    assert_eq!(app.register_inventory().unwrap().inventory(), &changed);
    assert!(matches!(
        app.reload().unwrap().registers.inventory,
        crate::RegisterInventoryState::Failed { .. }
    ));
    assert!(
        app.register_inventory()
            .unwrap_err()
            .to_string()
            .contains("explicitly selected SVD")
    );
    std::fs::write(&selected, "invalid XML").unwrap();
    assert!(matches!(
        app.reload().unwrap().registers.inventory,
        crate::RegisterInventoryState::Failed { .. }
    ));
    assert!(
        app.register_inventory()
            .unwrap_err()
            .to_string()
            .contains("invalid explicitly selected SVD")
    );

    // Unavailable declared defaults retain distinct missing/failed states;
    // invalid UTF-8 remains recoverable as opaque source bytes.
    std::fs::remove_file(root.join("base.svd")).unwrap();
    let missing = load(&base).unwrap();
    assert!(
        missing
            .sources
            .iter()
            .any(|source| source.kind == "svd" && source.state == SourceState::Missing)
    );
    let opaque = [0xff, 0x80, 0x42];
    std::fs::write(root.join("base.svd"), opaque).unwrap();
    let failed = load(&base).unwrap();
    assert!(
        failed.sources.iter().any(
            |source| source.kind == "svd" && matches!(source.state, SourceState::Failed { .. })
        )
    );
    assert!(
        failed
            .evidence
            .values()
            .any(|evidence| evidence.kind == "opaque-source"
                && evidence.payload["bytes"] == json!(opaque))
    );
}

fn geometry(width: u32) -> Vec<open_esp_radio_register_model::RegisterGeometry> {
    open_esp_radio_register_model::svd_geometry(&format!(r#"<device schemaVersion="1.3"><name>Test</name><version>1</version><description>fixture</description><addressUnitBits>8</addressUnitBits><width>32</width><peripherals><peripheral><name>DEV</name><baseAddress>4096</baseAddress><registers><register><name>CONTROL</name><description>opaque behavior</description><addressOffset>0</addressOffset><size>{width}</size><fields><field><name>FLAG</name><description>unknown behavior</description><bitOffset>0</bitOffset><bitWidth>1</bitWidth></field></fields></register></registers></peripheral></peripherals></device>"#)).unwrap()
}

fn modeled() -> RegisterInventory {
    let mut inventory = RegisterInventory::default();
    inventory.import_geometry(
        "chip",
        "cpu",
        "model",
        "model-geometry",
        &json!("model-revision"),
        geometry(32),
    );
    inventory
}

fn facts(registers: Vec<Value>) -> Value {
    json!({"schema_version":6,"command":"mmio discover","analysis_mode":"best-effort","access_count_mode":"maximum-per-path","completeness_claim":false,"code_selection":{"symbols":"all","symbol_prefix":""},"ranges":[{"name":"dev","start":"0x1000","end_exclusive":"0x1010"}],"artifacts":[],"registers":registers,"diagnostics":[],"observations":[]})
}

fn access(address: u32, width: u8, function: &str, modified: u32, inverted: u32) -> Value {
    json!({"address":format!("{address:#x}"),"width":width,"name":"UNMAPPED","reads":1,"writes":1,"read_functions":[function],"write_functions":[function],"read_sites":[{"function":function,"pc":"0x20"}],"write_sites":[{"function":function,"pc":"0x24"}],"write_patterns":[{"occurrences":1,"modified_mask":format!("{modified:#x}"),"candidate_bit_ranges":"","preserved_mask":"0x0","inverted_mask":format!("{inverted:#x}"),"forced_zero_mask":"0x0","forced_one_mask":"0x0","read_derived_mask":"0x0","dynamic_mask":"0x0","functions":[function]}]})
}

fn import(inventory: &mut RegisterInventory, document: Value) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("facts.json");
    std::fs::write(&path, serde_json::to_vec(&document).unwrap()).unwrap();
    inventory
        .import_facts("chip", "cpu", &path, &std::fs::read(&path).unwrap())
        .unwrap();
}

#[test]
fn mixed_width_lanes_share_physical_identity_without_losing_sites() {
    let mut inventory = modeled();
    import(
        &mut inventory,
        facts(vec![
            access(0x1000, 32, "word", 3, 0),
            access(0x1002, 16, "half", 5, 0),
            access(0x1003, 8, "byte", 1, 0),
        ]),
    );
    inventory.finish();
    assert_eq!(inventory.registers.len(), 1);
    let register = inventory.at_address(0x1003)[0];
    assert_eq!(register.width(), Some(32));
    assert_eq!(register.access_widths, BTreeSet::from([8, 16, 32]));
    assert_eq!(register.functions.len(), 3);
    assert_eq!(register.coverage.modified, 0x0105_0003);
    assert_eq!(
        register
            .evidence
            .iter()
            .filter(|id| inventory.evidence[*id].kind == "discovery-access")
            .count(),
        3
    );
}

#[test]
fn model_only_unknown_semantics_and_unknown_complement_are_queryable() {
    let mut inventory = modeled();
    inventory.finish();
    let register = inventory.at_address(0x1000)[0];
    assert!(register.semantics.is_unknown());
    assert_eq!(register.coverage.unobserved, Some(u32::MAX));
    assert!(
        register
            .fields
            .values()
            .any(|field| field.kind == "unknown" && field.mask == Some(0xffff_fffe))
    );
    assert_eq!(
        inventory
            .select(&InventoryQuery {
                unknown: true,
                ..Default::default()
            })
            .len(),
        1
    );
    for field in register.fields.values() {
        assert!(
            field
                .evidence
                .iter()
                .all(|id| inventory.evidence.contains_key(id))
        );
    }
}

#[test]
fn overlapping_and_noncontiguous_masks_keep_alternatives_and_full_word() {
    let mut inventory = modeled();
    import(
        &mut inventory,
        facts(vec![access(0x1000, 32, "word", u32::MAX, u32::MAX)]),
    );
    let id = inventory.at_address(0x1000)[0].id.clone();
    let evidence = inventory.record(
        "ir",
        "bit-use",
        &json!("revision"),
        json!({"masks":[3,12,15,5]}),
    );
    for bits in [3, 12, 15, 5] {
        RegisterInventory::bit_use(
            inventory.registers.get_mut(&id).unwrap(),
            bits,
            "candidate",
            &evidence,
        );
    }
    inventory.finish();
    let register = &inventory.registers[&id];
    let masks = register
        .fields
        .values()
        .filter_map(|field| field.mask)
        .collect::<BTreeSet<_>>();
    assert!(BTreeSet::from([1, 4, 3, 12, 15, u32::MAX]).is_subset(&masks));
    assert!(!masks.contains(&7));
    assert!(
        register
            .fields
            .values()
            .any(|field| field.kind == "opaque" && field.mask == Some(u32::MAX))
    );
}

#[test]
fn conflicting_geometry_keeps_all_claims_and_disables_false_complement() {
    let mut inventory = modeled();
    inventory.import_geometry(
        "chip",
        "cpu",
        "svd",
        "svd-geometry",
        &json!("svd-revision"),
        geometry(16),
    );
    inventory.finish();
    let register = inventory.at_address(0x1000)[0];
    assert!(matches!(
        register.physical_width,
        KnowledgeProperty::Conflicted { .. }
    ));
    assert_eq!(register.coverage.unobserved, None);
    assert_eq!(
        inventory
            .select(&InventoryQuery {
                conflicted: true,
                ..Default::default()
            })
            .len(),
        1
    );
}

#[test]
fn source_order_round_trip_and_repeated_import_preserve_evidence() {
    let mut a = modeled();
    let mut b = modeled();
    let mut left = geometry(32);
    left[0].name = "FIRST".to_owned();
    let mut right = geometry(32);
    right[0].name = "SECOND".to_owned();
    for (inventory, reverse) in [(&mut a, false), (&mut b, true)] {
        for (source, geometry) in if reverse {
            vec![("b", right.clone()), ("a", left.clone())]
        } else {
            vec![("a", left.clone()), ("b", right.clone())]
        } {
            inventory.import_geometry("chip", "cpu", source, "geometry", &json!(source), geometry);
        }
        inventory.finish();
    }
    a.import_geometry("chip", "cpu", "a", "geometry", &json!("a"), left);
    a.finish();
    assert_eq!(a, b);
    assert_eq!(
        a,
        serde_json::from_slice(&serde_json::to_vec(&a).unwrap()).unwrap()
    );
}

#[test]
fn discovery_diagnostics_and_domains_remain_accessible_without_named_catalog() {
    let mut inventory = RegisterInventory::default();
    let mut document = facts(vec![]);
    document["diagnostics"] =
        json!([{"function":"loop","scope":"budget","message":"frontier at 0x40"}]);
    document["observations"] = json!([{"kind":"indexed-mmio","function":"loop","site":64,"width":32,"address_expression":"0x1000 + (arg0 & 63) * 4","addresses":(0..64).map(|i|0x1000+i*4).collect::<Vec<_>>()}]);
    import(&mut inventory, document);
    inventory.finish();
    assert_eq!(inventory.registers.len(), 64);
    assert_eq!(inventory.address_domains.len(), 1);
    assert!(
        inventory
            .gaps
            .iter()
            .any(|gap| gap.reason.contains("frontier"))
    );
    assert!(
        inventory
            .registers
            .values()
            .all(|register| register.names.is_unknown() && register.width().is_none())
    );
}

#[test]
fn trace_and_hint_only_subjects_preserve_banks_sequence_and_payload() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("observations.json");
    let subject = RegisterSubject {
        chip: "chip".into(),
        address_space: "pbus".into(),
        route: "indirect".into(),
        bank: Some("1".into()),
        address: 7,
    };
    let first = json!({"id":"trace:1","subject":subject,"kind":"trace-read","access_width":8,"payload":{"sequence":1,"device":"revision-a","value":5}});
    let mut second = first.clone();
    second["id"] = json!("hint:1");
    second["subject"]["bank"] = json!("2");
    second["kind"] = json!("hint");
    second["payload"] = json!({"format":"flags=%x","argument":2,"confidence":"heuristic"});
    let document = json!({"schema_version":1,"artifact":"capture-sha256","applicability":{"chip-revision":"a"},"observations":[first,second],"gaps":[]});
    std::fs::write(&path, serde_json::to_vec(&document).unwrap()).unwrap();
    let mut inventory = RegisterInventory::default();
    inventory
        .import_observations(&path, false, "chip", "cpu")
        .unwrap();
    inventory.finish();
    assert_eq!(inventory.registers.len(), 2);
    assert_eq!(
        inventory
            .select(&InventoryQuery {
                text: Some("flags=%x".into()),
                ..Default::default()
            })
            .len(),
        1
    );
    assert!(inventory.evidence.values().any(|evidence| {
        evidence.identity.to_string().contains("revision-a")
            || evidence.identity.to_string().contains("chip-revision")
    }));
    assert!(
        inventory
            .registers
            .values()
            .all(|register| register.names.is_unknown())
    );
}

#[test]
fn old_discovery_schema_is_rejected_without_compatibility() {
    let mut document = facts(vec![]);
    document["schema_version"] = json!(5);
    assert!(
        crate::artifacts::parse_mmio_facts(&document.to_string())
            .unwrap_err()
            .to_string()
            .contains("expected schema_version 6")
    );
}

#[test]
fn derived_unknown_evidence_has_no_dangling_links_and_is_idempotent() {
    let mut inventory = modeled();
    inventory.finish();
    inventory.validate().unwrap();
    let first = inventory.clone();
    inventory.finish();
    inventory.validate().unwrap();
    assert_eq!(inventory, first);
    let register = inventory.at_address(0x1000)[0];
    let field = register
        .fields
        .values()
        .find(|field| field.kind == "unknown")
        .unwrap();
    let record = &inventory.evidence[field.evidence.first().unwrap()];
    assert_eq!(record.kind, "geometry-complement");
    assert!(
        record.payload["inputs"]
            .as_array()
            .unwrap()
            .iter()
            .all(|id| inventory.evidence.contains_key(id.as_str().unwrap()))
    );
}

#[test]
fn accepting_a_name_does_not_close_field_semantics_or_coverage_questions() {
    let mut inventory = modeled();
    inventory.finish();
    let questions = inventory.questions();
    assert!(
        !questions
            .iter()
            .any(|question| question.dimension == "name")
    );
    assert!(
        questions
            .iter()
            .any(|question| question.dimension == "field-semantics")
    );
    assert!(
        questions
            .iter()
            .any(|question| question.dimension == "coverage")
    );
}

#[test]
fn region_holes_use_actual_byte_extents_without_inventing_a_register_stride() {
    let mut inventory = modeled();
    import(&mut inventory, facts(vec![access(0x1002, 8, "byte", 1, 0)]));
    inventory.finish();
    let region = &inventory.regions[0];
    assert_eq!(region.geometry_gaps, [(0x1004, 0x1010)]);
    assert_eq!(
        region.observation_gaps,
        [(0x1000, 0x1002), (0x1003, 0x1010)]
    );
    assert_eq!(
        inventory.at_address(0x1002)[0].coverage.unobserved,
        Some(0xff00ffff)
    );
}

#[test]
fn wide_geometry_exposes_unknown_bits_above_machine_word() {
    let mut inventory = RegisterInventory::default();
    inventory.import_geometry(
        "chip",
        "cpu",
        "model",
        "model-geometry",
        &json!("wide"),
        geometry(64),
    );
    import(
        &mut inventory,
        facts(vec![access(0x1004, 32, "upper", 5, 0)]),
    );
    inventory.finish();
    let register = inventory.at_address(0x1000)[0];
    assert_eq!(register.coverage.unobserved_intervals, Some(vec![(0, 32)]));
    assert_eq!(register.coverage.undescribed_intervals, Some(vec![(1, 64)]));
    assert!(
        register
            .fields
            .values()
            .any(|field| field.kind == "unknown" && field.offset == 1 && field.width == 63)
    );
    assert!(
        register
            .fields
            .values()
            .any(|field| field.kind == "candidate" && field.offset == 34 && field.width == 1)
    );
    inventory.validate().unwrap();
}

#[test]
fn stale_replay_preserves_observations_without_certifying_freshness() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("replay.json");
    let document = json!({"schema_version":4,"command":"execute replay","manifest":{"path":directory.path().join("missing.toml"),"sha256":"old"},"artifact":{"path":directory.path().join("missing.elf"),"sha256":"old"},"diagnostic_contracts":{"calls":[]},"complete":true,"phases":[{"name":"read","symbol":"entry","completion":{"kind":"returned"},"steps":1,"calls":[],"fifo_lifecycle":[],"memory_observations":[],"register_observations":[{"sequence":0,"access":"read","address":4096,"width":32,"value":42,"pc":null,"region":"dev","register":null}]}]});
    std::fs::write(&path, serde_json::to_vec(&document).unwrap()).unwrap();
    let mut inventory = RegisterInventory::default();
    inventory
        .import_observations(&path, false, "chip", "cpu")
        .unwrap();
    inventory.finish();
    assert_eq!(inventory.at_address(4096).len(), 1);
    assert!(
        inventory
            .sources
            .iter()
            .any(|source| matches!(source.state, SourceState::Stale { .. }))
    );
    assert!(inventory.at_address(4096)[0].functions.is_empty());
    assert!(
        inventory
            .evidence
            .values()
            .any(|evidence| evidence.kind == "trace-access" && evidence.payload["value"] == 42)
    );
    let text = serde_json::to_string(&document).unwrap();
    assert!(crate::artifacts::parse_replay_evidence(&text).is_err());
    let mut obsolete = document;
    obsolete["schema_version"] = json!(3);
    assert!(crate::artifacts::parse_replay_observations(&obsolete.to_string()).is_err());
}

#[test]
fn invalid_knowledge_state_cannot_enter_query_store() {
    let mut inventory = modeled();
    inventory.registers.values_mut().next().unwrap().names = KnowledgeProperty::Known {
        claims: BTreeSet::new(),
    };
    assert!(inventory.validate().is_err());
}

#[test]
fn failed_source_keeps_opaque_input_and_reason() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("invalid.json");
    std::fs::write(&path, "{unfinished: source}").unwrap();
    let mut inventory = RegisterInventory::default();
    inventory.unavailable(&path, "observations", Some("invalid JSON".to_owned()));
    assert!(matches!(
        inventory.sources[0].state,
        SourceState::Failed { .. }
    ));
    assert!(
        inventory
            .evidence
            .values()
            .any(|evidence| evidence.kind == "opaque-source"
                && evidence.payload["text"] == "{unfinished: source}")
    );
}

#[test]
fn ir_and_discovery_union_is_idempotent_and_preserves_disjoint_masks() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("linked");
    let report = crate::artifacts::render_linked_ir_fixture(
        Vec::new(),
        vec![crate::LinkedMmioRegister {
            address: 4096,
            width: 32,
            access_width_candidates: vec![16, 32],
            names: Vec::new(),
            read_shapes: 1,
            write_shapes: 1,
            poll_shapes: 0,
            predicate_shapes: 1,
            static_shapes: 1,
            indexed_candidate_shapes: 0,
            whole_register_write_shapes: 0,
            whole_register_predicate_shapes: 0,
            whole_register_poll_shapes: 0,
            read_modify_write_shapes: 0,
            write_masks: vec![12],
            predicate_masks: vec![15],
            poll_masks: Vec::new(),
            candidate_bit_ranges: Vec::new(),
            field_candidates: Vec::new(),
            functions: Vec::new(),
        }],
    );
    crate::artifacts::write_fixture_bundle(&path, &report).unwrap();
    let mut inventory = modeled();
    import(
        &mut inventory,
        facts(vec![access(4096, 32, "discovery", 3, 0)]),
    );
    inventory
        .import_ir(
            "chip",
            "cpu",
            &path,
            &crate::application::artifact_store::PublishedIrEvidence {
                reader: std::sync::Arc::new(crate::artifacts::LinkedIrReader::open(&path).unwrap()),
                manifest: std::fs::read(path.join("manifest.json")).unwrap(),
                members: crate::artifacts::bundle_files(&path)
                    .map(|input| {
                        (
                            input.file_name().unwrap().to_string_lossy().into_owned(),
                            format!("{:x}", Sha256::digest(std::fs::read(&input).unwrap())),
                        )
                    })
                    .collect(),
            },
        )
        .unwrap();
    inventory.finish();
    let first = inventory.clone();
    inventory
        .import_ir(
            "chip",
            "cpu",
            &path,
            &crate::application::artifact_store::PublishedIrEvidence {
                reader: std::sync::Arc::new(crate::artifacts::LinkedIrReader::open(&path).unwrap()),
                manifest: std::fs::read(path.join("manifest.json")).unwrap(),
                members: crate::artifacts::bundle_files(&path)
                    .map(|input| {
                        (
                            input.file_name().unwrap().to_string_lossy().into_owned(),
                            format!("{:x}", Sha256::digest(std::fs::read(&input).unwrap())),
                        )
                    })
                    .collect(),
            },
        )
        .unwrap();
    inventory.finish();
    assert_eq!(first, inventory);
    let register = inventory.at_address(4096)[0];
    let masks = register
        .fields
        .values()
        .filter_map(|field| field.mask)
        .collect::<BTreeSet<_>>();
    assert!(BTreeSet::from([3, 12, 15]).is_subset(&masks));
    assert!(
        register
            .evidence
            .iter()
            .any(|id| inventory.evidence[id].kind == "linked-register")
    );
    assert!(
        register
            .evidence
            .iter()
            .any(|id| inventory.evidence[id].kind == "discovery-access")
    );
}

#[test]
fn code_coverage_is_a_gap_not_a_register_address() {
    let mut document = facts(Vec::new());
    document["observations"] = json!([{"kind":"code-coverage","source":"fixture","section":".text","address":4096,"size":32,"complete":false,"reason":"unrecovered boundary"}]);
    let mut inventory = RegisterInventory::default();
    import(&mut inventory, document);
    assert!(inventory.registers.is_empty());
    assert!(
        inventory
            .gaps
            .iter()
            .any(|gap| gap.reason == "unrecovered boundary")
    );
    assert!(
        inventory
            .evidence
            .values()
            .any(|evidence| evidence.kind == "code-coverage")
    );
}

#[test]
fn svd_without_physical_size_keeps_address_and_declared_fields() {
    let xml = r#"<device schemaVersion="1.3"><name>Test</name><version>1</version><description>fixture</description><addressUnitBits>8</addressUnitBits><width>32</width><peripherals><peripheral><name>DEV</name><baseAddress>4096</baseAddress><registers><register><name>OPAQUE</name><addressOffset>0</addressOffset><fields><field><name>FLAG</name><bitOffset>0</bitOffset><bitWidth>1</bitWidth></field></fields></register></registers></peripheral></peripherals></device>"#;
    let geometry = open_esp_radio_register_model::svd_geometry(xml).unwrap();
    assert_eq!(geometry[0].width, None);
    let mut inventory = RegisterInventory::default();
    inventory.import_geometry(
        "chip",
        "cpu",
        "svd",
        "svd-geometry",
        &json!("size-unknown"),
        geometry,
    );
    inventory.finish();
    let register = inventory.at_address(4096)[0];
    assert!(register.physical_width.is_unknown());
    assert!(
        register
            .fields
            .values()
            .any(|field| field.names.values().contains(&"FLAG".to_owned()))
    );
    assert_eq!(register.coverage.unobserved_intervals, None);
    let wide = xml
        .replace(
            "<addressOffset>0</addressOffset>",
            "<addressOffset>0</addressOffset><size>64</size>",
        )
        .replace("<bitWidth>1</bitWidth>", "<bitWidth>64</bitWidth>");
    let mut wide_inventory = RegisterInventory::default();
    wide_inventory.import_geometry(
        "chip",
        "cpu",
        "svd",
        "svd-geometry",
        &json!("wide-field"),
        open_esp_radio_register_model::svd_geometry(&wide).unwrap(),
    );
    wide_inventory.finish();
    let wide_register = wide_inventory.at_address(4096)[0];
    assert_eq!(wide_register.coverage.described, u32::MAX);
    assert_eq!(
        wide_register.coverage.undescribed_intervals,
        Some(Vec::new())
    );
    assert_eq!(
        wide_register.coverage.unobserved_intervals,
        Some(vec![(0, 64)])
    );
}

fn published_facts_project(root: &Path) -> std::path::PathBuf {
    let manifest = query_project(root);
    let mut config = std::fs::read_to_string(&manifest).unwrap();
    config.push_str(
        "\n[registers]\nmodel = \"model.toml\"\nfacts = \"facts.json\"\nowned-ranges = [\"dev\"]\n",
    );
    std::fs::write(&manifest, config).unwrap();
    manifest
}

fn publish_facts(manifest: &Path, bytes: &[u8]) {
    use crate::application::{output_set::OutputSet, query_store::QueryStore};
    let paths = [manifest.parent().unwrap().join("facts.json")];
    let outputs = OutputSet::new(&paths, false).unwrap();
    outputs
        .file(0, "fixture MMIO")
        .unwrap()
        .bytes(bytes)
        .unwrap();
    QueryStore::open_analysis_epoch(manifest)
        .unwrap()
        .publish_analysis_outputs(&outputs.receipts().unwrap())
        .unwrap();
}

#[test]
fn register_discovery_uses_selected_epoch_before_first_inventory_query() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let manifest = published_facts_project(root);
    let before = facts(vec![access(0x1000, 32, "before", 1, 0)]);
    publish_facts(&manifest, &serde_json::to_vec(&before).unwrap());
    let old = crate::BlobrayApplication::open(&manifest).unwrap();
    old.resolved
        .artifacts
        .read_output(&root.join("facts.json"))
        .unwrap()
        .unwrap();
    let after = facts(vec![access(0x1004, 32, "after", 2, 0)]);
    publish_facts(&manifest, &serde_json::to_vec(&after).unwrap());
    std::fs::remove_file(root.join("facts.json")).unwrap();
    let retained = old.register_inventory().unwrap();
    assert_eq!(retained.inventory().at_address(0x1000).len(), 1);
    assert!(retained.inventory().at_address(0x1004).is_empty());
    assert!(
        retained.inventory().at_address(0x1000)[0]
            .functions
            .contains("before")
    );
    let fresh = crate::BlobrayApplication::open(&manifest).unwrap();
    let current = fresh.register_inventory().unwrap();
    assert_eq!(current.inventory().at_address(0x1004).len(), 1);
    assert!(current.inventory().at_address(0x1000).is_empty());
    assert_ne!(retained.id(), current.id());
}

#[test]
fn invalid_published_discovery_retains_opaque_bytes_without_reading_repaired_export() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let manifest = published_facts_project(root);
    publish_facts(&manifest, b"broken published evidence");
    let repair = facts(vec![access(0x1000, 32, "unpublished", 1, 0)]);
    std::fs::write(
        root.join("facts.json"),
        serde_json::to_vec(&repair).unwrap(),
    )
    .unwrap();
    let app = crate::BlobrayApplication::open(&manifest).unwrap();
    let snapshot = app.register_inventory().unwrap();
    let inventory = snapshot.inventory();
    assert!(inventory.at_address(0x1000).is_empty());
    assert!(
        inventory
            .sources
            .iter()
            .any(|source| source.kind == "discovery"
                && matches!(source.state, SourceState::Failed { .. }))
    );
    assert!(
        inventory
            .evidence
            .values()
            .any(|evidence| evidence.kind == "opaque-source"
                && evidence.payload["text"] == "broken published evidence")
    );
}

#[test]
fn cold_register_query_neither_creates_cache_nor_requires_analysis_writer() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let manifest = query_project(root);
    std::fs::write(root.join("base.svd"), wide_svd(0x1000)).unwrap();
    let entries = || {
        std::fs::read_dir(root)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect::<BTreeSet<_>>()
    };
    let before = entries();
    let first = crate::BlobrayApplication::open(&manifest)
        .unwrap()
        .register_inventory()
        .unwrap();
    assert_eq!(
        entries(),
        before,
        "query must not create a generated/cache directory"
    );
    let writer =
        crate::application::query_store::QueryStore::open_analysis_epoch(&manifest).unwrap();
    let fresh = crate::BlobrayApplication::open(&manifest).unwrap();
    assert_eq!(fresh.register_inventory().unwrap().id(), first.id());
    drop(writer);
}
