use super::*;
use object::{Object as _, ObjectSection as _};
const CODE: &[u8] = &[0x13, 0x05, 0xa0, 0x02, 0x67, 0x80, 0, 0];
fn captured_range(thin: bool) -> (Fixture, u32, u64) {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    app::create_project(&project).unwrap();
    let app = application(&dir.path().join("runtime"));
    let mut obj = Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
    let text = obj.add_section(vec![], b".text.exact".to_vec(), SectionKind::Text);
    obj.append_section_data(text, &[&[0xff; 8][..], CODE, &[0xff; 8][..]].concat(), 2);
    // No symbols; the empty writer's SYMTAB is changed to opaque non-allocated
    // bytes so both parser symbol tables are absent, as in a stripped object.
    let mut bytes = obj.write().unwrap();
    let shoff = u32::from_le_bytes(bytes[32..36].try_into().unwrap()) as usize;
    let count = u16::from_le_bytes(bytes[48..50].try_into().unwrap()) as usize;
    for i in 1..count {
        let at = shoff + i * 40;
        if u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap()) == object::elf::SHT_SYMTAB
        {
            bytes[at + 4..at + 8].copy_from_slice(&object::elf::SHT_PROGBITS.to_le_bytes());
        }
    }
    let file = object::File::parse(bytes.as_slice()).unwrap();
    assert!(file.symbols().next().is_none());
    let text = file.section_by_name(".text.exact").unwrap();
    let section = text.index().0 as u32;
    let file_start = text.file_range().unwrap().0 + 8;
    let path = dir.path().join("entry.a");
    fs::write(dir.path().join("entry.o"), &bytes).unwrap();
    fs::write(&path, support::archive(&[(b"entry.o", &bytes)], thin)).unwrap();
    app.import(
        &project,
        vec![app::ImportInput {
            role: "code".into(),
            path,
            expected: None,
        }],
        Target::Riscv32Ilp32,
        budget(),
    )
    .unwrap();
    let inventory = app::inventory(&project, None).unwrap();
    let object = inventory.revision.inputs[0]
        .inventory
        .as_ref()
        .unwrap()
        .objects[0]
        .id
        .clone();
    let revision = inventory.revision_id;
    let request = FunctionRequest {
        revision: Some(revision.clone()),
        source: FunctionSource::Input { input: 0 },
        selector: FunctionSelector::Range {
            object,
            section,
            extent: CodeRange {
                start: 8,
                length: 8,
            },
        },
        extent: None,
    };
    (
        Fixture {
            dir,
            project,
            app,
            revision,
            request,
        },
        section,
        file_start,
    )
}
fn propose(
    f: &Fixture,
    section: u32,
    extent: CodeRange,
    file_start: u64,
) -> Result<app::RunRecord> {
    let inventory = app::inventory(&f.project, None).unwrap();
    let payload = inventory.revision.inputs[0]
        .inventory
        .as_ref()
        .unwrap()
        .objects[0]
        .content
        .clone()
        .unwrap();
    f.app
        .start_knowledge(
            &f.project,
            &KnowledgeChange {
                expected_base: None,
                actor: "reviewer".into(),
                reason: "exact fixture bytes".into(),
                action: KnowledgeAction::Propose {
                    proposal: KnowledgeProposal {
                        note: Some("known fixture instruction boundary".into()),
                        subject: "entry-boundary".to_owned().try_into().unwrap(),
                        occurrence: KnowledgeOccurrence {
                            revision: f.revision.clone(),
                            source: f.request.source.clone(),
                            object: f.request.selector.object().clone(),
                            symbol: None,
                        },
                        claim: KnowledgeClaim::ExecutableRange { section, extent },
                        evidence: vec![EvidenceRef::Source {
                            payload,
                            range: CodeRange {
                                start: file_start,
                                length: 8,
                            },
                        }],
                    },
                },
            },
            budget(),
        )
        .map(|run| run.wait())
}
#[test]
fn invalid_explicit_code_ranges_fail_before_analysis_or_knowledge_publication() {
    let (f, section, file_start) = captured_range(false);
    for (selected_section, extent, override_extent) in [
        (
            section,
            CodeRange {
                start: 9,
                length: 4,
            },
            None,
        ),
        (
            section,
            CodeRange {
                start: 8,
                length: 0,
            },
            None,
        ),
        (
            section,
            CodeRange {
                start: 16,
                length: 16,
            },
            None,
        ),
        (
            0,
            CodeRange {
                start: 0,
                length: 8,
            },
            None,
        ),
        (
            section,
            CodeRange {
                start: 8,
                length: 8,
            },
            Some(CodeRange {
                start: 8,
                length: 4,
            }),
        ),
        (
            section,
            CodeRange {
                start: u64::MAX - 1,
                length: 8,
            },
            None,
        ),
    ] {
        let mut request = f.request.clone();
        request.selector = FunctionSelector::Range {
            object: f.request.selector.object().clone(),
            section: selected_section,
            extent,
        };
        request.extent = override_extent;
        let run = f
            .app
            .start_analyze_function(&f.project, request, budget())
            .unwrap()
            .wait();
        assert_eq!(run.state, RunState::Failed, "{run:?}");
        assert!(run.analysis.is_none());
        if override_extent.is_none() {
            match propose(&f, selected_section, extent, file_start) {
                Ok(result) => {
                    assert_eq!(result.state, RunState::Failed, "{result:?}");
                    assert!(result.knowledge.is_none());
                }
                Err(error) => assert_eq!(error.code, ErrorCode::InvalidRequest),
            }
        }
    }
    assert_eq!(
        app::inventory(&f.project, None).unwrap().revision_id,
        f.revision
    );
}

#[test]
fn explicit_range_planning_rejects_duplicates_and_absent_occurrences() {
    let (f, section, _) = captured_range(false);
    let range = FunctionRange {
        source: f.request.source.clone(),
        object: f.request.selector.object().clone(),
        section,
        extent: CodeRange {
            start: 8,
            length: 8,
        },
    };
    let mut absent = range.clone();
    absent.object.location = ObjectLocation::ArchiveMember { ordinal: 99 };
    for ranges in [vec![range.clone(), range], vec![absent]] {
        let decoder = blobray_backend_riscv::RiscvDecoder;
        let result = f.app.query(
            &f.project,
            app::ReadQuery::PlanInvestigation {
                request: InvestigationRequest {
                    ranges,
                    ..Default::default()
                },
                producer: FunctionProducer {
                    decoder: decoder.identity().into(),
                    semantics: decoder.semantic_identity().into(),
                },
            },
            budget(),
        );
        assert_eq!(result.err().unwrap().code, ErrorCode::InvalidRequest);
    }
    assert_eq!(
        app::inventory(&f.project, None).unwrap().revision_id,
        f.revision
    );
}
