//! Calls and out-of-function jumps of one function by a linear sweep over its
//! whole extent, independent of which code its control-flow graph reaches.
use crate::image::{Function, function_bytes, read_only_word};
use oer_riscv_decode::{Extension, Extensions, Float, Inst, Instruction, Reg, Register, decode};
use oer_riscv_model::*;
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum TransferKind {
    /// A linking call: the callee runs below the caller's frame.
    Call,
    /// A jump out of the function: a tail call, or an indirect jump whose
    /// target is unknown.
    Tail,
}

/// Where an unresolved indirect transfer's target register came from: the
/// last instruction of the linear sweep that wrote it.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum TargetSource {
    /// A load from an `sp`-relative slot: a function pointer spilled by
    /// `core::hint::black_box`, which stack-slot value tracking resolves.
    StackSlot,
    /// A load through another register: a vtable entry or a stored function
    /// pointer.
    Memory,
    /// Any other register value.
    Register,
}

/// Where an indexed dispatch's table starts.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TableBase {
    /// A constant the sweep computed.
    Address(u32),
    /// A register unchanged from the indexing `add` to the transfer, whose
    /// value the value analysis knows at the transfer.
    Register(u8),
}

/// One call or out-of-function jump.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Transfer {
    pub site: u32,
    /// The target address; `None` for an indirect transfer the sweep and the
    /// value analysis cannot resolve.
    pub target: Option<u32>,
    pub kind: TransferKind,
    /// For an unresolved target, where its register came from.
    pub source: Option<TargetSource>,
    /// For an unresolved target loaded as `table[index]`, the table.
    pub table: Option<TableBase>,
}

/// What an instruction writes to an integer register, for [`TargetSource`].
#[derive(Clone, Copy)]
enum Write {
    Load { from_sp: bool },
    Other,
}

/// The integer register `instruction` writes, and how.
fn write(instruction: &Instruction) -> Option<(u8, Write)> {
    let other = |dest: u8| Some((dest, Write::Other));
    match *instruction {
        Instruction::Base(inst) => match inst {
            Inst::Lb { dest, base, .. }
            | Inst::Lbu { dest, base, .. }
            | Inst::Lh { dest, base, .. }
            | Inst::Lhu { dest, base, .. }
            | Inst::Lw { dest, base, .. } => Some((
                dest.0,
                Write::Load {
                    from_sp: base.0 == 2,
                },
            )),
            Inst::Lui { dest, .. }
            | Inst::Auipc { dest, .. }
            | Inst::Jal { dest, .. }
            | Inst::Jalr { dest, .. }
            | Inst::Addi { dest, .. }
            | Inst::Slti { dest, .. }
            | Inst::Sltiu { dest, .. }
            | Inst::Xori { dest, .. }
            | Inst::Ori { dest, .. }
            | Inst::Andi { dest, .. }
            | Inst::Slli { dest, .. }
            | Inst::Srli { dest, .. }
            | Inst::Srai { dest, .. }
            | Inst::Add { dest, .. }
            | Inst::Sub { dest, .. }
            | Inst::Sll { dest, .. }
            | Inst::Slt { dest, .. }
            | Inst::Sltu { dest, .. }
            | Inst::Xor { dest, .. }
            | Inst::Srl { dest, .. }
            | Inst::Sra { dest, .. }
            | Inst::Or { dest, .. }
            | Inst::And { dest, .. }
            | Inst::Mul { dest, .. }
            | Inst::Mulh { dest, .. }
            | Inst::Mulhsu { dest, .. }
            | Inst::Mulhu { dest, .. }
            | Inst::Div { dest, .. }
            | Inst::Divu { dest, .. }
            | Inst::Rem { dest, .. }
            | Inst::Remu { dest, .. }
            | Inst::LrW { dest, .. }
            | Inst::ScW { dest, .. }
            | Inst::AmoW { dest, .. } => other(dest.0),
            _ => None,
        },
        Instruction::Extension(Extension::Integer { dest, .. } | Extension::Csr { dest, .. }) => {
            other(dest)
        }
        Instruction::Extension(Extension::Memory {
            load: true,
            register,
            base,
            ..
        }) => Some((register, Write::Load { from_sp: base == 2 })),
        Instruction::Float(Float::Operation {
            operands: [Some(Register::Integer(dest)), ..],
            ..
        }) => other(dest),
        _ => None,
    }
}

