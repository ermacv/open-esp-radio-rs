//! Saved MMIO catalogue over the selected analyses.
use crate::*;
use blobray_analysis::navigation::Facts;
use std::cell::RefCell;

fn invalid(s: &str) -> Error {
    Error::new(ErrorCode::InvalidRequest, s)
}
#[derive(Clone, Copy)]
struct Sample {
    address: u32,
    width: u8,
    mask: bool,
    access: bool,
}
pub(crate) fn query(
    project: &Project,
    request: &RegisterQuery,
    memory: &WorkingMemory,
    c: &mut dyn RunControl,
    emit: &mut dyn FnMut(&RegisterRecord, &mut dyn RunControl) -> Result<()>,
) -> Result<RegisterSummary> {
    validate_ranges(&request.ranges)?;
    // One cloned bounded analysis row during serialization. The retained
    // sample array has its own reservation.
    let _envelope = memory.reserve(4 * 1024 * 1024, c.position())?;
    let mut summary = RegisterSummary {
        schema: 2,
        request: request.clone(),
        selected_analyses: 0,
        partial_analyses: 0,
        unavailable_entries: 0,
        observations: 0,
        unresolved_addresses: 0,
        alternative_observations: 0,
        candidate_addresses: 0,
    };
    let mut samples = AdmittedVec::new(memory);
    let output = RefCell::new(emit);
    let navigation = NavigationQuery {
        scope: request.scope.clone(),
        filter: NavigationFilter::Functions { function: None },
    };
    let nav = crate::navigation::query_inspected(
        project,
        &navigation,
        memory,
        c,
        &mut |_, _, _, _| Ok(()),
        &mut |function, _, records, facts, c| {
            c.phase(RunPhase::AnalyzeValues)?;
            observe(records, facts, &request.ranges, c, &mut |candidate, c| {
                let Candidate {
                    record,
                    fact,
                    width,
                    address,
                    alternative,
                    mask,
                } = candidate;
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
                            mask: mask.is_some(),
                            access: matches!(fact, FunctionRecord::MemoryAccess { .. }),
                        },
                        c.position(),
                    )?;
                }
                Ok(())
            })
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
        let (mut local, mut masks) = (0, 0);
        while end < samples.len() && samples[end].address == address {
            c.checkpoint(1)?;
            let s = samples[end];
            if widths.last() != Some(&s.width) {
                widths.push(s.width);
            }
            masks += u64::from(s.mask);
            local += u64::from(s.access);
            end += 1;
        }
        output.borrow_mut()(
            &RegisterRecord::Address {
                address,
                access_widths: widths,
                local_accesses: local,
                mask_observations: masks,
            },
            c,
        )?;
        summary.candidate_addresses += 1;
        start = end;
    }
    Ok(summary)
}
/// One candidate address a function's record accesses.
pub(crate) struct Candidate<'a> {
    pub record: u64,
    pub fact: &'a FunctionRecord,
    /// Access width in bytes.
    pub width: u8,
    pub address: Option<u32>,
    pub alternative: Option<u8>,
    pub mask: Option<RegisterMask>,
}

type Observer<'a> = dyn FnMut(Candidate<'_>, &mut dyn RunControl) -> Result<()> + 'a;

/// Every candidate address the analyzed function `records` access: memory
/// accesses with the bits a read-modify-write replaces, and masked reads.
/// A resolved address outside every range of nonempty `ranges` is skipped.
pub(crate) fn observe(
    records: &[FunctionRecord],
    facts: &Facts<'_, '_>,
    ranges: &[ImageRegion],
    c: &mut dyn RunControl,
    emit: &mut Observer<'_>,
) -> Result<()> {
    let mut observe = |record: u64,
                       width: u8,
                       address: &AbstractValue,
                       mask: Option<RegisterMask>,
                       c: &mut dyn RunControl| {
        let fact = &records[record as usize];
        candidates(address, c, &mut |address, alternative, c| {
            c.checkpoint(ranges.len() as u64 + 1)?;
            if let Some(address) = address
                && !ranges.is_empty()
                && !ranges.iter().any(|r| {
                    u64::from(address) < r.end().unwrap_or(0)
                        && u64::from(r.start) < u64::from(address) + u64::from(width)
                })
            {
                return Ok(());
            }
            emit(
                Candidate {
                    record,
                    fact,
                    width,
                    address,
                    alternative,
                    mask,
                },
                c,
            )
        })
    };
    facts.accesses(c, &mut |access, c| {
        let mask = access.value.and_then(|v| {
            blobray_analysis::registers::write_mask(facts, access.address, access.width, v)
        });
        observe(access.record, access.width, access.address, mask, c)
    })?;
    for (record, fact) in records.iter().enumerate() {
        c.checkpoint(1)?;
        if let FunctionRecord::Expression { expression, .. } = fact
            && let Some((address, width, mask)) =
                blobray_analysis::registers::read_mask(facts, expression)
        {
            observe(record as u64, width, address, Some(mask), c)?;
        }
    }
    Ok(())
}

/// Validate explicit candidate intervals: at most 256, each nonempty and
/// inside the address space.
pub(crate) fn validate_ranges(ranges: &[ImageRegion]) -> Result<()> {
    if ranges.len() > 256 || ranges.iter().any(|r| r.length == 0 || r.end().is_none()) {
        return Err(invalid(
            "register query requires at most 256 valid address intervals",
        ));
    }
    Ok(())
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
