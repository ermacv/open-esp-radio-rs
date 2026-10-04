//! Worst-case stack bounds of a static RV32 image from its machine code,
//! with the companions it calls into, such as the chip's ROM ELF.
//!
//! Every function is a defined code symbol of the image. Its frame is the
//! compiler's `.stack_sizes` entry, cross-checked with the deepest entry-relative
//! `sp` the bounded value analysis of `oer-riscv-analysis` observes; a function
//! without an entry takes the observed depth only when its control-flow graph
//! is complete. Its callees are every direct call and out-of-function jump of a
//! linear sweep over its whole extent, which also reaches code behind jump
//! tables the control-flow graph does not expand, plus the transfers whose
//! target the value analysis resolves. A transfer through `table[index]`
//! reaches each entry of the table when its length is exact: a bounds check
//! or mask on the index, or the size of a data object the table starts in an
//! unwritable section, such as an interrupt handler table. The sweep already
//! covers a jump's entries inside the function; a table of unknown length
//! stays unresolved. A function's bound is its frame or, if
//! deeper, a callee's bound below the `sp` of the transfer that reaches it: the
//! value analysis's depth at that site, or the whole frame at a site it did not
//! reach. It never underestimates the code it reaches.
//!
//! Bounds fail closed: a root has a number only when nothing it reaches is
//! unresolved. Otherwise [`Bound::reasons`] counts what is, by reason, and
//! [`Reason`] names the stage that closes it.

mod cha;
mod contexts;
mod dwarf;
mod image;
mod interrupts;
mod mir;
mod relocations;
mod resolutions;
mod summaries;
mod sweep;
mod trap;
mod wakers;

use oer_riscv_model::{Error, ErrorCode, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

pub use cha::{TypeFacts, function_pointer_resolutions};
pub use contexts::{HartStack, LevelStack, Stacks, interrupt_stacks};
pub use dwarf::Dwarf;
pub use image::{Function, functions, stack_sizes};
pub use interrupts::{Field, TableEntry, TableLayout, interrupt_table};
pub use mir::{MirFacts, mir_resolutions};
use oer_riscv_analysis::KnownJump;
pub use relocations::{address_taken, taken_addresses};
pub use resolutions::{Fact, Resolution, Resolutions};
pub use summaries::{Summary, parse as parse_summaries};
pub use sweep::{Load, TableBase, TargetSource, Transfer, TransferKind};
pub use trap::{TrapEntry, trap_entry, vector_table};
pub use wakers::{waker_resolutions, waker_vtables};

/// Where a function's frame comes from.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FrameSource {
    /// The compiler's `.stack_sizes` entry, equal to or above the observed depth.
    StackSizes,
    /// The deepest observed `sp` of a complete control-flow graph.
    Observed,
    /// A reviewed summary of a companion function the machine code alone
    /// does not bound.
    Summary,
}

/// What the analysis established about one function.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FunctionFacts {
    pub function: Function,
    /// Bytes below the entry `sp`, or `None` when no source establishes it.
    pub frame: Option<u64>,
    pub source: Option<FrameSource>,
    /// The deepest entry-relative `sp` the value analysis observed.
    pub observed: Option<u64>,
    /// Whether the control-flow graph covered the function without gaps.
    pub complete: bool,
    /// Direct calls and out-of-function jumps, by site.
    pub transfers: Vec<Transfer>,
    /// Bytes below the entry `sp` at each transfer the value analysis
    /// reached; a transfer it did not reach is taken at the whole frame.
    pub site_depths: BTreeMap<u32, u64>,
    /// The exact argument registers `a0..a7` at each direct call the value
    /// analysis reached, by site; `None` where a value is not exact.
    pub call_arguments: BTreeMap<u32, [Option<u32>; 8]>,
    /// The base of the table each unresolved transfer loads its target
    /// from as `table[index]`, where it is known, by site.
    pub table_bases: BTreeMap<u32, u32>,
    /// The exact integer registers at each unresolved transfer the value
    /// analysis reached, by site.
    pub unresolved_registers: BTreeMap<u32, [Option<u32>; 32]>,
}

