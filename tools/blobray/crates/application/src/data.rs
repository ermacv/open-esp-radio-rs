//! Captured data research and export; no execution, inferred layout, or hardware lookup.
use crate::*;
use std::io::Write;
fn invalid(message: &str) -> Error {
    Error::new(ErrorCode::InvalidRequest, message)
}

/// Resolve an exact object once, then borrow all its data from one prepared owner.
pub(crate) fn with_object<T>(
    project: &Project,
    occurrence: &KnowledgeOccurrence,
    memory: &WorkingMemory,
    c: &mut dyn RunControl,
    consume: impl FnOnce(
        &ArtifactId,
        &mut blobray_artifacts::PreparedObject<'_, '_>,
        &mut dyn RunControl,
    ) -> Result<T>,
) -> Result<T> {
    let payload = match &occurrence.source {
        FunctionSource::Input { input } => {
            let scope = if let Some(symbol) = &occurrence.symbol {
                InspectionScope::Symbol {
                    input: *input,
                    symbol: symbol.clone(),
                }
            } else {
                InspectionScope::Object {
                    input: *input,
                    object: occurrence.object.clone(),
                }
            };
            let mut probe = crate::selection::Probe::new(&scope);
            project.read_inventory(Some(&occurrence.revision), memory, c, &mut probe)?;
            if !probe.found {
                return Err(invalid("data occurrence absent from selected revision"));
            }
            probe
                .binding
                .and_then(|b| b.payload)
                .ok_or_else(|| invalid("data object bytes unavailable"))?
        }
        FunctionSource::Image { image } => {
            let image = project.image(image, c)?;
            if image.manifest.plan.recipe.revision != occurrence.revision
                || occurrence.object
                    != (ObjectId {
                        artifact: image.manifest.elf.clone(),
                        location: ObjectLocation::Standalone,
                    })
            {
                return Err(invalid("data image and occurrence differ"));
            }
            image.manifest.elf
        }
    };
    let container = project.open_payload(&occurrence.object.artifact, c)?;
    let run = |source: &dyn ByteSource, c: &mut dyn RunControl| {
        blobray_artifacts::with_prepared_object(source, &payload, memory, c, |object, c| {
            if let Some(symbol) = &occurrence.symbol {
                object.validate_data_symbol(&occurrence.object, symbol)?;
            }
            consume(&payload, object, c)
        })
    };
    match occurrence.object.location {
        ObjectLocation::Standalone => run(&container, c),
        ObjectLocation::ArchiveMember { ordinal } => {
            let mut cursor = MemberCursor::new(&container, c)?;
            while let Some(member) = cursor.next(memory, c)? {
                if member.ordinal == ordinal {
                    return if let Some((offset, length)) = member.payload {
                        run(&SourceRange::new(&container, offset, length)?, c)
                    } else {
                        run(&project.open_payload(&payload, c)?, c)
                    };
                }
            }
            Err(invalid("captured data member missing"))
        }
    }
}

