//! Relocation-normalized fingerprints of vendor RV32 functions.
//!
//! A function's fingerprint covers its code bytes with every field a
//! relocation patches masked by the RISC-V relocation type, followed by the
//! relocation stream: type and, for a target inside the function's own
//! section, its offset from the function start. External targets enter only
//! the named fingerprint, so a function whose code is unchanged but whose
//! obfuscated callees were renamed keeps its code fingerprint.
//!
//! Instructions are split by the RISC-V length encoding (two bytes unless the
//! low two bits are set), giving a token sequence for similarity between
//! changed functions.
use crate::Result;
use object::elf;
use object::read::archive::ArchiveFile;
use object::{
    Object, ObjectSection, ObjectSymbol, RelocationFlags, RelocationTarget, SymbolKind,
    SymbolSection,
};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

/// One defined function of an archive member or ELF.
#[derive(Clone, Debug)]
pub struct Function {
    /// Archive member, or empty for a standalone ELF.
    pub member: String,
    pub name: String,
    /// Code fingerprint: masked bytes and relocation kinds.
    pub code: String,
    /// Code fingerprint plus external relocation target names.
    pub named: String,
    /// One token per instruction, for similarity.
    pub tokens: Vec<u64>,
}

struct Relocation {
    offset: usize,
    kind: u32,
    /// Offset from the function start of a target in the same section, or
    /// the external target's name.
    target: Target,
}

enum Target {
    Local(i64),
    External(String),
}

/// Bit masks of the fields a relocation of `kind` patches, at the
/// relocation offset: (byte length, mask of the first word, mask of a
/// second word for paired relocations).
fn masks(kind: u32) -> (usize, u32, u32) {
    match kind {
        elf::R_RISCV_HI20
        | elf::R_RISCV_PCREL_HI20
        | elf::R_RISCV_GOT_HI20
        | elf::R_RISCV_TPREL_HI20
        | elf::R_RISCV_JAL => (4, 0xffff_f000, 0),
        elf::R_RISCV_LO12_I | elf::R_RISCV_PCREL_LO12_I | elf::R_RISCV_TPREL_LO12_I => {
            (4, 0xfff0_0000, 0)
        }
        elf::R_RISCV_LO12_S
        | elf::R_RISCV_PCREL_LO12_S
        | elf::R_RISCV_TPREL_LO12_S
        | elf::R_RISCV_BRANCH => (4, 0xfe00_0f80, 0),
        elf::R_RISCV_CALL | elf::R_RISCV_CALL_PLT => (8, 0xffff_f000, 0xfff0_0000),
        elf::R_RISCV_RVC_BRANCH => (2, 0x1c7c, 0),
        elf::R_RISCV_RVC_JUMP => (2, 0x1ffc, 0),
        // Linker hints that patch nothing.
        elf::R_RISCV_RELAX | elf::R_RISCV_ALIGN => (0, 0, 0),
        // Data and every other patching relocation: the whole word.
        _ => (4, u32::MAX, 0),
    }
}

fn mask(code: &mut [u8], relocation: &Relocation) {
    let (length, first, second) = masks(relocation.kind);
    let at = relocation.offset;
    let apply = |code: &mut [u8], at: usize, mask: u32, width: usize| {
        if at + width > code.len() {
            return;
        }
        let mut word = [0u8; 4];
        word[..width].copy_from_slice(&code[at..at + width]);
        let value = u32::from_le_bytes(word) & !mask;
        code[at..at + width].copy_from_slice(&value.to_le_bytes()[..width]);
    };
    match length {
        0 => {}
        2 => apply(code, at, first, 2),
        8 => {
            apply(code, at, first, 4);
            apply(code, at + 4, second, 4);
        }
        _ => apply(code, at, first, 4),
    }
}

fn hex(digest: impl AsRef<[u8]>) -> String {
    digest.as_ref().iter().map(|b| format!("{b:02x}")).collect()
}

fn token(bytes: &[u8], relocations: &[&Relocation], named: bool) -> u64 {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    for relocation in relocations {
        hasher.update(relocation.kind.to_le_bytes());
        match &relocation.target {
            Target::Local(offset) => hasher.update(offset.to_le_bytes()),
            Target::External(name) if named => hasher.update(name.as_bytes()),
            Target::External(_) => hasher.update(b"external"),
        }
    }
    u64::from_le_bytes(hasher.finalize()[..8].try_into().expect("eight bytes"))
}