/// Why a reachable site or function leaves a bound unknown.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum Reason {
    /// An indirect call through a pointer the frame keeps across calls,
    /// which the local value analysis does not follow.
    StackSlotCall,
    /// An indirect call through a loaded vtable entry or function pointer.
    LoadedCall,
    /// An indirect call through another register value.
    RegisterCall,
    /// An indirect jump that is not a table of known length: a tail call
    /// through a pointer, such as `core::fmt::write` to `write_str`, or a
    /// `match` whose jump table no bounds check limits.
    IndirectJump,
    /// A transfer to an address outside the image's code: ROM.
    OutsideImage,
    /// A transfer into the middle of a function.
    IntoFunction,
    /// A function without a frame record and with an incomplete control-flow
    /// graph: hand-written assembly.
    Unframed,
    /// A call-graph cycle.
    Recursion,
    /// An indirect site whose facts found no target and do not prove it
    /// reaches none: a waker call without vtables, a field without candidates.
    NoCandidate,
}

impl Reason {
    /// The stage of the stack analysis that resolves this reason.
    pub fn closed_by(self) -> &'static str {
        match self {
            Reason::StackSlotCall => "2: MIR call facts",
            Reason::LoadedCall | Reason::RegisterCall => "2: MIR call facts",
            Reason::IndirectJump => "2: MIR call facts",
            Reason::OutsideImage => "1c: ROM companion",
            Reason::IntoFunction => "review",
            Reason::Unframed => "1c: assembly frames",
            Reason::Recursion => "a reviewed recursion bound",
            Reason::NoCandidate => "2: MIR call facts",
        }
    }
}

impl fmt::Display for Reason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Reason::StackSlotCall => "indirect call through a stack slot",
            Reason::LoadedCall => "indirect call through a loaded pointer",
            Reason::RegisterCall => "indirect call through a register",
            Reason::IndirectJump => "indirect jump out of a function",
            Reason::OutsideImage => "transfer outside the image",
            Reason::IntoFunction => "transfer into a function's middle",
            Reason::Unframed => "function without a frame",
            Reason::Recursion => "recursion",
            Reason::NoCandidate => "indirect site whose facts found no target",
        })
    }
}

/// A root's worst-case bound, or why it has none.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Bound {
    pub root: u32,
    /// Bytes below the root's entry `sp` on its deepest path; `None` unless
    /// nothing the root reaches is unresolved.
    pub bytes: Option<u64>,
    /// Bytes on the deepest path over what is resolved: the bound when
    /// nothing is unresolved, otherwise neither an upper nor a lower bound
    /// (`partial + ?`), since a hole may reach deeper and a path may be
    /// infeasible.
    pub partial: u64,
    /// What the root reaches that is unresolved: the site (or, for an
    /// unframed function or a cycle, the function) and its reason.
    pub unresolved: Vec<(u32, Reason)>,
    /// Deepest path over what is resolved, root first, with each function's
    /// frame. A partial path when [`Self::bytes`] is `None`.
    pub path: Vec<(u32, u64)>,
}

impl Bound {
    /// The unresolved sites and functions, counted by reason.
    pub fn reasons(&self) -> BTreeMap<Reason, usize> {
        let mut counts = BTreeMap::new();
        for (_, reason) in &self.unresolved {
            *counts.entry(*reason).or_insert(0) += 1;
        }
        counts
    }
}

/// Frames, transfers and bounds of one image and its companions.
#[derive(Clone, Debug)]
pub struct Analysis {
    pub functions: BTreeMap<u32, FunctionFacts>,
    /// Address ranges of the executable sections of every analysed ELF.
    pub code: Vec<(u32, u32)>,
    /// The summaries this image used: those of the functions it reaches.
    pub summaries: BTreeSet<String>,
}

