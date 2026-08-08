//! RV32 instruction-decoder boundary and relocation-aware call helpers.

use rv_asm::{Imm, Inst, IsCompressed, Reg, Xlen};

use crate::{Error, Result};

use super::model::{ArtifactSymbolDefinition, DecodedInstruction};

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
