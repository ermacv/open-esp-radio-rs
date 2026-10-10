//! Compiler jump tables of relocatable objects, proven by relocations.
//!
//! GCC dispatches a RV32 `switch` as
//! `lui/addi table; slli index, 2; add; lw target; jalr zero, 0(target)`
//! with the table in read-only data and one `R_RISCV_32` relocation per entry.
//! After a first analysis the jump's base register holds
//! `load4[(index << 2) + table]`; a bounds check `bltu limit, index` (or
//! `bgeu index, limit`) on the same index fixes the entry count. Each entry's
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

/// The smallest entry count a bounds check on `index` among `records` states.
fn bound(records: &[FunctionRecord], index: &AbstractValue) -> Option<u32> {
    records
        .iter()
        .filter_map(|record| match record {
            FunctionRecord::Condition {
                test: BranchTest::Ltu,
                left,
                right,
                ..
            } if right == index => constant(left)?.checked_add(1),
            FunctionRecord::Condition {
                test: BranchTest::Geu,
                left,
                right,
                ..
            } if left == index => constant(right),
            _ => None,
        })
        .filter(|entries| (1..=MAX_ENTRIES).contains(entries))
        .min()
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
            bound(records, index),
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
                entries,
            });
        }
    }
    jumps
}

#[cfg(test)]
mod tests;
