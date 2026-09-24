use super::interfaces::{cli, propose, review};
use super::*;
fn navigate(f: &Fixture, q: &NavigationQuery) -> serde_json::Value {
    let path = f.dir.path().join("navigate.json");
    fs::write(&path, serde_json::to_vec(q).unwrap()).unwrap();
    cli(f, &["navigate", "--request", path.to_str().unwrap()])
}
fn scope(f: &Fixture, analysis: FunctionAnalysisId) -> NavigationScope {
    NavigationScope {
        revision: f.revision.clone(),
        publications: vec![],
        analyses: vec![analysis],
        knowledge: None,
    }
}
#[test]
fn context_navigation_uses_logical_signature_arguments_and_explicit_unknown_mappings() {
    for profile in 0..4 {
        let words = if profile >= 2 {
            vec![0x00012503u32, 0x00452583, 0x00b52423, 0x00008067]
        } else {
            vec![0x00462683u32, 0x00d62423, 0x00008067]
        };
        let code: Vec<_> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        let bytes = object(&code, code.len() as u64, false);
        let f = fixture(bytes.clone(), profile % 2 == 0);
        let run = analyze(&f);
        assert_eq!(run.state, RunState::Completed);
        let analysis = run.analysis.unwrap();
        let argument = if profile >= 2 { 8 } else { 1 };
        let word = if profile >= 2 { 8 } else { 2 };
        let mut arguments = Vec::new();
        for i in 0..argument {
            arguments.push(CallArgument {
                role: format!("fixture.arg{i}").try_into().unwrap(),
                value_type: AbiValueType::Integer {
                    bits: if profile >= 2 { 32 } else { 64 },
                    signed: false,
                },
            });
        }
        arguments.push(CallArgument {
            role: "fixture.context".to_owned().try_into().unwrap(),
            value_type: AbiValueType::Pointer { nullable: false },
        });
        let signature = (profile != 1).then_some(CallSignature {
            arguments,
            result: AbiValueType::Void,
            variadic: false,
        });
        let p: KnowledgeProposal = serde_json::from_value(serde_json::json!({
            "subject":"fixture.context-navigation","occurrence":{"revision":f.revision,"source":f.request.source,
                "object":f.request.selector.object(),"symbol":f.request.selector.symbol()},
            "claim":{"kind":"function","contract":{"selector":f.request.selector,"abi":"riscv-integer","signature":signature,
                "name":"fields","role":null,"return_role":null,"summary":"fixture field access","applicability":"fixture only",
                "contexts":[{"argument":argument,"name":"state","start":0,"length":12,"fields":[
                    {"offset":4,"width":4,"name":"source","value_type":null,"access":"read","role":null},
                    {"offset":8,"width":4,"name":"destination","value_type":null,"access":"write","role":null}]}],"preconditions":[]}},
            "evidence":[{"kind":"source","payload":ArtifactId::of_bytes(&bytes),"range":{"start":0,"length":bytes.len()}}],"note":null
        })).unwrap();
        let proposed = propose(&f, p, None);
        assert_eq!(proposed.state, RunState::Completed, "{proposed:?}");
        let entries = cli(&f, &["knowledge", "show"]);
        let id: AssertionId = entries["records"][0]["value"]["id"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
        let mut q = NavigationQuery {
            scope: scope(&f, analysis),
            filter: NavigationFilter::Context {
                assertion: id.clone(),
                field: None,
                access: None,
                arguments: vec![],
            },
        };
        q.scope.knowledge = proposed.knowledge.clone();
        assert!(
            f.app
                .query(
                    &f.project,
                    app::ReadQuery::Navigate { request: q.clone() },
                    budget()
                )
                .is_err()
        );
        let accepted = review(&f, proposed.knowledge.unwrap(), id, ReviewDecision::Accept);
        assert_eq!(accepted.state, RunState::Completed, "{accepted:?}");
        q.scope.knowledge = accepted.knowledge;
        if profile == 1 {
            assert!(
                f.app
                    .query(
                        &f.project,
                        app::ReadQuery::Navigate { request: q.clone() },
                        budget()
                    )
                    .is_err()
            );
            let NavigationFilter::Context { arguments, .. } = &mut q.filter else {
                panic!()
            };
            arguments.push(ArgumentWord { argument, word });
        }
        let observed = navigate(&f, &q);
        let accesses: Vec<_> = observed["records"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| r["value"]["kind"] == "access")
            .collect();
        assert_eq!(accesses.len(), 2, "{observed}");
        assert_eq!(accesses[0]["value"]["matches"][0]["field"], "source");
        assert_eq!(accesses[0]["value"]["matches"][0]["argument"], argument);
        assert_eq!(accesses[1]["value"]["matches"][0]["field"], "destination");
        assert!(accesses.iter().all(|r| r["value"]["issue"].is_null()));
        let NavigationFilter::Context { access, field, .. } = &mut q.filter else {
            panic!()
        };
        *access = Some(AccessDirection::Writers);
        *field = Some(ContextFieldKey {
            argument,
            name: "destination".into(),
        });
        let writes = navigate(&f, &q);
        assert_eq!(writes["records"].as_array().unwrap().len(), 1);
        fs::remove_file(f.dir.path().join("entry.o")).unwrap();
        fs::remove_file(f.dir.path().join("entry.a")).unwrap();
        assert_eq!(navigate(&f, &q), writes);
        if profile != 1 {
            let NavigationFilter::Context { arguments, .. } = &mut q.filter else {
                panic!()
            };
            arguments.push(ArgumentWord {
                argument,
                word: word - 1,
            });
            assert!(
                f.app
                    .query(
                        &f.project,
                        app::ReadQuery::Navigate { request: q },
                        budget()
                    )
                    .is_err()
            );
        }
    }
}
fn bss_object() -> Vec<u8> {
    let mut o = Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
    let text = o.add_section(vec![], b".text.entry".to_vec(), SectionKind::Text);
    let data = o.add_section(
        vec![],
        b".bss.buffer".to_vec(),
        SectionKind::UninitializedData,
    );
    o.append_section_bss(data, 16, 4);
    let words = [
        0x000002b7u32,
        0x00028293,
        0x0002a503,
        0x00a2a223,
        0x00008067,
    ];
    let code: Vec<_> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    o.append_section_data(text, &code, 4);
    o.add_symbol(Symbol {
        name: b"entry".to_vec(),
        value: 0,
        size: code.len() as u64,
        kind: SymbolKind::Text,
        scope: SymbolScope::Linkage,
        weak: false,
        section: SymbolSection::Section(text),
        flags: SymbolFlags::None,
    });
    let buffer = o.add_symbol(Symbol {
        name: b"buffer".to_vec(),
        value: 0,
        size: 16,
        kind: SymbolKind::Data,
        scope: SymbolScope::Linkage,
        weak: false,
        section: SymbolSection::Section(data),
        flags: SymbolFlags::None,
    });
    for (offset, r_type) in [
        (0, object::elf::R_RISCV_HI20),
        (4, object::elf::R_RISCV_LO12_I),
    ] {
        o.add_relocation(
            text,
            Relocation {
                offset,
                symbol: buffer,
                addend: 0,
                flags: RelocationFlags::Elf { r_type },
            },
        )
        .unwrap();
    }
    o.write().unwrap()
}
#[test]
fn bss_reader_writer_navigation_uses_physical_ranges_without_fabricating_data_bytes() {
    use object::{Object as _, ObjectSection as _};
    for thin in [false, true] {
        let bytes = bss_object();
        let section = object::File::parse(bytes.as_slice())
            .unwrap()
            .section_by_name(".bss.buffer")
            .unwrap()
            .index()
            .0 as u32;
        let f = fixture(bytes, thin);
        let run = analyze(&f);
        assert_eq!(run.state, RunState::Completed);
        let occurrence = KnowledgeOccurrence {
            revision: f.revision.clone(),
            source: f.request.source.clone(),
            object: f.request.selector.object().clone(),
            symbol: None,
        };
        let selector = DataSelector::Section {
            section,
            offset: 2,
            length: 4,
        };
        let mut q = NavigationQuery {
            scope: scope(&f, run.analysis.unwrap()),
            filter: NavigationFilter::Object {
                occurrence: Box::new(occurrence.clone()),
                selector: selector.clone(),
                access: None,
            },
        };
        let all = navigate(&f, &q);
        assert!(all["summary"]["summary"]["data"]["file_range"].is_null());
        let rows = all["records"].as_array().unwrap();
        assert_eq!(rows.len(), 2, "{all}");
        assert_eq!(rows[0]["value"]["matches"][0]["offset"], -2);
        assert_eq!(rows[1]["value"]["matches"][0]["offset"], 2);
        assert!(
            rows.iter()
                .all(|r| r["value"]["matches"][0]["partial_overlap"] == true)
        );
        let NavigationFilter::Object { access, .. } = &mut q.filter else {
            panic!()
        };
        *access = Some(AccessDirection::Writers);
        let writes = navigate(&f, &q);
        assert_eq!(writes["records"].as_array().unwrap().len(), 1);
        assert!(
            f.app
                .query(
                    &f.project,
                    app::ReadQuery::Data {
                        request: DataRequest {
                            occurrence,
                            ranges: vec![selector],
                            analyses: vec![],
                            pointer_table: None
                        }
                    },
                    budget()
                )
                .is_err()
        );
        fs::remove_file(f.dir.path().join("entry.a")).unwrap();
        fs::remove_file(f.dir.path().join("entry.o")).unwrap();
        assert_eq!(navigate(&f, &q), writes);
    }
}
