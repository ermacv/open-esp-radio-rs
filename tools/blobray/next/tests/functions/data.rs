use super::*;
fn data_object(relocations: bool) -> Vec<u8> {
    data_object_bytes(relocations, &[0xfb, 0xff, 3, 0])
}
fn data_object_bytes(relocations: bool, table_bytes: &[u8]) -> Vec<u8> {
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
fn request(f: &Fixture, names: &[&[u8]]) -> DataRequest {
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
        occurrence: KnowledgeOccurrence {
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
    knowledge: Vec<KnowledgeEntry>,
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
    fn knowledge_entry(&mut self, r: &KnowledgeEntry, _: &mut dyn RunControl) -> Result<()> {
        self.knowledge.push(r.clone());
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
fn proposal(f: &Fixture) -> DataProposalRequest {
    let req = request(f, &[b"table"]);
    DataProposalRequest {
        occurrence: req.occurrence,
        analyses: Vec::new(),
        subject: "table".to_owned().try_into().unwrap(),
        selector: req.ranges[0].clone(),
        layout: (IntegerTable {
            encoding: IntegerEncoding {
                width: 2,
                signed: true,
                byte_order: DataByteOrder::Little,
            },
            count: 2,
            stride: 2,
        })
        .into(),
        purpose: "test coefficients".into(),
        applicability: "exact captured fixture only".into(),
        expected_base: None,
        actor: "reviewer".into(),
        reason: "verified bytes".into(),
    }
}
pub(super) fn review(
    f: &Fixture,
    proposed: app::RunRecord,
    decision: ReviewDecision,
    supersedes: Option<AssertionId>,
) -> (KnowledgeRevisionId, AssertionId) {
    assert_eq!(proposed.state, RunState::Completed, "{proposed:?}");
    let base = proposed.knowledge.unwrap();
    let entries = collect(
        f,
        app::ReadQuery::Knowledge {
            revision: Some(base.clone()),
            history: false,
        },
    )
    .knowledge;
    let id = entries
        .iter()
        .find(|e| e.state == AssertionState::Proposed)
        .unwrap()
        .id
        .clone();
    let result = f
        .app
        .start_knowledge(
            &f.project,
            &KnowledgeChange {
                expected_base: Some(base),
                actor: "reviewer".into(),
                reason: "checked exact evidence".into(),
                action: KnowledgeAction::Review {
                    assertion: id.clone(),
                    decision,
                    supersedes,
                },
            },
            budget(),
        )
        .unwrap()
        .wait();
    assert_eq!(result.state, RunState::Completed, "{result:?}");
    (result.knowledge.unwrap(), id)
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
fn table_review_supersession_rejection_and_source_free_export() {
    let f = fixture(data_object(false), false);
    let p = proposal(&f);
    let proposed = f
        .app
        .start_propose_data(&f.project, p.clone(), budget())
        .unwrap()
        .wait();
    let pending_base = proposed.knowledge.clone().unwrap();
    let pending = collect(
        &f,
        app::ReadQuery::Knowledge {
            revision: Some(pending_base.clone()),
            history: false,
        },
    )
    .knowledge[0]
        .id
        .clone();
    assert!(
        f.app
            .query(
                &f.project,
                app::ReadQuery::ReviewedData {
                    revision: pending_base,
                    assertion: pending
                },
                budget()
            )
            .is_err()
    );
    let (base, id) = review(&f, proposed, ReviewDecision::Accept, None);
    let values = collect(
        &f,
        app::ReadQuery::ReviewedData {
            revision: base.clone(),
            assertion: id.clone(),
        },
    )
    .data;
    assert!(values.iter().any(|r| matches!(
        r,
        DataRecord::Integer {
            index: 0,
            signed: Some(-5),
            ..
        }
    )));
    assert!(values.iter().any(|r| matches!(
        r,
        DataRecord::Integer {
            index: 1,
            signed: Some(3),
            ..
        }
    )));
    let mut replacement = p;
    replacement.expected_base = Some(base.clone());
    let DataLayout::Integer(layout) = &mut replacement.layout else {
        panic!("integer layout")
    };
    layout.encoding.signed = false;
    let entry = collect(
        &f,
        app::ReadQuery::Knowledge {
            revision: Some(base.clone()),
            history: false,
        },
    )
    .knowledge
    .remove(0);
    let KnowledgeClaim::IntegerTable { selector, .. } = entry.proposal.claim else {
        panic!()
    };
    assert!(matches!(selector, DataSelector::Section { .. }));
    replacement.selector = selector; // physical alias of the original symbol selector
    let proposed = f
        .app
        .start_propose_data(&f.project, replacement.clone(), budget())
        .unwrap()
        .wait();
    let (new_base, new_id) = review(&f, proposed, ReviewDecision::Accept, Some(id.clone()));
    assert!(
        f.app
            .query(
                &f.project,
                app::ReadQuery::ReviewedData {
                    revision: new_base.clone(),
                    assertion: id.clone()
                },
                budget()
            )
            .is_err()
    );
    collect(
        &f,
        app::ReadQuery::ReviewedData {
            revision: base,
            assertion: id,
        },
    ); // historical acceptance remains meaningful
    replacement.expected_base = Some(new_base);
    replacement.purpose = "rejected change".into();
    let proposed = f
        .app
        .start_propose_data(&f.project, replacement, budget())
        .unwrap()
        .wait();
    let (head, rejected) = review(&f, proposed, ReviewDecision::Reject, None);
    assert!(
        f.app
            .query(
                &f.project,
                app::ReadQuery::ReviewedData {
                    revision: head.clone(),
                    assertion: rejected
                },
                budget()
            )
            .is_err()
    );
    fs::remove_file(f.dir.path().join("entry.a")).unwrap();
    fs::remove_file(f.dir.path().join("entry.o")).unwrap();
    let moved = f.dir.path().join("moved");
    fs::rename(&f.project, &moved).unwrap();
    let mut out = f
        .app
        .query(
            &moved,
            app::ReadQuery::ReviewedData {
                revision: head,
                assertion: new_id,
            },
            budget(),
        )
        .unwrap();
    out.export_data(&f.dir.path().join("accepted-export"), &|| false)
        .unwrap();
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
    let mut bad = proposal(&f);
    let DataLayout::Integer(layout) = &mut bad.layout else {
        panic!("integer layout")
    };
    layout.count = 3;
    let run = f
        .app
        .start_propose_data(&f.project, bad, budget())
        .unwrap()
        .wait();
    assert_eq!(run.state, RunState::Failed);
    assert!(
        collect(
            &f,
            app::ReadQuery::Knowledge {
                revision: None,
                history: false
            }
        )
        .knowledge
        .is_empty()
    );
    assert_eq!(
        app::inventory(&f.project, None).unwrap().revision_id,
        current
    );
}
#[test]
fn relocated_data_retains_raw_evidence_without_inventing_integer_values() {
    let f = fixture(data_object(true), false);
    let p = proposal(&f);
    let proposed = f
        .app
        .start_propose_data(&f.project, p, budget())
        .unwrap()
        .wait();
    let (revision, assertion) = review(&f, proposed, ReviewDecision::Accept, None);
    let records = collect(
        &f,
        app::ReadQuery::ReviewedData {
            revision,
            assertion,
        },
    )
    .data;
    assert!(
        records
            .iter()
            .any(|r| matches!(r, DataRecord::Relocation { .. }))
    );
    assert!(
        records
            .iter()
            .any(|r| matches!(r, DataRecord::Unresolved { .. }))
    );
    assert!(
        !records
            .iter()
            .any(|r| matches!(r, DataRecord::Integer { .. }))
    );
}
#[test]
fn constants_require_exact_known_analysis_operand_and_export_instruction_evidence() {
    let f = fixture(data_object(false), false);
    let result = analyze(&f);
    assert_eq!(result.state, RunState::Completed, "{result:?}");
    let id = result.analysis.unwrap();
    let mut req = request(&f, &[]);
    req.analyses.push(id.clone());
    let observations = collect(&f, app::ReadQuery::Data { request: req }).data;
    let ordinal = observations
        .iter()
        .find_map(|r| match r {
            DataRecord::Analysis {
                ordinal, record, ..
            } if matches!(
                &**record,
                FunctionRecord::Value {
                    value: AbstractValue::Constant { value: 42 },
                    ..
                }
            ) =>
            {
                Some(*ordinal)
            }
            _ => None,
        })
        .unwrap();
    let mut p = ConstantProposalRequest {
        analysis: id,
        record: ordinal,
        operand: ConstantOperand::Value,
        value: 41,
        subject: "answer".to_owned().try_into().unwrap(),
        purpose: "fixture constant".into(),
        applicability: "selected function".into(),
        expected_base: None,
        actor: "reviewer".into(),
        reason: "instruction evidence".into(),
    };
    assert_eq!(
        f.app
            .start_propose_constant(&f.project, p.clone(), budget())
            .unwrap()
            .wait()
            .state,
        RunState::Failed
    );
    p.value = 42;
    let proposed = f
        .app
        .start_propose_constant(&f.project, p, budget())
        .unwrap()
        .wait();
    let (revision, assertion) = review(&f, proposed, ReviewDecision::Accept, None);
    let records = collect(
        &f,
        app::ReadQuery::ReviewedData {
            revision,
            assertion,
        },
    )
    .data;
    assert!(records.iter().any(|r| matches!(r, DataRecord::Analysis { record, .. } if matches!(&**record, FunctionRecord::Instruction { .. }))));
    assert!(
        !records
            .iter()
            .any(|r| matches!(r, DataRecord::Bytes { .. } | DataRecord::Integer { .. }))
    );
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
fn data_delivery_cancellation_and_original_work_budget_do_not_publish() {
    let f = fixture(data_object(false), false);
    let req = request(&f, &[b"table"]);
    let mut output = f
        .app
        .query(&f.project, app::ReadQuery::Data { request: req }, budget())
        .unwrap();
    let destination = f.dir.path().join("cancelled");
    assert_eq!(
        output.export_data(&destination, &|| true).unwrap_err().code,
        ErrorCode::Cancelled
    );
    assert!(!destination.exists());
    let mut limited = budget();
    limited.max_work_units = Some(1);
    let run = f
        .app
        .start_propose_data(&f.project, proposal(&f), limited)
        .unwrap()
        .wait();
    assert_eq!(run.state, RunState::ResourceLimited);
    assert_eq!(run.error.unwrap().code, ErrorCode::ResourceLimited);
    assert!(run.knowledge.is_none());
    assert!(run.resolved_operation.is_none());
    assert!(
        collect(
            &f,
            app::ReadQuery::Knowledge {
                revision: None,
                history: false
            }
        )
        .knowledge
        .is_empty()
    );
}

#[test]
fn reviewed_integer_width_byte_order_and_stride_preserve_signed_values() {
    for width in [1, 2, 4, 8] {
        for order in [DataByteOrder::Little, DataByteOrder::Big] {
            let mut bytes = Vec::new();
            for value in [-5i64, 3] {
                let encoded = match order {
                    DataByteOrder::Little => value.to_le_bytes(),
                    DataByteOrder::Big => value.to_be_bytes(),
                };
                let slice = match order {
                    DataByteOrder::Little => &encoded[..width],
                    DataByteOrder::Big => &encoded[8 - width..],
                };
                bytes.extend_from_slice(slice);
                if value == -5 {
                    bytes.push(0xa5);
                }
            }
            let f = fixture(data_object_bytes(false, &bytes), false);
            let mut p = proposal(&f);
            let DataLayout::Integer(layout) = &mut p.layout else {
                panic!("integer layout")
            };
            layout.encoding.width = width as u8;
            layout.encoding.byte_order = order;
            layout.stride = width as u64 + 1;
            let proposed = f
                .app
                .start_propose_data(&f.project, p, budget())
                .unwrap()
                .wait();
            let (revision, assertion) = review(&f, proposed, ReviewDecision::Accept, None);
            let records = collect(
                &f,
                app::ReadQuery::ReviewedData {
                    revision,
                    assertion,
                },
            )
            .data;
            let values: Vec<_> = records
                .iter()
                .filter_map(|r| {
                    if let DataRecord::Integer { signed, .. } = r {
                        *signed
                    } else {
                        None
                    }
                })
                .collect();
            assert_eq!(values, [-5, 3], "width={width} order={order:?}");
        }
    }
}

#[test]
fn generic_captured_table_occurrences_match_specialized_review_and_export() {
    use object::{Object as _, ObjectSection as _};
    for thin in [false, true] {
        let raw = data_object(false);
        let f = fixture(raw.clone(), thin);
        let req = request(&f, &[b"table"]);
        let DataSelector::Symbol { symbol, .. } = &req.ranges[0] else {
            panic!()
        };
        let elf = object::File::parse(raw.as_slice()).unwrap();
        let section = elf.section_by_name(".rodata.table").unwrap();
        let DataLayout::Integer(layout) = proposal(&f).layout else {
            panic!("integer layout")
        };
        let mut occurrence = req.occurrence;
        occurrence.symbol = Some(symbol.clone());
        let selector = DataSelector::Section {
            section: section.index().0 as u32,
            offset: 0,
            length: 4,
        };
        let valid = KnowledgeProposal {
            subject: "captured.table".to_owned().try_into().unwrap(),
            occurrence,
            claim: KnowledgeClaim::IntegerTable {
                selector: selector.clone(),
                layout: layout.clone(),
                purpose: "fixture coefficients".into(),
                applicability: "exact object".into(),
            },
            evidence: vec![EvidenceRef::Source {
                payload: ArtifactId::of_bytes(&raw),
                range: CodeRange {
                    start: section.file_range().unwrap().0,
                    length: 4,
                },
            }],
            note: None,
        };
        for table in [SymbolTableKind::Static, SymbolTableKind::Dynamic] {
            let mut invalid = valid.clone();
            let sym = invalid.occurrence.symbol.as_mut().unwrap();
            sym.index = u64::MAX;
            sym.table = table;
            let failed = f
                .app
                .start_knowledge(
                    &f.project,
                    &KnowledgeChange {
                        expected_base: None,
                        actor: "test".into(),
                        reason: "absent symbol".into(),
                        action: KnowledgeAction::Propose { proposal: invalid },
                    },
                    budget(),
                )
                .unwrap()
                .wait();
            assert_eq!(failed.state, RunState::Failed, "{failed:?}");
            assert!(failed.knowledge.is_none());
            assert!(
                collect(
                    &f,
                    app::ReadQuery::Knowledge {
                        revision: None,
                        history: false
                    }
                )
                .knowledge
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
                    reason: "exact source evidence".into(),
                    action: KnowledgeAction::Propose { proposal: valid },
                },
                budget(),
            )
            .unwrap()
            .wait();
        let (revision, assertion) = review(&f, proposed, ReviewDecision::Accept, None);
        fs::remove_file(f.dir.path().join("entry.o")).unwrap();
        fs::remove_file(f.dir.path().join("entry.a")).unwrap();
        let mut output = f
            .app
            .query(
                &f.project,
                app::ReadQuery::ReviewedData {
                    revision,
                    assertion,
                },
                budget(),
            )
            .unwrap();
        let destination = f.dir.path().join("generic-export");
        output.export_data(&destination, &|| false).unwrap();
        assert_eq!(
            fs::read(destination.join("data.bin")).unwrap(),
            [0xfb, 0xff, 3, 0]
        );
    }
}

#[test]
fn integer_ranges_require_known_nonoverlapping_relocation_writes() {
    use object::{Object as _, ObjectSection as _};
    // R_RISCV_32 ends exactly at the range start, starts exactly at its end,
    // or intersects it; R_RISCV_64 crosses the start. Unknown widths never
    // establish disjointness. NONE supplies no write at all.
    for (site, kind, overlap, unknown) in [
        (0, object::elf::R_RISCV_32, 0, 0),
        (8, object::elf::R_RISCV_32, 0, 0),
        (4, object::elf::R_RISCV_32, 1, 0),
        (0, object::elf::R_RISCV_64, 1, 0),
        (0, object::elf::R_RISCV_HI20, 0, 1),
        (0, object::elf::R_RISCV_NONE, 0, 0),
    ] {
        let raw = data_object_relocation(
            &[0, 0, 0, 0, 0xfb, 0xff, 3, 0, 0, 0, 0, 0],
            Some((site, kind)),
        );
        let elf = object::File::parse(raw.as_slice()).unwrap();
        let index = elf.section_by_name(".rodata.table").unwrap().index().0 as u32;
        let f = fixture(raw.clone(), false);
        let mut request = proposal(&f);
        request.selector = DataSelector::Section {
            section: index,
            offset: 4,
            length: 4,
        };
        let proposed = f
            .app
            .start_propose_data(&f.project, request, budget())
            .unwrap()
            .wait();
        let (revision, assertion) = review(&f, proposed, ReviewDecision::Accept, None);
        fs::remove_file(f.dir.path().join("entry.o")).unwrap();
        fs::remove_file(f.dir.path().join("entry.a")).unwrap();
        let query = app::ReadQuery::ReviewedData {
            revision,
            assertion,
        };
        let records = collect(&f, query.clone()).data;
        assert_eq!(
            records
                .iter()
                .filter(|r| matches!(r, DataRecord::Relocation { .. }))
                .count(),
            1
        );
        let values: Vec<_> = records
            .iter()
            .filter_map(|r| match r {
                DataRecord::Integer { signed, .. } => *signed,
                _ => None,
            })
            .collect();
        if overlap == 0 && unknown == 0 {
            assert_eq!(values, [-5, 3]);
            assert!(
                !records
                    .iter()
                    .any(|r| matches!(r, DataRecord::Unresolved { .. }))
            );
        } else {
            assert!(values.is_empty());
            assert!(
                records
                    .iter()
                    .any(|r| matches!(r, DataRecord::Unresolved { .. }))
            );
        }
        let mut output = f.app.query(&f.project, query, budget()).unwrap();
        let destination = f.dir.path().join("range-export");
        output.export_data(&destination, &|| false).unwrap();
        let manifest: DataManifest =
            serde_json::from_slice(&fs::read(destination.join("manifest.json")).unwrap()).unwrap();
        assert_eq!(manifest.schema, 3);
        assert_eq!(manifest.spans[0].section_relocations, 1);
        assert_eq!(manifest.spans[0].overlapping_relocations, overlap);
        assert_eq!(manifest.spans[0].unknown_relocation_extents, unknown);
        assert_eq!(
            fs::read(destination.join("data.bin")).unwrap(),
            [0xfb, 0xff, 3, 0]
        );
    }
}

#[test]
fn invalid_relocation_write_extent_cannot_publish_an_unaffected_table() {
    use object::{Object as _, ObjectSection as _};
    let raw = data_object_relocation(&[0; 12], Some((10, object::elf::R_RISCV_32)));
    let elf = object::File::parse(raw.as_slice()).unwrap();
    let section = elf.section_by_name(".rodata.table").unwrap().index().0 as u32;
    let f = fixture(raw.clone(), false);
    let mut p = proposal(&f);
    p.selector = DataSelector::Section {
        section,
        offset: 0,
        length: 4,
    };
    let run = f
        .app
        .start_propose_data(&f.project, p, budget())
        .unwrap()
        .wait();
    assert_eq!(run.state, RunState::Failed, "{run:?}");
    assert!(run.knowledge.is_none());
    assert!(
        run.error
            .unwrap()
            .message
            .contains("relocation write exceeds")
    );
    assert!(
        collect(
            &f,
            app::ReadQuery::Knowledge {
                revision: None,
                history: false
            }
        )
        .knowledge
        .is_empty()
    );
}

#[test]
fn dynamic_occurrences_keep_physical_indices_through_review_export_and_reopen() {
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
        let table = symbols
            .iter()
            .find(|s| s.id.table == SymbolTableKind::Dynamic && s.name.as_deref() == Some(b"table"))
            .unwrap();
        let table = table.id.clone();
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
        let mut p = proposal(&f);
        p.selector = DataSelector::Symbol {
            symbol: table.clone(),
            length: None,
        };
        p.occurrence.symbol = Some(table.clone());
        for bad in [
            SymbolId {
                index: u64::MAX,
                ..table.clone()
            },
            SymbolId {
                table: SymbolTableKind::Static,
                ..table.clone()
            },
            SymbolId {
                table_section: 0,
                ..table.clone()
            },
        ] {
            let mut invalid = p.clone();
            invalid.occurrence.symbol = Some(bad);
            let run = f
                .app
                .start_propose_data(&f.project, invalid, budget())
                .unwrap()
                .wait();
            assert_eq!(run.state, RunState::Failed, "{run:?}");
            assert!(run.knowledge.is_none());
        }
        let proposed = f
            .app
            .start_propose_data(&f.project, p, budget())
            .unwrap()
            .wait();
        let (revision, assertion) = review(&f, proposed, ReviewDecision::Accept, None);
        let values = collect(
            &f,
            app::ReadQuery::ReviewedData {
                revision: revision.clone(),
                assertion: assertion.clone(),
            },
        )
        .data;
        let integers: Vec<_> = values
            .iter()
            .filter_map(|r| match r {
                DataRecord::Integer { signed, .. } => *signed,
                _ => None,
            })
            .collect();
        assert_eq!(integers, [-5, 3]);
        // New application instance reads captured source and accepted knowledge.
        f.app = application(&f.dir.path().join("reopened-runtime"));
        let mut output = f
            .app
            .query(
                &f.project,
                app::ReadQuery::ReviewedData {
                    revision,
                    assertion,
                },
                budget(),
            )
            .unwrap();
        let destination = f.dir.path().join("dynamic-export");
        output.export_data(&destination, &|| false).unwrap();
        assert_eq!(
            fs::read(destination.join("data.bin")).unwrap(),
            [0xfb, 0xff, 3, 0]
        );
        let manifest: DataManifest =
            serde_json::from_slice(&fs::read(destination.join("manifest.json")).unwrap()).unwrap();
        assert_eq!(manifest.request.occurrence.symbol, Some(table));
        f.app
            .query(
                &f.project,
                app::ReadQuery::Analysis {
                    id: analysis_id,
                    export: false,
                },
                budget(),
            )
            .unwrap();
    }
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

#[test]
fn ambiguous_symbol_tables_and_dynamic_relocations_fail_explicitly() {
    let duplicate =
        support::dynamic_symbols(support::dynamic_symbols(data_object(false), false), false);
    let mut dynamic_relocation = support::dynamic_symbols(data_object(true), false);
    let shoff = u32::from_le_bytes(dynamic_relocation[32..36].try_into().unwrap()) as usize;
    let count = u16::from_le_bytes(dynamic_relocation[48..50].try_into().unwrap()) as usize;
    for i in 1..count {
        let at = shoff + i * 40;
        if u32::from_le_bytes(dynamic_relocation[at + 4..at + 8].try_into().unwrap())
            == object::elf::SHT_RELA
        {
            dynamic_relocation[at + 24..at + 28]
                .copy_from_slice(&((count - 1) as u32).to_le_bytes());
        }
    }
    for (bytes, expected) in [
        (duplicate, ErrorCode::InvalidRequest),
        (dynamic_relocation, ErrorCode::Incompatible),
    ] {
        let f = fixture(bytes, false);
        let result = f
            .app
            .start_propose_data(&f.project, proposal(&f), budget())
            .unwrap()
            .wait();
        assert_eq!(result.state, RunState::Failed, "{result:?}");
        assert_eq!(result.error.unwrap().code, expected);
        assert!(result.knowledge.is_none());
        assert_eq!(
            app::inventory(&f.project, None).unwrap().revision_id,
            f.revision
        );
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
fn pointer_tables_keep_null_addresses_defined_and_external_symbols_through_review_and_export() {
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
        let mut p = proposal(&f);
        p.layout = DataLayout::Pointers(req.pointer_table.unwrap());
        let proposed = f
            .app
            .start_propose_data(&f.project, p, budget())
            .unwrap()
            .wait();
        let (base, assertion) = review(&f, proposed, ReviewDecision::Accept, None);
        fs::remove_file(f.dir.path().join("entry.a")).unwrap();
        fs::remove_file(f.dir.path().join("entry.o")).unwrap();
        let mut output = application(&f.dir.path().join("reader"))
            .query(
                &f.project,
                app::ReadQuery::ReviewedData {
                    revision: base,
                    assertion,
                },
                budget(),
            )
            .unwrap();
        let destination = f.dir.path().join("pointer-export");
        output.export_data(&destination, &|| false).unwrap();
        let manifest: DataManifest =
            serde_json::from_slice(&fs::read(destination.join("manifest.json")).unwrap()).unwrap();
        assert_eq!(
            manifest.pointer_producer.as_deref(),
            Some("rv32-absolute-rela/1")
        );
        assert_eq!(
            manifest.pointers.unwrap(),
            PointerSummary {
                entries: 4,
                nulls: 1,
                addresses: 1,
                defined_symbols: 1,
                external_symbols: 1,
                unresolved: 0
            }
        );
        let exported: Vec<DataRecord> = fs::read_to_string(destination.join("records.jsonl"))
            .unwrap()
            .lines()
            .map(|s| serde_json::from_str(s).unwrap())
            .collect();
        assert_eq!(pointers(&exported), values);
        assert_eq!(
            exported
                .iter()
                .filter(|r| matches!(r, DataRecord::Relocation { .. }))
                .count(),
            2
        );
        assert_eq!(
            fs::read(destination.join("data.bin")).unwrap(),
            [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x60]
        );
    }
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
#[test]
fn invalid_pointer_layouts_and_budget_exhaustion_publish_nothing() {
    let f = fixture(pointer_object(&[]), false);
    for layout in [
        PointerTable {
            count: 0,
            stride: 4,
        },
        PointerTable {
            count: 4,
            stride: 3,
        },
        PointerTable {
            count: u64::MAX,
            stride: 4,
        },
        PointerTable {
            count: 3,
            stride: 4,
        },
    ] {
        let mut p = proposal(&f);
        p.layout = DataLayout::Pointers(layout.clone());
        let run = f
            .app
            .start_propose_data(&f.project, p, budget())
            .unwrap()
            .wait();
        assert_eq!(run.state, RunState::Failed, "{run:?}");
        assert!(run.knowledge.is_none());
        let mut req = request(&f, &[b"table"]);
        req.pointer_table = Some(layout);
        assert!(
            f.app
                .query(&f.project, app::ReadQuery::Data { request: req }, budget())
                .is_err()
        );
    }
    let mut req = request(&f, &[b"table"]);
    req.pointer_table = Some(PointerTable {
        count: 4,
        stride: 4,
    });
    let mut limited = budget();
    limited.max_work_units = Some(1);
    let result = f
        .app
        .query(&f.project, app::ReadQuery::Data { request: req }, limited);
    assert_eq!(result.err().unwrap().code, ErrorCode::ResourceLimited);
    assert_eq!(
        app::inventory(&f.project, None).unwrap().revision_id,
        f.revision
    );
    assert!(
        collect(
            &f,
            app::ReadQuery::Knowledge {
                revision: None,
                history: false
            }
        )
        .knowledge
        .is_empty()
    );
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
