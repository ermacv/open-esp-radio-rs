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
        .start_analyze_project(&f.project, plan, budget())
        .unwrap()
        .wait();
    assert_eq!(run.state, RunState::Completed, "{:?}", run.error);
    run.publication.unwrap()
}
fn cli(f: &Fixture, args: &[&str]) -> serde_json::Value {
    let mut command = Command::new(env!("CARGO_BIN_EXE_blobray-next"));
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
fn prepared_and_imported_images_share_analysis_and_cli_navigation() {
    let f = fixture(false, true);
    let image = prepared(&f);
    let mappings = cli(&f, &["image", "--id", image.as_str()]);
    assert!(
        mappings["records"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["kind"] == "mapping" && r["value"]["exact"] == true)
    );
    let bytes = export(&f, image.clone());
    let elf = object::File::parse(bytes.as_slice()).unwrap();
    let helper = elf
        .symbols()
        .find(|s| s.name().ok() == Some("helper"))
        .unwrap()
        .address();
    fs::remove_file(f.dir.path().join("entry.a")).unwrap();
    fs::remove_file(f.dir.path().join("helper.a")).unwrap();
    let publication = analyze(&f, Some(image.clone()));
    let functions = cli(
        &f,
        &["functions", "--id", publication.as_str(), "--name", "entry"],
    );
    assert_eq!(functions["records"].as_array().unwrap().len(), 1);
    let calls = cli(
        &f,
        &[
            "calls",
            "--id",
            publication.as_str(),
            "--callee",
            &format!("0x{helper:x}"),
        ],
    );
    assert_eq!(calls["records"].as_array().unwrap().len(), 1, "{calls}");
    let saved = serde_json::to_string(&calls).unwrap();
    assert!(saved.contains("image-address"));
    assert!(saved.contains(image.as_str()));
    // Import identical linked bytes as an ordinary captured input, then analyze
    // current without constructing a JSON request or a saved plan.
    let input = f.dir.path().join("linked.elf");
    fs::write(&input, &bytes).unwrap();
    f.app
        .import(
            &f.project,
            vec![app::ImportInput {
                role: "firmware".into(),
                path: input.clone(),
                expected: None,
            }],
            Target::Riscv32Ilp32,
            budget(),
        )
        .unwrap();
    fs::remove_file(input).unwrap();
    let auto = cli(&f, &["analyze-project"]);
    let imported = auto["run"]["publication"].as_str().unwrap();
    let calls = cli(
        &f,
        &[
            "calls",
            "--id",
            imported,
            "--callee",
            &format!("0x{helper:x}"),
        ],
    );
    assert_eq!(calls["records"].as_array().unwrap().len(), 1, "{calls}");
    // A prepared image still selects its own original revision after current changes.
    let repeat = analyze(&f, Some(image));
    assert_eq!(repeat, publication);
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
    let output = Command::new(env!("CARGO_BIN_EXE_blobray-next"))
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
fn declared_float_abi_does_not_block_integer_research_or_hide_incompatible_link_inputs() {
    let mut entry = object(true);
    let mut helper = object(false);
    for bytes in [&mut entry, &mut helper] {
        bytes[36..40].copy_from_slice(&object::elf::EF_RISCV_FLOAT_ABI_DOUBLE.to_le_bytes());
    }
    let f = custom_fixture(vec![("entry.o", entry.clone()), ("helper.o", helper)], 0, 0);
    let image = prepared(&f);
    let summary = cli(&f, &["image", "--id", image.as_str()]);
    assert_eq!(summary["summary"]["manifest"]["abi"], "ilp32d");
    let publication = analyze(&f, Some(image));
    let calls = cli(&f, &["calls", "--id", publication.as_str()]);
    assert_eq!(calls["records"].as_array().unwrap().len(), 1);
    let analysis = calls["records"][0]["value"]["analysis"].as_str().unwrap();
    let function = cli(&f, &["analysis", "--id", analysis]);
    assert_eq!(function["summary"]["manifest"]["recipe"]["abi"], "ilp32d");
    // LLD owns compatibility of the actual selected input ABIs. A mismatch is
    // a retained linker failure, never a forced soft-float reinterpretation.
    let f = custom_fixture(vec![("entry.o", entry), ("helper.o", object(false))], 0, 0);
    let plan = f
        .app
        .link_plan(&f.project, f.request.clone(), &linker(), budget())
        .unwrap();
    assert!(plan.description().ready());
    let run = f
        .app
        .start_prepare_image(&f.project, plan.description(), &linker(), budget())
        .unwrap()
        .wait();
    assert_eq!(run.state, RunState::Failed);
    assert_eq!(run.error.unwrap().code, ErrorCode::LinkFailed);
    assert!(run.image.is_none());
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
        .start_analyze_project(&f.project, plan, budget())
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

#[test]
fn executable_without_section_tables_retains_unknown_code_coverage() {
    let f = fixture(false, true);
    let image = prepared(&f);
    let mut bytes = export(&f, image);
    bytes[32..36].fill(0); // ELF32 e_shoff
    bytes[48..52].fill(0); // e_shnum/e_shstrndx
    let input = f.dir.path().join("stripped.elf");
    fs::write(&input, bytes).unwrap();
    f.app
        .import(
            &f.project,
            vec![app::ImportInput {
                role: "stripped".into(),
                path: input,
                expected: None,
            }],
            Target::Riscv32Ilp32,
            budget(),
        )
        .unwrap();
    let publication = analyze(&f, None);
    let result = cli(&f, &["investigation", "--id", publication.as_str()]);
    let coverage = &result["summary"]["manifest"]["coverage"];
    assert_eq!(coverage["functions"], 0);
    assert!(coverage["gaps"].as_u64().unwrap() > 0);
}

fn data_object() -> Vec<u8> {
    let mut obj = Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
    let text = obj.add_section(Vec::new(), b".text.entry".to_vec(), SectionKind::Text);
    let ro = obj.add_section(
        Vec::new(),
        b".rodata.values".to_vec(),
        SectionKind::ReadOnlyData,
    );
    let rw = obj.add_section(Vec::new(), b".data.values".to_vec(), SectionKind::Data);
    obj.append_section_data(ro, &[0x80, 0xff, 0xff, 0xff], 4);
    obj.append_section_data(rw, &[0x2a, 0, 0, 0], 4);
    let ro_symbol = obj.add_symbol(symbol(
        b"constants",
        SymbolSection::Section(ro),
        4,
        SymbolKind::Data,
    ));
    let rw_symbol = obj.add_symbol(symbol(
        b"mutable",
        SymbolSection::Section(rw),
        4,
        SymbolKind::Data,
    ));
    // lui t0,ro; lb/lbu/lh/lhu/lw; lui t1,rw; lw; jalr x0,t1,0
    let code: Vec<u8> = [
        0x000002b7u32,
        0x00028503,
        0x0002c583,
        0x00029603,
        0x0002d683,
        0x0002a703,
        0x00000337,
        0x00032783,
        0x600003b7,
        0x0003a803,
        0x00078067,
    ]
    .iter()
    .flat_map(|w| w.to_le_bytes())
    .collect();
    obj.append_section_data(text, &code, 4);
    obj.add_symbol(symbol(
        b"entry",
        SymbolSection::Section(text),
        code.len() as u64,
        SymbolKind::Text,
    ));
    for (offset, symbol, kind) in [
        (0, ro_symbol, object::elf::R_RISCV_HI20),
        (4, ro_symbol, object::elf::R_RISCV_LO12_I),
        (8, ro_symbol, object::elf::R_RISCV_LO12_I),
        (12, ro_symbol, object::elf::R_RISCV_LO12_I),
        (16, ro_symbol, object::elf::R_RISCV_LO12_I),
        (20, ro_symbol, object::elf::R_RISCV_LO12_I),
        (24, rw_symbol, object::elf::R_RISCV_HI20),
        (28, rw_symbol, object::elf::R_RISCV_LO12_I),
    ] {
        obj.add_relocation(
            text,
            Relocation {
                offset,
                symbol,
                addend: 0,
                flags: RelocationFlags::Elf { r_type: kind },
            },
        )
        .unwrap();
    }
    obj.write().unwrap()
}
#[test]
fn image_loads_use_readonly_bytes_and_correct_signedness() {
    let mut f = fixture(false, false);
    let input = f.dir.path().join("values.o");
    fs::write(&input, data_object()).unwrap();
    f.app
        .import(
            &f.project,
            vec![app::ImportInput {
                role: "code".into(),
                path: input,
                expected: None,
            }],
            Target::Riscv32Ilp32,
            budget(),
        )
        .unwrap();
    let snapshot = app::inventory(&f.project, None).unwrap();
    f.request.revision = Some(snapshot.revision_id);
    f.request.entry.symbol = snapshot.revision.inputs[0]
        .inventory
        .as_ref()
        .unwrap()
        .objects[0]
        .elf
        .as_ref()
        .unwrap()
        .symbols
        .iter()
        .find(|s| s.name.as_deref() == Some(b"entry"))
        .unwrap()
        .id
        .clone();
    let image = prepared(&f);
    let publication = analyze(&f, Some(image));
    let functions = cli(&f, &["functions", "--id", publication.as_str()]);
    let member = &functions["records"][0]["value"];
    let analysis = member["outcome"]["analysis"]
        .as_str()
        .unwrap_or_else(|| panic!("{functions}"));
    let records = cli(&f, &["analysis", "--id", analysis]);
    let values: Vec<FunctionRecord> = records["records"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| serde_json::from_value(r["value"].clone()).unwrap())
        .collect();
    for (register, expected) in [
        (10, 0xffffff80),
        (11, 0x80),
        (12, 0xffffff80),
        (13, 0xff80),
        (14, 0xffffff80),
    ] {
        assert!(values.iter().any(|r| matches!(r,FunctionRecord::Value { register: actual, value: AbstractValue::Constant { value }, .. } if *actual==register && *value==expected)), "x{register}: {values:?}");
    }
    for register in [15, 16] {
        assert!(values.iter().any(|r| matches!(r, FunctionRecord::Value { register: actual, value: AbstractValue::Expression { .. }, .. } if *actual == register)));
    }
    assert!(values.iter().any(|r| matches!(
        r,
        FunctionRecord::Transfer {
            target: AbstractValue::Unknown,
            call: false,
            ..
        }
    )));
    assert!(values.iter().any(|r| matches!(
        r,
        FunctionRecord::SemanticGap {
            reason: SemanticGapReason::OpaqueCall,
            ..
        }
    )));
    let unresolved = cli(
        &f,
        &["calls", "--id", publication.as_str(), "--unresolved-only"],
    );
    assert_eq!(unresolved["records"].as_array().unwrap().len(), 1);
    let ro_address = values
        .iter()
        .find_map(|r| match r {
            FunctionRecord::MemoryAccess {
                offset: 0x10000004,
                address: AbstractValue::Constant { value },
                ..
            } => Some(*value),
            _ => None,
        })
        .unwrap();
    let references = cli(
        &f,
        &[
            "find-references",
            "--id",
            publication.as_str(),
            "--address",
            &format!("0x{ro_address:x}"),
        ],
    );
    let findings = references["records"].as_array().unwrap();
    assert!(
        findings
            .iter()
            .any(|r| r["value"]["record"]["kind"] == "reference"),
        "{references}"
    );
    assert!(
        findings
            .iter()
            .any(|r| r["value"]["record"]["kind"] == "memory-access")
    );
}

#[test]
fn native_research_resolves_calls_and_keeps_unknown_abi_explicit() {
    let f = fixture(false, true);
    let image = prepared(&f);
    let publication = analyze(&f, Some(image));
    let unresolved = cli(
        &f,
        &["research", "--id", publication.as_str(), "--name", "entry"],
    );
    let unresolved = cli(
        &f,
        &[
            "analysis",
            "--id",
            unresolved["run"]["analysis"].as_str().unwrap(),
        ],
    );
    assert!(unresolved["records"].as_array().unwrap().iter().any(|r| {
        r["value"]["kind"] == "call-resolution"
            && r["value"]["reason"]
                .as_str()
                .is_some_and(|s| s.contains("ABI"))
    }));
    let result = cli(
        &f,
        &[
            "research",
            "--id",
            publication.as_str(),
            "--name",
            "entry",
            "--abi-contract",
            "riscv-integer",
        ],
    );
    let id = result["run"]["analysis"].as_str().unwrap();
    let result = cli(&f, &["analysis", "--id", id]);
    assert!(
        result["records"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["value"]["kind"] == "call-resolution" && r["value"]["analysis"].is_string()),
        "{result}"
    );
    assert!(
        result["records"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["value"]["kind"] == "return-value")
    );
    cli(&f, &["doctor"]);
}

#[test]
fn image_mmio_knowledge_round_trip_has_native_commands_and_retained_evidence() {
    let mut obj = Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
    let text = obj.add_section(Vec::new(), b".iram.vendor".to_vec(), SectionKind::Text);
    // lui t0,0x60000; lw a1,0(t0); or a1,a1,a0; sw a1,0(t0); ret
    let words: [u32; 5] = [0x600002b7, 0x0002a583, 0x00a5e5b3, 0x00b2a023, 0x00008067];
    let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    obj.append_section_data(text, &bytes, 4);
    obj.add_symbol(symbol(
        b"entry",
        SymbolSection::Section(text),
        bytes.len() as u64,
        SymbolKind::Text,
    ));
    let f = custom_fixture(vec![("entry.o", obj.write().unwrap())], 0, 0);
    let image = prepared(&f);
    let publication = analyze(&f, Some(image.clone()));
    let r = cli(
        &f,
        &[
            "research",
            "--id",
            publication.as_str(),
            "--name",
            "entry",
            "--abi-contract",
            "riscv-integer",
        ],
    );
    let id = r["run"]["analysis"].as_str().unwrap();
    let facts = cli(&f, &["analysis", "--id", id]);
    assert!(
        facts["records"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["value"]["expression"]["kind"] == "load")
    );
    assert!(
        facts["records"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["value"]["expression"]["op"] == "or")
    );
    let proposed = cli(
        &f,
        &[
            "knowledge",
            "propose-register",
            "--analysis",
            id,
            "--subject",
            "radio.control",
            "--name",
            "CONTROL",
            "--address",
            "0x60000000",
            "--field",
            "ENABLE:0:1",
            "--actor",
            "test",
            "--reason",
            "reviewed synthetic hardware contract",
        ],
    );
    let base = proposed["run"]["knowledge"].as_str().unwrap();
    let assertions = cli(&f, &["knowledge", "show"]);
    let assertion = assertions["records"][0]["value"]["id"].as_str().unwrap();
    let accepted = cli(
        &f,
        &[
            "knowledge",
            "accept",
            "--base",
            base,
            "--assertion",
            assertion,
            "--actor",
            "test",
            "--reason",
            "accepted declared register",
        ],
    );
    let knowledge = accepted["run"]["knowledge"].as_str().unwrap();
    let r = cli(
        &f,
        &[
            "research",
            "--id",
            publication.as_str(),
            "--name",
            "entry",
            "--abi-contract",
            "riscv-integer",
            "--knowledge",
            knowledge,
        ],
    );
    let id = r["run"]["analysis"].as_str().unwrap();
    let facts = cli(&f, &["analysis", "--id", id]);
    assert_eq!(
        facts["records"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| r["value"]["kind"] == "mmio")
            .count(),
        2
    );
    assert_eq!(
        facts["summary"]["manifest"]["recipe"]["source"]["image"],
        image.as_str()
    );
    let backup = f.dir.path().join("research.blobray");
    cli(&f, &["backup", "--output", backup.to_str().unwrap()]);
    let moved = f.dir.path().join("restored");
    let output = Command::new(env!("CARGO_BIN_EXE_blobray-next"))
        .args([
            "restore",
            "--backup",
            backup.to_str().unwrap(),
            "--project",
            moved.to_str().unwrap(),
            "--limit-mode",
            "watchdog",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let output = Command::new(env!("CARGO_BIN_EXE_blobray-next"))
        .args([
            "analysis",
            "--id",
            id,
            "--project",
            moved.to_str().unwrap(),
            "--limit-mode",
            "watchdog",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("CONTROL"));
}

#[test]
fn explicit_captured_rom_definition_links_and_resolves_across_publications() {
    let source = fixture(false, true);
    let image = prepared(&source);
    let rom = export(&source, image);
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
    let publication = analyze(&f, None);
    let image = prepared(&f);
    let pid = analyze(&f, Some(image));
    let r = cli(
        &f,
        &[
            "research",
            "--id",
            pid.as_str(),
            "--name",
            "entry",
            "--abi-contract",
            "riscv-integer",
            "--companion-publication",
            publication.as_str(),
        ],
    );
    let facts = cli(
        &f,
        &["analysis", "--id", r["run"]["analysis"].as_str().unwrap()],
    );
    assert!(
        facts["records"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["value"]["kind"] == "call-resolution" && r["value"]["analysis"].is_string()),
        "{facts}"
    );
    f.request.layout.code.start = 0x10000000;
    assert!(
        f.app
            .link_plan(&f.project, f.request.clone(), &linker(), budget())
            .is_err()
    );
}

#[test]
fn recursive_research_retains_partial_local_facts_without_recursing() {
    let mut obj = Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
    let section = obj.add_section(Vec::new(), b".text.entry".to_vec(), SectionKind::Text);
    // jal ra,0; ret: explicit self call.
    obj.append_section_data(section, &[0xef, 0, 0, 0, 0x67, 0x80, 0, 0], 4);
    obj.add_symbol(symbol(
        b"entry",
        SymbolSection::Section(section),
        8,
        SymbolKind::Text,
    ));
    let f = custom_fixture(vec![("entry.o", obj.write().unwrap())], 0, 0);
    let image = prepared(&f);
    let pid = analyze(&f, Some(image));
    let r = cli(
        &f,
        &[
            "research",
            "--id",
            pid.as_str(),
            "--name",
            "entry",
            "--abi-contract",
            "riscv-integer",
        ],
    );
    assert_eq!(r["run"]["complete"], false);
    let facts = cli(
        &f,
        &["analysis", "--id", r["run"]["analysis"].as_str().unwrap()],
    );
    assert!(facts["records"].as_array().unwrap().iter().any(|r| {
        r["value"]["reason"]
            .as_str()
            .is_some_and(|s| s.contains("recursive component"))
    }));
}
