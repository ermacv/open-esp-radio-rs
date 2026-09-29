use super::*;
fn data_object(relocations: bool) -> Vec<u8> {
    data_object_bytes(relocations, &[0xfb, 0xff, 3, 0])
}
pub(super) fn data_object_bytes(relocations: bool, table_bytes: &[u8]) -> Vec<u8> {
    data_object_relocation(
        table_bytes,
        relocations.then_some((0, object::elf::R_RISCV_32)),
    )
}
fn data_object_relocation(table_bytes: &[u8], relocation: Option<(u64, u32)>) -> Vec<u8> {
    let mut obj = Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
    let text = obj.add_section(Vec::new(), b".text.entry".to_vec(), SectionKind::Text);
    let code = [0x13, 0x05, 0xa0, 0x02, 0x67, 0x80, 0, 0]; // a0=42; return
    obj.append_section_data(text, &code, 2);
    obj.add_symbol(symbol(
        b"entry",
        SymbolSection::Section(text),
        8,
        SymbolKind::Text,
    ));
    let data = obj.add_section(
        Vec::new(),
        b".rodata.table".to_vec(),
        SectionKind::ReadOnlyData,
    );
    obj.append_section_data(data, table_bytes, 2);
    obj.add_symbol(symbol(
        b"table",
        SymbolSection::Section(data),
        table_bytes.len() as u64,
        SymbolKind::Data,
    ));
    let initial = obj.add_section(Vec::new(), b".data.initial".to_vec(), SectionKind::Data);
    obj.append_section_data(initial, &[10, 20, 30, 40], 1);
    obj.add_symbol(symbol(
        b"initial",
        SymbolSection::Section(initial),
        4,
        SymbolKind::Data,
    ));
    let bss = obj.add_section(
        Vec::new(),
        b".bss.empty".to_vec(),
        SectionKind::UninitializedData,
    );
    obj.append_section_bss(bss, 16, 4);
    obj.add_symbol(symbol(
        b"empty",
        SymbolSection::Section(bss),
        16,
        SymbolKind::Data,
    ));
    if let Some((offset, relocation_type)) = relocation {
        let external = obj.add_symbol(symbol(
            b"outside",
            SymbolSection::Undefined,
            0,
            SymbolKind::Data,
        ));
        obj.add_relocation(
            data,
            Relocation {
                offset,
                symbol: external,
                addend: 0,
                flags: RelocationFlags::Elf {
                    r_type: relocation_type,
                },
            },
        )
        .unwrap();
    }
    obj.write().unwrap()
}
pub(super) fn request(f: &Fixture, names: &[&[u8]]) -> DataRequest {
    let inventory = app::inventory(&f.project, Some(&f.revision)).unwrap();
    let elf = inventory.revision.inputs[0]
        .inventory
        .as_ref()
        .unwrap()
        .objects[1]
        .elf
        .as_ref()
        .unwrap();
    DataRequest {
        pointer_table: None,
        occurrence: Occurrence {
            revision: f.revision.clone(),
            source: f.request.source.clone(),
            object: f.request.selector.object().clone(),
            symbol: None,
        },
        ranges: names
            .iter()
            .map(|name| DataSelector::Symbol {
                symbol: elf
                    .symbols
                    .iter()
                    .find(|s| s.name.as_deref() == Some(*name))
                    .unwrap()
                    .id
                    .clone(),
                length: None,
            })
            .collect(),
        analyses: Vec::new(),
    }
}
#[derive(Default)]
struct Collected {
    data: Vec<DataRecord>,
}
impl ElfSink for Collected {
    fn section(&mut self, _: &SectionRecord, _: &mut dyn RunControl) -> Result<()> {
        Ok(())
    }
    fn symbol(&mut self, _: &SymbolRecord, _: &mut dyn RunControl) -> Result<()> {
        Ok(())
    }
    fn relocation(&mut self, _: &RelocationRecord, _: &mut dyn RunControl) -> Result<()> {
        Ok(())
    }
    fn diagnostic(&mut self, _: &Diagnostic, _: &mut dyn RunControl) -> Result<()> {
        Ok(())
    }
}
impl InventorySink for Collected {}
impl app::DoctorSink for Collected {
    fn error(&mut self, e: &Error, _: &mut dyn RunControl) -> Result<()> {
        Err(e.clone())
    }
    fn unfinished(&mut self, _: &RunId, _: &mut dyn RunControl) -> Result<()> {
        Ok(())
    }
}
impl app::QuerySink for Collected {
    fn data(&mut self, r: &DataRecord, _: &mut dyn RunControl) -> Result<()> {
        self.data.push(r.clone());
        Ok(())
    }
    fn summary(&mut self, _: &app::QuerySummary, _: &mut dyn RunControl) -> Result<()> {
        Ok(())
    }
}
fn collect(f: &Fixture, query: app::ReadQuery) -> Collected {
    let mut out = f.app.query(&f.project, query, budget()).unwrap();
    let mut records = Collected::default();
    out.records(&|| false, &mut records).unwrap();
    records
}
#[test]
fn data_ranges_share_one_prepared_object_and_export_exact_initialization_bytes() {
    for thin in [false, true] {
        let raw = data_object(false);
        let f = fixture(raw.clone(), thin);
        let req = request(&f, &[b"table", b"initial"]);
        fs::remove_file(f.dir.path().join("entry.o")).unwrap();
        fs::remove_file(f.dir.path().join("entry.a")).unwrap();
        let mut out = f
            .app
            .query(
                &f.project,
                app::ReadQuery::Data {
                    request: req.clone(),
                },
                budget(),
            )
            .unwrap();
        let app::QuerySummary::Data { manifest } = out.summary() else {
            panic!()
        };
        assert_eq!(
            manifest.request.occurrence.object.location,
            ObjectLocation::ArchiveMember { ordinal: 1 }
        );
        assert!(!manifest.spans[0].writable);
        assert!(manifest.spans[1].writable);
        assert_eq!(manifest.spans[1].export_offset, 4);
        let saved = manifest.clone();
        let metrics = out.report.diagnostics.progress.unwrap().measurements;
        assert_eq!(metrics.objects_prepared, 1);
        assert_eq!(metrics.sections_prepared, 2);
        assert_eq!(metrics.object_read_bytes, raw.len() as u64);
        assert_eq!(metrics.object_hash_bytes, raw.len() as u64);
        let output = f.dir.path().join("data-export");
        out.export_data(&output, &|| false).unwrap();
        assert_eq!(fs::read(output.join("object.elf")).unwrap(), raw);
        assert_eq!(
            fs::read(output.join("data.bin")).unwrap(),
            [0xfb, 0xff, 3, 0, 10, 20, 30, 40]
        );
        for span in &saved.spans {
            let range = span.file_range;
            assert_eq!(
                ArtifactId::of_bytes(
                    &raw[range.start as usize..(range.start + range.length) as usize]
                ),
                span.digest
            );
        }
        assert!(out.export_data(&output, &|| false).is_err());
        let mut wrong = req;
        if let DataSelector::Symbol { symbol, .. } = &mut wrong.ranges[0] {
            symbol.object.location = ObjectLocation::ArchiveMember { ordinal: 0 };
        }
        assert!(
            f.app
                .query(
                    &f.project,
                    app::ReadQuery::Data { request: wrong },
                    budget()
                )
                .is_err()
        );
    }
}

