//! Structural ELF/archive loading and instruction decoding.
//!
//! This module deliberately does not invoke binutils. Symbol boundaries and
//! instruction bytes come from the binary containers themselves.

use std::{fs, path::Path};

use object::{
    FileKind, Object, ObjectSection, ObjectSymbol, SymbolKind, read::archive::ArchiveFile,
};
use rv_asm::{Imm, Inst, IsCompressed, Xlen};

use crate::{Error, Result};

#[derive(Clone, Debug)]
pub struct BinarySymbol {
    pub member: Option<String>,
    pub name: String,
    pub address: u64,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DecodedInstruction {
    pub address: u64,
    pub width: u8,
    pub instruction: Inst,
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

fn collect_object_symbols(
    data: &[u8],
    member: Option<&str>,
    prefix: &str,
    output: &mut Vec<BinarySymbol>,
) -> Result<()> {
    let file = object::File::parse(data)?;
    if file.architecture() != object::Architecture::Riscv32 {
        return Err(format!("artifact member {member:?} is not RISC-V 32-bit").into());
    }
    if !file.is_little_endian() {
        return Err(format!("artifact member {member:?} is not little-endian").into());
    }

    for symbol in file.symbols() {
        if symbol.kind() != SymbolKind::Text
            || !symbol.is_definition()
            || !(symbol.is_global() || symbol.is_weak())
            || symbol.size() == 0
        {
            continue;
        }
        let name = symbol.name()?;
        if !name.starts_with(prefix) {
            continue;
        }
        let section_index = symbol
            .section_index()
            .ok_or_else(|| format!("text symbol {name} has no section"))?;
        let section = file.section_by_index(section_index)?;
        let section_data = section.data()?;
        let start = symbol
            .address()
            .checked_sub(section.address())
            .ok_or_else(|| format!("symbol {name} precedes its section"))?
            as usize;
        let end = start
            .checked_add(symbol.size() as usize)
            .ok_or_else(|| format!("symbol {name} size overflows"))?;
        let bytes = section_data
            .get(start..end)
            .ok_or_else(|| format!("symbol {name} exceeds its section"))?
            .to_vec();
        output.push(BinarySymbol {
            member: member.map(str::to_owned),
            name: name.to_owned(),
            address: symbol.address(),
            bytes,
        });
    }
    Ok(())
}

pub fn load_symbols(path: &Path, prefix: &str) -> Result<Vec<BinarySymbol>> {
    let data = fs::read(path)?;
    let mut symbols = Vec::new();
    match FileKind::parse(data.as_slice())? {
        FileKind::Archive => {
            let archive = ArchiveFile::parse(data.as_slice())?;
            for member in archive.members() {
                let member = member?;
                let name = String::from_utf8_lossy(member.name()).into_owned();
                let member_data = member.data(data.as_slice())?;
                if matches!(FileKind::parse(member_data), Ok(FileKind::Elf32)) {
                    collect_object_symbols(member_data, Some(&name), prefix, &mut symbols)?;
                }
            }
        }
        FileKind::Elf32 => collect_object_symbols(&data, None, prefix, &mut symbols)?,
        kind => return Err(format!("unsupported artifact kind: {kind:?}").into()),
    }
    symbols.sort_by(|left, right| (&left.member, &left.name).cmp(&(&right.member, &right.name)));
    Ok(symbols)
}

pub fn decode_symbol(symbol: &BinarySymbol) -> Result<Vec<DecodedInstruction>> {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn oracle(name: &str) -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("_oracles")
            .join(name)
    }

    #[test]
    fn structural_loader_reproduces_both_vendor_inventories() {
        assert_eq!(
            load_symbols(&oracle("esp32s31_rev0_rom.elf"), "phy_")
                .unwrap()
                .len(),
            305
        );
        assert_eq!(load_symbols(&oracle("libphy.a"), "").unwrap().len(), 161);
    }

    #[test]
    fn decoder_reads_mixed_width_rom_code_without_objdump() {
        let symbols = load_symbols(&oracle("esp32s31_rev0_rom.elf"), "phy_disable_agc").unwrap();
        let symbol = symbols
            .iter()
            .find(|symbol| symbol.name == "phy_disable_agc")
            .unwrap();
        let decoded = decode_symbol(symbol).unwrap();
        assert_eq!(decoded.len(), 6);
        assert_eq!(decoded.first().unwrap().width, 4);
        assert_eq!(decoded.last().unwrap().width, 2);
    }

    #[test]
    fn compressed_andi_immediate_is_sign_extended() {
        // c.andi a5, -2
        let (instruction, width) = Inst::decode(0x9b_f9, Xlen::Rv32).unwrap();
        let Inst::Andi { imm, .. } = instruction else {
            panic!("expected C.ANDI, got {instruction}");
        };
        assert_eq!(width, IsCompressed::Yes);
        assert_eq!(imm.as_u32(), 0x3e); // rv-asm 0.2.1 decoder behavior
        assert_eq!(andi_immediate(imm, 2), 0xffff_fffe);
    }
}
