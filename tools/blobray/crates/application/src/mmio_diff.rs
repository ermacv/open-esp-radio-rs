//! Two builds of the same firmware compared by their MMIO behavior, function
//! by function: the register-access observations inside the given windows,
//! without what changes between builds of unchanged code: symbol hashes,
//! code layout, branch offsets, the addresses of loaded data, and a stored
//! address of a sized symbol that moved: a stored value matches an
//! identical access first, then the same symbol and offset, then the same
//! value. Any other stored image address compares by value.
use crate::in_process::Executable;
use crate::*;
use std::collections::BTreeMap;

/// One function's MMIO accesses in one build, every instance of its name
/// together.
#[derive(Default)]
struct Accesses {
    analyzed: bool,
    blocked: bool,
    /// Some analyzed instance is incomplete.
    incomplete: bool,
    accesses: Vec<MmioAccess>,
    unresolved: u64,
}

impl Accesses {
    fn state(&self) -> MmioFunctionState {
        match (self.blocked, self.analyzed) {
            (true, _) => MmioFunctionState::Blocked,
            (false, true) => MmioFunctionState::Analyzed,
            (false, false) => MmioFunctionState::Absent,
        }
    }
}

/// The name a function keeps across builds: demangled, without crate
/// disambiguators or legacy hashes.
fn stable_name(name: Option<&[u8]>) -> String {
    let name = String::from_utf8_lossy(name.unwrap_or_default());
    format!("{:#}", rustc_demangle::demangle(&name))
}

/// Whether the `width` bytes at `address` overlap a window, as
/// `register-accesses` selects them.
fn in_ranges(address: u32, width: u8, ranges: &[ImageRegion]) -> bool {
    ranges.iter().any(|r| {
        u64::from(address) < r.end().unwrap_or(0)
            && u64::from(r.start) < u64::from(address) + u64::from(width)
    })
}

fn operation(fact: &FunctionRecord) -> Option<(MmioOperation, Option<u8>)> {
    match fact {
        FunctionRecord::MemoryAccess { access, width, .. } => Some((
            match access {
                MemoryKind::Load => MmioOperation::Load,
                MemoryKind::Store => MmioOperation::Store,
                MemoryKind::LoadReserved => MmioOperation::LoadReserved,
                MemoryKind::StoreConditional => MmioOperation::StoreConditional,
                MemoryKind::Atomic => MmioOperation::Atomic,
                MemoryKind::FloatLoad => MmioOperation::FloatLoad,
                MemoryKind::FloatStore => MmioOperation::FloatStore,
            },
            Some(*width),
        )),
        FunctionRecord::Expression { .. } => Some((MmioOperation::ReadSelection, None)),
        _ => None,
    }
}

/// The MMIO access of one observation, or `None` for one outside the
/// windows or unresolved (`unresolved` then counts one outside the stack).
fn access(
    observation: crate::registers::Candidate<'_>,
    ranges: &[ImageRegion],
    symbols: &Symbols,
    unresolved: &mut u64,
) -> Option<MmioAccess> {
    let (operation, width) = operation(observation.fact)?;
    // The observations already overlap a window: the ranges select them.
    let location = match (observation.address, observation.indexed) {
        (Some(address), _) => MmioLocation::At { address },
        (None, Some(progression)) => MmioLocation::Indexed { progression },
        (None, None) => {
            // An access through the stack is never MMIO.
            let through_stack = matches!(
                observation.fact,
                FunctionRecord::MemoryAccess {
                    address: AbstractValue::EntryStack { .. },
                    ..
                }
            );
            if operation != MmioOperation::ReadSelection && !through_stack {
                *unresolved += 1;
            }
            return None;
        }
    };
    let stored = observation
        .stored
        .into_iter()
        .flatten()
        .map(|run| MmioStoredBits {
            low: run.low,
            width: run.width,
            source: match run.source {
                StoredBitsSource::Constant { value } => MmioStoredSource::Constant {
                    value,
                    symbol: symbols
                        .at(value)
                        .filter(|_| run.low == 0 && run.width == 32)
                        .map(|(name, offset)| MmioSymbolOffset {
                            name: name.to_owned(),
                            offset,
                        }),
                },
                StoredBitsSource::EntryRegister { register, low } => {
                    MmioStoredSource::EntryRegister { register, low }
                }
                StoredBitsSource::Load {
                    address,
                    width,
                    low,
                    same_word,
                } => MmioStoredSource::Load {
                    address: address.filter(|&address| in_ranges(address, width, ranges)),
                    width,
                    low,
                    same_word,
                },
                StoredBitsSource::Unknown => MmioStoredSource::Unknown,
            },
        })
        .collect();
    Some(MmioAccess {
        location,
        operation,
        width,
        mask: observation.mask,
        stored,
    })
}