/// Analyze every function of the static RV32 executable `elf` and of the
/// `companions` it calls into, such as the chip's ROM ELF: a transfer from
/// one into another reaches the callee's frame like any other call. No two
/// ELFs may place code at the same address. `summaries` stand for companion
/// functions the machine code alone does not bound ([`Summary`]).
pub fn analyze(elf: &[u8], companions: &[&[u8]], summaries: &[Summary]) -> Result<Analysis> {
    let mut analysis = Analysis {
        functions: BTreeMap::new(),
        code: Vec::new(),
        summaries: BTreeSet::new(),
    };
    let mut image = None;
    for elf in std::iter::once(elf).chain(companions.iter().copied()) {
        let Analysis {
            functions, code, ..
        } = analyze_one(elf)?;
        for &(start, end) in &code {
            if analysis
                .code
                .iter()
                .any(|&(other_start, other_end)| start < other_end && other_start < end)
            {
                return Err(Error::new(
                    ErrorCode::Integrity,
                    format!("code at {start:#010x}..{end:#010x} overlaps another ELF's"),
                ));
            }
        }
        analysis.code.extend(code);
        image.get_or_insert_with(|| functions.keys().copied().collect::<BTreeSet<u32>>());
        analysis.functions.extend(functions);
    }
    analysis.summaries = summaries::apply(&mut analysis, &image.unwrap_or_default(), summaries)?;
    Ok(analysis)
}

/// The facts and executable ranges of one ELF.
fn analyze_one(elf: &[u8]) -> Result<Analysis> {
    let relocations = relocations::Relocations::read(elf)?;
    let functions = image::functions(elf)?;
    let sizes = image::stack_sizes(elf)?;
    let code = image::executable_ranges(elf)?;
    let functions = image::observing(elf, |observe| {
        let mut facts = BTreeMap::new();
        for function in functions {
            facts.insert(
                function.address,
                function_facts(elf, function, &relocations, &sizes, observe)?,
            );
        }
        Ok(facts)
    })?;
    Ok(Analysis {
        functions,
        code,
        summaries: BTreeSet::new(),
    })
}

/// The base of the table a transfer loads its target from, when known.
fn table_base(transfer: &Transfer, observation: &image::Observation) -> Option<u32> {
    match transfer.table {
        Some(TableBase::Address(address)) => Some(address),
        Some(TableBase::Register(r)) => observation
            .site_registers
            .get(&transfer.site)
            .and_then(|registers| registers[r as usize]),
        None => None,
    }
}

/// The entries of the table a transfer loads its target from, when the
/// table is a sized object or a compiler jump table the relocations name.
fn table_entries(
    elf: &[u8],
    relocations: &relocations::Relocations,
    transfer: &Transfer,
    observation: &image::Observation,
) -> Result<Option<Vec<u32>>> {
    let address = table_base(transfer, observation);
    Ok(match address {
        Some(address) => image::table(elf, address)?.or_else(|| relocations.jump_table(address)),
        None => None,
    })
}

