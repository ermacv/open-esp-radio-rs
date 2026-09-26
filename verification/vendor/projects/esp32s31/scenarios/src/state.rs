//! Vendor state the compared cases write but no relation compares.
//!
//! Blobray reports the persistent vendor bytes each compared case writes:
//! writable image segments, session RAM and session allocations. A byte is
//! compared when the case's relation selects it through a projection field,
//! a memory pair or the write timeline. Every byte a case writes without
//! comparing it is either reviewed by a decision below, with its reason, or
//! reported as unprojected in the evidence index. A decision that matches no
//! unprojected byte fails the run.
use crate::harness::{Result, invalid};
use blobray_domain::{ExecutionCase, ExecutionEvidence, LayoutProjection, WrittenRange};
use object::{Object, ObjectSymbol, SymbolKind};
use std::collections::{BTreeMap, BTreeSet};

/// A reviewed decision on vendor state the scenarios write without comparing
/// it: bytes `start..end` of a vendor data symbol.
#[derive(Clone, Copy, Debug)]
pub struct Decision {
    pub reason: &'static str,
    pub places: &'static [(&'static str, u32, u32)],
}

/// Reviewed unprojected vendor state.
pub const DECISIONS: &[Decision] = &[];

/// One vendor byte, by data symbol and offset; a byte outside every sized
/// data symbol is named by the address of the nearest symbol below it, such
/// as a section-local anchor.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Byte {
    pub symbol: String,
    pub offset: u32,
}

/// Sized data symbols of the vendor executables, by address.
#[derive(Default)]
pub struct Symbols {
    ranges: BTreeMap<u32, (u32, String)>,
    anchors: BTreeSet<u32>,
}

impl Symbols {
    pub fn of(executables: &[&[u8]]) -> Result<Self> {
        let (mut ranges, mut anchors) = (BTreeMap::new(), BTreeSet::new());
        for bytes in executables {
            let file = object::File::parse(*bytes)?;
            for symbol in file.symbols() {
                if !symbol.is_undefined()
                    && let Ok(address) = u32::try_from(symbol.address())
                {
                    anchors.insert(address);
                }
                if symbol.kind() != SymbolKind::Data || symbol.size() == 0 || symbol.is_undefined()
                {
                    continue;
                }
                let (Ok(address), Ok(size), Ok(name)) = (
                    u32::try_from(symbol.address()),
                    u32::try_from(symbol.size()),
                    symbol.name(),
                ) else {
                    continue;
                };
                ranges.entry(address).or_insert((size, name.to_owned()));
            }
        }
        Ok(Self { ranges, anchors })
    }

    pub fn name(&self, address: u32) -> Byte {
        match self.ranges.range(..=address).next_back() {
            Some((start, (size, name))) if address - start < *size => Byte {
                symbol: name.clone(),
                offset: address - start,
            },
            _ => {
                let anchor = self.anchors.range(..=address).next_back().copied();
                let anchor = anchor.unwrap_or(address);
                Byte {
                    symbol: format!("{anchor:#010x}"),
                    offset: address - anchor,
                }
            }
        }
    }
}

/// Vendor byte ranges the relation of `case` compares, or `None` when it
/// compares every vendor write through the write timeline.
fn compared(
    case: &ExecutionCase,
    projections: &[LayoutProjection],
) -> Result<Option<Vec<(u64, u64)>>> {
    let Some(relation) = &case.relation else {
        return Ok(Some(vec![]));
    };
    if relation.events.timeline.writes {
        return Ok(None);
    }
    let mut ranges = vec![];
    if let Some(selected) = &relation.projection {
        let projection = projections
            .iter()
            .find(|p| {
                blobray_application::in_process::projection_ref(p)
                    .ok()
                    .as_ref()
                    == Some(selected)
            })
            .ok_or_else(|| invalid(format!("{}: projection is not reviewed", case.name)))?;
        for field in projection
            .fields
            .iter()
            .filter(|f| f.final_state || f.timeline)
        {
            let length = field.byte_length()?;
            let address = projection.vendor.address(field.vendor, length)?;
            ranges.push((u64::from(address), u64::from(address) + u64::from(length)));
        }
    }
    for pair in &relation.memory {
        let selection = case
            .vendor
            .observe_memory
            .get(usize::from(pair.vendor))
            .ok_or_else(|| invalid(format!("{}: memory pair outside selections", case.name)))?;
        ranges.push((
            u64::from(selection.address),
            u64::from(selection.address) + u64::from(selection.length),
        ));
    }
    Ok(Some(ranges))
}

