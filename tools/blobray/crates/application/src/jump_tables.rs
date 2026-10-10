//! Compiler jump tables of relocatable objects, proven by relocations.
//!
//! GCC dispatches a RV32 `switch` as
//! `lui/addi table; slli index, 2; add; lw target; jalr zero, 0(target)`
//! with the table in read-only data and one `R_RISCV_32` relocation per entry.
//! After a first analysis the jump's base register holds
//! `load4[(index << 2) + table]`; the bounds check whose in-range edge is the
//! only way into the dispatch block, such as `bltu limit, index`, fixes the
//! entry count. Each entry's
//! relocation then names its case label. A dispatch whose table lacks a
//! relocation for any selected entry, whose index has no bound, or whose
//! entries leave the function stays an indirect gap.
use crate::*;
use std::collections::BTreeMap;

/// `R_RISCV_32`.
const R_RISCV_32: u32 = 1;
/// `SHT_RELA` and `SHT_REL`.
const RELOCATION_SECTIONS: [u32; 2] = [4, 9];
/// Table lengths beyond this are not taken from a bound.
const MAX_ENTRIES: u32 = 4096;

/// The symbol and relocation facts of one object that table entries need.
#[derive(Default)]
pub(crate) struct TableFacts {
    /// Relocation section index to the section it relocates.
    relocated: BTreeMap<u32, u32>,
    /// `(symbol table, index)` to the defining section and value.
    symbols: BTreeMap<(u32, u64), (Option<u32>, u64)>,
    /// `R_RISCV_32` relocations by relocation section and offset.
    words: BTreeMap<(u32, u64), (u32, u32, i64)>,
}

impl TableFacts {
    pub(crate) fn section(&mut self, r: &SectionRecord) {
        if RELOCATION_SECTIONS.contains(&r.section_type) {
            self.relocated.insert(r.index, r.info);
        }
    }

    pub(crate) fn symbol(&mut self, r: &SymbolRecord, section: Option<u32>) {
        self.symbols
            .insert((r.id.table_section, r.id.index), (section, r.value));
    }

    pub(crate) fn relocation(&mut self, r: &RelocationRecord) {
        if r.relocation_type == R_RISCV_32 {
            self.words.insert(
                (r.section, r.offset),
                (
                    r.symbol_table_section,
                    r.symbol_index,
                    r.addend.unwrap_or(0),
                ),
            );
        }
    }

    /// The `(section, offset)` the word at `offset` of `section` points to.
    fn word(&self, section: u32, offset: u64) -> Option<(u32, u64)> {
        let (relocations, _) = self
            .relocated
            .iter()
            .find(|(_, relocated)| **relocated == section)?;
        let (table, index, addend) = *self.words.get(&(*relocations, offset))?;
        let (target_section, value) = *self.symbols.get(&(table, u64::from(index)))?;
        Some((target_section?, value.checked_add_signed(addend)?))
    }
}

fn constant(value: &AbstractValue) -> Option<u32> {
    match value {
        AbstractValue::Constant { value } => Some(*value),
        _ => None,
    }
}

/// `(index, table symbol, addend)` when `value` is `(index << 2) + table`.
fn scaled_table<'a>(
    value: &'a AbstractValue,
    expressions: &BTreeMap<u32, &'a Expression>,
) -> Option<(&'a AbstractValue, &'a SymbolId, i64)> {
    let expression = |value: &AbstractValue| match value {
        AbstractValue::Expression { id } => expressions.get(id).copied(),
        _ => None,
    };
    let Expression::Integer { op, left, right } = expression(value)? else {
        return None;
    };
    let symbol = |value: &'a AbstractValue| match value {
        AbstractValue::Symbol { symbol, addend } => Some((symbol, *addend)),
        _ => None,
    };
    let index = |value: &'a AbstractValue| match expression(value)? {
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
/// shape, such as a check elsewhere in the function, states no bound.
fn guarding_bound(records: &[FunctionRecord], index: &AbstractValue, block: u64) -> Option<u32> {
    let mut incoming = records.iter().filter_map(|record| match record {
        FunctionRecord::Edge {
            from,
            target: Some(target),
            relation,
            external: false,
        } if *target == block => Some((*from, *relation)),
        _ => None,
    });
    let (branch, relation) = incoming.next()?;
    if incoming.next().is_some() {
        return None;
    }
    let (test, left, right) = records.iter().find_map(|record| match record {
        FunctionRecord::Condition {
            offset,
            test,
            left,
            right,
        } if *offset == branch => Some((*test, left, right)),
        _ => None,
    })?;
    let entries = match (test, relation) {
        // `bltu limit, index` leaves the range when taken.
        (BranchTest::Ltu, EdgeKind::Fallthrough) if right == index => {
            constant(left)?.checked_add(1)
        }
        // `bgeu index, limit` leaves the range when taken.
        (BranchTest::Geu, EdgeKind::Fallthrough) if left == index => constant(right),
        // `bltu index, limit` enters the range when taken.
        (BranchTest::Ltu, EdgeKind::Taken) if left == index => constant(right),
        // `bgeu limit, index` enters the range when taken.
        (BranchTest::Geu, EdgeKind::Taken) if right == index => constant(left)?.checked_add(1),
        _ => None,
    }?;
    (1..=MAX_ENTRIES).contains(&entries).then_some(entries)
}