/// One function's facts: its sweep, then the value analysis over a graph
/// that follows every jump table known so far, again for each table the
/// analysis's register values newly locate.
fn function_facts(
    elf: &[u8],
    function: Function,
    relocations: &relocations::Relocations,
    sizes: &BTreeMap<u32, u64>,
    observe: &mut dyn FnMut(&Function, &[KnownJump]) -> Result<image::Observation>,
) -> Result<FunctionFacts> {
    let swept = sweep::transfers(elf, &function, relocations)?;
    let inside =
        |target: u32| target >= function.address && target - function.address < function.size;
    let known = |site: u32, entries: &[u32]| KnownJump {
        site: u64::from(site),
        targets: entries
            .iter()
            .filter(|&&entry| entry != 0)
            .map(|entry| entry & !1)
            .filter(|&target| inside(target))
            .map(u64::from)
            .collect(),
    };
    let mut jumps: Vec<KnownJump> = swept
        .jumps
        .iter()
        .map(|(site, entries)| known(*site, entries))
        .collect();
    let observation = loop {
        let observation = observe(&function, &jumps)?;
        let before = jumps.len();
        for transfer in &swept.transfers {
            if transfer.kind != TransferKind::Tail
                || transfer.target.is_some()
                || jumps
                    .iter()
                    .any(|jump| jump.site == u64::from(transfer.site))
            {
                continue;
            }
            if let Some(entries) = table_entries(elf, relocations, transfer, &observation)? {
                jumps.push(known(transfer.site, &entries));
            }
        }
        if jumps.len() == before {
            break observation;
        }
    };
    let mut transfers = swept.transfers;
    let table_bases = transfers
        .iter()
        .filter(|t| t.target.is_none())
        .filter_map(|t| Some((t.site, table_base(t, &observation)?)))
        .collect();
    let unresolved_registers = transfers
        .iter()
        .filter(|t| t.target.is_none())
        .filter_map(|t| Some((t.site, *observation.site_registers.get(&t.site)?)))
        .collect();
    let mut dispatched = Vec::new();
    // Sites whose table the link's relocations or a sized object name:
    // their entries replace whatever the value analysis read.
    let mut tabled = BTreeSet::new();
    for transfer in std::mem::take(&mut transfers) {
        let Some(entries) = table_entries(elf, relocations, &transfer, &observation)? else {
            dispatched.push(transfer);
            continue;
        };
        tabled.insert(transfer.site);
        let mut targets: Vec<u32> = entries
            .iter()
            // A Rust function pointer is never null: a zero entry is an
            // empty `Option<fn>` slot the code tests before calling.
            .filter(|&&entry| entry != 0)
            .map(|entry| entry & !1)
            .filter(|&target| transfer.kind == TransferKind::Call || !inside(target))
            .collect();
        targets.sort_unstable();
        targets.dedup();
        dispatched.extend(targets.into_iter().map(|target| Transfer {
            target: Some(target),
            source: None,
            table: None,
            load: None,
            ..transfer
        }));
    }
    transfers = dispatched;
    // A jump whose every target the value analysis found inside the
    // function stays there: the sweep covers its code.
    transfers.retain(|t| t.target.is_some() || !observation.local_jumps.contains(&t.site));
    // The value analysis's targets of a site are a superset of its
    // runtime targets; a jump's targets inside the function are already
    // in the sweep.
    for (site, target, kind) in observation.resolved {
        if tabled.contains(&site) {
            continue;
        }
        transfers.retain(|t| !(t.site == site && t.target.is_none()));
        if kind == TransferKind::Tail && inside(target) {
            continue;
        }
        if !transfers
            .iter()
            .any(|t| t.site == site && t.target == Some(target))
        {
            transfers.push(Transfer {
                site,
                target: Some(target),
                kind,
                source: None,
                table: None,
                load: None,
            });
        }
    }
    transfers.sort_by_key(|t| (t.site, t.target));
    let call_arguments = transfers
        .iter()
        .filter(|t| t.kind == TransferKind::Call && t.target.is_some())
        .filter_map(|t| {
            let registers = observation.site_registers.get(&t.site)?;
            Some((t.site, core::array::from_fn(|i| registers[10 + i])))
        })
        .collect();
    let depth = observation.depth;
    let recorded = sizes.get(&function.address).copied();
    let (frame, source) = match (recorded, depth) {
        (Some(recorded), Some(depth)) if depth > recorded => {
            return Err(Error::new(
                ErrorCode::Integrity,
                format!(
                    "{}: observed sp depth {depth} exceeds its .stack_sizes {recorded}",
                    function.label()
                ),
            ));
        }
        (Some(recorded), _) => (Some(recorded), Some(FrameSource::StackSizes)),
        (None, Some(depth)) if observation.complete => (Some(depth), Some(FrameSource::Observed)),
        (None, _) => (None, None),
    };
    Ok(FunctionFacts {
        function,
        frame,
        source,
        observed: depth,
        call_arguments,
        table_bases,
        unresolved_registers,
        complete: observation.complete,
        transfers,
        site_depths: observation.site_depths,
    })
}

impl Analysis {
    /// The worst-case bound of the function at `root`.
    pub fn bound(&self, root: u32) -> Result<Bound> {
        self.bound_with(root, &Resolutions::new())
    }

    /// [`Self::bound`] with `resolutions` for the indirect sites the
    /// analysis left unresolved: such a site reaches each of its targets.
    pub fn bound_with(&self, root: u32, resolutions: &Resolutions) -> Result<Bound> {
        if !self.functions.contains_key(&root) {
            return Err(Error::new(
                ErrorCode::NotFound,
                format!("{root:#010x} is not a function start"),
            ));
        }
        let mut walk = Walk {
            analysis: self,
            resolutions,
            memo: BTreeMap::new(),
            stack: Vec::new(),
            unresolved: BTreeSet::new(),
        };
        let (bytes, path) = walk.visit(root);
        Ok(Bound {
            root,
            bytes: walk.unresolved.is_empty().then_some(bytes),
            partial: bytes,
            unresolved: walk.unresolved.into_iter().collect(),
            path,
        })
    }