pub(crate) fn propose_data(
    project: &Project,
    request: &DataProposalRequest,
    memory: &WorkingMemory,
    c: &mut dyn RunControl,
) -> Result<KnowledgeChange> {
    let (mut evidence, selector) = with_object(
        project,
        &request.occurrence,
        memory,
        c,
        |payload, object, c| {
            object.with_data(
                &request.occurrence.object,
                &request.selector,
                c,
                |view, _| {
                    Ok((
                        vec![EvidenceRef::Source {
                            payload: payload.clone(),
                            range: view.span.file_range,
                        }],
                        DataSelector::Section {
                            section: view.span.section,
                            offset: view.span.section_range.start,
                            length: view.span.section_range.length,
                        },
                    ))
                },
            )
        },
    )?;
    if request.analyses.len() > 32 {
        return Err(invalid("at most 32 supporting analyses"));
    }
    evidence.extend(request.analyses.iter().map(|id| EvidenceRef::Analysis {
        analysis: id.clone(),
        record: None,
    }));
    Ok(KnowledgeChange {
        expected_base: request.expected_base.clone(),
        actor: request.actor.clone(),
        reason: request.reason.clone(),
        action: KnowledgeAction::Propose {
            proposal: KnowledgeProposal {
                subject: request.subject.clone(),
                occurrence: request.occurrence.clone(),
                claim: KnowledgeClaim::IntegerTable {
                    selector,
                    layout: request.layout.clone(),
                    purpose: request.purpose.clone(),
                    applicability: request.applicability.clone(),
                },
                evidence,
                note: None,
            },
        },
    })
}
pub(crate) fn propose_constant(
    project: &Project,
    request: &ConstantProposalRequest,
    c: &mut dyn RunControl,
) -> Result<KnowledgeChange> {
    let analysis = project.analysis(&request.analysis, c)?;
    let recipe = analysis.manifest.recipe;
    Ok(KnowledgeChange {
        expected_base: request.expected_base.clone(),
        actor: request.actor.clone(),
        reason: request.reason.clone(),
        action: KnowledgeAction::Propose {
            proposal: KnowledgeProposal {
                subject: request.subject.clone(),
                occurrence: KnowledgeOccurrence {
                    revision: recipe.revision,
                    source: recipe.source,
                    object: recipe.symbol.object.clone(),
                    symbol: Some(recipe.symbol),
                },
                claim: KnowledgeClaim::Constant {
                    analysis: request.analysis.clone(),
                    record: request.record,
                    operand: request.operand.clone(),
                    value: request.value,
                    purpose: request.purpose.clone(),
                    applicability: request.applicability.clone(),
                },
                evidence: vec![EvidenceRef::Analysis {
                    analysis: request.analysis.clone(),
                    record: Some(request.record),
                }],
                note: None,
            },
        },
    })
}

/// Physical checks are repeated on proposal, review and export against retained bytes.
pub(crate) fn validate_table(
    payload: &ArtifactId,
    view: &blobray_artifacts::DataView<'_>,
    proposal: &KnowledgeProposal,
) -> Result<()> {
    let KnowledgeClaim::IntegerTable { layout, .. } = &proposal.claim else {
        return Ok(());
    };
    if layout.byte_length() != Some(view.span.file_range.length) {
        return Err(invalid(
            "table layout must describe the exact selected byte range",
        ));
    }
    if !proposal.evidence.contains(&EvidenceRef::Source {
        payload: payload.clone(),
        range: view.span.file_range,
    }) {
        return Err(invalid("table requires exact captured byte evidence"));
    }
    // The interpretation is of captured bytes. Relocations remain observations,
    // never silently applied numeric values; pointer tables need their own profile.
    Ok(())
}
pub(crate) fn validate_constant(
    proposal: &KnowledgeProposal,
    analysis: &FunctionAnalysisId,
    ordinal: u64,
    record: &FunctionRecord,
) -> Result<()> {
    if let KnowledgeClaim::Constant {
        analysis: expected,
        record: index,
        operand,
        value,
        ..
    } = &proposal.claim
        && expected == analysis
        && *index == ordinal
        && operand.select(record) != Some(&AbstractValue::Constant { value: *value })
    {
        return Err(invalid(
            "constant claim does not match a known value at the selected analysis record",
        ));
    }
    Ok(())
}

