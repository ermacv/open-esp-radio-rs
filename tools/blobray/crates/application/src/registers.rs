//! Saved MMIO catalogue over the selected analyses.
use crate::*;
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
            let mut observe = |record: u64,
                               width: u8,
                               address: &AbstractValue,
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
                                    mask: mask.is_some(),
                                    access: matches!(fact, FunctionRecord::MemoryAccess { .. }),
                                },
                                c.position(),
                            )?;
                        }
                        Ok(())
                    };
                candidates(address, c, &mut candidate)
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
