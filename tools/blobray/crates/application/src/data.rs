//! Captured data research and export; no execution, inferred layout, or hardware lookup.
use crate::*;
use std::io::Write;
fn invalid(message: &str) -> Error {
    Error::new(ErrorCode::InvalidRequest, message)
}

/// Resolve an exact object once, then borrow all its data from one prepared owner.
pub(crate) fn with_object<T>(
    project: &Project,
    occurrence: &Occurrence,
    memory: &WorkingMemory,
    c: &mut dyn RunControl,
    consume: impl FnOnce(
        &ArtifactId,
        &mut blobray_artifacts::PreparedObject<'_, '_>,
        &mut dyn RunControl,
    ) -> Result<T>,
) -> Result<T> {
    crate::occurrence::with_source(project, occurrence, memory, c, |capture, c| {
        capture.with_prepared(memory, c, |object, c| consume(capture.payload, object, c))
    })
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
    decoder: Option<&dyn FunctionSemantics>,
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
    if request.pointer_table.is_some() && request.ranges.len() != 1 {
        return Err(invalid(
            "pointer observations require exactly one selected range",
        ));
    }
    let pointer_producer = if request.pointer_table.is_some() {
        Some(decoder.and_then(|d| d.pointer_identity()).ok_or_else(|| {
            Error::new(
                ErrorCode::Incompatible,
                "pointer relocation profile unavailable",
            )
        })?)
    } else {
        None
    };
    let mut pointers = None;
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
                        emit(
                            &DataRecord::Bytes {
                                range: index as u32,
                                offset: (chunk * WORK_BLOCK) as u64,
                                bytes: bytes.to_vec(),
                            },
                            c,
                        )?;
                    }
                    if !spans
                        .iter()
                        .any(|s: &DataSpan| s.section == view.span.section)
                    {
                        for relocation in view.relocations {
                            emit(
                                &DataRecord::Relocation {
                                    section: view.span.section,
                                    relocation: relocation.clone(),
                                },
                                c,
                            )?;
                        }
                    }
                    if let Some(layout) = &request.pointer_table {
                        let image = view.address_space == CodeAddressSpace::Image;
                        let start = view
                            .span
                            .section_range
                            .start
                            .checked_add(if image { view.section_address } else { 0 })
                            .ok_or_else(|| invalid("pointer range address overflow"))?;
                        pointers = Some(blobray_analysis::pointers::analyze(
                            blobray_analysis::pointers::PointerInput {
                                bytes: view.bytes,
                                start,
                                image,
                                unknown_write_extents: view.span.unknown_relocation_extents != 0,
                                max_write_bytes: view.max_relocation_width,
                                relocations: view.relocations,
                            },
                            layout,
                            decoder.unwrap(),
                            c,
                            &mut emit,
                        )?);
                    }
                    let mut span = view.span;
                    span.export_offset = export_offset;
                    export_offset = export_offset
                        .checked_add(span.file_range.length)
                        .ok_or_else(|| invalid("export size overflow"))?;
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
            || *recipe.selector.object() != request.occurrence.object
            || recipe.payload != payload
        {
            return Err(invalid(
                "supporting analysis belongs to another data occurrence",
            ));
        }
        let mut ordinal = 0;
        blobray_store::visit_jsonl(&lease.records, c, |record: FunctionRecord, c| {
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
    let manifest = DataManifest {
        schema: 4,
        request: request.clone(),
        payload,
        spans,
        analyses,
        pointer_producer: pointer_producer.map(str::to_owned),
        pointers,
    };
    let mut out = disk.create(&stage.join("manifest.json"))?;
    write_control_message(&mut out, &manifest)?;
    out.sync_all().map_err(storage_io)?;
    Ok(manifest)
}
