//! RV32 instruction-decoder boundary and relocation-aware call helpers.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use rv_asm::{Imm, Inst, IsCompressed, Reg, Xlen};

use crate::{Error, FloatingRoundingMode, Result};

use super::model::{
    AnalysisInstruction, ArtifactSymbolDefinition, DecodedInstruction, UnsupportedInstruction,
    UnsupportedInstructionClass,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FloatingMemoryAccess {
    Load,
    Store,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FloatingMemoryInstruction {
    pub address: u64,
    pub instruction_width: u8,
    pub access: FloatingMemoryAccess,
    pub floating_register: u8,
    pub base: Reg,
    pub offset: i32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FloatingDataOperation {
    MoveFromInteger,
    MoveToInteger,
    SignCopy,
    SignNegate,
    SignXor,
    CompareLessOrEqual,
    CompareLess,
    CompareEqual,
    SignedWordToSingle,
    SubtractSingle,
    DivideSingle,
    FusedMultiplyAddSingle,
    SingleToSignedWord,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FloatingDataInstruction {
    pub operation: FloatingDataOperation,
    pub destination: u8,
    pub source1: u8,
    pub source2: u8,
    pub source3: Option<u8>,
    pub rounding: Option<FloatingRoundingMode>,
}

/// Return the architectural immediate for ANDI.
///
/// rv-asm 0.2.1 decodes the six-bit C.ANDI immediate as unsigned even though
/// the RISC-V C extension defines it as sign-extended. Keep the workaround at
/// the decode boundary so every analysis engine observes the same value.
pub fn andi_immediate(imm: Imm, width: u8) -> u32 {
    if width == 2 {
        (((imm.as_u32() & 0x3f) << 26) as i32 >> 26) as u32
    } else {
        imm.as_u32()
    }
}

pub fn decode_symbol(symbol: &ArtifactSymbolDefinition) -> Result<Vec<DecodedInstruction>> {
    let mut decoded = Vec::new();
    let mut offset = 0_usize;
    while offset < symbol.bytes.len() {
        let remaining = &symbol.bytes[offset..];
        if remaining.len() < 2 {
            return Err(format!("truncated instruction in {} at +{offset:#x}", symbol.name).into());
        }
        let compressed = Inst::first_byte_is_compressed(remaining[0]);
        let width = if compressed { 2 } else { 4 };
        let instruction_bytes = remaining
            .get(..width)
            .ok_or_else(|| format!("truncated instruction in {} at +{offset:#x}", symbol.name))?;
        let mut word = [0_u8; 4];
        word[..width].copy_from_slice(instruction_bytes);
        let (instruction, decoded_width) = Inst::decode(u32::from_le_bytes(word), Xlen::Rv32)
            .map_err(|error| -> Error {
                format!(
                    "cannot decode {} at {:#x}: {error}",
                    symbol.name,
                    symbol.address + offset as u64
                )
                .into()
            })?;
        let expected_width = if decoded_width == IsCompressed::Yes {
            2
        } else {
            4
        };
        if width != expected_width {
            return Err(format!("decoder width disagreement in {}", symbol.name).into());
        }
        decoded.push(DecodedInstruction {
            address: symbol.address + offset as u64,
            width: width as u8,
            instruction,
        });
        offset += width;
    }
    Ok(decoded)
}

/// Decode a complete symbol for best-effort structural analysis.
///
/// Unsupported but architecturally sized instructions become explicit stream
/// items. Later instructions can still contribute evidence, while the
/// unsupported operation keeps the result fail-closed. Concrete execution
/// deliberately uses [`decode_symbol`] instead.
pub fn decode_symbol_for_analysis(
    symbol: &ArtifactSymbolDefinition,
) -> Result<Vec<AnalysisInstruction>> {
    let mut decoded = Vec::new();
    let mut offset = 0_usize;
    while offset < symbol.bytes.len() {
        let remaining = &symbol.bytes[offset..];
        if remaining.len() < 2 {
            return Err(format!("truncated instruction in {} at +{offset:#x}", symbol.name).into());
        }
        let compressed = Inst::first_byte_is_compressed(remaining[0]);
        let width = if compressed { 2 } else { 4 };
        let instruction_bytes = remaining
            .get(..width)
            .ok_or_else(|| format!("truncated instruction in {} at +{offset:#x}", symbol.name))?;
        let mut word = [0_u8; 4];
        word[..width].copy_from_slice(instruction_bytes);
        let raw = u32::from_le_bytes(word);
        let address = symbol.address + offset as u64;
        match Inst::decode(raw, Xlen::Rv32) {
            Ok((instruction, decoded_width)) => {
                let expected_width = if decoded_width == IsCompressed::Yes {
                    2
                } else {
                    4
                };
                if width != expected_width {
                    return Err(format!("decoder width disagreement in {}", symbol.name).into());
                }
                decoded.push(AnalysisInstruction::Supported(DecodedInstruction {
                    address,
                    width: width as u8,
                    instruction,
                }));
            }
            Err(_) => decoded.push(AnalysisInstruction::Unsupported(classify_unsupported(
                address,
                width as u8,
                raw,
            ))),
        }
        offset += width;
    }
    Ok(decoded)
}

/// Return unsupported instructions reachable from the symbol entry by a
/// conservative intraprocedural CFG walk.
///
/// This deliberately tracks only instruction addresses. It neither performs
/// value provenance nor follows calls, so artifact-wide inventories can avoid
/// treating padding after a return as a blocker without duplicating the much
/// heavier structural-analysis state.
pub fn reachable_unsupported_instructions(
    symbol: &ArtifactSymbolDefinition,
) -> Result<Vec<UnsupportedInstruction>> {
    let instructions = decode_symbol_for_analysis(symbol)?;
    if instructions.is_empty() {
        return Ok(Vec::new());
    }
    let instruction_indices = instructions
        .iter()
        .enumerate()
        .map(|(index, instruction)| (instruction.address() as u32, index))
        .collect::<BTreeMap<_, _>>();
    let mut pending = VecDeque::from([0usize]);
    let mut visited = BTreeSet::new();
    let mut blockers = BTreeMap::new();

    while let Some(index) = pending.pop_front() {
        if !visited.insert(index) {
            continue;
        }
        let decoded_or_blocker = instructions[index];
        let mut successors = Vec::with_capacity(2);
        match decoded_or_blocker {
            AnalysisInstruction::Unsupported(blocker) => {
                blockers.insert(blocker.address, blocker);
                if blocker.linear_control_flow {
                    successors.push(index + 1);
                }
            }
            AnalysisInstruction::Supported(decoded) => match decoded.instruction {
                Inst::Jal { offset, dest } => {
                    if dest == Reg::ZERO {
                        let target = (decoded.address as u32).wrapping_add(offset.as_i32() as u32);
                        if let Some(target) = instruction_indices.get(&target) {
                            successors.push(*target);
                        }
                    } else {
                        successors.push(index + 1);
                    }
                }
                Inst::Jalr { dest, .. } => {
                    if dest != Reg::ZERO {
                        successors.push(index + 1);
                    }
                }
                Inst::Beq { offset, .. }
                | Inst::Bne { offset, .. }
                | Inst::Blt { offset, .. }
                | Inst::Bge { offset, .. }
                | Inst::Bltu { offset, .. }
                | Inst::Bgeu { offset, .. } => {
                    let target = (decoded.address as u32).wrapping_add(offset.as_i32() as u32);
                    if let Some(target) = instruction_indices.get(&target) {
                        successors.push(*target);
                    }
                    successors.push(index + 1);
                }
                Inst::Ebreak => {}
                _ => successors.push(index + 1),
            },
        }
        for successor in successors {
            if successor < instructions.len() && !visited.contains(&successor) {
                pending.push_back(successor);
            }
        }
    }

    Ok(blockers.into_values().collect())
}

/// Best-effort mnemonic for human review of an unsupported instruction.
///
/// The name is descriptive evidence only; returning a mnemonic does not claim
/// that structural or concrete semantics are implemented.
pub fn unsupported_instruction_mnemonic(width: u8, raw: u32) -> &'static str {
    if raw == 0 {
        return "illegal-zero";
    }
    if width == 2 {
        return match (raw & 0x3, (raw >> 13) & 0x7) {
            (0, 3) => "c.flw",
            (0, 7) => "c.fsw",
            (2, 3) => "c.flwsp",
            (2, 7) => "c.fswsp",
            _ => "unsupported-compressed",
        };
    }

    let opcode = raw & 0x7f;
    let funct3 = (raw >> 12) & 0x7;
    let funct7 = (raw >> 25) & 0x7f;
    match opcode {
        0x07 => match funct3 {
            2 => "flw",
            3 => "fld",
            _ => "load-fp",
        },
        0x27 => match funct3 {
            2 => "fsw",
            3 => "fsd",
            _ => "store-fp",
        },
        0x43 => match (raw >> 25) & 0x3 {
            0 => "fmadd.s",
            1 => "fmadd.d",
            _ => "fmadd",
        },
        0x47 => match (raw >> 25) & 0x3 {
            0 => "fmsub.s",
            1 => "fmsub.d",
            _ => "fmsub",
        },
        0x4b => match (raw >> 25) & 0x3 {
            0 => "fnmsub.s",
            1 => "fnmsub.d",
            _ => "fnmsub",
        },
        0x4f => match (raw >> 25) & 0x3 {
            0 => "fnmadd.s",
            1 => "fnmadd.d",
            _ => "fnmadd",
        },
        0x53 => match (funct7, funct3, (raw >> 20) & 0x1f) {
            (0x00, _, _) => "fadd.s",
            (0x01, _, _) => "fadd.d",
            (0x04, _, _) => "fsub.s",
            (0x05, _, _) => "fsub.d",
            (0x08, _, _) => "fmul.s",
            (0x09, _, _) => "fmul.d",
            (0x0c, _, _) => "fdiv.s",
            (0x0d, _, _) => "fdiv.d",
            (0x10, 0, _) => "fsgnj.s",
            (0x10, 1, _) => "fsgnjn.s",
            (0x10, 2, _) => "fsgnjx.s",
            (0x11, 0, _) => "fsgnj.d",
            (0x11, 1, _) => "fsgnjn.d",
            (0x11, 2, _) => "fsgnjx.d",
            (0x14, 0, _) => "fmin.s",
            (0x14, 1, _) => "fmax.s",
            (0x15, 0, _) => "fmin.d",
            (0x15, 1, _) => "fmax.d",
            (0x2c, _, _) => "fsqrt.s",
            (0x2d, _, _) => "fsqrt.d",
            (0x50, 0, _) => "fle.s",
            (0x50, 1, _) => "flt.s",
            (0x50, 2, _) => "feq.s",
            (0x51, 0, _) => "fle.d",
            (0x51, 1, _) => "flt.d",
            (0x51, 2, _) => "feq.d",
            (0x60, _, 0) => "fcvt.w.s",
            (0x60, _, 1) => "fcvt.wu.s",
            (0x61, _, 0) => "fcvt.w.d",
            (0x61, _, 1) => "fcvt.wu.d",
            (0x68, _, 0) => "fcvt.s.w",
            (0x68, _, 1) => "fcvt.s.wu",
            (0x69, _, 0) => "fcvt.d.w",
            (0x69, _, 1) => "fcvt.d.wu",
            (0x70, 0, _) => "fmv.x.w",
            (0x70, 1, _) => "fclass.s",
            (0x71, 0, _) => "fmv.x.d",
            (0x71, 1, _) => "fclass.d",
            (0x78, 0, _) => "fmv.w.x",
            (0x79, 0, _) => "fmv.d.x",
            _ => "op-fp",
        },
        0x73 if funct3 != 0 => match funct3 {
            1 => "csrrw",
            2 => "csrrs",
            3 => "csrrc",
            5 => "csrrwi",
            6 => "csrrsi",
            7 => "csrrci",
            _ => "csr",
        },
        0x73 => match raw {
            0x1020_0073 => "sret",
            0x1050_0073 => "wfi",
            0x3020_0073 => "mret",
            _ => "system",
        },
        0x0b | 0x2b | 0x5b | 0x7b => "vendor-custom",
        0x57 => "vector",
        _ => "invalid",
    }
}

/// Decode RV32F word loads/stores needed for structural memory provenance.
/// Arithmetic and floating-point execution remain deliberately outside this
/// narrow analysis-only helper.
pub fn decode_floating_memory_instruction(
    blocker: UnsupportedInstruction,
) -> Option<FloatingMemoryInstruction> {
    if blocker.class != UnsupportedInstructionClass::FloatingPoint {
        return None;
    }
    let raw = blocker.raw;
    let decoded = if blocker.width == 2 {
        let quadrant = raw & 0x3;
        let funct3 = (raw >> 13) & 0x7;
        match (quadrant, funct3) {
            (0, 3) => FloatingMemoryInstruction {
                address: blocker.address,
                instruction_width: blocker.width,
                access: FloatingMemoryAccess::Load,
                floating_register: 8 + ((raw >> 2) & 0x7) as u8,
                base: Reg(8 + ((raw >> 7) & 0x7) as u8),
                offset: ((((raw >> 5) & 0x1) << 6)
                    | (((raw >> 10) & 0x7) << 3)
                    | (((raw >> 6) & 0x1) << 2)) as i32,
            },
            (0, 7) => FloatingMemoryInstruction {
                address: blocker.address,
                instruction_width: blocker.width,
                access: FloatingMemoryAccess::Store,
                floating_register: 8 + ((raw >> 2) & 0x7) as u8,
                base: Reg(8 + ((raw >> 7) & 0x7) as u8),
                offset: ((((raw >> 5) & 0x1) << 6)
                    | (((raw >> 10) & 0x7) << 3)
                    | (((raw >> 6) & 0x1) << 2)) as i32,
            },
            (2, 3) => FloatingMemoryInstruction {
                address: blocker.address,
                instruction_width: blocker.width,
                access: FloatingMemoryAccess::Load,
                floating_register: ((raw >> 7) & 0x1f) as u8,
                base: Reg::SP,
                offset: ((((raw >> 2) & 0x3) << 6)
                    | (((raw >> 12) & 0x1) << 5)
                    | (((raw >> 4) & 0x7) << 2)) as i32,
            },
            (2, 7) => FloatingMemoryInstruction {
                address: blocker.address,
                instruction_width: blocker.width,
                access: FloatingMemoryAccess::Store,
                floating_register: ((raw >> 2) & 0x1f) as u8,
                base: Reg::SP,
                offset: ((((raw >> 7) & 0x3) << 6) | (((raw >> 9) & 0xf) << 2)) as i32,
            },
            _ => return None,
        }
    } else {
        let opcode = raw & 0x7f;
        let funct3 = (raw >> 12) & 0x7;
        if funct3 != 2 {
            return None;
        }
        match opcode {
            0x07 => FloatingMemoryInstruction {
                address: blocker.address,
                instruction_width: blocker.width,
                access: FloatingMemoryAccess::Load,
                floating_register: ((raw >> 7) & 0x1f) as u8,
                base: Reg(((raw >> 15) & 0x1f) as u8),
                offset: (raw as i32) >> 20,
            },
            0x27 => {
                let immediate = ((raw >> 7) & 0x1f) | ((raw >> 25) << 5);
                FloatingMemoryInstruction {
                    address: blocker.address,
                    instruction_width: blocker.width,
                    access: FloatingMemoryAccess::Store,
                    floating_register: ((raw >> 20) & 0x1f) as u8,
                    base: Reg(((raw >> 15) & 0x1f) as u8),
                    offset: ((immediate << 20) as i32) >> 20,
                }
            }
            _ => return None,
        }
    };
    Some(decoded)
}

/// Decode the reviewed RV32F subset used by structural value flow.
pub fn decode_floating_data_instruction(
    blocker: UnsupportedInstruction,
) -> Option<FloatingDataInstruction> {
    if blocker.class != UnsupportedInstructionClass::FloatingPoint || blocker.width != 4 {
        return None;
    }
    let raw = blocker.raw;
    let opcode = raw & 0x7f;
    let funct7 = (raw >> 25) & 0x7f;
    let funct3 = (raw >> 12) & 0x7;
    let destination = ((raw >> 7) & 0x1f) as u8;
    let source1 = ((raw >> 15) & 0x1f) as u8;
    let source2 = ((raw >> 20) & 0x1f) as u8;
    let source3 = ((raw >> 27) & 0x1f) as u8;
    let rounding = || match funct3 {
        0 => Some(FloatingRoundingMode::NearestEven),
        1 => Some(FloatingRoundingMode::TowardZero),
        2 => Some(FloatingRoundingMode::Down),
        3 => Some(FloatingRoundingMode::Up),
        4 => Some(FloatingRoundingMode::NearestMaxMagnitude),
        7 => Some(FloatingRoundingMode::Dynamic),
        _ => None,
    };
    let (operation, source3, rounding) = match opcode {
        0x43 if ((raw >> 25) & 0x3) == 0 => (
            FloatingDataOperation::FusedMultiplyAddSingle,
            Some(source3),
            Some(rounding()?),
        ),
        0x53 => match (funct7, funct3, source2) {
            (0x78, 0, 0) => (FloatingDataOperation::MoveFromInteger, None, None),
            (0x70, 0, 0) => (FloatingDataOperation::MoveToInteger, None, None),
            (0x10, 0, _) => (FloatingDataOperation::SignCopy, None, None),
            (0x10, 1, _) => (FloatingDataOperation::SignNegate, None, None),
            (0x10, 2, _) => (FloatingDataOperation::SignXor, None, None),
            (0x50, 0, _) => (FloatingDataOperation::CompareLessOrEqual, None, None),
            (0x50, 1, _) => (FloatingDataOperation::CompareLess, None, None),
            (0x50, 2, _) => (FloatingDataOperation::CompareEqual, None, None),
            (0x68, _, 0) => (
                FloatingDataOperation::SignedWordToSingle,
                None,
                Some(rounding()?),
            ),
            (0x04, _, _) => (
                FloatingDataOperation::SubtractSingle,
                None,
                Some(rounding()?),
            ),
            (0x0c, _, _) => (FloatingDataOperation::DivideSingle, None, Some(rounding()?)),
            (0x60, _, 0) => (
                FloatingDataOperation::SingleToSignedWord,
                None,
                Some(rounding()?),
            ),
            _ => return None,
        },
        _ => return None,
    };
    Some(FloatingDataInstruction {
        operation,
        destination,
        source1,
        source2,
        source3,
        rounding,
    })
}

fn classify_unsupported(address: u64, width: u8, raw: u32) -> UnsupportedInstruction {
    let (class, integer_destination, linear_control_flow) = if raw == 0 {
        // All-zero halfwords are illegal RISC-V encodings. Toolchains also use
        // them as fill after noreturn calls or branches, so preserve the site
        // without claiming either reachability or a decoding defect.
        (
            UnsupportedInstructionClass::ZeroFillOrIllegalTrap,
            None,
            false,
        )
    } else if width == 2 {
        // RV32 C.FLW/C.FSW and their stack-pointer forms use funct3 011/111.
        let funct3 = (raw >> 13) & 0x7;
        let quadrant = raw & 0x3;
        if matches!((quadrant, funct3), (0, 3 | 7) | (2, 3 | 7)) {
            (UnsupportedInstructionClass::FloatingPoint, None, true)
        } else {
            (UnsupportedInstructionClass::Invalid, None, false)
        }
    } else {
        let opcode = raw & 0x7f;
        match opcode {
            // LOAD-FP, STORE-FP, fused multiply-add family and OP-FP.
            0x07 | 0x27 | 0x43 | 0x47 | 0x4b | 0x4f => {
                (UnsupportedInstructionClass::FloatingPoint, None, true)
            }
            // Only comparisons, float-to-integer conversions, moves to X and
            // classification write the integer register file. Arithmetic and
            // moves between F registers must not destroy an unrelated X
            // register that happens to share the same encoded index.
            0x53 => {
                let funct7 = (raw >> 25) & 0x7f;
                let integer_destination = matches!(funct7, 0x50 | 0x51 | 0x60 | 0x61 | 0x70 | 0x71)
                    .then_some(((raw >> 7) & 0x1f) as u8)
                    .filter(|register| *register != 0);
                (
                    UnsupportedInstructionClass::FloatingPoint,
                    integer_destination,
                    true,
                )
            }
            // CSR instructions are sequential and may define an integer rd.
            0x73 if ((raw >> 12) & 0x7) != 0 => (
                classify_csr(((raw >> 20) & 0xfff) as u16),
                Some(((raw >> 7) & 0x1f) as u8).filter(|register| *register != 0),
                true,
            ),
            // Privileged/system instructions may alter execution state or PC.
            0x73 => (UnsupportedInstructionClass::System, None, false),
            // RISC-V reserves these four major opcodes for custom extensions.
            0x0b | 0x2b | 0x5b | 0x7b => (UnsupportedInstructionClass::VendorCustom, None, false),
            // Vector and other standard-extension encodings not modeled here.
            0x57 => (UnsupportedInstructionClass::OtherExtension, None, false),
            _ => (UnsupportedInstructionClass::Invalid, None, false),
        }
    };
    UnsupportedInstruction {
        address,
        width,
        raw,
        class,
        integer_destination,
        linear_control_flow,
    }
}

fn classify_csr(csr: u16) -> UnsupportedInstructionClass {
    match csr {
        // FFLAGS, FRM and FCSR belong to the F extension.
        0x001..=0x003 => UnsupportedInstructionClass::FloatingPointCsr,
        // Standard CSR allocation reserves these privilege-level windows for
        // custom extensions. Target packs may identify the concrete owners.
        0x2c0..=0x2ff
        | 0x5c0..=0x5ff
        | 0x6c0..=0x6ff
        | 0x7c0..=0x7ff
        | 0x9c0..=0x9ff
        | 0xac0..=0xaff
        | 0xbc0..=0xbff
        | 0xcc0..=0xcff
        | 0xdc0..=0xdff
        | 0xec0..=0xeff => UnsupportedInstructionClass::VendorCsr,
        _ => UnsupportedInstructionClass::Csr,
    }
}

/// Classify the JALR half of a standard two-instruction RISC-V call
/// relocation. Returns `None` for malformed or non-standard link registers.
pub fn relocated_call_is_tail(
    symbol: &ArtifactSymbolDefinition,
    relocation_address: u32,
) -> Option<bool> {
    let jalr_address = relocation_address.checked_add(4)?;
    let instruction = decode_symbol(symbol)
        .ok()?
        .into_iter()
        .find(|decoded| decoded.address == u64::from(jalr_address))?
        .instruction;
    match instruction {
        Inst::Jalr {
            dest: Reg::ZERO, ..
        } => Some(true),
        Inst::Jalr { dest: Reg::RA, .. } => Some(false),
        _ => None,
    }
}
