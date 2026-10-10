//! Compiler jump tables of relocatable objects, proven by relocations.
//!
//! GCC dispatches a RV32 `switch` as
//! `lui/addi table; slli index, 2; add; lw target; jalr zero, 0(target)`
//! with the table in read-only data and one `R_RISCV_32` relocation per entry.
//! After a first analysis the jump's base register holds
//! `load4[(index << 2) + table]` in the analysis' `CallInputs` at the jump; the bounds check whose in-range edge is the
//! only way into the dispatch block, such as `bltu limit, index`, fixes the
//! entry count. Each entry's
//! relocation then names its case label. A dispatch whose table lacks a
//! relocation for any selected entry, whose index has no bound, or whose
//! entries leave the function stays an indirect gap.
use crate::*;

/// `R_RISCV_32`.
const R_RISCV_32: u32 = 1;
/// `SHT_RELA`. `SHT_REL` keeps each entry's addend in the table word itself,
/// which this recognizer does not read, so a REL table is never proven.
const SHT_RELA: u32 = 4;
/// `SHT_PROGBITS`, `SHF_WRITE`, `SHF_ALLOC`, `SHF_EXECINSTR`.
const SHT_PROGBITS: u32 = 1;
const SHF_WRITE: u64 = 0x1;
const SHF_ALLOC: u64 = 0x2;
const SHF_EXECINSTR: u64 = 0x4;
/// Table lengths beyond this are not taken from a bound.
const MAX_ENTRIES: u32 = 4096;
/// `ra`, the return-address register: a `jalr zero, 0(ra)` is a return.
const RA: u8 = 1;

/// One section of the object, as table entries need it.
#[derive(Clone, Copy)]
struct SectionFact {
    index: u32,
    section_type: u32,
    /// The section a relocation section relocates.
    info: u32,
    /// Allocated, read-only, non-executable data: where a table can live.
    read_only_data: bool,
}

/// `((symbol table, index), (defining section, value))`.
type SymbolFact = ((u32, u64), (Option<u32>, u64));
/// `((relocation section, offset), (symbol table, symbol, addend))`.
type WordFact = ((u32, u64), (u32, u32, i64));
/// `(relocated section, its only RELA section)`.
type RelocatedFact = (u32, Option<u32>);

/// The symbol and relocation facts of one object that table entries need,
/// held in admitted working memory. Only `R_RISCV_32` RELA relocations of
/// read-only data are kept, so debug and other relocations cost nothing.
pub(crate) struct TableFacts<'m> {
    sections: AdmittedVec<'m, SectionFact>,
    /// `((symbol table, index), (defining section, value))`, sorted by key
    /// once the object has been read.
    symbols: AdmittedVec<'m, SymbolFact>,
    /// `((relocation section, offset), (symbol table, symbol, addend))`,
    /// sorted by key once the object has been read.
    words: AdmittedVec<'m, WordFact>,
    /// Each relocated section's RELA section, sorted by the relocated section
    /// once the object has been read; a section with two has none.
    relocated: AdmittedVec<'m, RelocatedFact>,
}

impl<'m> TableFacts<'m> {
    pub(crate) fn new(memory: &'m WorkingMemory) -> Self {
        Self {
            sections: AdmittedVec::new(memory),
            symbols: AdmittedVec::new(memory),
            words: AdmittedVec::new(memory),
            relocated: AdmittedVec::new(memory),
        }
    }

    pub(crate) fn section(&mut self, r: &SectionRecord, c: &mut dyn RunControl) -> Result<()> {
        self.sections.push(
            SectionFact {
                index: r.index,
                section_type: r.section_type,
                info: r.info,
                read_only_data: r.section_type == SHT_PROGBITS
                    && r.flags & SHF_ALLOC != 0
                    && r.flags & (SHF_WRITE | SHF_EXECINSTR) == 0,
            },
            c.position(),
        )
    }

    pub(crate) fn symbol(
        &mut self,
        r: &SymbolRecord,
        section: Option<u32>,
        c: &mut dyn RunControl,
    ) -> Result<()> {
        self.symbols.push(
            ((r.id.table_section, r.id.index), (section, r.value)),
            c.position(),
        )
    }

    pub(crate) fn relocation(
        &mut self,
        r: &RelocationRecord,
        c: &mut dyn RunControl,
    ) -> Result<()> {
        let Some(addend) = r.addend else {
            return Ok(());
        };
        if r.relocation_type != R_RISCV_32 {
            return Ok(());
        }
        // Sections stream in index order before their contents, so the
        // relocation section is found by a binary search; a target not seen
        // yet is checked when a table is read. Were the order ever different,
        // a missed section drops the word and no table is proven from it.
        match self.section_fact(r.section) {
            Some(relocations) if relocations.section_type == SHT_RELA => {
                if self
                    .section_fact(relocations.info)
                    .is_some_and(|target| !target.read_only_data)
                {
                    return Ok(());
                }
            }
            _ => return Ok(()),
        }
        self.words.push(
            (
                (r.section, r.offset),
                (r.symbol_table_section, r.symbol_index, addend),
            ),
            c.position(),
        )
    }