/// The symbols one input of a build defines, by address: what a stored
/// address names across builds.
#[derive(Default)]
struct Symbols(Vec<(u64, u64, String)>);

impl Symbols {
    /// The symbol `address` lies in, and the offset into it.
    fn at(&self, address: u32) -> Option<(&str, u32)> {
        let address = u64::from(address);
        let after = self.0.partition_point(|&(start, _, _)| start <= address);
        let (start, end, name) = self.0[..after].last()?;
        (address < *end).then(|| (name.as_str(), (address - start) as u32))
    }
}

/// The sized symbols each static executable among `inputs` defines; none
/// for a relocatable input, whose addresses are relocations, not words.
fn symbols<'m>(
    inputs: &[Executable],
    memory: &'m WorkingMemory,
    control: &mut dyn RunControl,
    admitted: &mut Vec<MemoryReservation<'m>>,
) -> Result<Vec<Symbols>> {
    let mut out = Vec::new();
    for input in inputs {
        let inventory = crate::captured::inventory(input, memory, control)?;
        let mut symbols = Symbols::default();
        for object in &inventory.objects {
            let Some(elf) = object.elf.as_ref().filter(|elf| elf.object_type == 2) else {
                continue;
            };
            for symbol in &elf.symbols {
                control.checkpoint(1)?;
                // A defined function or data object with a size.
                if symbol.size == 0
                    || symbol.raw_section == 0
                    || !matches!(symbol.symbol_type, 1 | 2)
                {
                    continue;
                }
                let name = stable_name(symbol.name.as_deref());
                admit(
                    memory,
                    admitted,
                    name.len() + size_of::<(u64, u64, String)>(),
                    control,
                )?;
                symbols
                    .0
                    .push((symbol.value, symbol.value + symbol.size, name));
            }
        }
        symbols.0.sort_unstable();
        out.push(symbols);
    }
    Ok(out)
}

/// What one build knows of every function, by stable name, how many it
/// blocks and why code was not analyzed. A build with no analyzed function
/// is an error: nothing of its behavior is known.
#[allow(clippy::too_many_arguments)]
fn collect<'m>(
    build: &str,
    inputs: &[Executable],
    abi: Option<CallAbi>,
    ranges: &[ImageRegion],
    decoder: &dyn FunctionSemantics,
    memory: &'m WorkingMemory,
    control: &mut dyn RunControl,
    admitted: &mut Vec<MemoryReservation<'m>>,
) -> Result<(BTreeMap<String, Accesses>, u64, Vec<MmioGap>)> {
    let symbols = symbols(inputs, memory, control, admitted)?;
    let mut functions: BTreeMap<String, Accesses> = BTreeMap::new();
    let mut blocked = 0;
    let mut gaps = Vec::new();
    crate::library::analyze_library(inputs, abi, decoder, memory, control, &mut |outcome, c| {
        match outcome {
            crate::library::LibraryOutcome::Analyzed(analyzed) => {
                let name = stable_name(analyzed.function.name.as_deref());
                admit(memory, admitted, name.len() + size_of::<Accesses>(), c)?;
                let entry = functions.entry(name).or_default();
                entry.analyzed = true;
                entry.incomplete |= !analyzed.complete();
                c.phase(RunPhase::AnalyzeValues)?;
                let facts = blobray_analysis::navigation::Facts::new(analyzed.records, memory, c)?;
                crate::registers::observe(
                    analyzed.records,
                    &facts,
                    memory,
                    ranges,
                    c,
                    &mut |o, c| {
                        let input = &symbols[analyzed.function.input as usize];
                        if let Some(access) = access(o, ranges, input, &mut entry.unresolved) {
                            admit(
                                memory,
                                admitted,
                                size_of::<MmioAccess>()
                                    + access.stored.len() * size_of::<MmioStoredBits>(),
                                c,
                            )?;
                            entry.accesses.push(access);
                        }
                        Ok(())
                    },
                )
            }
            crate::library::LibraryOutcome::Blocked { function, .. } => {
                blocked += 1;
                let name = stable_name(function.name.as_deref());
                admit(memory, admitted, name.len() + size_of::<Accesses>(), c)?;
                functions.entry(name).or_default().blocked = true;
                Ok(())
            }
            crate::library::LibraryOutcome::Gap {
                input,
                object,
                reason,
            } => {
                admit(memory, admitted, reason.len() + size_of::<MmioGap>(), c)?;
                gaps.push(MmioGap {
                    input,
                    object: object.cloned(),
                    reason: reason.to_owned(),
                });
                Ok(())
            }
        }
    })?;
    if !functions.values().any(|function| function.analyzed) {
        let why = match (gaps.first(), blocked) {
            (Some(gap), _) => gap.reason.clone(),
            (None, 0) => "it defines none".to_owned(),
            (None, blocked) => format!("its {blocked} functions are blocked"),
        };
        return Err(Error::new(
            ErrorCode::InvalidRequest,
            format!("the {build} build has no analyzed function: {why}"),
        ));
    }
    Ok((functions, blocked, gaps))
}

