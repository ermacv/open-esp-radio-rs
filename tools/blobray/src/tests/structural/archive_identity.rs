//! Physical source identity through archive resolution, analysis and export.

use super::super::*;
use std::io::Write as _;

#[test]
fn repeated_archive_members_retain_interface_instruction_owners() {
    use object::write::{Object, Symbol, SymbolSection};
    use object::{
        Architecture, BinaryFormat, Endianness, SectionKind, SymbolFlags, SymbolKind, SymbolScope,
    };

    let mut object = Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
    let section = object.add_section(Vec::new(), b".text".to_vec(), SectionKind::Text);
    // Store arg1 through arg0, load a callback, call it, encounter an unsupported word.
    let bytes = [0x00b52223_u32, 0x00052283, 0x000280e7, 0x00000000]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect::<Vec<_>>();
    object.append_section_data(section, &bytes, 4);
    object.add_symbol(Symbol {
        name: b"dispatch".to_vec(),
        value: 0,
        size: bytes.len() as u64,
        kind: SymbolKind::Text,
        scope: SymbolScope::Compilation,
        weak: false,
        section: SymbolSection::Section(section),
        flags: SymbolFlags::None,
    });
    let bytes = object.write().unwrap();
    let mut archive = b"!<arch>\n".to_vec();
    for _ in 0..2 {
        writeln!(
            archive,
            "{:<16}{:<12}{:<6}{:<6}{:<8}{:<10}`",
            "same.o/",
            0,
            0,
            0,
            "100644",
            bytes.len()
        )
        .unwrap();
        archive.extend_from_slice(&bytes);
        if !bytes.len().is_multiple_of(2) {
            archive.push(b'\n');
        }
    }
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("raw.a");
    std::fs::write(&path, archive).unwrap();
    let captures = crate::source_set::CapturedSourceSet::capture([path.clone()]);
    let discovery = crate::analysis::discover_project_interfaces(
        &captures,
        &[("source-artifact:fixture".into(), path)],
        &Default::default(),
        None,
    )
    .unwrap();
    let document = crate::artifacts::build_interface_facts(&discovery).unwrap();
    let facts = crate::interfaces::InterfaceFacts::from_document(
        crate::artifacts::parse_interface_facts(&serde_json::to_string(&document).unwrap())
            .unwrap(),
    )
    .unwrap();
    assert_eq!(facts.calls.len(), 2);
    assert_eq!(facts.assignments.len(), 2);
    assert_eq!(
        facts.tables.len(),
        2,
        "arg0 belongs to its physical function"
    );
    let owners = facts
        .calls
        .iter()
        .map(|call| call.owner.clone())
        .collect::<BTreeSet<_>>();
    assert_eq!(owners.len(), 2);
    assert_eq!(facts.calls[0].function, facts.calls[1].function);
    assert_eq!(facts.calls[0].member, facts.calls[1].member);
    assert_eq!(facts.calls[0].site, facts.calls[1].site);
    assert_eq!(
        facts
            .assignments
            .iter()
            .map(|assignment| assignment.owner.clone())
            .collect::<BTreeSet<_>>(),
        owners
    );
    assert_eq!(
        facts
            .decode_blockers
            .iter()
            .map(|blocker| blocker.owner.clone())
            .collect::<BTreeSet<_>>(),
        owners
    );
    assert_eq!(
        facts
            .gaps
            .iter()
            .map(|gap| gap.evidence.owner.clone())
            .collect::<BTreeSet<_>>(),
        owners
    );
    for owner in owners {
        assert!(matches!(
            owner,
            crate::artifact::CodeIdentity::Symbol {
                location: crate::SymbolLocation {
                    object: crate::ObjectLocation::ArchiveMember { .. },
                    ..
                },
                ..
            }
        ));
    }
}

