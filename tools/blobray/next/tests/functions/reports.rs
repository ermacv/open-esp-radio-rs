use super::*;
fn publication(f: &Fixture) -> PublicationId {
    let decoder = blobray_backend_riscv::RiscvDecoder;
    let run = f
        .app
        .start_analyze_project(
            &f.project,
            InvestigationInput::Automatic {
                request: InvestigationRequest::default(),
                producer: FunctionProducer {
                    decoder: decoder.identity().into(),
                    semantics: decoder.semantic_identity().into(),
                },
            },
            budget(),
        )
        .unwrap()
        .wait();
    assert_eq!(run.state, RunState::Completed, "{:?}", run.error);
    run.publication.unwrap()
}
fn cli(f: &Fixture, args: &[&str]) -> serde_json::Value {
    let output = Command::new(env!("CARGO_BIN_EXE_blobray"))
        .args(["--format", "json", args[0], "--project"])
        .arg(&f.project)
        .args(["--limit-mode", "watchdog"])
        .args(&args[1..])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}
#[test]
fn coverage_counts_alias_union_and_unselected_executable_sections_without_changing_claim() {
    let mut obj = Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
    let text = obj.add_section(vec![], b".text".to_vec(), SectionKind::Text);
    obj.append_section_data(
        text,
        &[0x67, 0x80, 0, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0],
        4,
    );
    for name in [b"entry".as_slice(), b"alias"] {
        obj.add_symbol(symbol(
            name,
            SymbolSection::Section(text),
            4,
            SymbolKind::Text,
        ));
    }
    let other = obj.add_section(vec![], b".text.unclassified".to_vec(), SectionKind::Text);
    obj.append_section_data(other, &[1, 0, 1, 0, 1, 0, 1, 0], 2);
    let f = fixture(obj.write().unwrap(), false);
    let id = publication(&f);
    let report = cli(&f, &["coverage", "--id", id.as_str()]);
    assert_eq!(
        report["assessment"]["coverage"]["scope"],
        "selected-function-extents"
    );
    assert_eq!(report["assessment"]["coverage"]["status"], "complete");
    let extents = &report["summary"]["extents"];
    assert_eq!(extents["objects"], 2);
    assert_eq!(extents["executable_bytes"], 48);
    assert_eq!(extents["selected_extent_bytes"], 8);
    assert_eq!(extents["outside_selected_extent_bytes"], 40);
    assert_eq!(extents["unknowns"], 0);
    assert!(
        report["records"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["value"]["classification"] == "outside-selected-extents")
    );
    fs::remove_file(f.dir.path().join("entry.a")).unwrap();
    fs::remove_file(f.dir.path().join("entry.o")).unwrap();
    assert_eq!(report, cli(&f, &["coverage", "--id", id.as_str()]));
    let usage = cli(&f, &["storage-usage"]);
    assert_eq!(usage["summary"]["usage"]["publications"], 1);
    assert!(
        usage["summary"]["usage"]["cas"]["logical_bytes"]
            .as_u64()
            .unwrap()
            > 0
    );
    assert_eq!(usage["summary"]["usage"]["reachability_assessed"], false);
    assert_eq!(usage, cli(&f, &["storage-usage"]));
}
#[test]
fn single_function_releases_large_elf_before_research_loading() {
    let mut obj = Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
    let text = obj.add_section(vec![], b".text".to_vec(), SectionKind::Text);
    obj.append_section_data(text, &[0x67, 0x80, 0, 0], 4);
    obj.add_symbol(symbol(
        b"entry",
        SymbolSection::Section(text),
        4,
        SymbolKind::Text,
    ));
    let data = obj.add_section(vec![], b".rodata".to_vec(), SectionKind::ReadOnlyData);
    obj.append_section_data(data, &vec![0; 8 * 1024 * 1024], 4);
    let f = fixture(obj.write().unwrap(), false);
    let id = publication(&f);
    let mut request = f.request.clone();
    request.research = Some(ResearchOptions {
        publication: id,
        companions: vec![],
        abi: Some(CallAbi::RiscvInteger),
        knowledge: None,
    });
    let mut limits = budget();
    limits.working_memory_bytes = Some(12 * 1024 * 1024);
    let run = f
        .app
        .start_analyze_function(&f.project, request, limits)
        .unwrap()
        .wait();
    assert_eq!(run.state, RunState::Completed, "{:?}", run.error);
    let phases = run.diagnostics.unwrap().progress.unwrap().phases;
    assert!(phases.prepare_object.peak_reserved_bytes >= 8 * 1024 * 1024);
    assert!(phases.load_research.peak_reserved_bytes < 8 * 1024 * 1024);
    assert!(phases.compose_research.peak_reserved_bytes < 8 * 1024 * 1024);
}

#[test]
fn unknown_extents_and_malformed_members_remain_visible_and_queries_do_not_publish() {
    let bytes = object(&[0x67, 0x80, 0, 0], 0, false);
    let f = fixture(bytes.clone(), false);
    let input = f.dir.path().join("partial.a");
    let mut archive = support::archive(&[(b"entry.o", &bytes)], false);
    archive.extend_from_slice(b"malformed archive tail");
    fs::write(&input, archive).unwrap();
    f.app
        .import(
            &f.project,
            vec![app::ImportInput {
                role: "partial".into(),
                path: input,
                expected: None,
            }],
            Target::Riscv32Ilp32,
            budget(),
        )
        .unwrap();
    let id = publication(&f);
    let report = cli(&f, &["coverage", "--id", id.as_str()]);
    assert_eq!(report["assessment"]["coverage"]["status"], "partial");
    assert!(report["summary"]["extents"]["unknowns"].as_u64().unwrap() > 0);
    assert_eq!(report["summary"]["extents"]["selected_extent_bytes"], 0);
    let before = cli(&f, &["storage-usage"]);
    let mut tiny = budget();
    tiny.max_work_units = Some(1);
    assert_eq!(
        f.app
            .query(
                &f.project,
                app::ReadQuery::Coverage { id: id.clone() },
                tiny
            )
            .err()
            .unwrap()
            .code,
        ErrorCode::ResourceLimited
    );
    let mut tiny = budget();
    tiny.working_memory_bytes = Some(1);
    assert_eq!(
        f.app
            .query(&f.project, app::ReadQuery::StorageUsage, tiny)
            .err()
            .unwrap()
            .code,
        ErrorCode::ResourceLimited
    );
    assert_eq!(before, cli(&f, &["storage-usage"]));
    assert_eq!(report, cli(&f, &["coverage", "--id", id.as_str()]));
}