    /// Order the facts for lookup once the object has been read.
    pub(crate) fn finish(&mut self, c: &mut dyn RunControl) -> Result<()> {
        c.checkpoint((self.sections.len() + self.symbols.len() + self.words.len()) as u64)?;
        self.sections.sort_unstable_by_key(|section| section.index);
        self.symbols.sort_unstable_by_key(|(key, _)| *key);
        self.words.sort_unstable_by_key(|(key, _)| *key);
        for section in self.sections.iter() {
            if section.section_type == SHT_RELA {
                self.relocated
                    .push((section.info, Some(section.index)), c.position())?;
            }
        }
        self.relocated.sort_unstable_by_key(|(target, _)| *target);
        // A section relocated by two RELA sections names neither.
        let mut i = 0;
        while i + 1 < self.relocated.len() {
            if self.relocated[i].0 == self.relocated[i + 1].0 {
                self.relocated[i].1 = None;
                self.relocated[i + 1].1 = None;
            }
            i += 1;
        }
        Ok(())
    }

    fn section_fact(&self, index: u32) -> Option<SectionFact> {
        let at = self
            .sections
            .binary_search_by_key(&index, |section| section.index)
            .ok()?;
        Some(self.sections[at])
    }

    fn symbol_value(&self, table: u32, index: u64) -> Option<(Option<u32>, u64)> {
        let at = self
            .symbols
            .binary_search_by_key(&(table, index), |(key, _)| *key)
            .ok()?;
        Some(self.symbols[at].1)
    }

    /// The `(section, offset)` the word at `offset` of read-only data
    /// `section` points to, by its RELA relocation.
    fn word(&self, section: u32, offset: u64) -> Option<(u32, u64)> {
        if !self.section_fact(section)?.read_only_data {
            return None;
        }
        let at = self
            .relocated
            .binary_search_by_key(&section, |(target, _)| *target)
            .ok()?;
        let relocations = self.relocated[at].1?;
        let at = self
            .words
            .binary_search_by_key(&(relocations, offset), |(key, _)| *key)
            .ok()?;
        let (table, index, addend) = self.words[at].1;
        let (target_section, value) = self.symbol_value(table, u64::from(index))?;
        Some((target_section?, value.checked_add_signed(addend)?))
    }
}

fn constant(value: &AbstractValue) -> Option<u32> {
    match value {
        AbstractValue::Constant { value } => Some(*value),
        _ => None,
    }
}

/// The expression `value` names in a dense id index.
fn expression_of<'a>(
    value: &AbstractValue,
    expressions: &[&'a Expression],
) -> Option<&'a Expression> {
    match value {
        AbstractValue::Expression { id } => expressions.get(*id as usize).copied(),
        _ => None,
    }
}

/// `(index, table symbol, addend)` when `value` is `(index << 2) + table`.
fn scaled_table<'a>(
    value: &'a AbstractValue,
    expressions: &[&'a Expression],
) -> Option<(&'a AbstractValue, &'a SymbolId, i64)> {
    let Expression::Integer { op, left, right } = expression_of(value, expressions)? else {
        return None;
    };
    let symbol = |value: &'a AbstractValue| match value {
        AbstractValue::Symbol { symbol, addend } => Some((symbol, *addend)),
        _ => None,
    };
    let index = |value: &'a AbstractValue| match expression_of(value, expressions)? {
        Expression::Integer {
            op: IntegerOp::Shl,
            left,
            right,
        } if constant(right) == Some(2) => Some(left),
        _ => None,
    };
    match op {
        IntegerOp::Add => {
            let (scaled, (table, addend)) = match (symbol(left), symbol(right)) {
                (Some(table), None) => (right, table),
                (None, Some(table)) => (left, table),
                _ => return None,
            };
            Some((index(scaled)?, table, addend))
        }
        IntegerOp::ShiftAdd2 => {
            let (table, addend) = symbol(right)?;
            Some((left, table, addend))
        }
        _ => None,
    }
}

