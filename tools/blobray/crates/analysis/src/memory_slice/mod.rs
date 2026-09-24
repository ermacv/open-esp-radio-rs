//! Bounded local definitions at a saved publication point.
use crate::navigation::Facts;
use blobray_domain::*;
mod cycles;
mod index;
pub(crate) mod locations;
mod reaching;
use index::{Index, writes};
fn invalid(s: &str) -> Error {
    Error::new(ErrorCode::InvalidRequest, s)
}
fn integrity(s: &str) -> Error {
    Error::new(ErrorCode::Integrity, s)
}
fn charge_sort(n: usize, c: &mut dyn RunControl) -> Result<()> {
    c.checkpoint(n as u64 * (n.max(1).ilog2() as u64 + 1))
}
fn valid_width(width: u8) -> Result<()> {
    if matches!(width, 1 | 2 | 4 | 8) {
        Ok(())
    } else {
        Err(invalid("memory span width must be 1, 2, 4 or 8 bytes"))
    }
}
struct Location<'m> {
    value: SliceLocation,
    _capacity: MemoryReservation<'m>,
}
struct State {
    step: reaching::Step,
    record: Option<usize>,
    flags: u8,
}
fn flag(issue: SliceIssue) -> u8 {
    1 << issue as u8
}
const ISSUES: [SliceIssue; 7] = [
    SliceIssue::PartialControlFlow,
    SliceIssue::UnknownWrite,
    SliceIssue::MayAlias,
    SliceIssue::PartialOverlap,
    SliceIssue::DynamicIdentity,
    SliceIssue::CallClobber,
    SliceIssue::UnsupportedSemantics,
];
fn selections<'m>(
    index: &Index<'_>,
    recipe: &FunctionRecipe,
    request: &MemorySliceQuery,
    memory: &'m WorkingMemory,
    c: &mut dyn RunControl,
    unknown: &mut u64,
    emit: &mut dyn FnMut(&MemorySliceRecord, &mut dyn RunControl) -> Result<()>,
) -> Result<AdmittedVec<'m, Location<'m>>> {
    if request.locations.len() > 256 {
        return Err(invalid("too many selected memory locations"));
    }
    let mut out = AdmittedVec::new(memory);
    let mut add = |value: SliceLocation, c: &mut dyn RunControl| -> Result<()> {
        valid_width(value.width)?;
        let capacity = memory.reserve(locations::bytes(&value), c.position())?;
        out.push(
            Location {
                value,
                _capacity: capacity,
            },
            c.position(),
        )
    };
    if request.locations.is_empty() {
        for access in index.accesses.iter() {
            c.checkpoint(1)?;
            if !index.before[access.node] || !writes(access.kind) {
                continue;
            }
            for location in &access.locations {
                c.checkpoint(1)?;
                add(location.clone(), c)?;
            }
            if access.locations.is_empty() || access.issue.is_some() {
                *unknown += 1;
                emit(
                    &MemorySliceRecord::UnknownLocation {
                        record: access.record as u64,
                        issue: access.issue,
                    },
                    c,
                )?;
            }
        }
    } else {
        for selection in &request.locations {
            c.checkpoint(1)?;
            match selection {
                MemorySliceSelection::Access { record } => {
                    c.checkpoint(index.by_record.len().max(1).ilog2() as u64 + 1)?;
                    let found = index
                        .by_record
                        .binary_search_by(|i| (index.accesses[*i].record as u64).cmp(record))
                        .map_err(|_| {
                            invalid("selected location is not a local memory access record")
                        })?;
                    let access = &index.accesses[index.by_record[found]];
                    if access.locations.is_empty() || access.issue.is_some() {
                        return Err(invalid(
                            "selected access has no fully identified location; inspect its unknown-address evidence",
                        ));
                    }
                    for location in &access.locations {
                        add(location.clone(), c)?;
                    }
                }
                MemorySliceSelection::Address { address, width } => add(
                    SliceLocation {
                        address: SliceAddress::Scoped {
                            source: recipe.source.clone(),
                            object: recipe.selector.object().clone(),
                            address: *address,
                        },
                        width: *width,
                    },
                    c,
                )?,
                MemorySliceSelection::Stack { offset, width } => add(
                    SliceLocation {
                        address: SliceAddress::Stack { offset: *offset },
                        width: *width,
                    },
                    c,
                )?,
                MemorySliceSelection::Argument {
                    word,
                    offset,
                    width,
                } => {
                    if request.abi.is_none() || *word >= 64 {
                        return Err(invalid(
                            "argument location needs an explicit ABI and word below 64",
                        ));
                    }
                    add(
                        SliceLocation {
                            address: SliceAddress::Path {
                                path: AccessPath {
                                    root: AccessRoot::EntryWord {
                                        function: recipe.selector.clone(),
                                        word: *word,
                                    },
                                    path: vec![],
                                    offset: *offset,
                                },
                            },
                            width: *width,
                        },
                        c,
                    )?;
                }
            }
        }
    }
    charge_sort(out.len(), c)?;
    out.sort_unstable_by(|a, b| a.value.cmp(&b.value));
    let mut kept = 0;
    for read in 0..out.len() {
        c.checkpoint(1)?;
        if kept == 0 || out[read].value != out[kept - 1].value {
            out.swap(read, kept);
            kept += 1;
        }
    }
    while out.len() > kept {
        out.pop();
    }
    Ok(out)
}
fn states<'m>(
    index: &Index<'_>,
    location: &SliceLocation,
    memory: &'m WorkingMemory,
    c: &mut dyn RunControl,
) -> Result<AdmittedVec<'m, State>> {
    let mut states = AdmittedVec::new(memory);
    for node in index.instructions.iter() {
        c.checkpoint(1)?;
        let flags = node.clobber.map_or(0, |(_, issue)| flag(issue));
        states.push(
            State {
                step: reaching::Step {
                    barrier: flags != 0,
                    ..Default::default()
                },
                record: None,
                flags,
            },
            c.position(),
        )?;
    }
    for access in index.accesses.iter() {
        c.checkpoint(1)?;
        if !index.before[access.node] || !writes(access.kind) {
            continue;
        }
        let state = &mut states[access.node];
        state.record = Some(access.record);
        if access.issue.is_some() || access.locations.is_empty() {
            state.flags |= flag(SliceIssue::UnknownWrite);
        }
        let mut exact = 0;
        for candidate in &access.locations {
            c.checkpoint(1)?;
            match locations::relation(candidate, location) {
                locations::Relation::Exact => {
                    state.step.definition = true;
                    exact += 1;
                }
                locations::Relation::Covering => {
                    state.step.definition = true;
                    exact += 1;
                    state.flags |= flag(SliceIssue::PartialOverlap);
                }
                locations::Relation::Partial => {
                    state.step.definition = true;
                    state.flags |= flag(SliceIssue::PartialOverlap);
                }
                locations::Relation::Dynamic => {
                    state.step.definition = true;
                    state.flags |= flag(SliceIssue::DynamicIdentity);
                }
                locations::Relation::MayAlias => state.flags |= flag(SliceIssue::MayAlias),
                locations::Relation::Disjoint => (),
            }
        }
        state.step.barrier = state.flags != 0;
        state.step.kills = exact > 0
            && exact == access.locations.len()
            && access.issue.is_none()
            && index.instructions[access.node].clobber.is_none()
            && access.kind != MemoryKind::StoreConditional;
    }
    Ok(states)
}
fn witness<'m>(
    index: &Index<'_>,
    parents: &[Option<usize>],
    start: usize,
    memory: &'m WorkingMemory,
    c: &mut dyn RunControl,
) -> Result<(Vec<u64>, MemoryReservation<'m>)> {
    let mut count = 1usize;
    let mut node = start;
    while node < parents.len() {
        c.checkpoint(1)?;
        count = count
            .checked_add(1)
            .filter(|n| *n <= parents.len() + 1)
            .ok_or_else(|| integrity("cyclic memory-definition witness"))?;
        node =
            parents[node].ok_or_else(|| integrity("memory definition has no structural suffix"))?;
    }
    let capacity = memory.reserve((count * std::mem::size_of::<u64>()) as u64, c.position())?;
    let mut out = Vec::new();
    out.try_reserve_exact(count).map_err(|_| {
        Error::new(
            ErrorCode::ResourceLimited,
            "memory witness allocation refused",
        )
    })?;
    node = start;
    while node < parents.len() {
        c.checkpoint(1)?;
        out.push(index.instructions[node].offset);
        node = parents[node].unwrap();
    }
    out.push(index.instructions[index.anchor].offset);
    Ok((out, capacity))
}
pub fn inspect(
    recipe: &FunctionRecipe,
    coverage: FunctionCoverage,
    records: &[FunctionRecord],
    request: &MemorySliceQuery,
    memory: &WorkingMemory,
    c: &mut dyn RunControl,
    emit: &mut dyn FnMut(&MemorySliceRecord, &mut dyn RunControl) -> Result<()>,
) -> Result<MemorySliceSummary> {
    c.phase(RunPhase::AnalyzeValues)?;
    let _construction = memory.reserve(1024 * 1024, c.position())?;
    let facts = Facts::new(records, memory, c)?;
    let index = Index::new(records, recipe, coverage, request, &facts, memory, c)?;
    let mut summary = MemorySliceSummary {
        schema: 1,
        request: request.clone(),
        anchor_offset: index.instructions[index.anchor].offset,
        locations: 0,
        definitions: 0,
        incoming_locations: 0,
        unknown_incoming_locations: 0,
        barriers: 0,
        unknown_locations: 0,
        coverage,
    };
    let locations = selections(
        &index,
        recipe,
        request,
        memory,
        c,
        &mut summary.unknown_locations,
        emit,
    )?;
    for (ordinal, location) in locations.iter().enumerate() {
        c.checkpoint(1)?;
        let states = states(&index, &location.value, memory, c)?;
        let mut steps = AdmittedVec::new(memory);
        for state in states.iter() {
            steps.push(state.step, c.position())?;
        }
        let reached = reaching::search(
            &index.edges,
            &steps,
            &index.reachable,
            index.entry,
            index.anchor,
            memory,
            c,
        )?;
        let mut flags = if index.closed {
            0
        } else {
            flag(SliceIssue::PartialControlFlow)
        };
        for node in reached.barriers.iter() {
            flags |= states[*node].flags;
        }
        summary.locations += 1;
        summary.definitions += reached.definitions.len() as u64;
        let incoming = if !index.closed || reached.incoming && flags != 0 {
            IncomingState::Unknown
        } else if reached.incoming {
            IncomingState::Possible
        } else {
            IncomingState::Overwritten
        };
        summary.incoming_locations += u64::from(incoming == IncomingState::Possible);
        summary.unknown_incoming_locations += u64::from(incoming == IncomingState::Unknown);
        let issues: Vec<_> = ISSUES
            .into_iter()
            .filter(|i| flags & flag(*i) != 0)
            .collect();
        emit(
            &MemorySliceRecord::Location {
                index: ordinal as u64,
                location: location.value.clone(),
                incoming,
                definitions: reached.definitions.len() as u64,
                issues,
            },
            c,
        )?;
        if !index.closed {
            summary.barriers += 1;
            emit(
                &MemorySliceRecord::Barrier {
                    location: ordinal as u64,
                    record: None,
                    offset: summary.anchor_offset,
                    issue: SliceIssue::PartialControlFlow,
                },
                c,
            )?;
        }
        for &node in reached.barriers.iter() {
            c.checkpoint(1)?;
            for issue in ISSUES {
                if states[node].flags & flag(issue) == 0 {
                    continue;
                }
                let record = if matches!(
                    issue,
                    SliceIssue::CallClobber | SliceIssue::UnsupportedSemantics
                ) {
                    index.instructions[node].clobber.map(|(record, _)| record)
                } else {
                    states[node].record
                };
                summary.barriers += 1;
                emit(
                    &MemorySliceRecord::Barrier {
                        location: ordinal as u64,
                        record: record.map(|r| r as u64),
                        offset: index.instructions[node].offset,
                        issue,
                    },
                    c,
                )?;
            }
        }
        for &node in reached.definitions.iter() {
            c.checkpoint(1)?;
            let record = states[node]
                .record
                .ok_or_else(|| integrity("memory definition has no source record"))?;
            let (witness, _witness) = witness(&index, &reached.parents, node, memory, c)?;
            let class = if flags != 0 {
                DefinitionClass::Candidate
            } else if reached.definitions.len() == 1 && !reached.incoming && states[node].step.kills
            {
                DefinitionClass::Must
            } else {
                DefinitionClass::Alternative
            };
            let fact = &records[record];
            let _output = memory.reserve(
                fact.allocated_bytes() + std::mem::size_of::<FunctionRecord>() as u64,
                c.position(),
            )?;
            emit(
                &MemorySliceRecord::Definition {
                    location: ordinal as u64,
                    record: record as u64,
                    offset: index.instructions[node].offset,
                    class,
                    fact: Box::new(fact.clone()),
                    witness,
                },
                c,
            )?;
        }
    }
    Ok(summary)
}
