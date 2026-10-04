//! The relocations a link kept with `--emit-relocs`, each checked against
//! the bytes it describes: what the toolchain knows about a jump table, read
//! back instead of reconstructed from machine code.
use object::{Object, ObjectSection, ObjectSymbol, RelocationFlags, RelocationTarget};
use oer_riscv_model::{Error, ErrorCode, Result};
use std::collections::BTreeMap;

/// `R_RISCV_32`: a word that holds an address.
const R_RISCV_32: u32 = 1;

/// The address words of an image's allocated data, by address.
pub(crate) struct Relocations {
    words: BTreeMap<u32, u32>,
    /// Starts of the compiler's jump tables (`.LJTI*` labels), with the next
    /// symbol of their section or its end.
    tables: BTreeMap<u32, u32>,
}

impl Relocations {
    /// Read the relocations `elf` kept; none when it kept none.
    pub(crate) fn read(elf: &[u8]) -> Result<Self> {
        let invalid = |message: String| Error::new(ErrorCode::Integrity, message);
        let file = object::File::parse(elf).map_err(|_| invalid("invalid ELF".into()))?;
        let mut words = BTreeMap::new();
        for section in file.sections() {
            let Ok(data) = section.data() else { continue };
            let object::SectionFlags::Elf { sh_flags } = section.flags() else {
                continue;
            };
            let allocated = sh_flags & u64::from(object::elf::SHF_ALLOC) != 0;
            let code = sh_flags & u64::from(object::elf::SHF_EXECINSTR) != 0;
            if !allocated || code || data.is_empty() {
                continue;
            }
            for (offset, relocation) in section.relocations() {
                let RelocationFlags::Elf { r_type: R_RISCV_32 } = relocation.flags() else {
                    continue;
                };
                let base = match relocation.target() {
                    RelocationTarget::Symbol(index) => file
                        .symbol_by_index(index)
                        .map_err(|_| invalid("relocation to a missing symbol".into()))?
                        .address(),
                    RelocationTarget::Section(index) => file
                        .section_by_index(index)
                        .map_err(|_| invalid("relocation to a missing section".into()))?
                        .address(),
                    // No symbol: the addend is the address.
                    RelocationTarget::Absolute => 0,
                    _ => return Err(invalid("relocation without a target".into())),
                };
                let target = u32::try_from(base as i64 + relocation.addend())
                    .map_err(|_| invalid("relocation target beyond RV32".into()))?;
                let address =
                    u32::try_from(offset).map_err(|_| invalid("relocation beyond RV32".into()))?;
                let at = usize::try_from(offset - section.address())
                    .map_err(|_| invalid("relocation outside its section".into()))?;
                let word = data.get(at..at + 4).ok_or_else(|| {
                    invalid(format!("relocation at {address:#010x} outside its section"))
                })?;
                let held = u32::from_le_bytes([word[0], word[1], word[2], word[3]]);
                if held != target {
                    return Err(invalid(format!(
                        "relocation at {address:#010x} names {target:#010x}, the word holds {held:#010x}"
                    )));
                }
                words.insert(address, target);
            }
        }
        // Every symbol address of each section, to find where a table ends.
        let mut addresses: BTreeMap<usize, Vec<u64>> = BTreeMap::new();
        for symbol in file.symbols() {
            if let Some(index) = symbol.section_index() {
                addresses.entry(index.0).or_default().push(symbol.address());
            }
        }
        for list in addresses.values_mut() {
            list.sort_unstable();
            list.dedup();
        }
        let mut tables = BTreeMap::new();
        for symbol in file.symbols() {
            let Ok(name) = symbol.name() else { continue };
            if !name.starts_with(".LJTI") {
                continue;
            }
            let start = symbol.address();
            let Some(index) = symbol.section_index() else {
                continue;
            };
            let Ok(section) = file.section_by_index(index) else {
                continue;
            };
            let list = &addresses[&index.0];
            let next = list
                .get(list.partition_point(|&address| address <= start))
                .copied()
                .unwrap_or(section.address() + section.size());
            let (Ok(start), Ok(next)) = (u32::try_from(start), u32::try_from(next)) else {
                continue;
            };
            tables.insert(start, next);
        }
        Ok(Self { words, tables })
    }

    /// Every relocated word: its address and the address it holds.
    pub(crate) fn words(&self) -> impl Iterator<Item = (u32, u32)> + '_ {
        self.words.iter().map(|(&at, &holds)| (at, holds))
    }

    /// The address the relocated word at `address` holds, if a relocation
    /// names it.
    pub(crate) fn word(&self, address: u32) -> Option<u32> {
        self.words.get(&address).copied()
    }

    /// Whether the compiler labelled a jump table at `base`.
    pub(crate) fn is_jump_table(&self, base: u32) -> bool {
        self.tables.contains_key(&base)
    }

    /// The entries of the jump table the compiler labelled at `base`: its
    /// relocated words up to the first word without one or the next symbol.
    pub(crate) fn jump_table(&self, base: u32) -> Option<Vec<u32>> {
        let end = *self.tables.get(&base)?;
        let entries: Vec<u32> = (base..end)
            .step_by(4)
            .map_while(|address| self.words.get(&address).copied())
            .collect();
        (!entries.is_empty()).then_some(entries)
    }
}

/// Every address an allocated section of `elf` takes: a relocation's target
/// other than a call, a jump or a branch.
pub fn taken_addresses(elf: &[u8]) -> Result<std::collections::BTreeSet<u32>> {
    // R_RISCV_BRANCH, _JAL, _CALL, _CALL_PLT, _RVC_BRANCH, _RVC_JUMP and
    // _RELAX, which only marks the pair before it.
    const TRANSFERS: [u32; 7] = [16, 17, 18, 19, 44, 45, 51];
    let invalid = |message: &str| Error::new(ErrorCode::Integrity, message.to_owned());
    let file = object::File::parse(elf).map_err(|_| invalid("invalid ELF"))?;
    let mut taken = std::collections::BTreeSet::new();
    for section in file.sections() {
        let object::SectionFlags::Elf { sh_flags } = section.flags() else {
            continue;
        };
        if sh_flags & u64::from(object::elf::SHF_ALLOC) == 0 {
            continue;
        }
        for (_, relocation) in section.relocations() {
            let RelocationFlags::Elf { r_type } = relocation.flags() else {
                continue;
            };
            if TRANSFERS.contains(&r_type) {
                continue;
            }
            let base = match relocation.target() {
                RelocationTarget::Symbol(index) => file
                    .symbol_by_index(index)
                    .map_err(|_| invalid("relocation to a missing symbol"))?
                    .address(),
                RelocationTarget::Absolute => 0,
                _ => continue,
            };
            if let Ok(address) = u32::try_from(base as i64 + relocation.addend()) {
                taken.insert(address & !1);
            }
        }
    }
    Ok(taken)
}

/// Whether any allocated section of `elf` takes the address of `function`.
/// Without one, every transfer to it is a direct call the analysis sees.
pub fn address_taken(elf: &[u8], function: u32) -> Result<bool> {
    Ok(taken_addresses(elf)?.contains(&function))
}