#[test]
fn data_research_preserves_unknown_calls_and_partial_analysis_coverage() {
    let code = [0xe7, 0x80, 0x02, 0, 0x67, 0x80, 0, 0]; // jalr ra,t0; return
    let f = fixture(object(&code, code.len() as u64, false), false);
    let result = analyze(&f);
    assert_eq!(result.state, RunState::Completed, "{result:?}");
    let mut req = request(&f, &[]);
    req.analyses.push(result.analysis.unwrap());
    let mut output = f
        .app
        .query(&f.project, app::ReadQuery::Data { request: req }, budget())
        .unwrap();
    let app::QuerySummary::Data { manifest } = output.summary() else {
        panic!()
    };
    assert!(!manifest.analyses[0].semantics.unwrap().complete);
    let mut records = Collected::default();
    output.records(&|| false, &mut records).unwrap();
    assert!(records.data.iter().any(|r| matches!(r, DataRecord::Analysis { record, .. } if matches!(&**record, FunctionRecord::SemanticGap { reason: SemanticGapReason::OpaqueCall, .. }))));
    assert!(records.data.iter().any(|r| matches!(r, DataRecord::Analysis { record, .. } if matches!(&**record, FunctionRecord::CallInputs { .. }))));
}

#[test]
fn dynamic_function_selection_keeps_static_relocation_target_identity() {
    let bytes = object(
        &[
            0x97, 0, 0, 0, 0xe7, 0x80, 0, 0, 0x37, 5, 0, 0, 0x13, 5, 5, 0, 0x67, 0x80, 0, 0,
        ],
        20,
        true,
    );
    let mut f = fixture(support::dynamic_symbols(bytes, false), true);
    let inventory = app::inventory(&f.project, None).unwrap();
    let symbols = &inventory.revision.inputs[0]
        .inventory
        .as_ref()
        .unwrap()
        .objects[1]
        .elf
        .as_ref()
        .unwrap()
        .symbols;
    let dynamic = symbols
        .iter()
        .find(|s| s.id.table == SymbolTableKind::Dynamic && s.name.as_deref() == Some(b"entry"))
        .unwrap()
        .id
        .clone();
    assert_ne!(
        dynamic.index,
        f.request.selector.symbol().unwrap().clone().index
    );
    f.request.selector = dynamic.clone().into();
    let run = analyze(&f);
    assert_eq!(run.state, RunState::Completed, "{run:?}");
    let (manifest, records) = export(&f, run.analysis.unwrap());
    assert_eq!(manifest.recipe.selector.symbol().unwrap().clone(), dynamic);
    let references: Vec<_> = records
        .iter()
        .filter_map(|r| match r {
            FunctionRecord::Reference { raw, target, .. } => Some((raw, target)),
            _ => None,
        })
        .collect();
    assert_eq!(references.len(), 3);
    for (raw, target) in references {
        let expected = symbols
            .iter()
            .find(|s| {
                s.id.table == SymbolTableKind::Static && s.name.as_ref() == Some(&target.name)
            })
            .unwrap();
        assert_eq!(raw.target.symbol, expected.id);
        assert_eq!(target.symbol, expected.id);
    }
}

