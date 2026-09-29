//! Candidate memory addresses one analyzed function accesses.
use crate::*;
use blobray_analysis::navigation::Facts;

fn invalid(s: &str) -> Error {
    Error::new(ErrorCode::InvalidRequest, s)
}
/// One candidate address a function's record accesses.
pub(crate) struct Candidate<'a> {
    pub record: u64,
    pub fact: &'a FunctionRecord,
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