    /// The exact values argument register `a{register}` holds at every
    /// direct call of `callee`, or the first call site where it is not
    /// exact. Complete only for a callee whose address nothing takes
    /// ([`address_taken`]): every call of it is then a direct one.
    pub fn constant_arguments(&self, callee: u32, register: usize) -> Result<BTreeSet<u32>> {
        let mut values = BTreeSet::new();
        for facts in self.functions.values() {
            for transfer in &facts.transfers {
                if transfer.kind != TransferKind::Call || transfer.target != Some(callee) {
                    continue;
                }
                let value = facts
                    .call_arguments
                    .get(&transfer.site)
                    .and_then(|arguments| arguments.get(register).copied().flatten())
                    .ok_or_else(|| {
                        Error::new(
                            ErrorCode::Unavailable,
                            format!(
                                "a{register} at the call of {callee:#010x} at {:#010x} is not exact",
                                transfer.site
                            ),
                        )
                    })?;
                values.insert(value);
            }
        }
        Ok(values)
    }

    fn in_code(&self, address: u32) -> bool {
        self.code
            .iter()
            .any(|&(start, end)| address >= start && address < end)
    }
}

struct Walk<'a> {
    analysis: &'a Analysis,
    resolutions: &'a Resolutions,
    memo: BTreeMap<u32, (u64, Vec<(u32, u64)>)>,
    stack: Vec<u32>,
    unresolved: BTreeSet<(u32, Reason)>,
}

impl Walk<'_> {
    /// The deepest bytes below `address`'s entry `sp` over what is resolved,
    /// with its path. Unknown frames count as zero; they are reported.
    fn visit(&mut self, address: u32) -> (u64, Vec<(u32, u64)>) {
        if let Some(known) = self.memo.get(&address) {
            return known.clone();
        }
        if self.stack.contains(&address) {
            self.unresolved.insert((address, Reason::Recursion));
            return (0, Vec::new());
        }
        let facts = &self.analysis.functions[&address];
        let frame = facts.frame.unwrap_or_else(|| {
            self.unresolved.insert((address, Reason::Unframed));
            0
        });
        self.stack.push(address);
        // The deepest point of this function alone is its frame.
        let mut deepest = (frame, Vec::new());
        for transfer in &facts.transfers {
            let targets: Vec<u32> = match (transfer.target, self.resolutions.get(transfer.site)) {
                (Some(target), _) => vec![target],
                (None, Some(resolved)) if resolved.resolves() => {
                    resolved.targets.iter().copied().collect()
                }
                (None, Some(_)) => {
                    self.unresolved.insert((transfer.site, Reason::NoCandidate));
                    continue;
                }
                (None, None) => {
                    let reason = match (transfer.kind, transfer.source) {
                        (TransferKind::Tail, _) => Reason::IndirectJump,
                        (TransferKind::Call, Some(TargetSource::StackSlot)) => {
                            Reason::StackSlotCall
                        }
                        (TransferKind::Call, Some(TargetSource::Memory)) => Reason::LoadedCall,
                        (TransferKind::Call, _) => Reason::RegisterCall,
                    };
                    self.unresolved.insert((transfer.site, reason));
                    continue;
                }
            };
            let at = facts
                .site_depths
                .get(&transfer.site)
                .copied()
                .unwrap_or(frame);
            for target in targets {
                if !self.analysis.functions.contains_key(&target) {
                    let reason = if self.analysis.in_code(target) {
                        Reason::IntoFunction
                    } else {
                        Reason::OutsideImage
                    };
                    self.unresolved.insert((transfer.site, reason));
                    continue;
                }
                let below = self.visit(target);
                if at + below.0 > deepest.0 {
                    deepest = (at + below.0, below.1);
                }
            }
        }
        self.stack.pop();
        let mut path = vec![(address, frame)];
        path.extend(deepest.1);
        let result = (deepest.0, path);
        self.memo.insert(address, result.clone());
        result
    }
}

#[cfg(test)]
mod tests;
