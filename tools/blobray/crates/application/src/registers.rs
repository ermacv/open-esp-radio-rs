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
    /// The progression of an unresolved address computed from one index.
    pub indexed: Option<IndexedAddress>,
    pub mask: Option<RegisterMask>,
    /// The sources of a store's bits.
    pub stored: Option<Vec<StoredBits>>,
}

type Observer<'a> = dyn FnMut(Candidate<'_>, &mut dyn RunControl) -> Result<()> + 'a;

/// Every candidate address the analyzed function `records` access: memory
/// accesses with the bits a read-modify-write replaces, and masked reads.
/// A resolved address outside every range of nonempty `ranges` is skipped.
/// An unresolved address is always kept; it carries its indexed progression
/// when it has one and, with nonempty `ranges`, the progression may reach a
/// range: its base lies in one, or its bounded index reaches one.
pub(crate) fn observe<'a, 'm>(
    records: &'a [FunctionRecord],
    facts: &Facts<'a, 'm>,
    memory: &'m WorkingMemory,
    ranges: &[ImageRegion],
    c: &mut dyn RunControl,
    emit: &mut Observer<'_>,
) -> Result<()> {
    // A store's stored bits are evaluated only once one of its candidates
    // is in range, and once for all of them, so a store the ranges exclude
    // costs no work and no memo.
    let mut evaluator = blobray_analysis::registers::StoredBitsEvaluator::new(facts, memory);
    let mut observe = |record: u64,
                       width: u8,
                       address: &'a AbstractValue,
                       mask: Option<RegisterMask>,
                       stored_value: Option<&'a AbstractValue>,
                       c: &mut dyn RunControl| {
        let fact = &records[record as usize];
        let mut stored: Option<Vec<StoredBits>> = None;
        let indexed = match blobray_analysis::registers::indexed_address(facts, address, c)? {
            Some(indexed) if in_ranges(&indexed, width, ranges, c)? => Some(indexed),
            _ => None,
        };
        candidates(address, c, &mut |resolved, alternative, c| {
            c.checkpoint(ranges.len() as u64 + 1)?;
            if let Some(resolved) = resolved
                && !ranges.is_empty()
                && !ranges.iter().any(|r| {
                    u64::from(resolved) < r.end().unwrap_or(0)
                        && u64::from(r.start) < u64::from(resolved) + u64::from(width)
                })
            {
                return Ok(());
            }
            if let Some(value) = stored_value
                && stored.is_none()
            {
                stored = Some(blobray_analysis::registers::stored_bits(
                    &mut evaluator,
                    address,
                    width,
                    value,
                    c,
                )?);
            }
            emit(
                Candidate {
                    record,
                    fact,
                    address: resolved,
                    alternative,
                    indexed: if resolved.is_none() { indexed } else { None },
                    mask,
                    stored: stored.clone(),
                },
                c,
            )
        })
    };
    facts.accesses(c, &mut |access, c| {
        let mask = access.value.and_then(|v| {
            blobray_analysis::registers::write_mask(facts, access.address, access.width, v)
        });
        let stored_value = access
            .value
            .filter(|_| matches!(access.access, MemoryKind::Store));
        observe(
            access.record,
            access.width,
            access.address,
            mask,
            stored_value,
            c,
        )
    })?;
    for (record, fact) in records.iter().enumerate() {
        c.checkpoint(1)?;
        if let FunctionRecord::Expression { expression, .. } = fact
            && let Some((address, width, mask)) =
                blobray_analysis::registers::read_mask(facts, expression)
        {
            observe(record as u64, width, address, Some(mask), None, c)?;
        }
    }
    Ok(())
}

/// Index values of a bounded progression checked against the ranges; a
/// longer bound is judged by its base alone.
const MAX_CHECKED_INDICES: u32 = 4096;

/// Whether the `width`-byte accesses of `indexed` may reach a range of
/// nonempty `ranges`: its base does, or, for a bound up to
/// [`MAX_CHECKED_INDICES`], the address at some index does.
fn in_ranges(
    indexed: &IndexedAddress,
    width: u8,
    ranges: &[ImageRegion],
    c: &mut dyn RunControl,
) -> Result<bool> {
    let hits = |address: u32| {
        ranges.iter().any(|r| {
            u64::from(address) < r.end().unwrap_or(0)
                && u64::from(r.start) < u64::from(address) + u64::from(width)
        })
    };
    if ranges.is_empty() || hits(indexed.base) {
        return Ok(true);
    }
    match indexed.count {
        Some(count) if count <= MAX_CHECKED_INDICES => {
            c.checkpoint(u64::from(count) * (ranges.len() as u64 + 1))?;
            Ok((1..count).any(|i| hits(indexed.at(i))))
        }
        _ => Ok(false),
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    const NODES: u32 = 40;

    /// `x0 = a0; x(i+1) = x(i) + x(i)` and a store of `value` to 0x20000.
    fn records(value: AbstractValue) -> Vec<FunctionRecord> {
        let mut records: Vec<FunctionRecord> = (0..NODES)
            .map(|id| FunctionRecord::Expression {
                id,
                offset: 0,
                expression: if id == 0 {
                    Expression::EntryRegister { register: 10 }
                } else {
                    Expression::Integer {
                        op: IntegerOp::Add,
                        left: AbstractValue::Expression { id: id - 1 },
                        right: AbstractValue::Expression { id: id - 1 },
                    }
                },
            })
            .collect();
        records.push(FunctionRecord::MemoryAccess {
            offset: 4,
            access: MemoryKind::Store,
            width: 4,
            address: AbstractValue::Constant { value: 0x20000 },
            value: Some(value),
            relocation: None,
        });
        records
    }

    /// The checkpoints `observe` takes over `records` with `ranges`.
    fn work(records: &[FunctionRecord], ranges: &[ImageRegion]) -> u64 {
        let memory = WorkingMemory::new(1 << 20).unwrap();
        let facts = Facts::new(records, &memory, &mut || Ok(())).unwrap();
        let mut checkpoints = 0;
        let mut count = || {
            checkpoints += 1;
            Ok(())
        };
        observe(records, &facts, &memory, ranges, &mut count, &mut |_, _| {
            Ok(())
        })
        .unwrap();
        checkpoints
    }

    #[test]
    fn a_store_outside_the_ranges_evaluates_no_stored_bits() {
        let deep = records(AbstractValue::Expression { id: NODES - 1 });
        let flat = records(AbstractValue::Constant { value: 3 });
        let outside = [ImageRegion {
            start: 0x40000,
            length: 4,
        }];
        assert_eq!(
            work(&deep, &outside),
            work(&flat, &outside),
            "an excluded store's value is never walked"
        );
        let inside = [ImageRegion {
            start: 0x20000,
            length: 4,
        }];
        assert!(
            work(&deep, &inside) >= work(&flat, &inside) + u64::from(NODES),
            "an included store's value is walked, one checkpoint per node"
        );
    }
}
