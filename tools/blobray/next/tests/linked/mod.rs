use super::*;
use object::{Object as _, ObjectSymbol as _};

fn prepared(f: &Fixture) -> PreparedImageId {
    let plan = f
        .app
        .link_plan(&f.project, f.request.clone(), &linker(), budget())
        .unwrap();
    assert!(
        plan.description().ready(),
        "{:?}",
        plan.description().blockers
    );
    let run = f
        .app
        .start_prepare_image(&f.project, plan.description(), &linker(), budget())
        .unwrap()
        .wait();
    assert_eq!(run.state, RunState::Completed, "{:?}", run.error);
    run.image.unwrap()
}
fn analyze(f: &Fixture, image: Option<PreparedImageId>) -> PublicationId {
    let decoder = blobray_backend_riscv::RiscvDecoder;
    let result = f
        .app
        .query(
            &f.project,
            app::ReadQuery::PlanInvestigation {
                request: InvestigationRequest {
                    image,
                    ..Default::default()
                },
                producer: FunctionProducer {
                    decoder: decoder.identity().into(),
                    semantics: decoder.semantic_identity().into(),
                },
            },
            budget(),
        )
        .unwrap();
    let app::QuerySummary::InvestigationPlan { plan } = result.summary() else {
        panic!("plan")
    };
    let run = f
        .app
        .start_analyze_project(
            &f.project,
            blobray_domain::InvestigationInput::Plan {
                plan: (**plan).clone(),
            },
            budget(),
        )
        .unwrap()
        .wait();
    assert_eq!(run.state, RunState::Completed, "{:?}", run.error);
    run.publication.unwrap()
}
fn cli(f: &Fixture, args: &[&str]) -> serde_json::Value {
    let mut command = Command::new(env!("CARGO_BIN_EXE_blobray"));
    command.args(["--format", "json"]);
    command
        .arg(args[0])
        .arg("--project")
        .arg(&f.project)
        .args(["--limit-mode", "watchdog"]);
    command.args(&args[1..]);
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    if output.stdout.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&output.stdout).unwrap()
    }
}
fn export(f: &Fixture, image: PreparedImageId) -> Vec<u8> {
    let mut output = f
        .app
        .query(
            &f.project,
            app::ReadQuery::Image {
                id: image,
                export: true,
            },
            budget(),
        )
        .unwrap();
    let path = f.dir.path().join("linked-export");
    output.export_image(&path, &|| false).unwrap();
    fs::read(path.join("image.elf")).unwrap()
}
#[test]
fn native_link_selection_requires_one_defined_candidate_and_keeps_order() {
    let f = fixture(false, true);
    let path = linker();
    let result = cli(
        &f,
        &[
            "link-plan",
            "--entry",
            "entry",
            "--entry-input",
            "0",
            "--inputs",
            "0,1",
            "--code-start",
            "0x10000000",
            "--data-start",
            "0x20000000",
            "--linker",
            path.to_str().unwrap(),
        ],
    );
    let plan: LinkPlanDescription = serde_json::from_value(result).unwrap();
    assert_eq!(plan.recipe.inputs, vec![0, 1]);
    assert_eq!(plan.recipe.entry, f.request.entry);
    // Two physical archive occurrences with the same name must be offered for
    // selection. No first-match or weak/strong preference is permitted here.
    let input = f.dir.path().join("duplicates.a");
    let entry = object(true);
    fs::write(
        &input,
        support::archive(&[(b"a.o", &entry), (b"b.o", &entry)], false),
    )
    .unwrap();
    f.app
        .import(
            &f.project,
            vec![app::ImportInput {
                role: "ambiguous".into(),
                path: input,
                expected: None,
            }],
            Target::Riscv32Ilp32,
            budget(),
        )
        .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_blobray"))
        .args([
            "--format",
            "json",
            "link-plan",
            "--entry",
            "entry",
            "--entry-input",
            "0",
            "--inputs",
            "0",
            "--code-start",
            "0x10000000",
            "--data-start",
            "0x20000000",
            "--linker",
        ])
        .arg(path)
        .arg("--project")
        .arg(&f.project)
        .args(["--limit-mode", "watchdog"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let candidates: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(candidates["summary"]["matches"], 2);
    assert_eq!(candidates["records"].as_array().unwrap().len(), 2);
}

#[test]
fn overlapping_image_segments_fail_without_publication_and_low_memory_is_diagnosed() {
    let f = fixture(false, true);
    let image = prepared(&f);
    let mut bytes = export(&f, image.clone());
    let mut tiny = budget();
    tiny.working_memory_bytes = Some(1024 * 1024);
    let decoder = blobray_backend_riscv::RiscvDecoder;
    let result = f.app.query(
        &f.project,
        app::ReadQuery::PlanInvestigation {
            request: InvestigationRequest {
                image: Some(image),
                ..Default::default()
            },
            producer: FunctionProducer {
                decoder: decoder.identity().into(),
                semantics: decoder.semantic_identity().into(),
            },
        },
        tiny,
    );
    assert!(matches!(
        result,
        Err(Error {
            code: ErrorCode::ResourceLimited,
            ..
        })
    ));
    // ELF32 PT_LOAD address mutation: create two overlapping nonempty mappings.
    let phoff = u32::from_le_bytes(bytes[28..32].try_into().unwrap()) as usize;
    let size = u16::from_le_bytes(bytes[42..44].try_into().unwrap()) as usize;
    let count = u16::from_le_bytes(bytes[44..46].try_into().unwrap()) as usize;
    let loads: Vec<_> = (0..count)
        .map(|i| phoff + i * size)
        .filter(|p| u32::from_le_bytes(bytes[*p..*p + 4].try_into().unwrap()) == 1)
        .collect();
    assert!(loads.len() >= 2);
    let address: [u8; 4] = bytes[loads[0] + 8..loads[0] + 12].try_into().unwrap();
    bytes[loads[1] + 8..loads[1] + 12].copy_from_slice(&address);
    let input = f.dir.path().join("overlap.elf");
    fs::write(&input, bytes).unwrap();
    f.app
        .import(
            &f.project,
            vec![app::ImportInput {
                role: "invalid".into(),
                path: input,
                expected: None,
            }],
            Target::Riscv32Ilp32,
            budget(),
        )
        .unwrap();
    let planned = f
        .app
        .query(
            &f.project,
            app::ReadQuery::PlanInvestigation {
                request: InvestigationRequest::default(),
                producer: FunctionProducer {
                    decoder: decoder.identity().into(),
                    semantics: decoder.semantic_identity().into(),
                },
            },
            budget(),
        )
        .unwrap();
    let app::QuerySummary::InvestigationPlan { plan } = planned.summary() else {
        panic!("plan")
    };
    let run = f
        .app
        .start_analyze_project(
            &f.project,
            blobray_domain::InvestigationInput::Plan {
                plan: (**plan).clone(),
            },
            budget(),
        )
        .unwrap()
        .wait();
    assert_eq!(run.state, RunState::Failed);
    assert_eq!(run.error.unwrap().code, ErrorCode::Integrity);
    assert!(run.publication.is_none());
    let output = f
        .app
        .query(&f.project, app::ReadQuery::Publications, budget())
        .unwrap();
    assert!(matches!(
        output.summary(),
        app::QuerySummary::Publications { count: 0 }
    ));
}

fn external_definition_fixture(rom: Vec<u8>) -> Fixture {
    let mut obj = Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
    let section = obj.add_section(Vec::new(), b".text.entry".to_vec(), SectionKind::Text);
    obj.append_section_data(
        section,
        &[0x97, 0, 0, 0, 0xe7, 0x80, 0, 0, 0x67, 0x80, 0, 0],
        4,
    );
    obj.add_symbol(symbol(
        b"entry",
        SymbolSection::Section(section),
        12,
        SymbolKind::Text,
    ));
    let helper = obj.add_symbol(symbol(
        b"helper",
        SymbolSection::Undefined,
        0,
        SymbolKind::Unknown,
    ));
    obj.add_relocation(
        section,
        Relocation {
            offset: 0,
            symbol: helper,
            addend: 0,
            flags: RelocationFlags::Elf {
                r_type: object::elf::R_RISCV_CALL,
            },
        },
    )
    .unwrap();
    let mut f = custom_fixture(
        vec![("entry.o", obj.write().unwrap()), ("rom.elf", rom)],
        0,
        0,
    );
    f.request.inputs = vec![0];
    f.request.layout.code.start = 0x30000000;
    f.request.layout.data.start = 0x31000000;
    let inventory = app::inventory(&f.project, None).unwrap();
    let helper = inventory.revision.inputs[1]
        .inventory
        .as_ref()
        .unwrap()
        .objects[0]
        .elf
        .as_ref()
        .unwrap()
        .symbols
        .iter()
        .find(|s| s.name.as_deref() == Some(b"helper"))
        .unwrap()
        .id
        .clone();
    f.request.companions = vec![EntrySelection {
        input: 1,
        symbol: helper,
    }];
    f
}

/// Entry calling `helper` and storing the addresses of `value` and, when
/// `missing` is set, of an undefined `missing`, linked against `rom`.
fn proposal_fixture(rom: Vec<u8>, missing: bool) -> Fixture {
    let mut obj = Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
    let section = obj.add_section(Vec::new(), b".text.entry".to_vec(), SectionKind::Text);
    obj.append_section_data(
        section,
        &[
            0x97, 0, 0, 0, 0xe7, 0x80, 0, 0, 0x67, 0x80, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        ],
        4,
    );
    obj.add_symbol(symbol(
        b"entry",
        SymbolSection::Section(section),
        20,
        SymbolKind::Text,
    ));
    let mut references = vec![(0, b"helper".as_slice(), object::elf::R_RISCV_CALL)];
    references.push((12, b"value", object::elf::R_RISCV_32));
    if missing {
        references.push((16, b"missing", object::elf::R_RISCV_32));
    }
    for (offset, name, r_type) in references {
        // Default visibility, as in vendor objects; a hidden undefined name
        // cannot be left for a companion.
        let target = obj.add_symbol(Symbol {
            scope: object::write::SymbolScope::Dynamic,
            ..symbol(name, SymbolSection::Undefined, 0, SymbolKind::Unknown)
        });
        obj.add_relocation(
            section,
            Relocation {
                offset,
                symbol: target,
                addend: 0,
                flags: RelocationFlags::Elf { r_type },
            },
        )
        .unwrap();
    }
    let mut f = custom_fixture(
        vec![("entry.o", obj.write().unwrap()), ("rom.elf", rom)],
        0,
        0,
    );
    f.request.inputs = vec![0];
    f.request.layout.code.start = 0x30000000;
    f.request.layout.data.start = 0x31000000;
    f
}

/// The proposal and whether the command succeeded (no unresolved name).
fn propose(f: &Fixture, candidate: &str) -> (bool, Option<CompanionProposal>) {
    let path = f.dir.path().join("link-request.json");
    fs::write(&path, serde_json::to_vec(&f.request).unwrap()).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_blobray"))
        .args(["--format", "json", "propose-companions", "--project"])
        .arg(&f.project)
        .args(["--limit-mode", "watchdog", "--request"])
        .arg(&path)
        .arg("--linker")
        .arg(linker())
        .args(["--candidate", candidate])
        .output()
        .unwrap();
    let document: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap_or_default();
    (
        output.status.success(),
        serde_json::from_value(document["summary"]["proposal"].clone()).ok(),
    )
}

#[test]
fn trial_link_proposes_function_and_data_companions_and_reports_the_rest() {
    let source = fixture(false, true);
    let image = prepared(&source);
    let rom = export(&source, image);
    let with_missing = proposal_fixture(rom.clone(), true);
    let (success, proposal) = propose(&with_missing, "1");
    let proposal = proposal.unwrap();
    assert!(!success, "an unresolved name fails the proposal");
    assert_eq!(proposal.unresolved, ["missing"]);
    let names: Vec<_> = proposal.resolved.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["helper", "value"]);
    assert!(proposal.resolved.iter().all(|c| c.selection.input == 1));
    // Without the undefined name, the proposal alone closes the image and the
    // retained request lists each exact companion.
    let mut f = proposal_fixture(rom, false);
    let (success, proposal) = propose(&f, "1");
    let proposal = proposal.unwrap();
    assert!(success && proposal.unresolved.is_empty());
    f.request.companions = proposal.resolved.into_iter().map(|c| c.selection).collect();
    let image = prepared(&f);
    let recipe = cli(&f, &["image", "--id", image.as_str()]);
    assert_eq!(
        recipe["summary"]["manifest"]["plan"]["recipe"]["companions"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    // A candidate that is also a link input is rejected before linking.
    assert_eq!(propose(&f, "0"), (false, None));
}

#[test]
fn address_definition_does_not_authorize_analyzing_its_tls_carrier() {
    let source = fixture(false, true);
    let image = prepared(&source);
    let mut rom = export(&source, image);
    let old = u32::from_le_bytes(rom[28..32].try_into().unwrap()) as usize;
    let count = u16::from_le_bytes(rom[44..46].try_into().unwrap());
    let headers = rom[old..old + usize::from(count) * 32].to_vec();
    let new = rom.len() as u32;
    rom[28..32].copy_from_slice(&new.to_le_bytes());
    rom[44..46].copy_from_slice(&(count + 1).to_le_bytes());
    rom.extend_from_slice(&headers);
    for value in [object::elf::PT_TLS, 0, 0, 0, 0, 0, 4, 4] {
        rom.extend_from_slice(&value.to_le_bytes());
    }
    let f = external_definition_fixture(rom);
    let image = prepared(&f);
    assert!(!export(&f, image).is_empty());
    let run = f
        .app
        .start_analyze_function(
            &f.project,
            FunctionRequest {
                revision: Some(f.revision.clone()),
                source: FunctionSource::Input { input: 1 },
                selector: f.request.companions[0].symbol.clone().into(),
                research: None,
                extent: None,
            },
            budget(),
        )
        .unwrap()
        .wait();
    assert_eq!(run.state, RunState::Failed);
    assert_eq!(run.error.unwrap().code, ErrorCode::Incompatible);
    assert!(run.analysis.is_none());
}

#[test]
fn data_image_addresses_export_file_backing_and_reject_unmapped_ranges() {
    let f = fixture(false, true);
    let image = prepared(&f);
    let description = f
        .app
        .query(
            &f.project,
            app::ReadQuery::Image {
                id: image.clone(),
                export: false,
            },
            budget(),
        )
        .unwrap();
    let app::QuerySummary::Image { manifest, .. } = description.summary() else {
        panic!()
    };
    let mut request = DataRequest {
        pointer_table: None,
        occurrence: KnowledgeOccurrence {
            revision: manifest.plan.recipe.revision.clone(),
            source: FunctionSource::Image {
                image: image.clone(),
            },
            object: ObjectId {
                artifact: manifest.elf.clone(),
                location: ObjectLocation::Standalone,
            },
            symbol: None,
        },
        ranges: vec![DataSelector::Image {
            address: manifest.entry,
            length: 4,
        }],
        analyses: Vec::new(),
    };
    let request_file = f.dir.path().join("data.json");
    fs::write(&request_file, serde_json::to_vec(&request).unwrap()).unwrap();
    let destination = f.dir.path().join("data-export");
    cli(
        &f,
        &[
            "data",
            "--request",
            request_file.to_str().unwrap(),
            "--output",
            destination.to_str().unwrap(),
        ],
    );
    let exported: DataManifest =
        serde_json::from_slice(&fs::read(destination.join("manifest.json")).unwrap()).unwrap();
    let span = &exported.spans[0];
    assert_eq!(span.image_address, Some(manifest.entry));
    assert_ne!(span.image_address, Some(span.file_range.start));
    let raw = fs::read(destination.join("object.elf")).unwrap();
    assert_eq!(ArtifactId::of_bytes(&raw), manifest.elf);
    assert_eq!(
        fs::read(destination.join("data.bin")).unwrap(),
        &raw[span.file_range.start as usize..span.file_range.start as usize + 4]
    );
    // A read-only section inside a writable PT_LOAD is initialization, too.
    let mut writable = raw.clone();
    let phoff = u32::from_le_bytes(raw[28..32].try_into().unwrap()) as usize;
    let stride = u16::from_le_bytes(raw[42..44].try_into().unwrap()) as usize;
    let count = u16::from_le_bytes(raw[44..46].try_into().unwrap()) as usize;
    for index in 0..count {
        let at = phoff + index * stride;
        let kind = u32::from_le_bytes(raw[at..at + 4].try_into().unwrap());
        let flags = u32::from_le_bytes(raw[at + 24..at + 28].try_into().unwrap());
        if kind == object::elf::PT_LOAD && flags & object::elf::PF_X != 0 {
            writable[at + 24..at + 28]
                .copy_from_slice(&(object::elf::PF_R | object::elf::PF_W).to_le_bytes());
        }
    }
    let imported = f.dir.path().join("writable.elf");
    fs::write(&imported, &writable).unwrap();
    f.app
        .import(
            &f.project,
            vec![app::ImportInput {
                role: "initial".into(),
                path: imported,
                expected: None,
            }],
            Target::Riscv32Ilp32,
            budget(),
        )
        .unwrap();
    let revision = app::inventory(&f.project, None).unwrap().revision_id;
    let mut initial = request.clone();
    initial.occurrence.revision = revision;
    initial.occurrence.source = FunctionSource::Input { input: 0 };
    initial.occurrence.object.artifact = ArtifactId::of_bytes(&writable);
    let out = f
        .app
        .query(
            &f.project,
            app::ReadQuery::Data { request: initial },
            budget(),
        )
        .unwrap();
    let app::QuerySummary::Data { manifest: initial } = out.summary() else {
        panic!()
    };
    assert!(initial.spans[0].writable);
    request.ranges = vec![DataSelector::Image {
        address: 0xffff_f000,
        length: 4,
    }];
    assert!(
        f.app
            .query(&f.project, app::ReadQuery::Data { request }, budget())
            .is_err()
    );
}

#[test]
fn generic_image_table_review_and_export_validate_the_same_physical_symbol() {
    use object::ObjectSection as _;
    for pointers in [false, true] {
        let f = fixture(true, true);
        let image = prepared(&f);
        let raw = export(&f, image.clone());
        let elf = object::File::parse(raw.as_slice()).unwrap();
        let symbol = elf.symbol_by_name("value").unwrap();
        let section = elf
            .section_by_index(symbol.section_index().unwrap())
            .unwrap();
        let offset = symbol.address() - section.address();
        let payload = ArtifactId::of_bytes(&raw);
        let object = ObjectId {
            artifact: payload.clone(),
            location: ObjectLocation::Standalone,
        };
        let occurrence = KnowledgeOccurrence {
            revision: f.revision.clone(),
            source: FunctionSource::Image { image },
            object: object.clone(),
            symbol: Some(SymbolId {
                object,
                table: SymbolTableKind::Static,
                table_section: elf.section_by_name(".symtab").unwrap().index().0 as u32,
                index: symbol.index().0 as u64,
            }),
        };
        let selector = DataSelector::Section {
            section: section.index().0 as u32,
            offset,
            length: 4,
        };
        let layout = IntegerTable {
            encoding: IntegerEncoding {
                width: 4,
                signed: false,
                byte_order: DataByteOrder::Little,
            },
            count: 1,
            stride: 4,
        };
        let proposal = KnowledgeProposal {
            subject: "data.value".to_owned().try_into().unwrap(),
            occurrence: occurrence.clone(),
            claim: if pointers {
                KnowledgeClaim::PointerTable {
                    selector: selector.clone(),
                    layout: PointerTable {
                        count: 1,
                        stride: 4,
                    },
                    purpose: "fixture".into(),
                    applicability: "exact image".into(),
                }
            } else {
                KnowledgeClaim::IntegerTable {
                    selector: selector.clone(),
                    layout: layout.clone(),
                    purpose: "fixture".into(),
                    applicability: "exact image".into(),
                }
            },
            evidence: vec![EvidenceRef::Source {
                payload,
                range: CodeRange {
                    start: section.file_range().unwrap().0 + offset,
                    length: 4,
                },
            }],
            note: None,
        };
        for kind in 0..4 {
            let mut bad = proposal.clone();
            let sym = bad.occurrence.symbol.as_mut().unwrap();
            match kind {
                0 => sym.index = u64::MAX,
                1 => sym.table = SymbolTableKind::Dynamic,
                2 => sym.table_section = u32::MAX,
                _ => sym.object.artifact = ArtifactId::of_bytes(b"another object"),
            }
            let change = KnowledgeChange {
                expected_base: None,
                actor: "test".into(),
                reason: "invalid physical identity".into(),
                action: KnowledgeAction::Propose {
                    proposal: bad.clone(),
                },
            };
            assert!(
                f.app
                    .query(
                        &f.project,
                        app::ReadQuery::ValidateKnowledge {
                            change: change.clone()
                        },
                        budget()
                    )
                    .is_err()
            );
            match f.app.start_knowledge(&f.project, &change, budget()) {
                Ok(handle) => {
                    let run = handle.wait();
                    assert_eq!(run.state, RunState::Failed, "{run:?}");
                    assert!(run.knowledge.is_none());
                }
                Err(error) => assert_eq!(error.code, ErrorCode::InvalidRequest),
            }
            let specialized = f
                .app
                .start_propose_data(
                    &f.project,
                    DataProposalRequest {
                        occurrence: bad.occurrence,
                        analyses: vec![],
                        subject: bad.subject,
                        selector: selector.clone(),
                        layout: if pointers {
                            DataLayout::Pointers(PointerTable {
                                count: 1,
                                stride: 4,
                            })
                        } else {
                            layout.clone().into()
                        },
                        purpose: "fixture".into(),
                        applicability: "exact image".into(),
                        expected_base: None,
                        actor: "test".into(),
                        reason: "invalid identity".into(),
                    },
                    budget(),
                )
                .unwrap()
                .wait();
            assert_eq!(specialized.state, RunState::Failed, "{specialized:?}");
            assert!(specialized.knowledge.is_none());
            assert!(
                cli(&f, &["knowledge", "show"])["records"]
                    .as_array()
                    .unwrap()
                    .is_empty()
            );
        }
        let proposed = f
            .app
            .start_knowledge(
                &f.project,
                &KnowledgeChange {
                    expected_base: None,
                    actor: "test".into(),
                    reason: "valid data symbol".into(),
                    action: KnowledgeAction::Propose { proposal },
                },
                budget(),
            )
            .unwrap()
            .wait();
        assert_eq!(proposed.state, RunState::Completed, "{proposed:?}");
        let entries = cli(&f, &["knowledge", "show"]);
        let assertion: AssertionId = entries["records"][0]["value"]["id"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
        let reviewed = f
            .app
            .start_knowledge(
                &f.project,
                &KnowledgeChange {
                    expected_base: proposed.knowledge,
                    actor: "test".into(),
                    reason: "checked data identity".into(),
                    action: KnowledgeAction::Review {
                        assertion: assertion.clone(),
                        decision: ReviewDecision::Accept,
                        supersedes: None,
                    },
                },
                budget(),
            )
            .unwrap()
            .wait();
        assert_eq!(reviewed.state, RunState::Completed, "{reviewed:?}");
        let mut output = f
            .app
            .query(
                &f.project,
                app::ReadQuery::ReviewedData {
                    revision: reviewed.knowledge.unwrap(),
                    assertion,
                },
                budget(),
            )
            .unwrap();
        let destination = f.dir.path().join("reviewed-table");
        output.export_data(&destination, &|| false).unwrap();
        assert_eq!(
            fs::read(destination.join("data.bin")).unwrap(),
            [0x78, 0x56, 0x34, 0x12]
        );
    }
}

#[test]
fn image_data_relocation_overlap_uses_section_relative_coordinates() {
    use object::ObjectSection as _;
    let mut obj = Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
    let code = obj.add_section(Vec::new(), b".text.entry".to_vec(), SectionKind::Text);
    let data = obj.add_section(
        Vec::new(),
        b".rodata.values".to_vec(),
        SectionKind::ReadOnlyData,
    );
    obj.append_section_data(
        code,
        &[0x37, 0x05, 0, 0, 0x13, 0x05, 0x05, 0, 0x67, 0x80, 0, 0],
        4,
    );
    obj.append_section_data(data, &[0, 0, 0, 0, 0xfb, 0xff, 3, 0], 4);
    let entry = obj.add_symbol(symbol(
        b"entry",
        SymbolSection::Section(code),
        12,
        SymbolKind::Text,
    ));
    let table = obj.add_symbol(symbol(
        b"table",
        SymbolSection::Section(data),
        8,
        SymbolKind::Data,
    ));
    for (section, offset, target, r_type) in [
        (code, 0, table, object::elf::R_RISCV_HI20),
        (code, 4, table, object::elf::R_RISCV_LO12_I),
        (data, 0, entry, object::elf::R_RISCV_32),
    ] {
        obj.add_relocation(
            section,
            Relocation {
                offset,
                symbol: target,
                addend: 0,
                flags: RelocationFlags::Elf { r_type },
            },
        )
        .unwrap();
    }
    let f = custom_fixture(vec![("data.o", obj.write().unwrap())], 0, 0);
    let image = prepared(&f);
    let bytes = export(&f, image.clone());
    let elf = object::File::parse(bytes.as_slice()).unwrap();
    let symbol = elf.symbol_by_name("table").unwrap();
    let section = elf
        .section_by_index(symbol.section_index().unwrap())
        .unwrap();
    assert!(section.address() > section.size());
    for offset in [0, 4] {
        let request = DataRequest {
            pointer_table: Some(PointerTable {
                count: 1,
                stride: 4,
            }),
            occurrence: KnowledgeOccurrence {
                revision: f.revision.clone(),
                source: FunctionSource::Image {
                    image: image.clone(),
                },
                object: ObjectId {
                    artifact: ArtifactId::of_bytes(&bytes),
                    location: ObjectLocation::Standalone,
                },
                symbol: None,
            },
            ranges: vec![DataSelector::Image {
                address: symbol.address() + offset,
                length: 4,
            }],
            analyses: vec![],
        };
        let mut output = f
            .app
            .query(&f.project, app::ReadQuery::Data { request }, budget())
            .unwrap();
        let path = f.dir.path().join(format!("data-{offset}"));
        output.export_data(&path, &|| false).unwrap();
        let manifest: DataManifest =
            serde_json::from_slice(&fs::read(path.join("manifest.json")).unwrap()).unwrap();
        assert_eq!(
            manifest.spans[0].overlapping_relocations,
            u64::from(offset == 0)
        );
        assert_eq!(manifest.spans[0].unknown_relocation_extents, 0);
        assert_eq!(manifest.spans[0].section_relocations, 1);
        let records: Vec<DataRecord> = fs::read_to_string(path.join("records.jsonl"))
            .unwrap()
            .lines()
            .map(|s| serde_json::from_str(s).unwrap())
            .collect();
        let value = records
            .iter()
            .find_map(|r| match r {
                DataRecord::Pointer { value, .. } => Some(value),
                _ => None,
            })
            .unwrap();
        if offset == 0 {
            assert!(
                matches!(value, PointerValue::DefinedSymbol { symbol, addend: 0 } if symbol.index == elf.symbol_by_name("entry").unwrap().index().0 as u64)
            );
            assert_eq!(manifest.pointers.as_ref().unwrap().defined_symbols, 1);
        } else {
            assert_eq!(
                value,
                &PointerValue::Address {
                    value: 0x0003fffb,
                    image_address: true
                }
            );
            assert_eq!(manifest.pointers.as_ref().unwrap().addresses, 1);
        }
        if offset == 4 {
            assert_eq!(fs::read(path.join("data.bin")).unwrap(), [0xfb, 0xff, 3, 0]);
        }
    }
}

#[test]
fn reviewed_image_code_range_keeps_vma_identity_and_unions_symbol_coverage() {
    use object::ObjectSection as _;
    let f = fixture(false, true);
    let image = prepared(&f);
    let bytes = export(&f, image.clone());
    let elf = object::File::parse(bytes.as_slice()).unwrap();
    let helper = elf
        .symbols()
        .find(|s| s.name().ok() == Some("helper"))
        .unwrap();
    let section = elf
        .section_by_index(helper.section_index().unwrap())
        .unwrap();
    let extent = CodeRange {
        start: helper.address(),
        length: helper.size(),
    };
    let file_range = CodeRange {
        start: section.file_range().unwrap().0 + helper.address() - section.address(),
        length: helper.size(),
    };
    let object = ObjectId {
        artifact: ArtifactId::of_bytes(&bytes),
        location: ObjectLocation::Standalone,
    };
    let selector = FunctionSelector::Range {
        object: object.clone(),
        section: section.index().0 as u32,
        extent,
    };
    let source = FunctionSource::Image {
        image: image.clone(),
    };
    let run = f
        .app
        .start_analyze_function(
            &f.project,
            FunctionRequest {
                revision: Some(f.revision.clone()),
                source: source.clone(),
                selector: selector.clone(),
                research: None,
                extent: None,
            },
            budget(),
        )
        .unwrap()
        .wait();
    assert_eq!(run.state, RunState::Completed, "{run:?}");
    let analysis = run.analysis.unwrap();
    let proposed = f
        .app
        .start_knowledge(
            &f.project,
            &KnowledgeChange {
                expected_base: None,
                actor: "test".into(),
                reason: "exact image code".into(),
                action: KnowledgeAction::Propose {
                    proposal: KnowledgeProposal {
                        subject: "helper-boundary".to_owned().try_into().unwrap(),
                        occurrence: KnowledgeOccurrence {
                            revision: f.revision.clone(),
                            source,
                            object: object.clone(),
                            symbol: None,
                        },
                        claim: KnowledgeClaim::ExecutableRange {
                            section: section.index().0 as u32,
                            extent,
                        },
                        evidence: vec![
                            EvidenceRef::Source {
                                payload: object.artifact,
                                range: file_range,
                            },
                            EvidenceRef::Analysis {
                                analysis: analysis.clone(),
                                record: None,
                            },
                        ],
                        note: None,
                    },
                },
            },
            budget(),
        )
        .unwrap()
        .wait();
    assert_eq!(proposed.state, RunState::Completed, "{proposed:?}");
    let entries = cli(&f, &["knowledge", "show"]);
    let assertion: AssertionId = entries["records"][0]["value"]["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let accepted = f
        .app
        .start_knowledge(
            &f.project,
            &KnowledgeChange {
                expected_base: proposed.knowledge,
                actor: "test".into(),
                reason: "reviewed image bytes".into(),
                action: KnowledgeAction::Review {
                    assertion: assertion.clone(),
                    decision: ReviewDecision::Accept,
                    supersedes: None,
                },
            },
            budget(),
        )
        .unwrap()
        .wait();
    assert_eq!(accepted.state, RunState::Completed, "{accepted:?}");
    let baseline = analyze(&f, Some(image.clone()));
    let baseline_coverage = cli(&f, &["coverage", "--id", baseline.as_str()]);
    let decoder = blobray_backend_riscv::RiscvDecoder;
    let run = f
        .app
        .start_analyze_project(
            &f.project,
            InvestigationInput::Automatic {
                request: InvestigationRequest {
                    image: Some(image),
                    reviewed_extents: vec![ReviewedExtent {
                        revision: accepted.knowledge.unwrap(),
                        assertion,
                    }],
                    ..Default::default()
                },
                producer: FunctionProducer {
                    decoder: decoder.identity().into(),
                    semantics: decoder.semantic_identity().into(),
                },
            },
            budget(),
        )
        .unwrap()
        .wait();
    assert_eq!(run.state, RunState::Completed, "{run:?}");
    let coverage = cli(&f, &["coverage", "--id", run.publication.unwrap().as_str()]);
    assert_eq!(
        coverage["summary"]["extents"]["selected_extent_bytes"],
        baseline_coverage["summary"]["extents"]["selected_extent_bytes"]
    );
    assert_eq!(coverage["summary"]["extents"]["unknowns"], 0);
    fs::remove_file(f.dir.path().join("entry.a")).unwrap();
    fs::remove_file(f.dir.path().join("helper.a")).unwrap();
    let output = f
        .app
        .query(
            &f.project,
            app::ReadQuery::Analysis {
                id: analysis,
                export: false,
            },
            budget(),
        )
        .unwrap();
    let app::QuerySummary::Analysis { manifest, .. } = output.summary() else {
        panic!("analysis")
    };
    assert_eq!(manifest.recipe.selector, selector);
    assert_eq!(manifest.recipe.extent, extent);
    assert_eq!(manifest.recipe.address_space, CodeAddressSpace::Image);
}

#[test]
fn image_interface_data_roots_use_the_same_physical_validation_during_review() {
    use object::ObjectSection as _;
    let f = fixture(true, true);
    let image = prepared(&f);
    let bytes = export(&f, image.clone());
    let elf = object::File::parse(bytes.as_slice()).unwrap();
    let value = elf.symbol_by_name("value").unwrap();
    let payload = ArtifactId::of_bytes(&bytes);
    let object = ObjectId {
        artifact: payload.clone(),
        location: ObjectLocation::Standalone,
    };
    let symbol = SymbolId {
        object: object.clone(),
        table: SymbolTableKind::Static,
        table_section: elf.section_by_name(".symtab").unwrap().index().0 as u32,
        index: value.index().0 as u64,
    };
    let proposal:KnowledgeProposal=serde_json::from_value(serde_json::json!({
        "subject":"image.interface", "occurrence":{"revision":f.revision,"source":{"kind":"image","image":image},"object":object,"symbol":symbol},
        "claim":{"kind":"interface","contract":{
            "root":{"kind":"symbol","symbol":symbol,"addend":0},"path":[],"layout_version":"fixture/1","layout_bytes":4,"pointer_bytes":4,"abi":"riscv-integer","index_domains":[],"guards":[{"kind":"captured-payload","payload":payload}],
            "slots":[{"offset":0,"name":"slot","semantic":null,"signature":{"arguments":[],"result":{"kind":"void"},"variadic":false}}],"purpose":"physical data root","applicability":"declared interpretation only"
        }}, "evidence":[{"kind":"source","payload":payload,"range":{"start":0,"length":bytes.len()}}],"note":null
    })).unwrap();
    let change = |proposal| KnowledgeChange {
        expected_base: None,
        actor: "test".into(),
        reason: "physical identity".into(),
        action: KnowledgeAction::Propose { proposal },
    };
    for which in 0..3 {
        let mut bad = proposal.clone();
        bad.occurrence.symbol = None;
        let KnowledgeClaim::Interface { contract } = &mut bad.claim else {
            panic!()
        };
        let AccessRoot::Symbol { symbol, .. } = &mut contract.root else {
            panic!()
        };
        match which {
            0 => symbol.index = u64::MAX,
            1 => symbol.table = SymbolTableKind::Dynamic,
            _ => symbol.table_section = u32::MAX,
        }
        let failed = f
            .app
            .start_knowledge(&f.project, &change(bad), budget())
            .unwrap()
            .wait();
        assert_eq!(failed.state, RunState::Failed, "{failed:?}");
        assert!(failed.knowledge.is_none());
        assert!(
            cli(&f, &["knowledge", "show"])["records"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }
    let proposed = f
        .app
        .start_knowledge(&f.project, &change(proposal), budget())
        .unwrap()
        .wait();
    assert_eq!(proposed.state, RunState::Completed, "{proposed:?}");
    let entries = cli(&f, &["knowledge", "show"]);
    let assertion = entries["records"][0]["value"]["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let accepted = f
        .app
        .start_knowledge(
            &f.project,
            &KnowledgeChange {
                expected_base: proposed.knowledge,
                actor: "test".into(),
                reason: "captured data identity".into(),
                action: KnowledgeAction::Review {
                    assertion,
                    decision: ReviewDecision::Accept,
                    supersedes: None,
                },
            },
            budget(),
        )
        .unwrap()
        .wait();
    assert_eq!(accepted.state, RunState::Completed, "{accepted:?}");
    assert_eq!(
        cli(&f, &["knowledge", "show"])["records"][0]["value"]["state"],
        "accepted"
    );
}

#[test]
fn saved_trace_follows_a_linked_call_and_tail_without_promoting_may_effects() {
    let mut obj = Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
    // Preserve this entry's return link in s0; the supplied leaf does not modify it.
    let definitions: [(&str, &[u32]); 3] = [
        (
            "entry",
            &[
                0x00008413, 0x60000537, 0x00700593, 0x000000ef, 0x60000637, 0x00a62223, 0x00040093,
                0x00008067,
            ],
        ),
        ("tail", &[0x60000537, 0x00700593, 0x0000006f]),
        ("leaf", &[0x00b52023, 0x00158513, 0x00008067]),
    ];
    let mut sections = Vec::new();
    let mut symbols = Vec::new();
    for (name, code) in definitions {
        let section = obj.add_section(
            vec![],
            format!(".text.{name}").into_bytes(),
            SectionKind::Text,
        );
        obj.append_section_data(
            section,
            &code
                .iter()
                .flat_map(|w| w.to_le_bytes())
                .collect::<Vec<_>>(),
            4,
        );
        symbols.push(obj.add_symbol(symbol(
            name.as_bytes(),
            SymbolSection::Section(section),
            (code.len() * 4) as u64,
            SymbolKind::Text,
        )));
        sections.push(section);
    }
    for (from, offset) in [(0, 12), (1, 8)] {
        obj.add_relocation(
            sections[from],
            Relocation {
                offset,
                symbol: symbols[2],
                addend: 0,
                flags: RelocationFlags::Elf {
                    r_type: object::elf::R_RISCV_JAL,
                },
            },
        )
        .unwrap();
    }
    let mut f = custom_fixture(vec![("trace.o", obj.write().unwrap())], 0, 0);
    let snapshot = app::inventory(&f.project, None).unwrap();
    let tail = snapshot.revision.inputs[0]
        .inventory
        .as_ref()
        .unwrap()
        .objects[0]
        .elf
        .as_ref()
        .unwrap()
        .symbols
        .iter()
        .find(|s| s.name.as_deref() == Some(b"tail"))
        .unwrap()
        .id
        .clone();
    f.request.roots.push(EntrySelection {
        input: 0,
        symbol: tail,
    });
    let image = prepared(&f);
    let publication = analyze(&f, Some(image));
    let entries = cli(&f, &["functions", "--id", publication.as_str()]);
    let find = |name: &str| -> FunctionAnalysisId {
        entries["records"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["value"]["entry"]["name"] == serde_json::json!(name.as_bytes()))
            .unwrap()["value"]["outcome"]["analysis"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap()
    };
    let entry = find("entry");
    let build = IrBuildRequest {
        scope: NavigationScope {
            revision: app::inventory(&f.project, None).unwrap().revision_id,
            publications: vec![publication.clone()],
            analyses: vec![],
            knowledge: None,
        },
        profiles: vec![IrProfile {
            name: "calls".into(),
            roots: IrRoots::All,
            include_reachable: true,
        }],
    };
    let built = f
        .app
        .start_build_ir(&f.project, build, budget())
        .unwrap()
        .wait();
    assert_eq!(built.state, RunState::Completed, "{built:?}");
    let target = TraceTarget {
        ir: built.semantic_ir.unwrap(),
        profile: "calls".into(),
        entry,
        abi: CallAbi::RiscvInteger,
        registers: vec![],
    };
    let mut q = TraceRequest {
        left: target.clone(),
        right: Some(target),
        observation: TraceObservation {
            ranges: vec![ImageRegion {
                start: 0x60000000,
                length: 16,
            }],
            fences: true,
        },
    };
    let path = f.dir.path().join("trace-request.json");
    fs::write(&path, serde_json::to_vec(&q).unwrap()).unwrap();
    let output = cli(&f, &["trace", "--request", path.to_str().unwrap()]);
    assert_eq!(output["summary"]["summary"]["verdict"], "MATCH", "{output}");
    assert_eq!(output["summary"]["summary"]["left"]["invocations"], 2);
    let events: Vec<_> = output["records"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| &r["value"])
        .filter(|r| r["kind"] == "event" && r["side"] == "left")
        .map(|r| r["event"].clone())
        .collect();
    assert_eq!(events.len(), 2, "{output}");
    assert_eq!(events[0]["address"], 0x60000000u32);
    assert_eq!(events[0]["value"]["value"], 7);
    assert_eq!(events[1]["address"], 0x60000004u32);
    assert_eq!(events[1]["value"]["value"], 8);
    q.left.entry = find("tail");
    q.right = Some(q.left.clone());
    fs::write(&path, serde_json::to_vec(&q).unwrap()).unwrap();
    let tail = cli(&f, &["trace", "--request", path.to_str().unwrap()]);
    assert_eq!(tail["summary"]["summary"]["verdict"], "MATCH", "{tail}");
    assert_eq!(tail["summary"]["summary"]["left"]["invocations"], 2);
    assert_eq!(tail["summary"]["summary"]["left"]["events"], 1);
    fs::remove_file(f.dir.path().join("trace.o")).unwrap();
    assert_eq!(
        cli(&f, &["trace", "--request", path.to_str().unwrap()]),
        tail
    );
}