fn admit<'m>(
    memory: &'m WorkingMemory,
    admitted: &mut Vec<MemoryReservation<'m>>,
    bytes: usize,
    c: &mut dyn RunControl,
) -> Result<()> {
    c.checkpoint(1)?;
    admitted.push(memory.reserve(bytes as u64, c.position())?);
    Ok(())
}

/// `access` with every stored constant reduced by `reduce`.
fn keyed(
    access: &MmioAccess,
    reduce: impl Fn(u32, &Option<MmioSymbolOffset>) -> MmioStoredSource,
) -> MmioAccess {
    MmioAccess {
        stored: access
            .stored
            .iter()
            .map(|run| MmioStoredBits {
                source: match &run.source {
                    MmioStoredSource::Constant { value, symbol } => reduce(*value, symbol),
                    source => source.clone(),
                },
                ..run.clone()
            })
            .collect(),
        ..access.clone()
    }
}

/// Mark the unmatched accesses of `old` and `new` that equal ones of the
/// other side under `key`, as multisets.
fn pair(
    old: &[MmioAccess],
    new: &[MmioAccess],
    matched: &mut [Vec<bool>; 2],
    key: &dyn Fn(&MmioAccess) -> MmioAccess,
    c: &mut dyn RunControl,
) -> Result<()> {
    let open = |accesses: &[MmioAccess], done: &[bool]| -> Vec<(MmioAccess, usize)> {
        let mut keys: Vec<_> = accesses
            .iter()
            .enumerate()
            .filter(|(i, _)| !done[*i])
            .map(|(i, access)| (key(access), i))
            .collect();
        keys.sort_unstable();
        keys
    };
    let (before, after) = (open(old, &matched[0]), open(new, &matched[1]));
    let mut j = 0;
    for (access, i) in &before {
        c.checkpoint(1)?;
        while j < after.len() && after[j].0 < *access {
            j += 1;
        }
        if j < after.len() && after[j].0 == *access {
            matched[0][*i] = true;
            matched[1][after[j].1] = true;
            j += 1;
        }
    }
    Ok(())
}