/// Table lengths beyond this are not taken from a bound.
const MAX_ENTRIES: u32 = 4096;

/// What a register holds on the way to a table dispatch. `entries` is the
/// exact number of table words the index can select, when a bounds check or
/// mask on the index states it.
#[derive(Clone, Copy)]
enum Fact {
    Constant(u32),
    /// An index below `entries`.
    Bounded {
        entries: u32,
    },
    /// An index shifted left by two: a word offset.
    Scaled {
        entries: Option<u32>,
    },
    /// A table base plus a word offset.
    TableAddress {
        base: TableBase,
        entries: Option<u32>,
    },
    /// A word loaded from a table.
    TableEntry {
        base: TableBase,
        entries: Option<u32>,
    },
}

fn bounded(entries: u32) -> Option<Fact> {
    (1..=MAX_ENTRIES)
        .contains(&entries)
        .then_some(Fact::Bounded { entries })
}

/// How `instruction` changes the dispatch facts of the register it writes.
fn fact(instruction: &Instruction, pc: u32, facts: &[Option<Fact>; 32]) -> Option<Fact> {
    let get = |r: u8| {
        if r == 0 {
            Some(Fact::Constant(0))
        } else {
            facts[r as usize]
        }
    };
    match *instruction {
        Instruction::Base(Inst::Lui { uimm, .. }) => Some(Fact::Constant(uimm.as_u32())),
        Instruction::Base(Inst::Auipc { uimm, .. }) => {
            Some(Fact::Constant(pc.wrapping_add(uimm.as_u32())))
        }
        Instruction::Base(Inst::Addi { imm, src1, .. }) => match get(src1.0) {
            Some(Fact::Constant(value)) => Some(Fact::Constant(value.wrapping_add(imm.as_u32()))),
            _ => None,
        },
        Instruction::Base(Inst::Andi { imm, .. }) if imm.as_i32() >= 0 => bounded(imm.as_u32() + 1),
        Instruction::Base(Inst::Slli { imm, src1, .. }) if imm.as_u32() == 2 => {
            Some(Fact::Scaled {
                entries: match get(src1.0) {
                    Some(Fact::Bounded { entries }) => Some(entries),
                    _ => None,
                },
            })
        }
        Instruction::Base(Inst::Add { src1, src2, .. }) => {
            let base = |r: u8| match get(r) {
                Some(Fact::Constant(address)) => Some(TableBase::Address(address)),
                Some(Fact::Scaled { .. }) => None,
                _ => Some(TableBase::Register(r)),
            };
            let (base, entries) = match (get(src1.0), get(src2.0)) {
                (Some(Fact::Scaled { entries }), _) => (base(src2.0)?, entries),
                (_, Some(Fact::Scaled { entries })) => (base(src1.0)?, entries),
                _ => return None,
            };
            Some(Fact::TableAddress { base, entries })
        }
        Instruction::Base(Inst::Lw { offset, base, .. }) => match get(base.0) {
            Some(Fact::TableAddress {
                base: TableBase::Address(address),
                entries,
            }) => Some(Fact::TableEntry {
                base: TableBase::Address(address.wrapping_add(offset.as_u32())),
                entries,
            }),
            Some(Fact::TableAddress { base, entries }) if offset.as_i32() == 0 => {
                Some(Fact::TableEntry { base, entries })
            }
            _ => None,
        },
        _ => None,
    }
}

