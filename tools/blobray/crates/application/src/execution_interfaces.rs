//! Resolve selected reviewed contracts once, releasing captured ELF before sessions.
use crate::*;
pub(crate) struct PreparedTable<'m> {
    pub phase: usize,
    pub side: usize,
    pub ordinal: usize,
    pub declaration: RuntimeTable,
    pub contract: InterfaceContract,
    pub root: RuntimeRoot,
    pub definition: ArtifactId,
    _capacity: MemoryReservation<'m>,
}
fn same_object(a: &KnowledgeOccurrence, b: &KnowledgeOccurrence) -> bool {
    a.revision == b.revision && a.source == b.source && a.object == b.object
}
struct Pending<'a> {
    phase: usize,
    side: usize,
    ordinal: usize,
    target: &'a ExecutionTarget,
    input: &'a Invocation,
    table: &'a RuntimeTable,
}
pub(crate) fn prepare<'m>(
    project: &Project,
    request: &ExecutionRequest,
    memory: &'m WorkingMemory,
    c: &mut dyn RunControl,
) -> Result<AdmittedVec<'m, PreparedTable<'m>>> {
    let mut pending = AdmittedVec::new(memory);
    let mut result = AdmittedVec::new(memory);
    for (phase, case) in request.cases.iter().enumerate() {
        for (side, (input, target)) in std::iter::once((&case.vendor, &request.vendor))
            .chain(case.replacement.as_ref().zip(request.replacement.as_ref()))
            .enumerate()
        {
            for (ordinal, table) in input.tables.iter().enumerate() {
                c.checkpoint(1)?;
                pending.push(
                    Pending {
                        phase,
                        side,
                        ordinal,
                        target,
                        input,
                        table,
                    },
                    c.position(),
                )?;
            }
        }
    }
    c.checkpoint(pending.len() as u64 * (pending.len().max(1).ilog2() as u64 + 1))?;
    pending.sort_unstable_by(|a, b| a.table.review.knowledge.cmp(&b.table.review.knowledge));
    // Charge the adjacent group-boundary comparisons, not prefixes of the requests.
    c.checkpoint(pending.len() as u64)?;
    for group in pending.chunk_by(|a, b| a.table.review.knowledge == b.table.review.knowledge) {
        let p = &group[0];
        let snapshot = project.knowledge_snapshot(Some(&p.table.review.knowledge), memory, c)?;
        let mut selected = AdmittedVec::new(memory);
        for q in group {
            c.checkpoint(1)?;
            let entry = snapshot.get(&q.table.review.assertion, c)?.ok_or_else(|| {
                Error::new(
                    ErrorCode::NotFound,
                    "runtime interface assertion absent from selected snapshot",
                )
            })?;
            let occurrence = &entry.proposal.occurrence;
            if entry.state != AssertionState::Accepted
                || !matches!(entry.proposal.claim, KnowledgeClaim::Interface { .. })
            {
                return Err(Error::new(
                    ErrorCode::InvalidRequest,
                    "runtime table requires a selected accepted interface contract",
                ));
            }
            blobray_knowledge::validate_proposal(&entry.proposal)?;
            if occurrence.revision != q.target.revision
                || occurrence.object.location != ObjectLocation::Standalone
                || (occurrence.source != q.target.source
                    && !matches!(occurrence.source,FunctionSource::Input{input} if q.target.companions.contains(&input)))
            {
                return Err(Error::new(
                    ErrorCode::InvalidRequest,
                    "runtime interface occurrence is outside captured target mappings",
                ));
            }
            selected.push((q, entry), c.position())?;
        }
        c.checkpoint(selected.len() as u64 * (selected.len().max(1).ilog2() as u64 + 1))?;
        selected.sort_unstable_by(|(_, a), (_, b)| {
            let a = &a.proposal.occurrence;
            let b = &b.proposal.occurrence;
            (&a.revision, &a.source, &a.object).cmp(&(&b.revision, &b.source, &b.object))
        });
        c.checkpoint(selected.len() as u64)?;
        for group in selected
            .chunk_by(|(_, a), (_, b)| same_object(&a.proposal.occurrence, &b.proposal.occurrence))
        {
            let occurrence = &group[0].1.proposal.occurrence;
            crate::occurrence::with_source(project, occurrence, memory, c, |capture, c| {
                capture.with_prepared(memory, c, |object, c| {
                    for (q, entry) in group {
                        c.checkpoint(1)?;
                        result.push(
                            prepare_one(q, entry, object, capture.payload, memory, c)?,
                            c.position(),
                        )?;
                    }
                    Ok(())
                })
            })?;
        }
    }
    c.checkpoint(result.len() as u64 * (result.len().max(1).ilog2() as u64 + 1))?;
    result.sort_unstable_by_key(|t| (t.phase, t.side, t.ordinal));
    Ok(result)
}

fn prepare_one<'m>(
    q: &Pending<'_>,
    entry: &KnowledgeEntry,
    object: &mut blobray_artifacts::PreparedObject<'_, '_>,
    captured_payload: &ArtifactId,
    memory: &'m WorkingMemory,
    c: &mut dyn RunControl,
) -> Result<PreparedTable<'m>> {
    let occurrence = &entry.proposal.occurrence;
    let KnowledgeClaim::Interface { contract } = &entry.proposal.claim else {
        unreachable!()
    };
    c.checkpoint((contract.slots.len() * q.table.slots.len()) as u64)?;
    if contract.layout_bytes != q.table.seed.length
        || contract.slots.len() != q.table.slots.len()
        || !contract
            .slots
            .iter()
            .all(|s| q.table.slots.iter().any(|slot| slot.offset == s.offset))
    {
        return Err(Error::new(
            ErrorCode::InvalidRequest,
            "runtime placement differs from reviewed layout/slots",
        ));
    }
    for slot in &q.table.slots {
        c.checkpoint(contract.slots.len() as u64 + 1)?;
        let reviewed = contract
            .slots
            .iter()
            .find(|s| s.offset == slot.offset)
            .unwrap();
        if matches!(
            slot.target,
            RuntimeSlotTarget::Model { .. } | RuntimeSlotTarget::Service { .. }
        ) && (reviewed.semantic.is_none() || reviewed.signature.is_none())
        {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "modeled interface slot requires reviewed semantic and ABI binding",
            ));
        }
    }
    for guard in &contract.guards {
        if let InterfaceGuard::CapturedPayload { payload } = guard
            && payload != captured_payload
        {
            return Err(Error::new(
                ErrorCode::Integrity,
                "runtime interface captured-payload guard differs",
            ));
        }
    }
    if let Some(symbol) = &entry.proposal.occurrence.symbol {
        object.validate_data_symbol(&occurrence.object, symbol)?;
    }
    let root = object.runtime_root(&entry.proposal.occurrence, &contract.root, c)?;
    if let RuntimeRoot::EntryWord { entry, .. } = root
        && entry != q.input.entry
    {
        return Err(Error::new(
            ErrorCode::InvalidRequest,
            "runtime word root belongs to another phase entry",
        ));
    }
    let capacity = memory.reserve(
        q.table.payload_bytes() + contract.allocated_bytes() + 64,
        c.position(),
    )?;
    Ok(PreparedTable {
        phase: q.phase,
        side: q.side,
        ordinal: q.ordinal,
        declaration: q.table.clone(),
        contract: (**contract).clone(),
        root,
        definition: q.table.identity(c)?,
        _capacity: capacity,
    })
}