fn pointer_object(extra: &[(u64, u32)]) -> Vec<u8> {
    let mut obj = Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
    let code = obj.add_section(vec![], b".text.entry".to_vec(), SectionKind::Text);
    obj.append_section_data(code, &[0x67, 0x80, 0, 0], 2);
    let entry = obj.add_symbol(symbol(
        b"entry",
        SymbolSection::Section(code),
        4,
        SymbolKind::Text,
    ));
    let external = obj.add_symbol(symbol(
        b"outside",
        SymbolSection::Undefined,
        0,
        SymbolKind::Text,
    ));
    let table = obj.add_section(vec![], b".rodata.table".to_vec(), SectionKind::ReadOnlyData);
    obj.append_section_data(
        table,
        &[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x60],
        4,
    );
    obj.add_symbol(symbol(
        b"table",
        SymbolSection::Section(table),
        16,
        SymbolKind::Data,
    ));
    for (offset, target, addend, r_type) in [
        (0, entry, 0, object::elf::R_RISCV_32),
        (4, external, 4, object::elf::R_RISCV_32),
    ] {
        obj.add_relocation(
            table,
            Relocation {
                offset,
                symbol: target,
                addend,
                flags: RelocationFlags::Elf { r_type },
            },
        )
        .unwrap();
    }
    for &(offset, r_type) in extra {
        obj.add_relocation(
            table,
            Relocation {
                offset,
                symbol: entry,
                addend: 0,
                flags: RelocationFlags::Elf { r_type },
            },
        )
        .unwrap();
    }
    obj.write().unwrap()
}
fn pointers(records: &[DataRecord]) -> Vec<&PointerValue> {
    records
        .iter()
        .filter_map(|r| match r {
            DataRecord::Pointer { value, .. } => Some(value),
            _ => None,
        })
        .collect()
}
#[test]
fn unsupported_overlapping_and_partial_pointer_writes_remain_unresolved() {
    for (extra, issues) in [
        (
            vec![(0, object::elf::R_RISCV_32)],
            vec![Some(PointerIssue::OverlappingRelocations), None, None, None],
        ),
        (
            vec![(8, object::elf::R_RISCV_64)],
            vec![
                None,
                None,
                Some(PointerIssue::UnsupportedRelocation),
                Some(PointerIssue::UnsupportedRelocation),
            ],
        ),
        (
            vec![(10, object::elf::R_RISCV_32)],
            vec![
                None,
                None,
                Some(PointerIssue::PartialSlotWrite),
                Some(PointerIssue::PartialSlotWrite),
            ],
        ),
        (
            vec![(12, object::elf::R_RISCV_HI20)],
            vec![Some(PointerIssue::UnknownWriteExtent); 4],
        ),
        (vec![(8, object::elf::R_RISCV_NONE)], vec![None; 4]),
    ] {
        let f = fixture(pointer_object(&extra), false);
        let mut req = request(&f, &[b"table"]);
        req.pointer_table = Some(PointerTable {
            count: 4,
            stride: 4,
        });
        let observations = collect(&f, app::ReadQuery::Data { request: req }).data;
        for (value, expected) in pointers(&observations).iter().zip(issues) {
            let issue = match value {
                PointerValue::Unresolved { issue } => Some(*issue),
                _ => None,
            };
            assert_eq!(issue, expected, "{value:?}");
        }
    }
}
#[cfg(test)]
mod alternative_tests {
    use blobray_domain::*;
    #[test]
    fn persisted_alternatives_are_flat_bounded_and_canonical() {
        let good = r#"{"kind":"alternatives","values":[{"kind":"constant","value":1},{"kind":"constant","value":2}]}"#;
        let parsed: AbstractValue = serde_json::from_str(good).unwrap();
        assert!(parsed.contains_address(1));
        assert!(parsed.contains_address(2));
        assert!(!parsed.contains_address(3));
        assert_eq!(serde_json::to_string(&parsed).unwrap(), good);
        for bad in [
            r#"[]"#,
            r#"[{"kind":"constant","value":1}]"#,
            r#"[{"kind":"constant","value":2},{"kind":"constant","value":1}]"#,
            r#"[{"kind":"constant","value":1},{"kind":"constant","value":1}]"#,
            r#"[{"kind":"unknown"},{"kind":"constant","value":1}]"#,
            r#"[{"kind":"alternatives","values":[]},{"kind":"constant","value":1}]"#,
        ] {
            assert!(
                serde_json::from_str::<ValueAlternatives>(bad).is_err(),
                "{bad}"
            );
        }
        let too_many = serde_json::to_string(
            &(0..9)
                .map(|value| ValueAlternative::Constant { value })
                .collect::<Vec<_>>(),
        )
        .unwrap();
        assert!(serde_json::from_str::<ValueAlternatives>(&too_many).is_err());
    }
}