#[test]
fn relocated_interface_roots_and_navigation_do_not_join_same_name_members() {
    use object::write::{Object, Relocation, Symbol, SymbolSection};
    use object::{
        Architecture, BinaryFormat, Endianness, RelocationFlags, SectionKind, SymbolFlags,
        SymbolKind, SymbolScope,
    };
    for table_name in ["services", ""] {
        let mut object = Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
        let code = object.add_section(Vec::new(), b".text".to_vec(), SectionKind::Text);
        let bytes = [
            0x000002b7_u32,
            0x00028293,
            0x00b2a223,
            0x0002a303,
            0x000300e7,
            0x00008067,
        ]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect::<Vec<_>>();
        object.append_section_data(code, &bytes, 4);
        object.add_symbol(Symbol {
            name: b"dispatch".to_vec(),
            value: 0,
            size: bytes.len() as u64,
            kind: SymbolKind::Text,
            scope: SymbolScope::Compilation,
            weak: false,
            section: SymbolSection::Section(code),
            flags: SymbolFlags::None,
        });
        let data = object.add_section(Vec::new(), b".data".to_vec(), SectionKind::Data);
        object.append_section_data(data, &[0; 8], 4);
        let table = object.add_symbol(Symbol {
            name: table_name.as_bytes().to_vec(),
            value: 0,
            size: 8,
            kind: SymbolKind::Data,
            scope: SymbolScope::Compilation,
            weak: false,
            section: SymbolSection::Section(data),
            flags: SymbolFlags::None,
        });
        for (offset, r_type) in [
            (0, object::elf::R_RISCV_HI20),
            (4, object::elf::R_RISCV_LO12_I),
        ] {
            object
                .add_relocation(
                    code,
                    Relocation {
                        offset,
                        symbol: table,
                        addend: 0,
                        flags: RelocationFlags::Elf { r_type },
                    },
                )
                .unwrap();
        }
        let bytes = object.write().unwrap();
        let mut archive = b"!<arch>\n".to_vec();
        for _ in 0..2 {
            writeln!(
                archive,
                "{:<16}{:<12}{:<6}{:<6}{:<8}{:<10}`",
                "same.o/",
                0,
                0,
                0,
                "100644",
                bytes.len()
            )
            .unwrap();
            archive.extend_from_slice(&bytes);
            if !bytes.len().is_multiple_of(2) {
                archive.push(b'\n');
            }
        }
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("raw.a");
        std::fs::write(&path, archive).unwrap();
        let captures = crate::source_set::CapturedSourceSet::capture([path.clone()]);
        let discovery = crate::analysis::discover_project_interfaces(
            &captures,
            &[("source-artifact:fixture".into(), path)],
            &Default::default(),
            None,
        )
        .unwrap();
        let document = crate::artifacts::build_interface_facts(&discovery).unwrap();
        let facts_path = directory.path().join("interfaces.json");
        std::fs::write(&facts_path, serde_json::to_vec(&document).unwrap()).unwrap();
        let facts = crate::interfaces::InterfaceFacts::load(&facts_path).unwrap();
        assert_eq!(facts.calls.len(), 2);
        assert_eq!(facts.assignments.len(), 2);
        assert_eq!(facts.tables.len(), 2);
        assert_ne!(facts.tables[0].root, facts.tables[1].root);
        for call in &facts.calls {
            let crate::interfaces::InterfaceFactRoot::RelocatedSymbol {
                reference, symbol, ..
            } = &call.root
            else {
                panic!("relocated table")
            };
            assert_eq!(symbol, table_name);
            let open_radio_vendor_contracts::SymbolReference::Captured { location, .. } = reference
            else {
                panic!("captured reference")
            };
            assert_eq!(
                Some(location.object),
                call.owner.object().map(|(_, object)| object)
            );
            assert_eq!(call.root_linkage.symbols, vec![table_name.to_owned()]);
        }
        let inventory =
            crate::artifacts::build_symbol_inventory_document(&discovery.linkage, |_| true);
        std::fs::write(
            directory.path().join("symbols.json"),
            serde_json::to_vec(&inventory).unwrap(),
        )
        .unwrap();
        let manifest = directory.path().join("project.toml");
        std::fs::write(&manifest, "schema = 4\nid = \"fixture\"\ntarget-spec = \"target.toml\"\n[analysis.symbols]\noutput = \"symbols.json\"\n[interfaces]\nfacts = \"interfaces.json\"\n[analysis.navigation]\noutput = \"navigation.json\"\n").unwrap();
        let project = crate::ProjectSpec::load(&manifest).unwrap();
        let navigation = crate::navigation::build(&project).unwrap();
        let value = serde_json::to_value(&navigation).unwrap();
        let nodes = value["symbols"].as_array().unwrap();
        assert_eq!(
            nodes
                .iter()
                .filter(|node| node["name"] == "dispatch")
                .count(),
            2
        );
        let roots = nodes
            .iter()
            .filter(|node| !node["interface_roots"].as_array().unwrap().is_empty())
            .collect::<Vec<_>>();
        assert_eq!(roots.len(), 2);
        for root in roots {
            assert_eq!(root["name"], table_name);
            let links = root["interface_roots"].as_array().unwrap();
            assert_eq!(links.len(), 1);
            assert_eq!(
                root["occurrence"]["location"]["object"],
                links[0]["owner"]["location"]["object"]
            );
        }
        let navigation_path = directory.path().join("navigation.json");
        std::fs::write(&navigation_path, serde_json::to_vec(&navigation).unwrap()).unwrap();
        crate::navigation::inspect_report(&navigation_path).unwrap();
    }
}