/// The direct branch and jump targets inside the function: where paths merge,
/// so no register fact survives.
fn merges(bytes: &[u8], start: u32, end: u32) -> BTreeSet<u32> {
    let mut targets = BTreeSet::new();
    let mut offset = 0usize;
    while offset < bytes.len() {
        let pc = start + offset as u32;
        let Some((instruction, length)) = decode(&bytes[offset..], Extensions::ALL) else {
            offset += if bytes[offset] & 3 == 3 { 4 } else { 2 };
            continue;
        };
        let displacement = match instruction {
            Instruction::Base(
                Inst::Jal { offset, .. }
                | Inst::Beq { offset, .. }
                | Inst::Bne { offset, .. }
                | Inst::Blt { offset, .. }
                | Inst::Bge { offset, .. }
                | Inst::Bltu { offset, .. }
                | Inst::Bgeu { offset, .. },
            ) => Some(offset.as_u32()),
            _ => None,
        };
        if let Some(target) = displacement.map(|d| pc.wrapping_add(d))
            && target >= start
            && target < end
        {
            targets.insert(target);
        }
        offset += length;
    }
    targets
}

/// The `entries` words of the unwritable table at `base`, if all are there.
fn table_words(file: &object::File<'_>, base: u32, entries: u32) -> Option<Vec<u32>> {
    (0..entries)
        .map(|i| read_only_word(file, base.wrapping_add(4 * i)))
        .collect()
}

/// The fact an index takes on the fall-through path of a bounds check:
/// `bltu limit, index` falls through when `index <= limit`, `bgeu index,
/// limit` when `index < limit`.
fn fall_through(instruction: &Instruction, facts: &[Option<Fact>; 32]) -> Option<(u8, Fact)> {
    let constant = |r: u8| match (r, facts[r as usize]) {
        (0, _) => Some(0),
        (_, Some(Fact::Constant(value))) => Some(value),
        _ => None,
    };
    let (index, entries) = match *instruction {
        Instruction::Base(Inst::Bltu { src1, src2, .. }) => {
            (src2.0, constant(src1.0)?.checked_add(1)?)
        }
        Instruction::Base(Inst::Bgeu { src1, src2, .. }) => (src1.0, constant(src2.0)?),
        _ => return None,
    };
    (index != 0).then_some((index, bounded(entries)?))
}

/// The upper-immediate value `auipc`/`lui` gave a register, as the target base
/// of the immediately following `jalr`.
fn upper(instruction: &Instruction, pc: u32) -> Option<(u8, u32)> {
    match instruction {
        Instruction::Base(Inst::Lui { uimm, dest }) => Some((dest.0, uimm.as_u32())),
        Instruction::Base(Inst::Auipc { uimm, dest }) => {
            Some((dest.0, pc.wrapping_add(uimm.as_u32())))
        }
        _ => None,
    }
}