fn write_record(
    file: &mut blobray_store::TemporaryFile,
    record: &DataRecord,
    c: &mut dyn RunControl,
) -> Result<()> {
    c.checkpoint(1)?;
    write_control_message(&mut *file, record)?;
    file.write_all(b"\n").map_err(storage_io)
}
#[allow(clippy::too_many_arguments)]
pub(crate) fn prepare(
    stage: &Path,
    project: &Project,
    request: &DataRequest,
    accepted: Option<(KnowledgeRevisionId, KnowledgeEntry)>,
    memory: &WorkingMemory,
    disk: &blobray_store::TemporaryBudget,
    c: &mut dyn RunControl,
    emit: &mut dyn FnMut(&DataRecord, &mut dyn RunControl) -> Result<()>,
) -> Result<DataManifest> {
    if request.ranges.len() > 32
        || request.analyses.len() > 32
        || (request.ranges.is_empty() && request.analyses.is_empty())
    {
        return Err(invalid(
            "data research requires 1..32 ranges or analyses (at most 32 of each)",
        ));
    }
    // Bounded request, summary and one JSON record. Payloads stream in WORK_BLOCK chunks.
    let _envelope = memory.reserve(4 * 1024 * 1024, c.position())?;
    let mut bytes_out = disk.create(&stage.join("data.bin"))?;
    let mut records_out = disk.create(&stage.join("records.jsonl"))?;
    let mut emit = |record: &DataRecord, c: &mut dyn RunControl| {
        write_record(&mut records_out, record, c)?;
        emit(record, c)
    };
    let (payload, spans) = with_object(
        project,
        &request.occurrence,
        memory,
        c,
        |payload, object, c| {
            let mut captured = disk.create(&stage.join("object.elf"))?;
            for bytes in object.captured_bytes().chunks(WORK_BLOCK) {
                c.bytes(bytes.len())?;
                captured.write_all(bytes).map_err(storage_io)?;
            }
            captured.sync_all().map_err(storage_io)?;
            let mut spans = Vec::new();
            let mut export_offset = 0u64;
            for (index, selector) in request.ranges.iter().enumerate() {
                object.with_data(&request.occurrence.object, selector, c, |view, c| {
                for (chunk, bytes) in view.bytes.chunks(WORK_BLOCK).enumerate() {
                    c.bytes(bytes.len())?;
                    bytes_out.write_all(bytes).map_err(storage_io)?;
                    emit(&DataRecord::Bytes { range: index as u32, offset: (chunk * WORK_BLOCK) as u64, bytes: bytes.to_vec() }, c)?;
                }
                if !spans.iter().any(|s: &DataSpan| s.section == view.span.section) {
                    for relocation in view.relocations { emit(&DataRecord::Relocation { section: view.span.section, relocation: relocation.clone() }, c)?; }
                }
                if let Some((_, entry)) = &accepted
                    && let KnowledgeClaim::IntegerTable { layout, .. } = &entry.proposal.claim {
                    validate_table(payload, &view, &entry.proposal)?;
                    // Values decode *file initialization bytes*. Their classification is
                    // explicit; a section with relocations supplies bytes and relocations only.
                    if !view.relocations.is_empty() {
                        emit(&DataRecord::Unresolved { range: index as u32, reason: "section contains unapplied relocations; integer values are not resolved".into() }, c)?;
                    } else {
                        for i in 0..layout.count {
                            c.checkpoint(1)?;
                            let offset = i.checked_mul(layout.stride).ok_or_else(|| invalid("table offset overflow"))?;
                            let mut encoded = [0u8; 8];
                            let width = usize::from(layout.encoding.width);
                            let start = usize::try_from(offset).map_err(|_| invalid("table offset overflow"))?;
                            let slice = view.bytes.get(start..start+width).ok_or_else(|| invalid("table element out of bounds"))?;
                            let bits = match layout.encoding.byte_order {
                                DataByteOrder::Little => { encoded[..width].copy_from_slice(slice); u64::from_le_bytes(encoded) },
                                DataByteOrder::Big => { encoded[8-width..].copy_from_slice(slice); u64::from_be_bytes(encoded) },
                            };
                            let signed = layout.encoding.signed.then(|| ((bits << (64-width*8)) as i64) >> (64-width*8));
                            emit(&DataRecord::Integer { index: i, offset, bits, signed }, c)?;
                        }
                    }
                }
                let mut span = view.span;
                span.export_offset = export_offset;
                export_offset = export_offset.checked_add(span.file_range.length).ok_or_else(|| invalid("export size overflow"))?;
                spans.push(span);
                Ok(())
            })?;
            }
            Ok((payload.clone(), spans))
        },
    )?;
    let mut analyses = Vec::new();
    for id in &request.analyses {
        let lease = project.analysis(id, c)?;
        let recipe = &lease.manifest.recipe;
        if recipe.revision != request.occurrence.revision
            || recipe.source != request.occurrence.source
            || recipe.symbol.object != request.occurrence.object
            || recipe.payload != payload
        {
            return Err(invalid(
                "supporting analysis belongs to another data occurrence",
            ));
        }
        let mut ordinal = 0;
        blobray_store::visit_jsonl(&lease.records, c, |record: FunctionRecord, c| {
            if let Some((_, entry)) = &accepted {
                validate_constant(&entry.proposal, id, ordinal, &record)?;
            }
            let mut ranges = Vec::new();
            if let FunctionRecord::Reference {
                target,
                addend: Some(addend),
                known: true,
                ..
            } = &record
            {
                for (index, span) in spans.iter().enumerate() {
                    c.checkpoint(1)?;
                    let base = span.image_address.unwrap_or(span.section_range.start);
                    if target.section == Some(span.section)
                        && target
                            .offset
                            .checked_add_signed(*addend)
                            .is_some_and(|offset| {
                                offset >= base && offset < base + span.section_range.length
                            })
                    {
                        ranges.push(index as u32);
                    }
                }
            }
            emit(
                &DataRecord::Analysis {
                    analysis: id.clone(),
                    ordinal,
                    record: Box::new(record),
                    ranges,
                },
                c,
            )?;
            ordinal += 1;
            Ok(())
        })?;
        analyses.push(lease.manifest);
    }
    bytes_out.sync_all().map_err(storage_io)?;
    records_out.sync_all().map_err(storage_io)?;
    let (knowledge, accepted) = accepted.map_or((None, None), |(revision, entry)| {
        (Some(revision), Some(entry))
    });
    let manifest = DataManifest {
        schema: 1,
        request: request.clone(),
        payload,
        spans,
        analyses,
        knowledge,
        accepted,
    };
    let mut out = disk.create(&stage.join("manifest.json"))?;
    write_control_message(&mut out, &manifest)?;
    out.sync_all().map_err(storage_io)?;
    Ok(manifest)
}