/// The case value of entry zero: `c` when GCC rebased the switch value as
/// `index = value - c`, otherwise zero.
fn first_case(index: &AbstractValue, expressions: &BTreeMap<u32, &Expression>) -> i64 {
    let rebased = match index {
        AbstractValue::Expression { id } => expressions.get(id).copied(),
        _ => None,
    };
    match rebased {
        Some(Expression::Integer {
            op: IntegerOp::Add,
            right,
            ..
        }) => constant(right).map_or(0, |c| -i64::from(c as i32)),
        Some(Expression::Integer {
            op: IntegerOp::Sub,
            right,
            ..
        }) => constant(right).map_or(0, |c| i64::from(c as i32)),
        _ => 0,
    }
}

/// The jump-table dispatches among one function's first-pass `records`, with
/// their case targets in index order, for a function of `section` spanning
/// `extent`.
pub(crate) fn jump_tables(
    records: &[FunctionRecord],
    section: u32,
    extent: CodeRange,
    facts: &TableFacts,
) -> Vec<JumpTable> {
    let mut expressions = BTreeMap::new();
    let mut blocks = Vec::new();
    for record in records {
        match record {
            FunctionRecord::Expression { id, expression, .. } => {
                expressions.insert(*id, expression);
            }
            FunctionRecord::Block { start, end, .. } => blocks.push((*start, *end)),
            _ => {}
        }
    }
    let inside = |target: u64| target >= extent.start && target < extent.start + extent.length;
    let mut jumps = Vec::new();
    for record in records {
        let FunctionRecord::Instruction {
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
        } = record
        else {
            continue;
        };
        let Some(&(start, _)) = blocks
            .iter()
            .find(|(start, end)| start <= offset && offset < end)
        else {
            continue;
        };
        // The base register's last write before the jump in its block.
        let Some(target) = records
            .iter()
            .filter_map(|record| match record {
                FunctionRecord::Value {
                    offset: at,
                    register,
                    value,
                    ..
                } if register == base && *at >= start && at < offset => Some((*at, value)),
                _ => None,
            })
            .max_by_key(|(at, _)| *at)
            .map(|(_, value)| value)
        else {
            continue;
        };
        let Some(Expression::Load {
            address, width: 4, ..
        }) = (match target {
            AbstractValue::Expression { id } => expressions.get(id).copied(),
            _ => None,
        })
        else {
            continue;
        };
        let Some((index, table, addend)) = scaled_table(address, &expressions) else {
            continue;
        };
        let (Some(entries), Some(&(Some(table_section), value))) = (
            guarding_bound(records, index, start),
            facts.symbols.get(&(table.table_section, table.index)),
        ) else {
            continue;
        };
        let Some(base_offset) = value.checked_add_signed(addend) else {
            continue;
        };
        let targets: Option<Vec<u64>> = (0..u64::from(entries))
            .map(|i| match facts.word(table_section, base_offset + 4 * i) {
                Some((target_section, target)) if target_section == section && inside(target) => {
                    Some(target)
                }
                _ => None,
            })
            .collect();
        if let Some(entries) = targets {
            jumps.push(JumpTable {
                site: *offset,
                first_case: first_case(index, &expressions),
                entries,
            });
        }
    }
    jumps
}

#[cfg(test)]
mod tests;