#[test]
fn data_rejects_nobits_overflow_and_exhaustion_without_changing_current() {
    let f = fixture(data_object(false), false);
    let current = app::inventory(&f.project, None).unwrap().revision_id;
    let mut requests = vec![request(&f, &[b"empty"]), request(&f, &[b"table"])];
    if let DataSelector::Symbol { length, .. } = &mut requests[1].ranges[0] {
        *length = Some(u64::MAX);
    }
    let mut overflow = request(&f, &[b"table"]);
    overflow.ranges = vec![DataSelector::Section {
        section: 1,
        offset: u64::MAX,
        length: 1,
    }];
    requests.push(overflow);
    for req in requests {
        assert!(
            f.app
                .query(&f.project, app::ReadQuery::Data { request: req }, budget())
                .is_err()
        );
    }
    let mut limited = budget();
    limited.working_memory_bytes = Some(1024 * 1024);
    assert!(
        f.app
            .query(
                &f.project,
                app::ReadQuery::Data {
                    request: request(&f, &[b"table"])
                },
                limited
            )
            .is_err()
    );
    assert_eq!(
        app::inventory(&f.project, None).unwrap().revision_id,
        current
    );
}

#[test]
fn dynamic_occurrences_keep_physical_indices_through_analysis_and_reopen() {
    for only in [false, true] {
        let mut f = fixture(support::dynamic_symbols(data_object(false), only), false);
        let inventory = app::inventory(&f.project, None).unwrap();
        let symbols = &inventory.revision.inputs[0]
            .inventory
            .as_ref()
            .unwrap()
            .objects[1]
            .elf
            .as_ref()
            .unwrap()
            .symbols;
        let entry = symbols
            .iter()
            .find(|s| s.id.table == SymbolTableKind::Dynamic && s.name.as_deref() == Some(b"entry"))
            .unwrap();
        f.request.selector = entry.id.clone().into();
        let entry_id = f.request.selector.symbol().unwrap().clone();
        fs::remove_file(f.dir.path().join("entry.a")).unwrap();
        fs::remove_file(f.dir.path().join("entry.o")).unwrap();
        let result = analyze(&f);
        assert_eq!(result.state, RunState::Completed, "{result:?}");
        let analysis_id = result.analysis.unwrap();
        let (manifest, _) = export(&f, analysis_id.clone());
        assert_eq!(manifest.recipe.selector.symbol().unwrap().clone(), entry_id);
        assert_eq!(manifest.instructions, 2);
    }
}

#[test]
fn pointer_tables_keep_null_addresses_defined_and_external_symbols() {
    for thin in [false, true] {
        let f = fixture(pointer_object(&[]), thin);
        let mut req = request(&f, &[b"table"]);
        req.pointer_table = Some(PointerTable {
            count: 4,
            stride: 4,
        });
        let observations = collect(
            &f,
            app::ReadQuery::Data {
                request: req.clone(),
            },
        )
        .data;
        let values = pointers(&observations);
        assert_eq!(values.len(), 4);
        assert!(
            matches!(values[0], PointerValue::DefinedSymbol { symbol, addend: 0 } if symbol == f.request.selector.symbol().unwrap())
        );
        assert!(
            matches!(values[1], PointerValue::ExternalSymbol { symbol, addend: 4 } if &symbol.object == f.request.selector.object())
        );
        assert_eq!(values[2], &PointerValue::Null);
        assert_eq!(
            values[3],
            &PointerValue::Address {
                value: 0x60000000,
                image_address: false
            }
        );
    }
}