pub(crate) fn accepted_request(
    project: &Project,
    revision: &KnowledgeRevisionId,
    assertion: &AssertionId,
    memory: &WorkingMemory,
    c: &mut dyn RunControl,
) -> Result<(DataRequest, KnowledgeEntry)> {
    let snapshot = project.knowledge_snapshot(Some(revision), memory, c)?;
    let entry = snapshot
        .entries()
        .find(|e| &e.id == assertion)
        .ok_or_else(|| invalid("assertion not found in selected knowledge revision"))?;
    if entry.state != AssertionState::Accepted {
        return Err(invalid(
            "export requires an accepted, non-superseded assertion in the selected revision",
        ));
    }
    let ranges = match &entry.proposal.claim {
        KnowledgeClaim::IntegerTable { selector, .. } => vec![selector.clone()],
        KnowledgeClaim::Constant { .. } => Vec::new(),
        _ => return Err(invalid("assertion is not a table or constant")),
    };
    let mut analyses = Vec::new();
    for evidence in &entry.proposal.evidence {
        if let EvidenceRef::Analysis { analysis, .. } = evidence
            && !analyses.contains(analysis)
        {
            analyses.push(analysis.clone());
        }
    }
    Ok((
        DataRequest {
            occurrence: entry.proposal.occurrence.clone(),
            ranges,
            analyses,
        },
        entry.clone(),
    ))
}