/// Vendor bytes the compared cases of `records` write, and those a case
/// writes without comparing, for the cases `selected` names.
pub fn written(
    cases: &[ExecutionCase],
    selected: &BTreeSet<u32>,
    records: &[ExecutionEvidence],
    projections: &[LayoutProjection],
) -> Result<(BTreeSet<u32>, BTreeSet<u32>)> {
    let mut ranges: BTreeMap<u32, Vec<WrittenRange>> = BTreeMap::new();
    for record in records {
        if let ExecutionEvidence::Written {
            case,
            replacement: false,
            range,
        } = record
            && selected.contains(case)
        {
            ranges.entry(*case).or_default().push(*range);
        }
    }
    let (mut written, mut unprojected) = (BTreeSet::new(), BTreeSet::new());
    for (case, ranges) in ranges {
        let compared = compared(&cases[case as usize], projections)?;
        for range in ranges {
            for address in u64::from(range.address)..range.end() {
                let address = address as u32;
                written.insert(address);
                let covered = compared.as_ref().is_none_or(|c| {
                    c.iter()
                        .any(|(start, end)| (*start..*end).contains(&u64::from(address)))
                });
                if !covered {
                    unprojected.insert(address);
                }
            }
        }
    }
    Ok((written, unprojected))
}

/// Split `unprojected` into (reviewed, untriaged) under `decisions`.
pub fn classify(
    decisions: &[Decision],
    unprojected: &BTreeSet<Byte>,
) -> (BTreeSet<Byte>, BTreeSet<Byte>) {
    unprojected
        .iter()
        .cloned()
        .partition(|byte| reviewed(decisions, byte).is_some())
}

fn reviewed<'d>(decisions: &'d [Decision], byte: &Byte) -> Option<&'d (&'static str, u32, u32)> {
    decisions
        .iter()
        .flat_map(|d| d.places)
        .find(|(symbol, start, end)| {
            byte.symbol == *symbol && (*start..*end).contains(&byte.offset)
        })
}

/// Every place of every decision must still review an unprojected byte.
pub fn check(decisions: &[Decision], unprojected: &BTreeSet<Byte>) -> Result<()> {
    let matched: BTreeSet<_> = unprojected
        .iter()
        .filter_map(|byte| reviewed(decisions, byte))
        .collect();
    match decisions
        .iter()
        .flat_map(|d| d.places)
        .find(|place| !matched.contains(place))
    {
        Some((symbol, start, end)) => Err(invalid(format!(
            "state decision for {symbol}[{start:#x}..{end:#x}] matches no unprojected byte; \
             the state is compared or no longer written"
        ))),
        None => Ok(()),
    }
}

/// Coalesced ranges of `bytes`, by symbol.
pub fn ranges(bytes: &BTreeSet<Byte>) -> Vec<(String, u32, u32)> {
    let mut ranges: Vec<(String, u32, u32)> = vec![];
    for byte in bytes {
        match ranges.last_mut() {
            Some((symbol, offset, length))
                if *symbol == byte.symbol && *offset + *length == byte.offset =>
            {
                *length += 1
            }
            _ => ranges.push((byte.symbol.clone(), byte.offset, 1)),
        }
    }
    ranges
}

#[cfg(test)]
mod tests {
    use super::*;

    fn byte(symbol: &str, offset: u32) -> Byte {
        Byte {
            symbol: symbol.into(),
            offset,
        }
    }

    const DECIDED: &[Decision] = &[Decision {
        reason: "test",
        places: &[("state", 2, 4)],
    }];

    #[test]
    fn bytes_are_named_by_sized_symbols_or_the_nearest_anchor() {
        let symbols = Symbols {
            ranges: BTreeMap::from([(0x100, (4, "state".to_owned()))]),
            anchors: BTreeSet::from([0x100, 0x200]),
        };
        assert_eq!(symbols.name(0x103), byte("state", 3));
        assert_eq!(symbols.name(0x104), byte("0x00000100", 4));
        assert_eq!(symbols.name(0x20e), byte("0x00000200", 14));
        assert_eq!(symbols.name(0x80), byte("0x00000080", 0));
    }

    #[test]
    fn decisions_review_their_offsets_and_fail_when_stale() {
        let unprojected = BTreeSet::from([byte("state", 1), byte("state", 3), byte("other", 3)]);
        let (reviewed, untriaged) = classify(DECIDED, &unprojected);
        assert_eq!(reviewed, BTreeSet::from([byte("state", 3)]));
        assert_eq!(untriaged.len(), 2);
        check(DECIDED, &unprojected).unwrap();
        assert!(check(DECIDED, &BTreeSet::from([byte("state", 1)])).is_err());
    }

    #[test]
    fn ranges_coalesce_adjacent_bytes_of_one_symbol() {
        let bytes = BTreeSet::from([byte("a", 1), byte("a", 2), byte("a", 4), byte("b", 5)]);
        assert_eq!(
            ranges(&bytes),
            [
                ("a".to_owned(), 1, 2),
                ("a".to_owned(), 4, 1),
                ("b".to_owned(), 5, 1)
            ]
        );
    }
}