fn object(return_value: u8) -> Vec<u8> {
    use object::{
        Architecture, BinaryFormat, Endianness, RelocationFlags, SectionKind, SymbolFlags,
        SymbolKind, SymbolScope,
        write::{Object, Relocation, Symbol, SymbolSection},
    };
    let mut object = Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
    let section = object.add_section(Vec::new(), b".text".to_vec(), SectionKind::Text);
    let words = [
        0x97u32,
        0x80e7,
        0x8067,
        0x513 | (u32::from(return_value) << 20),
        0x8067,
    ];
    let bytes = words
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect::<Vec<_>>();
    object.append_section_data(section, &bytes, 4);
    object.add_symbol(Symbol {
        name: b"entry".to_vec(),
        value: 0,
        size: 12,
        kind: SymbolKind::Text,
        scope: SymbolScope::Linkage,
        weak: false,
        section: SymbolSection::Section(section),
        flags: SymbolFlags::None,
    });
    let helper = object.add_symbol(Symbol {
        name: b"helper".to_vec(),
        value: 12,
        size: 8,
        kind: SymbolKind::Text,
        scope: SymbolScope::Compilation,
        weak: false,
        section: SymbolSection::Section(section),
        flags: SymbolFlags::None,
    });
    object
        .add_relocation(
            section,
            Relocation {
                offset: 0,
                symbol: helper,
                addend: 0,
                flags: RelocationFlags::Elf {
                    r_type: object::elf::R_RISCV_CALL_PLT,
                },
            },
        )
        .unwrap();
    let data = object.add_section(Vec::new(), b".data".to_vec(), SectionKind::Data);
    object.append_section_data(data, &[return_value, 0, 0, 0], 4);
    object.add_symbol(Symbol {
        name: b"state".to_vec(),
        value: 0,
        size: 4,
        kind: SymbolKind::Data,
        scope: SymbolScope::Compilation,
        weak: false,
        section: SymbolSection::Section(data),
        flags: SymbolFlags::None,
    });
    object.write().unwrap()
}

#[test]
fn project_passes_and_exports_share_captured_sources_after_files_disappear() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("input.o");
    std::fs::write(&path, object(17)).unwrap();
    let local = directory.path().join("local.toml");
    std::fs::write(
        &local,
        "schema = 1\n[[inputs]]\nrole = \"source-artifact:fixture\"\npath = \"input.o\"\n",
    )
    .unwrap();
    let run = crate::run_spec::RunSpec::load(&local).unwrap();
    let captures = crate::source_set::CapturedSourceSet::capture([path.clone()]);
    let digest = captures.sha256(&path).unwrap().to_owned();
    std::fs::remove_file(&path).unwrap();
    let inputs = vec![("source-artifact:fixture".to_owned(), path.clone())];
    let linkage = crate::analysis::build_project_linkage_inventory(&captures, &inputs).unwrap();
    assert_eq!(linkage.artifacts[0].sha256, digest);
    let symbol_document = crate::artifacts::build_symbol_inventory_document(&linkage, |_| true);
    assert!(
        serde_json::to_string(&symbol_document)
            .unwrap()
            .contains(&digest)
    );

    let code = crate::analysis::EffectiveCodeCatalog::default();
    let map = MmioMap {
        registers: Vec::new(),
        regions: Vec::new(),
    };
    let mmio = crate::analysis::discover_mmio(crate::analysis::MmioDiscoveryRequest {
        captures: &captures,
        artifacts: &[("fixture".to_owned(), path.clone())],
        ranges: &[],
        symbol_prefix: "",
        code_symbol_selection: artifact::CodeSymbolSelection::All,
        svd: &map,
        effective_code: Some(&code),
        options: crate::analysis::MmioDiscoveryOptions { jobs: 1 },
    })
    .unwrap();
    assert_eq!(mmio.artifacts[0].functions, 2);
    assert_eq!(mmio.artifacts[0].sha256, digest);
    let mmio_document = crate::artifacts::build_mmio_facts(&mmio).unwrap();
    assert!(
        serde_json::to_string(&mmio_document)
            .unwrap()
            .contains(&digest)
    );
    let interfaces = crate::analysis::discover_project_interfaces(
        &captures,
        &inputs,
        &Default::default(),
        Some(&code),
    )
    .unwrap();
    assert_eq!(interfaces.functions, vec![2]);
    let interface_document = crate::artifacts::build_interface_facts(&interfaces).unwrap();
    assert!(
        serde_json::to_string(&interface_document)
            .unwrap()
            .contains(&digest)
    );

    let capture = captures.artifact(&path).unwrap();
    assert!(std::ptr::eq(capture, captures.artifact(&path).unwrap()));
    let entry = TEST_RISCV_HARNESS.contracts.entry_contract("none").unwrap();
    let resolver = ReferenceResolver::from_captured(
        capture,
        &[],
        &TEST_RISCV_HARNESS,
        entry,
        artifact::CodeSymbolSelection::All,
        &[],
    )
    .unwrap();
    let report = crate::analysis::build_linked_ir_for_source(
        &resolver,
        &map,
        crate::analysis::LinkedIrSourceOptions {
            symbol_prefix: "",
            source: "fixture",
            artifact_sha256: &digest,
            namespace_identities: true,
            include_reachable: true,
            jobs: 1,
            compact_projected_actions: false,
        },
    );
    let profile = crate::project_ir::ProjectIrProfile {
        id: "fixture".to_owned(),
        sources: vec!["fixture".to_owned()],
        roots: crate::project_ir::ProjectIrRoots::All,
        include_reachable: true,
        entry_contract: "none".to_owned(),
        output: directory.path().join("fixture.ir"),
    };
    let coverage = crate::application::coverage::build(&captures, &profile, &run, &report).unwrap();
    let covered = serde_json::to_value(coverage).unwrap();
    assert_eq!(covered["schema"], 2);
    assert_eq!(covered["roots"].as_array().unwrap().len(), 2);
    let artifacts = vec![crate::linked_ir_export::named_artifact_path("fixture", path).unwrap()];
    let reviewed = BTreeMap::new();
    let document = crate::artifacts::build_linked_ir_document(
        &artifacts,
        &[],
        &[],
        "",
        entry,
        crate::artifacts::LinkedIrPublication {
            captures: &captures,
            report: &report,
            reviewed_bindings: &reviewed,
        },
        true,
    )
    .unwrap();
    let encoded = serde_json::to_string(&document).unwrap();
    let stored = crate::artifacts::parse_linked_ir(&encoded).unwrap();
    assert_eq!(stored.functions.len(), 2);
    let serialized: serde_json::Value = serde_json::from_str(&encoded).unwrap();
    assert_eq!(serialized["artifacts"][0]["artifact"]["sha256"], digest);
    assert_eq!(serialized["data_objects"].as_array().unwrap().len(), 1);
    assert_eq!(serialized["data_objects"][0]["initializer_hex"], "11000000");
}