/// The accesses `old` has and `new` lacks, and those `new` has and `old`
/// lacks, as multisets. Identical accesses match first; of the rest, a
/// stored whole word in the same symbol at the same offset matches (the
/// object moved between the builds), then the same stored value whatever
/// symbol it lands in. Matching by symbol before value keeps a value that
/// one moved object took over from another from stealing either's pair.
fn unmatched(
    old: &[MmioAccess],
    new: &[MmioAccess],
    c: &mut dyn RunControl,
) -> Result<(Vec<MmioAccess>, Vec<MmioAccess>)> {
    let mut matched = [vec![false; old.len()], vec![false; new.len()]];
    pair(old, new, &mut matched, &|access| access.clone(), c)?;
    let by_symbol = |access: &MmioAccess| {
        keyed(access, |value, symbol| MmioStoredSource::Constant {
            value: if symbol.is_some() { 0 } else { value },
            symbol: symbol.clone(),
        })
    };
    pair(old, new, &mut matched, &by_symbol, c)?;
    let by_value = |access: &MmioAccess| {
        keyed(access, |value, _| MmioStoredSource::Constant {
            value,
            symbol: None,
        })
    };
    pair(old, new, &mut matched, &by_value, c)?;
    let rest = |accesses: &[MmioAccess], done: &[bool]| {
        accesses
            .iter()
            .zip(done)
            .filter(|(_, done)| !**done)
            .map(|(access, _)| access.clone())
            .collect()
    };
    Ok((rest(old, &matched[0]), rest(new, &matched[1])))
}

/// Compare the MMIO behavior of the builds `old` and `new` inside `ranges`.
/// Each build is analyzed as `register-accesses` analyzes it; a function is
/// matched across builds by its stable name, and every instance of a name
/// is compared as one multiset of accesses.
#[allow(clippy::too_many_arguments)]
pub fn mmio_diff(
    old: &[Executable],
    new: &[Executable],
    abi: Option<CallAbi>,
    ranges: &[ImageRegion],
    decoder: &dyn FunctionSemantics,
    memory: &WorkingMemory,
    control: &mut dyn RunControl,
) -> Result<MmioDiff> {
    crate::registers::validate_ranges(ranges)?;
    if ranges.is_empty() {
        return Err(Error::new(
            ErrorCode::InvalidRequest,
            "an MMIO comparison requires at least one address window",
        ));
    }
    let mut admitted = Vec::new();
    let (mut before, blocked_before, gaps_before) = collect(
        "old",
        old,
        abi,
        ranges,
        decoder,
        memory,
        control,
        &mut admitted,
    )?;
    let (mut after, blocked_after, gaps_after) = collect(
        "new",
        new,
        abi,
        ranges,
        decoder,
        memory,
        control,
        &mut admitted,
    )?;
    let with_accesses = |map: &BTreeMap<String, Accesses>| {
        map.values().filter(|f| !f.accesses.is_empty()).count() as u64
    };
    let functions = [with_accesses(&before), with_accesses(&after)];
    let mut names: Vec<String> = before.keys().chain(after.keys()).cloned().collect();
    names.sort_unstable();
    names.dedup();
    let mut changes = Vec::new();
    for name in names {
        control.checkpoint(1)?;
        let mut old = before.remove(&name).unwrap_or_default();
        let mut new = after.remove(&name).unwrap_or_default();
        let state = [old.state(), new.state()];
        // A blocked function's accesses are unknown: only its state compares.
        let compared = !state.contains(&MmioFunctionState::Blocked);
        old.accesses.sort_unstable();
        new.accesses.sort_unstable();
        let (removed, added) = if compared {
            unmatched(&old.accesses, &new.accesses, control)?
        } else {
            (Vec::new(), Vec::new())
        };
        // A function with no MMIO accesses in either build has no MMIO
        // behavior to differ, however its presence differs.
        let mmio = !old.accesses.is_empty()
            || !new.accesses.is_empty()
            || old.unresolved != 0
            || new.unresolved != 0;
        let changed = !removed.is_empty()
            || !added.is_empty()
            || old.unresolved != new.unresolved
            || (state[0] != state[1] && (mmio || !compared));
        if !changed {
            continue;
        }
        changes.push(MmioFunctionChange {
            function: name,
            state,
            complete: [!old.incomplete, !new.incomplete],
            removed,
            added,
            unresolved: [old.unresolved, new.unresolved],
        });
    }
    Ok(MmioDiff {
        ranges: ranges.to_vec(),
        functions,
        changes,
        blocked: [blocked_before, blocked_after],
        gaps: [gaps_before, gaps_after],
    })
}
