//! Calls and out-of-function jumps of one function by a linear sweep over its
//! whole extent, independent of which code its control-flow graph reaches.
use crate::image::{Function, function_bytes};
use oer_riscv_decode::{Extension, Extensions, Float, Inst, Instruction, Register, decode};
use oer_riscv_model::*;

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
        Instruction::Extension(Extension::Integer { dest, .. }) => other(dest),
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
    while offset < bytes.len() {
        let pc = start + offset as u32;
        let Some((instruction, length)) = decode(&bytes[offset..], Extensions::ALL) else {
            // An encoding outside the decoder's set (a CSR access, say) still
            // has its length in its low bits; it carries no direct transfer,
            // and the value analysis classifies it.
            offset += if bytes[offset] & 3 == 3 { 4 } else { 2 };
            previous = None;
            continue;
        };
        let resolved = |target: u32, kind| Transfer {
            site: pc,
            target: Some(target),
            kind,
            source: None,
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
                    _ => transfers.push(Transfer {
                        site: pc,
                        target: None,
                        kind,
                        source: Some(match writes[base.0 as usize] {
                            Some(Write::Load { from_sp: true }) => TargetSource::StackSlot,
                            Some(Write::Load { from_sp: false }) => TargetSource::Memory,
                            _ => TargetSource::Register,
                        }),
                    }),
                }
            }
            // cm.popret and cm.popretz return through ra.
            Instruction::Extension(Extension::Pop { ret: Some(_), .. }) => {}
            _ => {}
        }
        previous = upper(&instruction, pc);
        if let Some((register, how)) = write(&instruction) {
            writes[register as usize] = Some(how);
        }
        offset += length;
    }
    Ok(transfers)
}