/// Every call and out-of-function jump of `function` in `elf`.
pub fn transfers(elf: &[u8], function: &Function) -> Result<Vec<Transfer>> {
    let file =
        object::File::parse(elf).map_err(|_| Error::new(ErrorCode::Integrity, "invalid ELF"))?;
    let bytes = function_bytes(&file, function)?;
    let (start, end) = (function.address, function.address + function.size);
    let outside = |target: u32| target < start || target >= end;
    let mut transfers = Vec::new();
    let mut offset = 0usize;
    let mut previous: Option<(u8, u32)> = None;
    let mut writes = [None::<Write>; 32];
    let merges = merges(bytes, start, end);
    let mut facts = [None::<Fact>; 32];
    while offset < bytes.len() {
        let pc = start + offset as u32;
        if merges.contains(&pc) {
            facts = [None; 32];
        }
        let Some((instruction, length)) = decode(&bytes[offset..], Extensions::ALL) else {
            // An encoding outside the decoder's set (a CSR access, say) still
            // has its length in its low bits; it carries no direct transfer,
            // and the value analysis classifies it.
            offset += if bytes[offset] & 3 == 3 { 4 } else { 2 };
            previous = None;
            facts = [None; 32];
            continue;
        };
        let resolved = |target: u32, kind| Transfer {
            site: pc,
            target: Some(target),
            kind,
            source: None,
            table: None,
        };
        match instruction {
            Instruction::Base(Inst::Jal {
                offset: displacement,
                dest,
            }) => {
                let target = pc.wrapping_add(displacement.as_u32());
                if matches!(dest.0, 1 | 5) {
                    transfers.push(resolved(target, TransferKind::Call));
                } else if outside(target) {
                    transfers.push(resolved(target, TransferKind::Tail));
                }
            }
            Instruction::Base(Inst::Jalr {
                offset: displacement,
                base,
                dest,
            }) => {
                let link = matches!(dest.0, 1 | 5);
                let dispatched = match facts[base.0 as usize] {
                    Some(Fact::TableEntry {
                        base: TableBase::Address(table),
                        entries: Some(entries),
                    }) if displacement.as_i32() == 0 => table_words(&file, table, entries),
                    _ => None,
                };
                let kind = if link {
                    TransferKind::Call
                } else {
                    TransferKind::Tail
                };
                match previous {
                    Some((register, value)) if register == base.0 => {
                        let target = value.wrapping_add(displacement.as_u32()) & !1;
                        if link || outside(target) {
                            transfers.push(resolved(target, kind));
                        }
                    }
                    // `ret`: a return, not a transfer.
                    _ if dest.0 == 0 && matches!(base.0, 1 | 5) && displacement.as_i32() == 0 => {}
                    // A table of exactly known length: each entry is a
                    // call, or a jump out of the function; the sweep
                    // already covers jumps inside it.
                    _ if dispatched.is_some() => {
                        let mut targets: Vec<u32> = dispatched
                            .iter()
                            .flatten()
                            // A Rust function pointer is never null: a zero
                            // entry is an empty `Option<fn>` slot.
                            .filter(|&&word| word != 0)
                            .map(|word| word & !1)
                            .filter(|&target| link || outside(target))
                            .collect();
                        targets.sort_unstable();
                        targets.dedup();
                        transfers.extend(targets.into_iter().map(|target| resolved(target, kind)));
                    }
                    _ => transfers.push(Transfer {
                        site: pc,
                        target: None,
                        kind,
                        source: Some(match writes[base.0 as usize] {
                            Some(Write::Load { from_sp: true }) => TargetSource::StackSlot,
                            Some(Write::Load { from_sp: false }) => TargetSource::Memory,
                            _ => TargetSource::Register,
                        }),
                        table: match facts[base.0 as usize] {
                            Some(Fact::TableEntry { base, .. }) => Some(base),
                            _ => None,
                        },
                    }),
                }
            }
            // cm.popret and cm.popretz return through ra.
            Instruction::Extension(Extension::Pop { ret: Some(_), .. }) => {}
            _ => {}
        }
        previous = upper(&instruction, pc);
        let next = fact(&instruction, pc, &facts);
        if let Some((register, how)) = write(&instruction) {
            writes[register as usize] = Some(how);
            // A fact indexed off this register no longer holds.
            for fact in &mut facts {
                if let Some(
                    Fact::TableAddress {
                        base: TableBase::Register(r),
                        ..
                    }
                    | Fact::TableEntry {
                        base: TableBase::Register(r),
                        ..
                    },
                ) = *fact
                    && r == register
                {
                    *fact = None;
                }
            }
            facts[register as usize] = next;
        }
        if let Some((index, fact)) = fall_through(&instruction, &facts) {
            facts[index as usize] = Some(fact);
        }
        // Code after an unconditional transfer is reached only by a jump.
        if matches!(
            instruction,
            Instruction::Base(Inst::Jal { dest: Reg(0), .. } | Inst::Jalr { .. })
        ) {
            facts = [None; 32];
        }
        offset += length;
    }
    Ok(transfers)
}
