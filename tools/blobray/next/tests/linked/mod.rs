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
    let coverage = cli(&f, &["coverage", "--id", publication.as_str()]);
    assert_eq!(coverage["summary"]["extents"]["objects"], 1);
    assert!(
        coverage["summary"]["extents"]["executable_bytes"]
            .as_u64()
            .unwrap()
            > 0
    );
    assert_eq!(
        coverage["assessment"]["coverage"]["scope"],
        "selected-function-extents"
    );
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
    let metrics = &r["run"]["diagnostics"]["progress"]["measurements"];
    assert_eq!(metrics["knowledge_history_passes"], 1);
    // One selection pass in the worker, one indexed membership pass, and
    // one independent coordinator verification of the frozen selection.
    assert_eq!(metrics["publication_passes"], 3);
    assert_eq!(r["run"]["operation"]["kind"], "scenario");
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
    let output = Command::new(env!("CARGO_BIN_EXE_blobray"))
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
    let output = Command::new(env!("CARGO_BIN_EXE_blobray"))
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
    assert_eq!(r["run"]["assessment"]["coverage"]["status"], "partial");
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
fn diamond_research_keeps_shared_leaf_effects_for_both_parents_and_repeated_calls() {
    let mut obj = Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
    let definitions: &[(&str, &[u32])] = &[
        (
            "entry",
            &[
                0x60000537, 0x00b00593, 0x000000ef, 0x60000537, 0x01050513, 0x01600593, 0x000000ef,
                0x00008067,
            ],
        ),
        ("left", &[0x000000ef, 0x000000ef, 0x00008067]),
        ("right", &[0x000000ef, 0x00008067]),
        ("leaf", &[0x00b52023, 0x00008067]),
    ];
    let mut sections = Vec::new();
    let mut symbols = Vec::new();
    for (name, words) in definitions {
        let section = obj.add_section(
            Vec::new(),
            format!(".text.{name}").into_bytes(),
            SectionKind::Text,
        );
        let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        obj.append_section_data(section, &bytes, 4);
        symbols.push(obj.add_symbol(symbol(
            name.as_bytes(),
            SymbolSection::Section(section),
            bytes.len() as u64,
            SymbolKind::Text,
        )));
        sections.push(section);
    }
    for (from, offset, to) in [(0, 8, 1), (0, 24, 2), (1, 0, 3), (1, 4, 3), (2, 0, 3)] {
        obj.add_relocation(
            sections[from],
            Relocation {
                offset,
                symbol: symbols[to],
                addend: 0,
                flags: RelocationFlags::Elf {
                    r_type: object::elf::R_RISCV_JAL,
                },
            },
        )
        .unwrap();
    }
    let f = custom_fixture(vec![("diamond.o", obj.write().unwrap())], 0, 0);
    let image = prepared(&f);
    let publication = analyze(&f, Some(image));
    let leaf = cli(
        &f,
        &["functions", "--id", publication.as_str(), "--name", "leaf"],
    );
    let leaf_id = leaf["records"][0]["value"]["outcome"]["analysis"]
        .as_str()
        .unwrap();
    let research = cli(
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
    let facts = cli(
        &f,
        &[
            "analysis",
            "--id",
            research["run"]["analysis"].as_str().unwrap(),
        ],
    );
    let effects: Vec<_> = facts["records"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| &r["value"])
        .filter(|r| r["kind"] == "callee-effect" && r["analysis"] == leaf_id)
        .collect();
    assert_eq!(effects.len(), 3, "{facts}");
    for (address, value, count) in [(0x60000000u32, 11, 2), (0x60000010, 22, 1)] {
        assert_eq!(
            effects
                .iter()
                .filter(|r| r["address"]["value"] == address && r["value"]["value"] == value)
                .count(),
            count,
            "{facts}"
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
fn static_executable_with_only_dynamic_symbols_analyzes_captured_bytes() {
    let f = fixture(false, true);
    let object_path = f.dir.path().join("standalone.o");
    let image_path = f.dir.path().join("standalone.elf");
    fs::write(&object_path, object(false)).unwrap();
    let status = Command::new(linker())
        .args(["-m", "elf32lriscv", "-e", "helper", "-o"])
        .arg(&image_path)
        .arg(&object_path)
        .status()
        .unwrap();
    assert!(status.success());
    fs::write(
        &image_path,
        support::dynamic_symbols(fs::read(&image_path).unwrap(), true),
    )
    .unwrap();
    f.app
        .import(
            &f.project,
            vec![app::ImportInput {
                role: "image".into(),
                path: image_path.clone(),
                expected: None,
            }],
            Target::Riscv32Ilp32,
            budget(),
        )
        .unwrap();
    let inventory = app::inventory(&f.project, None).unwrap();
    let object = &inventory.revision.inputs[0]
        .inventory
        .as_ref()
        .unwrap()
        .objects[0];
    let symbol = object
        .elf
        .as_ref()
        .unwrap()
        .symbols
        .iter()
        .find(|s| s.name.as_deref() == Some(b"helper"))
        .unwrap()
        .id
        .clone();
    assert_eq!(symbol.table, SymbolTableKind::Dynamic);
    let request = FunctionRequest {
        revision: Some(inventory.revision_id),
        source: FunctionSource::Input { input: 0 },
        selector: (symbol.clone()).into(),
        extent: None,
        research: None,
    };
    fs::remove_file(image_path).unwrap();
    fs::remove_file(object_path).unwrap();
    let request_path = f.dir.path().join("dynamic-request.json");
    fs::write(&request_path, serde_json::to_vec(&request).unwrap()).unwrap();
    let result = cli(
        &f,
        &[
            "analyze-function",
            "--request",
            request_path.to_str().unwrap(),
        ],
    );
    let id: FunctionAnalysisId = serde_json::from_value(result["run"]["analysis"].clone()).unwrap();
    let output = f
        .app
        .query(
            &f.project,
            app::ReadQuery::Analysis { id, export: false },
            budget(),
        )
        .unwrap();
    let app::QuerySummary::Analysis { manifest, .. } = output.summary() else {
        panic!("analysis")
    };
    assert_eq!(manifest.recipe.selector.symbol().unwrap().clone(), symbol);
    assert_eq!(manifest.recipe.address_space, CodeAddressSpace::Image);
    assert_eq!(manifest.instructions, 1);
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
fn finite_pointer_loads_keep_both_callback_targets_in_queries_research_and_reopening() {
    let mut obj = Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
    let code = obj.add_section(Vec::new(), b".text.entry".to_vec(), SectionKind::Text);
    let data = obj.add_section(
        Vec::new(),
        b".rodata.callbacks".to_vec(),
        SectionKind::ReadOnlyData,
    );
    // Branch selects offset 0 or 4, reads a captured immutable pointer, then calls it.
    let words = [
        0x00050663u32,
        0x00000313,
        0x0080006f,
        0x00400313,
        0x000002b7,
        0x00028293,
        0x006282b3,
        0x0002a283,
        0x000280e7,
        0x00008067,
    ];
    obj.append_section_data(
        code,
        &words
            .iter()
            .flat_map(|w| w.to_le_bytes())
            .collect::<Vec<_>>(),
        4,
    );
    obj.add_symbol(symbol(
        b"entry",
        SymbolSection::Section(code),
        40,
        SymbolKind::Text,
    ));
    obj.append_section_data(data, &[0; 8], 4);
    let table = obj.add_symbol(symbol(
        b"callbacks",
        SymbolSection::Section(data),
        8,
        SymbolKind::Data,
    ));
    for (i, name) in ["left", "right"].iter().enumerate() {
        let section = obj.add_section(
            Vec::new(),
            format!(".text.{name}").into_bytes(),
            SectionKind::Text,
        );
        obj.append_section_data(section, &0x00008067u32.to_le_bytes(), 4);
        let target = obj.add_symbol(symbol(
            name.as_bytes(),
            SymbolSection::Section(section),
            4,
            SymbolKind::Text,
        ));
        obj.add_relocation(
            data,
            Relocation {
                offset: i as u64 * 4,
                symbol: target,
                addend: 0,
                flags: RelocationFlags::Elf {
                    r_type: object::elf::R_RISCV_32,
                },
            },
        )
        .unwrap();
    }
    for (offset, r_type) in [
        (16, object::elf::R_RISCV_HI20),
        (20, object::elf::R_RISCV_LO12_I),
    ] {
        obj.add_relocation(
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
    let f = custom_fixture(vec![("callbacks.o", obj.write().unwrap())], 0, 0);
    let image = prepared(&f);
    let raw = export(&f, image.clone());
    let elf = object::File::parse(raw.as_slice()).unwrap();
    let mut targets: Vec<u32> = ["left", "right"]
        .iter()
        .map(|name| elf.symbol_by_name(name).unwrap().address() as u32)
        .collect();
    targets.sort_unstable();
    let publication = analyze(&f, Some(image));
    for target in &targets {
        let calls = cli(
            &f,
            &[
                "calls",
                "--id",
                publication.as_str(),
                "--callee",
                &format!("0x{target:x}"),
            ],
        );
        assert_eq!(calls["records"].as_array().unwrap().len(), 1, "{calls}");
    }
    let unknown = cli(
        &f,
        &["calls", "--id", publication.as_str(), "--unresolved-only"],
    );
    assert_eq!(unknown["records"].as_array().unwrap().len(), 1, "{unknown}");
    let research = cli(
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
    let id = research["run"]["analysis"].as_str().unwrap();
    let before = cli(&f, &["analysis", "--id", id]);
    let records = before["records"].as_array().unwrap();
    let transfer = records
        .iter()
        .map(|r| &r["value"])
        .find(|r| r["kind"] == "transfer" && r["call"] == true)
        .unwrap();
    let expected: Vec<_> = targets
        .iter()
        .map(|address| serde_json::json!({"kind":"image-address","address":address}))
        .collect();
    assert_eq!(
        transfer["target"],
        serde_json::json!({"kind":"alternatives","values":expected})
    );
    assert!(
        records
            .iter()
            .any(|r| r["value"]["kind"] == "call-resolution" && r["value"]["analysis"].is_null())
    );
    assert!(
        !records
            .iter()
            .any(|r| r["value"]["kind"] == "callee-effect")
    );
    fs::remove_file(f.dir.path().join("callbacks.o")).unwrap();
    let output = f.dir.path().join("saved-alternatives");
    cli(
        &f,
        &[
            "export-analysis",
            "--id",
            id,
            "--output",
            output.to_str().unwrap(),
        ],
    );
    let reopened = cli(&f, &["analysis", "--id", id]);
    assert_eq!(before["records"], reopened["records"]);
    let saved = fs::read_to_string(output.join("records.jsonl")).unwrap();
    assert!(
        saved
            .lines()
            .map(|line| serde_json::from_str::<FunctionRecord>(line).unwrap())
            .any(|r| matches!(
                r,
                FunctionRecord::Transfer {
                    target: AbstractValue::Alternatives { .. },
                    ..
                }
            ))
    );
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
        let InterfaceRoot::Symbol { symbol, .. } = &mut contract.root else {
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
fn image_function_contracts_reject_data_symbols_and_preserve_review_after_source_removal() {
    use object::ObjectSection as _;
    for range in [false, true] {
        let f = fixture(true, true);
        let image = prepared(&f);
        let bytes = export(&f, image.clone());
        let elf = object::File::parse(bytes.as_slice()).unwrap();
        let payload = ArtifactId::of_bytes(&bytes);
        let object = ObjectId {
            artifact: payload.clone(),
            location: ObjectLocation::Standalone,
        };
        let symbol = |name: &str| {
            let s = elf.symbol_by_name(name).unwrap();
            SymbolId {
                object: object.clone(),
                table: SymbolTableKind::Static,
                table_section: elf.section_by_name(".symtab").unwrap().index().0 as u32,
                index: s.index().0 as u64,
            }
        };
        let entry = elf.symbol_by_name("entry").unwrap();
        let selector = if range {
            FunctionSelector::Range {
                object: object.clone(),
                section: entry.section_index().unwrap().0 as u32,
                extent: CodeRange {
                    start: entry.address(),
                    length: entry.size(),
                },
            }
        } else {
            symbol("entry").into()
        };
        let p: KnowledgeProposal = serde_json::from_value(serde_json::json!({
            "subject":"image.function","occurrence":{"revision":f.revision,"source":{"kind":"image","image":image},"object":object,"symbol":selector.symbol()},
            "claim":{"kind":"function","contract":{"selector":selector,"abi":"riscv-integer","signature":null,
                "name":"entry","role":null,"return_role":null,"summary":"exact entry identity with unknown signature",
                "contexts":[],"preconditions":[],"applicability":"captured fixture only"}},
            "evidence":[{"kind":"source","payload":payload,"range":{"start":0,"length":bytes.len()}}],"note":null
        })).unwrap();
        let change = |proposal| KnowledgeChange {
            expected_base: None,
            actor: "fixture".into(),
            reason: "captured function identity".into(),
            action: KnowledgeAction::Propose { proposal },
        };
        for which in 0..3 {
            let mut bad = p.clone();
            bad.occurrence.symbol = None;
            let KnowledgeClaim::Function { contract } = &mut bad.claim else {
                panic!()
            };
            let mut s = symbol("value");
            match which {
                0 => (),
                1 => s.index = u64::MAX,
                _ => s.table = SymbolTableKind::Dynamic,
            }
            contract.selector = s.into();
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
            .start_knowledge(&f.project, &change(p.clone()), budget())
            .unwrap()
            .wait();
        assert_eq!(proposed.state, RunState::Completed, "{proposed:?}");
        let entries = cli(&f, &["knowledge", "show"]);
        let assertion = entries["records"][0]["value"]["id"].as_str().unwrap();
        for path in fs::read_dir(f.dir.path()).unwrap() {
            let path = path.unwrap().path();
            if path.is_file()
                && matches!(path.extension().and_then(|s| s.to_str()), Some("o" | "a"))
            {
                fs::remove_file(path).unwrap();
            }
        }
        let accepted = cli(
            &f,
            &[
                "knowledge",
                "accept",
                "--base",
                proposed.knowledge.unwrap().as_str(),
                "--assertion",
                assertion,
                "--actor",
                "fixture",
                "--reason",
                "physical entry only",
            ],
        );
        let revision = accepted["run"]["knowledge"].as_str().unwrap();
        let saved = cli(&f, &["knowledge", "show", "--revision", revision]);
        assert_eq!(
            saved["records"][0]["value"]["proposal"],
            serde_json::to_value(p).unwrap()
        );
        let output = f.dir.path().join("function-contract.json");
        cli(
            &f,
            &[
                "knowledge",
                "export",
                "--revision",
                revision,
                "--output",
                output.to_str().unwrap(),
            ],
        );
        assert_eq!(
            cli(&f, &["knowledge", "show", "--revision", revision]),
            saved
        );
    }
}
