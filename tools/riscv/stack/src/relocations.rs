//! The relocations a link kept with `--emit-relocs`, each checked against
//! the bytes it describes: what the toolchain knows about a jump table, read
//! back instead of reconstructed from machine code.
use oer_elf::rv32::{self, Role};
use oer_riscv_model::{Error, ErrorCode, Result};
use std::collections::BTreeMap;

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
        let file = oer_elf::Elf::parse(elf).map_err(|_| invalid("invalid ELF".into()))?;
        let mut words = BTreeMap::new();
        for section in file.sections() {
            if !section.allocated || section.executable || section.data.is_empty() {
                continue;
            }
            let relocations = file
                .relocations(section.index)
                .map_err(|error| invalid(error.to_string()))?;
            for relocation in relocations {
                if rv32::kind(relocation.r_type).role != Role::Word {
                    continue;
                }
                let target = file
                    .target_address(&relocation)
                    .map_err(|error| invalid(error.to_string()))?;
                let target = u32::try_from(target)
                    .map_err(|_| invalid("relocation target beyond RV32".into()))?;
                let address = u32::try_from(relocation.at)
                    .map_err(|_| invalid("relocation beyond RV32".into()))?;
                let held = section.word(relocation.at).ok_or_else(|| {
                    invalid(format!("relocation at {address:#010x} outside its section"))
                })?;
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
            if let Some(index) = symbol.section {
                addresses.entry(index).or_default().push(symbol.address);
            }
        }
        for list in addresses.values_mut() {
            list.sort_unstable();
            list.dedup();
        }
        let mut tables = BTreeMap::new();
        for symbol in file.symbols() {
            if !symbol.name.starts_with(".LJTI") {
                continue;
            }
            let start = symbol.address;
            let Some(index) = symbol.section else {
                continue;
            };
            let Ok(section) = file.section(index) else {
                continue;
            };
            let list = &addresses[&index];
            let next = list
                .get(list.partition_point(|&address| address <= start))
                .copied()
                .unwrap_or(section.address + section.size);
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
/// other than a call, a jump, a branch or a linker hint.
pub fn taken_addresses(elf: &[u8]) -> Result<std::collections::BTreeSet<u32>> {
    let invalid = |message: String| Error::new(ErrorCode::Integrity, message);
    let file = oer_elf::Elf::parse(elf).map_err(|_| invalid("invalid ELF".into()))?;
    let mut taken = std::collections::BTreeSet::new();
    for section in file.sections().filter(|section| section.allocated) {
        let relocations = file
            .relocations(section.index)
            .map_err(|error| invalid(error.to_string()))?;
        for relocation in relocations {
            if rv32::transfers(relocation.r_type)
                || rv32::kind(relocation.r_type).role == Role::Hint
                || matches!(
                    relocation.target,
                    oer_elf::Target::Section(_) | oer_elf::Target::Other
                )
            {
                continue;
            }
            let target = file
                .target_address(&relocation)
                .map_err(|error| invalid(error.to_string()))?;
            if let Ok(address) = u32::try_from(target) {
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