#[test]
fn repeated_members_keep_local_calls_and_distinct_queryable_occurrences() {
    let mut archive = b"!<arch>\n".to_vec();
    for value in [1, 2] {
        let bytes = object(value);
        writeln!(
            archive,
            "{:<16}{:<12}{:<6}{:<6}{:<8}{:<10}`",
            "same.o/",
            0,
            0,
            0,
            "100644",
            bytes.len()
        )
        .unwrap();
        archive.extend_from_slice(&bytes);
        if !bytes.len().is_multiple_of(2) {
            archive.push(b'\n');
        }
    }
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("raw.a");
    std::fs::write(&path, archive).unwrap();
    let entry = TEST_RISCV_HARNESS.contracts.entry_contract("none").unwrap();
    let capture = artifact::CapturedArtifact::open(&path).unwrap();
    let resolver = ReferenceResolver::load_all_code_with_entry_contract(
        &path,
        &[],
        &TEST_RISCV_HARNESS,
        entry,
    )
    .unwrap();
    assert_eq!(resolver.symbols.len(), 4);
    assert_eq!(resolver.symbol_ids.len(), 4);
    assert!(
        resolver
            .select_symbol(Some("same.o"), "entry", Some(0))
            .unwrap_err()
            .to_string()
            .contains("ambiguous")
    );
    let entries = resolver
        .symbols
        .iter()
        .filter(|symbol| symbol.name == "entry")
        .collect::<Vec<_>>();
    assert_eq!(entries.len(), 2);
    assert_ne!(entries[0].identity, entries[1].identity);
    let map = MmioMap {
        registers: Vec::new(),
        regions: Vec::new(),
    };
    for (index, owner) in entries.iter().enumerate() {
        let (_, target) = resolver
            .relocated_calls
            .get(&StructuralCallSite::new(owner, 0))
            .unwrap();
        let callee = &resolver.symbols_by_address[&target.unwrap()];
        assert_eq!(owner.identity.object(), callee.identity.object());
        assert_eq!(
            resolver.trace_symbol(owner, &map).unwrap().return_value,
            SymbolicValue::Constant(index as u32 + 1)
        );
    }
    let mut report = crate::analysis::build_linked_ir_for_source(
        &resolver,
        &map,
        crate::analysis::LinkedIrSourceOptions {
            symbol_prefix: "",
            source: "fixture",
            artifact_sha256: entries[0].identity.artifact_sha256().unwrap(),
            namespace_identities: true,
            include_reachable: true,
            jobs: 1,
            compact_projected_actions: false,
        },
    );
    assert_eq!(report.functions.len(), 4);
    let captures = crate::source_set::CapturedSourceSet::capture([path.clone()]);
    let local = directory.path().join("local.toml");
    std::fs::write(
        &local,
        "schema = 1\n[[inputs]]\nrole = \"source-artifact:fixture\"\npath = \"raw.a\"\n",
    )
    .unwrap();
    let run = crate::run_spec::RunSpec::load(&local).unwrap();
    let profile = crate::project_ir::ProjectIrProfile {
        id: "fixture".to_owned(),
        sources: vec!["fixture".to_owned()],
        roots: crate::project_ir::ProjectIrRoots::All,
        include_reachable: true,
        entry_contract: "none".to_owned(),
        output: directory.path().join("fixture.ir"),
    };
    let omitted = report.functions.remove(0);
    let coverage = crate::application::coverage::build(&captures, &profile, &run, &report).unwrap();
    let covered = serde_json::to_value(coverage).unwrap();
    let roots = covered["roots"].as_array().unwrap();
    assert_eq!(roots.len(), 4);
    let missing = roots
        .iter()
        .filter(|root| root["outcome"] == "missing")
        .collect::<Vec<_>>();
    assert_eq!(
        missing.len(),
        1,
        "a same-name sibling must not account for an omitted occurrence"
    );
    assert_eq!(
        missing[0]["code_identity"],
        serde_json::to_value(&omitted.code_identity).unwrap()
    );
    report.functions.push(omitted);
    let artifacts =
        vec![crate::linked_ir_export::named_artifact_path("fixture", path.clone()).unwrap()];
    let reviewed = BTreeMap::new();
    let document = crate::artifacts::build_linked_ir_document(
        &artifacts,
        &[],
        &[],
        "",
        entry,
        crate::artifacts::LinkedIrPublication {
            captures: &captures,
            report: &report,
            reviewed_bindings: &reviewed,
        },
        true,
    )
    .unwrap();
    let encoded = serde_json::to_string(&document).unwrap();
    let persisted = crate::artifacts::parse_linked_ir(&encoded).unwrap();
    assert_eq!(persisted.data_objects.len(), 2);
    assert_ne!(
        persisted.data_objects[0].data_identity,
        persisted.data_objects[1].data_identity
    );
    assert_ne!(
        persisted.data_objects[0].occurrence,
        persisted.data_objects[1].occurrence
    );
    let mut tampered_data: serde_json::Value = serde_json::from_str(&encoded).unwrap();
    let mut duplicated_data = tampered_data.clone();
    let duplicate = duplicated_data["data_objects"][0].clone();
    duplicated_data["data_objects"]
        .as_array_mut()
        .unwrap()
        .push(duplicate);
    assert!(
        crate::artifacts::parse_linked_ir(&duplicated_data.to_string())
            .unwrap_err()
            .to_string()
            .contains("duplicate linked-IR data object identity")
    );
    assert_ne!(
        tampered_data["data_objects"][0]["initializer_hex"],
        tampered_data["data_objects"][1]["initializer_hex"]
    );
    tampered_data["data_objects"][0]["data_identity"] =
        tampered_data["data_objects"][1]["data_identity"].clone();
    assert!(
        crate::artifacts::parse_linked_ir(&tampered_data.to_string())
            .unwrap_err()
            .to_string()
            .contains("physical identity")
    );
    let bundle = directory.path().join("objects.ir");
    crate::artifacts::stage_linked_ir_bundle(&bundle, &document)
        .unwrap()
        .publish(&bundle)
        .unwrap();
    let reader = crate::artifacts::LinkedIrReader::open(&bundle).unwrap();
    let objects = reader.get_data_object("fixture", "state").unwrap();
    assert_eq!(objects.len(), 2);
    assert_ne!(objects[0].data_identity, objects[1].data_identity);
    assert_ne!(objects[0].occurrence, objects[1].occurrence);
    for object in &objects {
        let selected = reader
            .get_data_object("fixture", &object.occurrence)
            .unwrap();
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].data_identity, object.data_identity);
    }
    let index_path = bundle.join("data-object-index.json");
    let index_bytes = std::fs::read(&index_path).unwrap();
    let mut index: serde_json::Value = serde_json::from_slice(&index_bytes).unwrap();
    index["records"][0]["data_identity"] = index["records"][1]["data_identity"].clone();
    std::fs::write(&index_path, serde_json::to_vec(&index).unwrap()).unwrap();
    let error = crate::artifacts::LinkedIrReader::open(&bundle)
        .err()
        .unwrap();
    assert!(error.to_string().contains("physical identity"));
    std::fs::write(&index_path, index_bytes).unwrap();
    assert_eq!(
        report
            .functions
            .iter()
            .map(|function| &function.identity)
            .collect::<BTreeSet<_>>()
            .len(),
        4
    );
    let rendered =
        crate::artifacts::render_linked_ir_fixture(report.functions, report.mmio_registers);
    let stored = crate::artifacts::parse_linked_ir(&rendered).unwrap();
    assert_eq!(stored.functions.len(), 4);
    assert_eq!(
        stored
            .functions
            .iter()
            .map(|function| &function.occurrence)
            .collect::<BTreeSet<_>>()
            .len(),
        4
    );
    assert_eq!(
        stored
            .functions
            .iter()
            .map(|function| &function.code_identity)
            .collect::<BTreeSet<_>>()
            .len(),
        4
    );
    let mut tampered: serde_json::Value = serde_json::from_str(&rendered).unwrap();
    tampered["functions"][0]["code_identity"] = tampered["functions"][1]["code_identity"].clone();
    assert!(
        crate::artifacts::parse_linked_ir(&tampered.to_string())
            .unwrap_err()
            .to_string()
            .contains("physical identity")
    );
    // A locator path change never changes the captured definition identity.
    let moved = directory.path().join("moved.a");
    std::fs::rename(path, &moved).unwrap();
    let reloaded = ReferenceResolver::load_all_code_with_entry_contract(
        &moved,
        &[],
        &TEST_RISCV_HARNESS,
        entry,
    )
    .unwrap();
    assert_eq!(resolver.symbol_ids, reloaded.symbol_ids);
    std::fs::remove_file(&moved).unwrap();
    let from_capture = ReferenceResolver::from_captured(
        &capture,
        &[],
        &TEST_RISCV_HARNESS,
        entry,
        artifact::CodeSymbolSelection::All,
        &[],
    )
    .unwrap();
    assert_eq!(resolver.symbols, from_capture.symbols);
    assert_eq!(resolver.data_objects, from_capture.data_objects);
    assert_eq!(from_capture.data_objects.len(), 2);
    assert_eq!(
        from_capture
            .data_objects
            .iter()
            .map(|object| object.initializer[0])
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([1, 2])
    );
    assert!(std::ptr::eq(
        capture.data_objects().unwrap(),
        capture.data_objects().unwrap()
    ));
    for symbol in from_capture
        .symbols
        .iter()
        .filter(|symbol| symbol.name == "helper")
    {
        let direct = from_capture.trace_direct_symbol(symbol, &map).unwrap();
        let reference = from_capture.trace_symbol(symbol, &map).unwrap();
        assert_eq!(direct.return_value, reference.return_value);
    }
}