fn fingerprint(
    member: &str,
    name: &str,
    mut code: Vec<u8>,
    relocations: Vec<Relocation>,
) -> Function {
    for relocation in &relocations {
        mask(&mut code, relocation);
    }
    let mut by_offset: BTreeMap<usize, Vec<&Relocation>> = BTreeMap::new();
    for relocation in &relocations {
        by_offset
            .entry(relocation.offset)
            .or_default()
            .push(relocation);
    }
    let (mut code_hash, mut named_hash) = (Sha256::new(), Sha256::new());
    let mut tokens = vec![];
    let mut at = 0;
    while at < code.len() {
        let width = if code[at] & 3 == 3 { 4 } else { 2 }.min(code.len() - at);
        let here: Vec<&Relocation> = by_offset
            .range(at..at + width)
            .flat_map(|(_, list)| list.iter().copied())
            .collect();
        let bytes = &code[at..at + width];
        code_hash.update(token(bytes, &here, false).to_le_bytes());
        named_hash.update(token(bytes, &here, true).to_le_bytes());
        tokens.push(token(bytes, &here, false));
        at += width;
    }
    Function {
        member: member.to_owned(),
        name: name.to_owned(),
        code: hex(code_hash.finalize()),
        named: hex(named_hash.finalize()),
        tokens,
    }
}

fn object_functions(member: &str, data: &[u8], out: &mut Vec<Function>) -> Result<()> {
    let file = object::File::parse(data)?;
    // Relocations of each section, collected once.
    let mut section_relocations: BTreeMap<usize, Vec<(u64, object::Relocation)>> = BTreeMap::new();
    for section in file.sections() {
        section_relocations.insert(section.index().0, section.relocations().collect());
    }
    for symbol in file.symbols() {
        if symbol.kind() != SymbolKind::Text || symbol.size() == 0 || !symbol.is_definition() {
            continue;
        }
        let SymbolSection::Section(index) = symbol.section() else {
            continue;
        };
        let Ok(name) = symbol.name() else { continue };
        if name.is_empty() {
            continue;
        }
        let section = file.section_by_index(index)?;
        let Ok(data) = section.data() else { continue };
        let start = (symbol.address() - section.address()) as usize;
        let end = start + symbol.size() as usize;
        if end > data.len() {
            continue;
        }
        let mut relocations = vec![];
        for (offset, relocation) in section_relocations.get(&index.0).into_iter().flatten() {
            let offset = *offset as usize;
            if offset < start || offset >= end {
                continue;
            }
            let RelocationFlags::Elf { r_type } = relocation.flags() else {
                continue;
            };
            let target = match relocation.target() {
                RelocationTarget::Symbol(target) => {
                    let target = file.symbol_by_index(target)?;
                    match target.section() {
                        SymbolSection::Section(target_section)
                            if target_section == index
                                && (target.kind() == SymbolKind::Section
                                    || !target.is_global()) =>
                        {
                            Target::Local(
                                target.address() as i64 - section.address() as i64
                                    + relocation.addend()
                                    - start as i64,
                            )
                        }
                        _ => Target::External(target.name().unwrap_or_default().to_owned()),
                    }
                }
                _ => Target::External(String::new()),
            };
            relocations.push(Relocation {
                offset: offset - start,
                kind: r_type,
                target,
            });
        }
        out.push(fingerprint(
            member,
            name,
            data[start..end].to_vec(),
            relocations,
        ));
    }
    Ok(())
}

/// Whether `bytes` are an archive or an ELF object, the artifacts that hold
/// vendor functions; pinned source files are not.
pub fn is_binary(bytes: &[u8]) -> bool {
    bytes.starts_with(b"!<arch>\n") || bytes.starts_with(b"\x7fELF")
}

/// Every defined function of an archive's ELF members, or of one ELF.
pub fn functions(bytes: &[u8]) -> Result<Vec<Function>> {
    let mut out = vec![];
    if let Ok(archive) = ArchiveFile::parse(bytes) {
        for member in archive.members() {
            let member = member?;
            let name = String::from_utf8_lossy(member.name()).into_owned();
            let data = member.data(bytes)?;
            if object::File::parse(data).is_ok() {
                object_functions(&name, data, &mut out)?;
            }
        }
    } else {
        object_functions("", bytes, &mut out)?;
    }
    Ok(out)
}

/// Similarity of two token sequences: twice their longest common
/// subsequence over their total length.
pub fn similarity(a: &[u64], b: &[u64]) -> f64 {
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    let mut previous = vec![0u32; b.len() + 1];
    let mut current = vec![0u32; b.len() + 1];
    for x in a {
        for (j, y) in b.iter().enumerate() {
            current[j + 1] = if x == y {
                previous[j] + 1
            } else {
                current[j].max(previous[j + 1])
            };
        }
        std::mem::swap(&mut previous, &mut current);
    }
    2.0 * f64::from(previous[b.len()]) / (a.len() + b.len()) as f64
}

#[cfg(test)]
mod tests;