/// The entry count the bounds check guarding the dispatch block at `block`
/// states. The block must have exactly one incoming edge, from a conditional
/// branch on `index` against a constant, and that edge must be the branch's
/// in-range side: the fall-through of `bltu limit, index` / `bgeu index, limit`
/// or the taken edge of `bltu index, limit` / `bgeu limit, index`. Any other
/// shape, such as a check elsewhere in the function, states no bound. Every
/// visited record is charged.
fn guarding_bound(
    records: &[FunctionRecord],
    index: &AbstractValue,
    block: u64,
    c: &mut dyn RunControl,
) -> Result<Option<u32>> {
    let mut incoming = None;
    for record in records {
        c.checkpoint(1)?;
        if let FunctionRecord::Edge {
            from,
            target: Some(target),
            relation,
            external: false,
        } = record
            && *target == block
        {
            if incoming.is_some() {
                return Ok(None);
            }
            incoming = Some((*from, *relation));
        }
    }
    let Some((branch, relation)) = incoming else {
        return Ok(None);
    };
    let mut condition = None;
    for record in records {
        c.checkpoint(1)?;
        if let FunctionRecord::Condition {
            offset,
            test,
            left,
            right,
        } = record
            && *offset == branch
        {
            condition = Some((*test, left, right));
            break;
        }
    }
    let Some((test, left, right)) = condition else {
        return Ok(None);
    };
    let entries = match (test, relation) {
        // `bltu limit, index` leaves the range when taken.
        (BranchTest::Ltu, EdgeKind::Fallthrough) if right == index => {
            constant(left).and_then(|limit| limit.checked_add(1))
        }
        // `bgeu index, limit` leaves the range when taken.
        (BranchTest::Geu, EdgeKind::Fallthrough) if left == index => constant(right),
        // `bltu index, limit` enters the range when taken.
        (BranchTest::Ltu, EdgeKind::Taken) if left == index => constant(right),
        // `bgeu limit, index` enters the range when taken.
        (BranchTest::Geu, EdgeKind::Taken) if right == index => {
            constant(left).and_then(|limit| limit.checked_add(1))
        }
        _ => None,
    };
    Ok(entries.filter(|entries| (1..=MAX_ENTRIES).contains(entries)))
}

/// `value` without a zero extension to 8 or 16 bits (`andi 0xff`,
/// `zext.h`'s `and 0xffff`, or `slli k; srli k`), which GCC emits for a
/// narrow switch value.
fn unextended<'a>(value: &'a AbstractValue, expressions: &[&'a Expression]) -> &'a AbstractValue {
    match expression_of(value, expressions) {
        Some(Expression::Integer {
            op: IntegerOp::And,
            left,
            right,
        }) => match (constant(left), constant(right)) {
            (_, Some(0xff | 0xffff)) => left,
            (Some(0xff | 0xffff), _) => right,
            _ => value,
        },
        Some(Expression::Integer {
            op: IntegerOp::Shr,
            left,
            right,
        }) => match (expression_of(left, expressions), constant(right)) {
            (
                Some(Expression::Integer {
                    op: IntegerOp::Shl,
                    left: inner,
                    right: by,
                }),
                Some(k @ (16 | 24)),
            ) if constant(by) == Some(k) => inner,
            _ => value,
        },
        _ => value,
    }
}

/// The case value of entry zero: `c` when GCC rebased the switch value as
/// `index = value - c` (an addition of `-c` with the constant on either side
/// or a subtraction of `c`, under any narrow zero extension), zero when the
/// index is an opaque value itself, and unknown for any other shape: an
/// unrecognized index never claims case values.
fn first_case(index: &AbstractValue, expressions: &[&Expression]) -> Option<i64> {
    let index = unextended(index, expressions);
    let Some(expression) = expression_of(index, expressions) else {
        return Some(0);
    };
    match expression {
        Expression::Integer {
            op: IntegerOp::Add,
            left,
            right,
        } => match (constant(left), constant(right)) {
            (_, Some(c)) | (Some(c), _) => Some(-i64::from(c as i32)),
            _ => None,
        },
        Expression::Integer {
            op: IntegerOp::Sub,
            right,
            ..
        } => constant(right).map(|c| i64::from(c as i32)),
        Expression::Integer { .. } => None,
        _ => Some(0),
    }
}

/// The jump tables of one function with the working memory their entries and
/// the second pass's copies of them hold, released when this is dropped.
pub(crate) struct FoundTables<'m> {
    pub(crate) tables: Vec<JumpTable>,
    _reserved: AdmittedVec<'m, MemoryReservation<'m>>,
}

impl<'m> FoundTables<'m> {
    /// No tables: what a function's first pass follows.
    pub(crate) fn none(memory: &'m WorkingMemory) -> Self {
        Self {
            tables: Vec::new(),
            _reserved: AdmittedVec::new(memory),
        }
    }
}