#[test]
fn physical_data_relocations_survive_same_names_and_unnamed_targets() {
    use object::{
        Architecture, BinaryFormat, Endianness, RelocationFlags, SectionKind, SymbolFlags,
        SymbolKind, SymbolScope,
        write::{Object, Relocation, Symbol, SymbolSection},
    };
    for name in ["state", ""] {
        let mut archive = b"!<arch>\n".to_vec();
        for value in [17u8, 29] {
            let mut object =
                Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
            let code = object.add_section(Vec::new(), b".text".to_vec(), SectionKind::Text);
            object.append_section_data(
                code,
                &[0x00000537u32, 0x00052503, 0x00008067]
                    .into_iter()
                    .flat_map(u32::to_le_bytes)
                    .collect::<Vec<_>>(),
                4,
            );
            object.add_symbol(Symbol {
                name: b"read_state".to_vec(),
                value: 0,
                size: 12,
                kind: SymbolKind::Text,
                scope: SymbolScope::Linkage,
                weak: false,
                section: SymbolSection::Section(code),
                flags: SymbolFlags::None,
            });
            let data = object.add_section(Vec::new(), b".data".to_vec(), SectionKind::Data);
            object.append_section_data(data, &[value, 0, 0, 0], 4);
            let state = object.add_symbol(Symbol {
                name: name.as_bytes().to_vec(),
                value: 0,
                size: 4,
                kind: SymbolKind::Data,
                scope: SymbolScope::Compilation,
                weak: false,
                section: SymbolSection::Section(data),
                flags: SymbolFlags::None,
            });
            for (offset, r_type) in [
                (0, object::elf::R_RISCV_HI20),
                (4, object::elf::R_RISCV_LO12_I),
            ] {
                object
                    .add_relocation(
                        code,
                        Relocation {
                            offset,
                            symbol: state,
                            addend: 0,
                            flags: RelocationFlags::Elf { r_type },
                        },
                    )
                    .unwrap();
            }
            let bytes = object.write().unwrap();
            writeln!(
                archive,
                "{:<16}{:<12}{:<6}{:<6}{:<8}{:<10}`",
                "same.o/",
                0,
                0,
                0,
                "100644",
                bytes.len()
            )
            .unwrap();
            archive.extend_from_slice(&bytes);
            if !bytes.len().is_multiple_of(2) {
                archive.push(b'\n');
            }
        }
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("raw.a");
        std::fs::write(&path, archive).unwrap();
        let captures = crate::source_set::CapturedSourceSet::capture([path.clone()]);
        let capture = captures.artifact(&path).unwrap();
        for symbol in capture
            .code_symbols("", artifact::CodeSymbolSelection::All)
            .unwrap()
        {
            let body =
                artifact::inspect_captured_function(&path, capture, symbol.location).unwrap();
            let targets = body
                .instructions
                .iter()
                .flat_map(|instruction| &instruction.relocations)
                .map(|relocation| relocation.reference.definition_identity().unwrap())
                .collect::<BTreeSet<_>>();
            assert_eq!(targets.len(), 1);
            assert!(
                capture
                    .data_objects()
                    .unwrap()
                    .iter()
                    .any(|object| targets.contains(&object.identity))
            );
        }
        let entry = TEST_RISCV_HARNESS.contracts.entry_contract("none").unwrap();
        let resolver = ReferenceResolver::from_captured(
            capture,
            &[],
            &TEST_RISCV_HARNESS,
            entry,
            artifact::CodeSymbolSelection::All,
            &[],
        )
        .unwrap();
        let report = crate::analysis::build_linked_ir_for_source(
            &resolver,
            &MmioMap {
                registers: vec![],
                regions: vec![],
            },
            crate::analysis::LinkedIrSourceOptions {
                symbol_prefix: "",
                source: "fixture",
                artifact_sha256: capture.sha256(),
                namespace_identities: true,
                include_reachable: true,
                jobs: 1,
                compact_projected_actions: false,
            },
        );
        assert_eq!(report.functions.len(), 2);
        let artifacts =
            vec![crate::linked_ir_export::named_artifact_path("fixture", path).unwrap()];
        let reviewed = BTreeMap::new();
        let document = crate::artifacts::build_linked_ir_document(
            &artifacts,
            &[],
            &[],
            "",
            entry,
            crate::artifacts::LinkedIrPublication {
                captures: &captures,
                report: &report,
                reviewed_bindings: &reviewed,
            },
            true,
        )
        .unwrap();
        let encoded = serde_json::to_string(&document).unwrap();
        let parsed = crate::artifacts::parse_linked_ir(&encoded).unwrap();
        assert_eq!(parsed.data_objects.len(), 2);
        for object in &parsed.data_objects {
            assert_eq!(object.xrefs.len(), 2, "{encoded}");
            assert_ne!(object.xrefs[0].evidence, object.xrefs[1].evidence);
            assert_eq!(object.xrefs[0].function, object.xrefs[1].function);
            assert!(matches!(
                object.xrefs[0].association,
                crate::artifacts::DataObjectAssociation::PhysicalLocalDefinition
            ));
            let function = report
                .functions
                .iter()
                .find(|function| function.identity == object.xrefs[0].function)
                .unwrap();
            assert!(
                function
                    .memory_accesses
                    .iter()
                    .any(|access| match &access.object {
                        crate::LinkedMemoryObject::Global { reference, .. } =>
                            reference.definition_identity().as_ref() == Some(&object.data_identity),
                        _ => false,
                    })
            );
        }
        let bundle = directory.path().join("facts.ir");
        crate::artifacts::stage_linked_ir_bundle(&bundle, &document)
            .unwrap()
            .publish(&bundle)
            .unwrap();
        let reader = crate::artifacts::LinkedIrReader::open(&bundle).unwrap();
        for object in &parsed.data_objects {
            let reloaded = reader
                .get_data_object("fixture", &object.occurrence)
                .unwrap();
            assert_eq!(reloaded.len(), 1);
            assert_eq!(reloaded[0].xrefs.len(), 2);
            assert_eq!(reloaded[0].xrefs[0].function, object.xrefs[0].function);
        }
    }
}

