//! RV32 instruction-decoder boundary and relocation-aware call helpers.

use rv_asm::{Imm, Inst, IsCompressed, Reg, Xlen};

use crate::{Error, Result};

use super::model::{
    AnalysisInstruction, ArtifactSymbolDefinition, DecodedInstruction, UnsupportedInstruction,
    UnsupportedInstructionClass,
};

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
            // OP-FP also contains comparisons, conversions and moves whose
            // destination is an integer register. Invalidating rd for the
            // complete opcode is conservative for operations that write f-rd.
            0x53 => (
                UnsupportedInstructionClass::FloatingPoint,
                Some(((raw >> 7) & 0x1f) as u8).filter(|register| *register != 0),
                true,
            ),
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
