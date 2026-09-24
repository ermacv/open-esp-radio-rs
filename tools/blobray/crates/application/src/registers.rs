//! Saved MMIO catalogue. Knowledge applicability and scope acquisition stay here.
use crate::*;
use std::cell::RefCell;
mod index;

fn invalid(s: &str) -> Error {
    Error::new(ErrorCode::InvalidRequest, s)
}
#[derive(Clone, Copy)]
struct Sample {
    address: u32,
    width: u8,
    composed: bool,
    mask: bool,
    access: bool,
}
fn range(claim: &KnowledgeClaim) -> Option<(u64, u64)> {
    match claim {
        KnowledgeClaim::MmioRegister { register } => Some((
            register.address.into(),
            u64::from(register.address) + u64::from(register.width),
        )),
        KnowledgeClaim::MmioRegion { region } => {
            Some((region.range.start.into(), region.range.end()?))
        }
        _ => None,
    }
}
fn declaration_bytes(entry: &KnowledgeEntry) -> u64 {
    let p = &entry.proposal;
    let claim = match &p.claim {
        KnowledgeClaim::MmioRegion { region } => region.name.capacity() as u64,
        KnowledgeClaim::MmioRegister { register } => {
            register.name.capacity() as u64
                + (register.fields.capacity() * std::mem::size_of::<MmioField>()) as u64
                + register
                    .fields
                    .iter()
                    .map(|f| f.name.capacity() as u64)
                    .sum::<u64>()
        }
        _ => 0,
    };
    std::mem::size_of::<KnowledgeEntry>() as u64
        + claim
        + p.subject.allocated_bytes()
        + p.occurrence.source.allocated_bytes()
        + p.occurrence.object.artifact.allocated_bytes()
        + 5 * 64
        + p.evidence.capacity() as u64 * (std::mem::size_of::<EvidenceRef>() as u64 + 64)
        + p.note.as_ref().map_or(0, |s| s.capacity() as u64)
}
pub(crate) fn query(
    project: &Project,
    request: &RegisterQuery,
    memory: &WorkingMemory,
    c: &mut dyn RunControl,
    emit: &mut dyn FnMut(&RegisterRecord, &mut dyn RunControl) -> Result<()>,
) -> Result<RegisterSummary> {
    if request.ranges.len() > 256
        || request
            .ranges
            .iter()
            .any(|r| r.length == 0 || r.end().is_none())
    {
        return Err(invalid(
            "register query requires at most 256 valid address intervals",
        ));
    }
    // One cloned bounded knowledge/analysis row during serialization. The snapshot
    // and retained sample array have separate reservations.
    let _envelope = memory.reserve(4 * 1024 * 1024, c.position())?;
    let snapshot = project.knowledge_snapshot(request.scope.knowledge.as_ref(), memory, c)?;
    let mut declarations = AdmittedVec::new(memory);
    let mut summary = RegisterSummary {
        schema: 1,
        request: request.clone(),
        selected_analyses: 0,
        partial_analyses: 0,
        unavailable_entries: 0,
        declarations: 0,
        conflicts: 0,
        observations: 0,
        unresolved_addresses: 0,
        alternative_observations: 0,
        candidate_addresses: 0,
        matched_accepted: 0,
    };
    for entry in snapshot.entries() {
        c.checkpoint(1)?;
        if entry.proposal.occurrence.revision == request.scope.revision
            && range(&entry.proposal.claim).is_some()
        {
            blobray_knowledge::validate_proposal(&entry.proposal)?;
            let _clone = memory.reserve(declaration_bytes(entry), c.position())?;
            emit(
                &RegisterRecord::Declaration {
                    entry: Box::new(entry.clone()),
                },
                c,
            )?;
            declarations.push(entry, c.position())?;
            summary.declarations += 1;
        }
    }
    let index = index::Index::new(&declarations, memory, c)?;
    drop(declarations);
    index.conflicts(memory, c, &mut |left, right, c| {
        summary.conflicts += 1;
        emit(
            &RegisterRecord::Conflict {
                left: left.id.clone(),
                right: right.id.clone(),
            },
            c,
        )
    })?;
    let mut samples = AdmittedVec::new(memory);
    let output = RefCell::new(emit);
    let navigation = NavigationQuery {
        scope: NavigationScope {
            knowledge: None,
            ..request.scope.clone()
        },
        filter: NavigationFilter::Functions { function: None },
    };
    let nav = crate::navigation::query_inspected(
        project,
        &navigation,
        memory,
        c,
        &mut |_, _, _, _| Ok(()),
        &mut |function, manifest, records, facts, c| {
            c.phase(RunPhase::AnalyzeValues)?;
            let mut observe = |record: u64,
                               width: u8,
                               address: &AbstractValue,
                               composed: bool,
                               mask: Option<RegisterMask>,
                               c: &mut dyn RunControl| {
                let fact = &records[record as usize];
                let mut candidate =
                    |address: Option<u32>, alternative: Option<u8>, c: &mut dyn RunControl| {
                        c.checkpoint(request.ranges.len() as u64 + 1)?;
                        if let Some(address) = address
                            && !request.ranges.is_empty()
                            && !request.ranges.iter().any(|r| {
                                u64::from(address) < r.end().unwrap_or(0)
                                    && u64::from(r.start) < u64::from(address) + u64::from(width)
                            })
                        {
                            return Ok(());
                        }
                        summary.observations += 1;
                        summary.unresolved_addresses += u64::from(address.is_none());
                        summary.alternative_observations += u64::from(alternative.is_some());
                        output.borrow_mut()(
                            &RegisterRecord::Observation {
                                function: function.clone(),
                                record,
                                fact: Box::new(fact.clone()),
                                address,
                                alternative,
                                mask,
                            },
                            c,
                        )?;
                        if let Some(address) = address {
                            samples.push(
                                Sample {
                                    address,
                                    width,
                                    composed,
                                    mask: mask.is_some(),
                                    access: matches!(
                                        fact,
                                        FunctionRecord::MemoryAccess { .. }
                                            | FunctionRecord::CalleeEffect { .. }
                                    ),
                                },
                                c.position(),
                            )?;
                            // Only local facts establish applicability to this function's object.
                            // A composed effect retains its child evidence but cannot inherit the caller's declarations.
                            if !composed {
                                index.bindings(
                                    &manifest.recipe.source,
                                    manifest.recipe.selector.object(),
                                    u64::from(address),
                                    u64::from(address) + u64::from(width),
                                    c,
                                    &mut |entry, start, end, c| {
                                        let relation = if matches!(
                                            entry.proposal.claim,
                                            KnowledgeClaim::MmioRegion { .. }
                                        ) {
                                            RegisterMatchKind::Region
                                        } else if u64::from(address) >= start
                                            && u64::from(address) + u64::from(width) <= end
                                        {
                                            RegisterMatchKind::ContainedAccess
                                        } else {
                                            RegisterMatchKind::CrossingAccess
                                        };
                                        summary.matched_accepted +=
                                            u64::from(entry.state == AssertionState::Accepted);
                                        output.borrow_mut()(
                                            &RegisterRecord::Binding {
                                                function: function.clone(),
                                                record,
                                                address,
                                                assertion: entry.id.clone(),
                                                state: entry.state,
                                                relation,
                                            },
                                            c,
                                        )
                                    },
                                )?;
                            }
                        }
                        Ok(())
                    };
                candidates(address, c, &mut candidate)
            };
            facts.accesses(c, &mut |access, c| {
                let mask = access.value.and_then(|v| {
                    blobray_analysis::registers::write_mask(facts, access.address, access.width, v)
                });
                observe(
                    access.record,
                    access.width,
                    access.address,
                    access.origin.is_some(),
                    mask,
                    c,
                )
            })?;
            for (record, fact) in records.iter().enumerate() {
                c.checkpoint(1)?;
                if let FunctionRecord::Expression {
                    expression, origin, ..
                } = fact
                    && let Some((address, width, mask)) =
                        blobray_analysis::registers::read_mask(facts, expression)
                {
                    observe(
                        record as u64,
                        width,
                        address,
                        origin.is_some(),
                        Some(mask),
                        c,
                    )?;
                }
            }
            Ok(())
        },
        &mut |record, c| {
            output.borrow_mut()(
                &RegisterRecord::Scope {
                    record: Box::new(record.clone()),
                },
                c,
            )
        },
    )?;
    summary.selected_analyses = nav.selected_analyses;
    summary.partial_analyses = nav.partial_analyses;
    summary.unavailable_entries = nav.unavailable_entries;
    c.checkpoint(samples.len() as u64 * (samples.len().max(1).ilog2() as u64 + 1))?;
    samples.sort_unstable_by_key(|s| (s.address, s.width));
    let mut start = 0;
    while start < samples.len() {
        let address = samples[start].address;
        let mut end = start;
        let mut widths = Vec::with_capacity(256);
        let (mut local, mut composed, mut masks) = (0, 0, 0);
        while end < samples.len() && samples[end].address == address {
            c.checkpoint(1)?;
            let s = samples[end];
            if widths.last() != Some(&s.width) {
                widths.push(s.width);
            }
            masks += u64::from(s.mask);
            local += u64::from(s.access && !s.composed);
            composed += u64::from(s.access && s.composed);
            end += 1;
        }
        output.borrow_mut()(
            &RegisterRecord::Address {
                address,
                access_widths: widths,
                local_accesses: local,
                composed_effects: composed,
                mask_observations: masks,
            },
            c,
        )?;
        summary.candidate_addresses += 1;
        start = end;
    }
    Ok(summary)
}
type CandidateSink<'a> = dyn FnMut(Option<u32>, Option<u8>, &mut dyn RunControl) -> Result<()> + 'a;
fn candidates(
    value: &AbstractValue,
    c: &mut dyn RunControl,
    emit: &mut CandidateSink<'_>,
) -> Result<()> {
    match value {
        AbstractValue::Constant { value } | AbstractValue::ImageAddress { address: value } => {
            emit(Some(*value), None, c)
        }
        AbstractValue::Alternatives { values } => {
            for (i, value) in values.values().iter().enumerate() {
                let address = match value {
                    ValueAlternative::Constant { value }
                    | ValueAlternative::ImageAddress { address: value } => Some(*value),
                    _ => None,
                };
                emit(address, Some(i as u8), c)?;
            }
            Ok(())
        }
        _ => emit(None, None, c),
    }
}