#[test]
fn physical_data_address_candidates_survive_companions_export_and_reload() {
    use object::{
        Architecture, BinaryFormat, Endianness, SectionKind, SymbolFlags, SymbolKind, SymbolScope,
        write::{Object, Symbol, SymbolSection},
    };
    let linked_image = |value: u8, with_code: bool| {
        let mut object = Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
        let mut addresses = vec![0u32];
        if with_code {
            let text = object.add_section(Vec::new(), b".text".to_vec(), SectionKind::Text);
            addresses.push(0x1000_1000);
            object.append_section_data(
                text,
                &[0x10002537u32, 0x00452503, 0x00008067]
                    .into_iter()
                    .flat_map(u32::to_le_bytes)
                    .collect::<Vec<_>>(),
                4,
            );
            object.add_symbol(Symbol {
                name: b"read_state".to_vec(),
                value: 0x1000_1000,
                size: 12,
                kind: SymbolKind::Text,
                scope: SymbolScope::Linkage,
                weak: false,
                section: SymbolSection::Section(text),
                flags: SymbolFlags::None,
            });
        }
        let data = object.add_section(Vec::new(), b".data".to_vec(), SectionKind::Data);
        addresses.push(0x1000_2000);
        object.append_section_data(data, &[value; 64], 4);
        for (name, offset, size, scope) in [
            ("image", 0, 64, SymbolScope::Linkage),
            ("state", 4, 4, SymbolScope::Compilation),
            ("alias", 4, 4, SymbolScope::Linkage),
        ] {
            object.add_symbol(Symbol {
                name: name.as_bytes().to_vec(),
                value: 0x1000_2000 + offset,
                size,
                kind: SymbolKind::Data,
                scope,
                weak: false,
                section: SymbolSection::Section(data),
                flags: SymbolFlags::None,
            });
        }
        let mut bytes = object.write().unwrap();
        bytes[16..18].copy_from_slice(&object::elf::ET_EXEC.to_le_bytes());
        let headers = u32::from_le_bytes(bytes[32..36].try_into().unwrap()) as usize;
        for (section, address) in addresses.into_iter().enumerate().skip(1) {
            let start = headers + section * 40 + 12;
            bytes[start..start + 4].copy_from_slice(&address.to_le_bytes());
        }
        bytes
    };
    let directory = tempfile::tempdir().unwrap();
    let primary_path = directory.path().join("primary.elf");
    let companion_path = directory.path().join("companion.elf");
    std::fs::write(&primary_path, linked_image(17, true)).unwrap();
    std::fs::write(&companion_path, linked_image(29, false)).unwrap();
    let captures = crate::source_set::CapturedSourceSet::capture([
        primary_path.clone(),
        companion_path.clone(),
    ]);
    let primary = captures.artifact(&primary_path).unwrap();
    let companion = captures.artifact(&companion_path).unwrap();
    let entry = TEST_RISCV_HARNESS.contracts.entry_contract("none").unwrap();
    let resolver = ReferenceResolver::from_captured(
        primary,
        &[companion],
        &TEST_RISCV_HARNESS,
        entry,
        artifact::CodeSymbolSelection::All,
        &[],
    )
    .unwrap();
    let report = crate::analysis::build_linked_ir_for_source(
        &resolver,
        &MmioMap {
            registers: vec![],
            regions: vec![],
        },
        crate::analysis::LinkedIrSourceOptions {
            symbol_prefix: "",
            source: "fixture",
            artifact_sha256: primary.sha256(),
            namespace_identities: true,
            include_reachable: true,
            jobs: 1,
            compact_projected_actions: false,
        },
    );
    assert_eq!(report.functions.len(), 1);
    let function = &report.functions[0];
    let access = function
        .memory_accesses
        .iter()
        .find(|access| access.access == "read")
        .expect("RAM load");
    assert!(matches!(
        access.object,
        crate::LinkedMemoryObject::Absolute { .. }
    ));
    assert_eq!(access.data_address.candidates().len(), 6);
    let candidate_ids = access
        .data_address
        .candidates()
        .iter()
        .map(|candidate| candidate.identity.clone())
        .collect::<BTreeSet<_>>();
    assert_eq!(candidate_ids.len(), 6);
    assert!(function.pseudo.contains("Ambiguous"), "{}", function.pseudo);
    let mut artifact =
        crate::linked_ir_export::named_artifact_path("fixture", primary_path).unwrap();
    artifact.companions.push(companion_path);
    let artifacts = vec![artifact];
    let reviewed = BTreeMap::new();
    let document = crate::artifacts::build_linked_ir_document(
        &artifacts,
        &[],
        &[],
        "",
        entry,
        crate::artifacts::LinkedIrPublication {
            captures: &captures,
            report: &report,
            reviewed_bindings: &reviewed,
        },
        true,
    )
    .unwrap();
    let encoded = serde_json::to_string(&document).unwrap();
    let parsed = crate::artifacts::parse_linked_ir(&encoded).unwrap();
    assert_eq!(parsed.data_objects.len(), 6);
    for object in &parsed.data_objects {
        assert!(candidate_ids.contains(&object.data_identity));
        assert!(!object.xrefs.is_empty());
        assert!(object.xrefs.iter().all(|xref| matches!(
            xref.association,
            crate::artifacts::DataObjectAssociation::AddressRangeCandidate
        )));
        assert!(
            object
                .xrefs
                .iter()
                .all(|xref| xref.function == function.identity)
        );
    }
    let bundle = directory.path().join("facts.ir");
    crate::artifacts::stage_linked_ir_bundle(&bundle, &document)
        .unwrap()
        .publish(&bundle)
        .unwrap();
    let reader = crate::artifacts::LinkedIrReader::open(&bundle).unwrap();
    let restored = reader
        .get_function_by_identity(&function.identity)
        .unwrap()
        .unwrap();
    let restored_candidates = restored
        .instruction_effects
        .iter()
        .flat_map(|effect| match effect {
            crate::artifacts::StoredInstructionEffect::Memory { data_address, .. } => {
                data_address.candidates()
            }
            _ => &[],
        })
        .map(|candidate| candidate.identity.clone())
        .collect::<BTreeSet<_>>();
    assert_eq!(restored_candidates, candidate_ids);
    // Each image contributes the named symbol and its separate alias occurrence.
    let named = reader.get_data_object("fixture", "state").unwrap();
    assert_eq!(named.len(), 4);
    assert_eq!(
        named
            .iter()
            .map(|object| &object.data_identity)
            .collect::<BTreeSet<_>>()
            .len(),
        4
    );
    for object in &parsed.data_objects {
        assert_eq!(
            reader
                .get_data_object("fixture", &object.occurrence)
                .unwrap()
                .len(),
            1
        );
    }
    let mut missing: serde_json::Value = serde_json::from_str(&encoded).unwrap();
    missing["functions"][0]["memory_accesses"][0]
        .as_object_mut()
        .unwrap()
        .remove("data_address");
    assert!(
        crate::artifacts::parse_linked_ir(&missing.to_string())
            .unwrap_err()
            .to_string()
            .contains("data_address")
    );
    let mut undeclared: serde_json::Value = serde_json::from_str(&encoded).unwrap();
    undeclared["artifacts"][0]["companions"] = serde_json::json!([]);
    assert!(
        crate::artifacts::parse_linked_ir(&undeclared.to_string())
            .unwrap_err()
            .to_string()
            .contains("undeclared source artifact")
    );
}