/// The jump-table dispatches among one function's first-pass `records`, with
/// their case targets in index order, for a function of `section` spanning
/// `extent`. The expression and block indexes it builds are admitted in
/// `memory` and every record it visits is charged to `c`; a return
/// (`jalr zero, 0(ra)`) is never a dispatch and costs no further search.
pub(crate) fn jump_tables<'r, 'm>(
    records: &'r [FunctionRecord],
    section: u32,
    extent: CodeRange,
    facts: &TableFacts<'_>,
    memory: &'m WorkingMemory,
    c: &mut dyn RunControl,
) -> Result<FoundTables<'m>> {
    let mut expressions: AdmittedVec<'_, &'r Expression> = AdmittedVec::new(memory);
    let mut blocks: AdmittedVec<'_, (u64, u64)> = AdmittedVec::new(memory);
    let mut sites: AdmittedVec<'_, (u64, u8)> = AdmittedVec::new(memory);
    for record in records {
        c.checkpoint(1)?;
        match record {
            // Expression ids are dense and emitted in order; anything else
            // leaves the index unusable, and then no table is proven.
            FunctionRecord::Expression { id, expression, .. } => {
                if *id as usize != expressions.len() {
                    return Ok(FoundTables::none(memory));
                }
                expressions.push(expression, c.position())?;
            }
            FunctionRecord::Block { start, end, .. } => {
                blocks.push((*start, *end), c.position())?
            }
            FunctionRecord::Instruction {
                offset,
                decoded:
                    DecodedOp {
                        flow:
                            InstructionFlow::Indirect {
                                base,
                                offset: 0,
                                link: false,
                            },
                        ..
                    },
                ..
            } if *base != RA => sites.push((*offset, *base), c.position())?,
            _ => {}
        }
    }
    let inside = |target: u64| target >= extent.start && target < extent.start + extent.length;
    // One table per dispatch site at most, and the second pass's jump per
    // table: both are reserved before the list is allocated at its exact
    // capacity, so it never grows.
    let mut reserved = AdmittedVec::new(memory);
    let per_site = (std::mem::size_of::<JumpTable>()
        + std::mem::size_of::<oer_riscv_analysis::KnownJump>()) as u64;
    reserved.push(
        memory.reserve(sites.len() as u64 * per_site, c.position())?,
        c.position(),
    )?;
    let mut jumps = Vec::new();
    jumps
        .try_reserve_exact(sites.len())
        .map_err(|_| Error::new(ErrorCode::ResourceLimited, "jump table allocation refused"))?;
    for &(site, base) in sites.iter() {
        let mut block = None;
        for &(start, end) in blocks.iter() {
            c.checkpoint(1)?;
            if start <= site && site < end {
                block = Some(start);
                break;
            }
        }
        let Some(start) = block else {
            continue;
        };
        // The base register's value at the jump, from the analysis' record of
        // a relocatable object's dispatch: one the graph leaves without a
        // target on the first pass, and one this recognizer expanded on the
        // passes that follow, so each pass proves its tables again.
        let mut target = None;
        for record in records {
            c.checkpoint(1)?;
            if let FunctionRecord::CallInputs { offset, registers } = record
                && *offset == site
            {
                target = registers.get(usize::from(base));
                break;
            }
        }
        let Some(Expression::Load {
            address, width: 4, ..
        }) = target.and_then(|value| expression_of(value, &expressions))
        else {
            continue;
        };
        let Some((index, table, addend)) = scaled_table(address, &expressions) else {
            continue;
        };
        let Some(entries) = guarding_bound(records, index, start, c)? else {
            continue;
        };
        let Some((Some(table_section), value)) =
            facts.symbol_value(table.table_section, table.index)
        else {
            continue;
        };
        let Some(base_offset) = value.checked_add_signed(addend) else {
            continue;
        };
        c.checkpoint(u64::from(entries))?;
        // The entries, and the second pass's sorted copy of them, are
        // reserved before they are allocated, each at its exact capacity.
        let bytes = 2 * u64::from(entries) * std::mem::size_of::<u64>() as u64;
        reserved.push(memory.reserve(bytes, c.position())?, c.position())?;
        let mut targets = Vec::new();
        targets
            .try_reserve_exact(entries as usize)
            .map_err(|_| Error::new(ErrorCode::ResourceLimited, "jump table allocation refused"))?;
        for i in 0..u64::from(entries) {
            match facts.word(table_section, base_offset + 4 * i) {
                Some((target_section, target)) if target_section == section && inside(target) => {
                    targets.push(target)
                }
                _ => break,
            }
        }
        if targets.len() == entries as usize {
            jumps.push(JumpTable {
                site,
                first_case: first_case(index, &expressions),
                entries: targets,
            });
        }
    }
    Ok(FoundTables {
        tables: jumps,
        _reserved: reserved,
    })
}

#[cfg(test)]
mod tests;
