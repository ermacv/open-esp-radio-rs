use super::*;
fn data_object(relocations: bool) -> Vec<u8> {
    data_object_bytes(relocations, &[0xfb, 0xff, 3, 0])
}
fn data_object_bytes(relocations: bool, table_bytes: &[u8]) -> Vec<u8> {
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
    if relocations {
        let external = obj.add_symbol(symbol(
            b"outside",
            SymbolSection::Undefined,
            0,
            SymbolKind::Data,
        ));
        obj.add_relocation(
            data,
            Relocation {
                offset: 0,
                symbol: external,
                addend: 0,
                flags: RelocationFlags::Elf {
                    r_type: object::elf::R_RISCV_32,
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
        occurrence: KnowledgeOccurrence {
            revision: f.revision.clone(),
            source: f.request.source.clone(),
            object: f.request.symbol.object.clone(),
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
        layout: IntegerTable {
            encoding: IntegerEncoding {
                width: 2,
                signed: true,
                byte_order: DataByteOrder::Little,
            },
            count: 2,
            stride: 2,
        },
        purpose: "test coefficients".into(),
        applicability: "exact captured fixture only".into(),
        expected_base: None,
        actor: "reviewer".into(),
        reason: "verified bytes".into(),
    }
}
fn review(
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
    replacement.layout.encoding.signed = false;
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
    bad.layout.count = 3;
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
            p.layout.encoding.width = width as u8;
            p.layout.encoding.byte_order = order;
            p.layout.stride = width as u64 + 1;
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
        let layout = proposal(&f).layout;
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
