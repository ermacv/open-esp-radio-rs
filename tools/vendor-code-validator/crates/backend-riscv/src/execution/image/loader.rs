//! ELF loading, symbol collection and relocation resolution.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fs,
    path::Path,
};

use object::{
    Object, ObjectSection, ObjectSegment, ObjectSymbol, RelocationFlags, RelocationTarget,
    SectionKind, SymbolKind, SymbolSection,
};

use super::{ExecutableImage, RelocatedCall, Segment, UnresolvedRelocation};
use crate::Result;

impl ExecutableImage {
    pub fn load(path: &Path) -> Result<Self> {
        let bytes = fs::read(path)?;
        let file = object::File::parse(bytes.as_slice())?;
        if file.architecture() != object::Architecture::Riscv32 || !file.is_little_endian() {
            return Err("execution requires a little-endian RISC-V 32-bit ELF".into());
        }
        let mut segments = Vec::new();
        for segment in file.segments() {
            if segment.address() == 0 || segment.size() == 0 {
                continue;
            }
            let address = u32::try_from(segment.address())
                .map_err(|_| "load segment address does not fit RV32")?;
            let memory_size =
                u32::try_from(segment.size()).map_err(|_| "load segment size does not fit RV32")?;
            let data = segment.data()?;
            if data.len() > memory_size as usize {
                return Err("ELF segment file size exceeds its memory size".into());
            }
            segments.push(Segment {
                address,
                bytes: data.to_vec(),
                memory_size,
                writable: matches!(
                    segment.flags(),
                    object::SegmentFlags::Elf { p_flags }
                        if p_flags & object::elf::PF_W != 0
                ),
            });
        }
        segments.sort_by_key(|segment| segment.address);

        let mut symbols_by_name = HashMap::new();
        let mut symbols_by_address = BTreeMap::new();
        let mut symbol_sizes_by_address: BTreeMap<u32, u32> = BTreeMap::new();
        let mut local_text_symbols = BTreeSet::new();
        let mut call_trampoline_addresses = BTreeSet::new();
        let mut global_pointer = None;
        for symbol in file.symbols() {
            if !symbol.is_definition() || symbol.address() == 0 {
                continue;
            }
            let Ok(name) = symbol.name() else {
                continue;
            };
            let address = symbol.address() as u32;
            if name.starts_with("__call_") {
                call_trampoline_addresses.insert(address);
            }
            let absolute_callable =
                symbol.kind() == SymbolKind::Unknown && symbol.section() == SymbolSection::Absolute;
            if symbol.kind() == SymbolKind::Text
                || symbol.kind() == SymbolKind::Data
                || absolute_callable
            {
                symbols_by_name.insert(name.to_owned(), address);
            }
            if symbol.kind() == SymbolKind::Text || absolute_callable {
                symbols_by_address
                    .entry(address)
                    .or_insert_with(|| name.to_owned());
            }
            if symbol.kind() == SymbolKind::Text
                && let Ok(size) = u32::try_from(symbol.size())
                && size != 0
            {
                symbol_sizes_by_address
                    .entry(address)
                    .and_modify(|current| *current = (*current).max(size))
                    .or_insert(size);
                if !symbol.is_global() && !symbol.is_weak() {
                    local_text_symbols.insert(address);
                }
            }
            if name == "__global_pointer$" {
                global_pointer = Some(address);
            }
        }
        let mut relocated_calls_by_address = BTreeMap::new();
        let mut unresolved_relocations_by_address = BTreeMap::new();
        for section in file.sections() {
            for (offset, relocation) in section.relocations() {
                let RelocationFlags::Elf { r_type } = relocation.flags() else {
                    continue;
                };
                let section_start = section.address();
                let section_end = section_start.wrapping_add(section.size());
                let address = if offset >= section_start && offset < section_end {
                    offset
                } else {
                    section_start.wrapping_add(offset)
                } as u32;
                if !matches!(
                    r_type,
                    object::elf::R_RISCV_CALL | object::elf::R_RISCV_CALL_PLT
                ) {
                    if !matches!(
                        r_type,
                        object::elf::R_RISCV_NONE | object::elf::R_RISCV_RELAX
                    ) && matches!(
                        section.kind(),
                        SectionKind::Text
                            | SectionKind::ReadOnlyData
                            | SectionKind::ReadOnlyString
                            | SectionKind::Data
                            | SectionKind::Tls
                    ) && let RelocationTarget::Symbol(index) = relocation.target()
                    {
                        let symbol = file.symbol_by_index(index)?;
                        if !symbol.is_definition() && symbol.section() != SymbolSection::Absolute {
                            unresolved_relocations_by_address.insert(
                                address,
                                UnresolvedRelocation {
                                    name: symbol.name().unwrap_or("<unnamed>").to_owned(),
                                    r_type,
                                    width: unresolved_relocation_width(
                                        r_type,
                                        relocation.size(),
                                        section.kind() == SectionKind::Text,
                                    ),
                                },
                            );
                        }
                    }
                    continue;
                }
                let RelocationTarget::Symbol(index) = relocation.target() else {
                    continue;
                };
                let symbol = file.symbol_by_index(index)?;
                let name = symbol.name()?.to_owned();
                let target = (symbol.is_definition()
                    || symbol.section() == SymbolSection::Absolute)
                    .then_some(symbol.address() as u32)
                    .filter(|address| *address != 0);
                relocated_calls_by_address.insert(address, RelocatedCall { name, target });
            }
        }

        Ok(Self {
            segments,
            symbols_by_name,
            symbols_by_address,
            symbol_sizes_by_address,
            local_text_symbols,
            call_trampoline_addresses,
            relocated_calls_by_address,
            unresolved_relocations_by_address,
            global_pointer,
        })
    }

    pub fn add_companion(&mut self, path: &Path) -> Result<()> {
        let companion = Self::load(path)?;
        self.segments.extend(companion.segments);
        self.segments.sort_by_key(|segment| segment.address);
        for (name, address) in companion.symbols_by_name {
            self.symbols_by_name.entry(name).or_insert(address);
        }
        for (address, name) in companion.symbols_by_address {
            self.symbols_by_address.entry(address).or_insert(name);
        }
        for (address, size) in companion.symbol_sizes_by_address {
            self.symbol_sizes_by_address
                .entry(address)
                .and_modify(|current| *current = (*current).max(size))
                .or_insert(size);
        }
        self.local_text_symbols.extend(companion.local_text_symbols);
        self.call_trampoline_addresses
            .extend(companion.call_trampoline_addresses);
        for (address, call) in companion.relocated_calls_by_address {
            self.relocated_calls_by_address
                .entry(address)
                .or_insert(call);
        }
        for (address, relocation) in companion.unresolved_relocations_by_address {
            self.unresolved_relocations_by_address
                .entry(address)
                .or_insert(relocation);
        }
        self.resolve_external_relocations();
        Ok(())
    }

    pub(in crate::execution) fn resolve_external_relocations(&mut self) {
        for call in self.relocated_calls_by_address.values_mut() {
            if call.target.is_none() {
                call.target = self.symbols_by_name.get(&call.name).copied();
            }
        }
    }
}

fn unresolved_relocation_width(r_type: u32, reported_bits: u8, executable_text: bool) -> u8 {
    // Espressif RV32 vendor objects use R_RISCV_64 on a four-byte instruction
    // image for unresolved pointer materialization. The relocation name still
    // describes the source C type; it does not make an RV32 instruction eight
    // bytes wide.
    if executable_text && r_type == object::elf::R_RISCV_64 {
        return 4;
    }
    let reported_bytes = reported_bits.div_ceil(8);
    if reported_bytes != 0 {
        return reported_bytes.min(8);
    }
    match r_type {
        object::elf::R_RISCV_64 => 8,
        object::elf::R_RISCV_RVC_BRANCH | object::elf::R_RISCV_RVC_JUMP => 2,
        // One poisoned byte is sufficient to reject a use of an unfamiliar
        // relocation without guessing its encoding. Known RV32 instruction
        // and pointer relocations occupy four bytes.
        object::elf::R_RISCV_32
        | object::elf::R_RISCV_RELATIVE
        | object::elf::R_RISCV_HI20
        | object::elf::R_RISCV_LO12_I
        | object::elf::R_RISCV_LO12_S
        | object::elf::R_RISCV_PCREL_HI20
        | object::elf::R_RISCV_PCREL_LO12_I
        | object::elf::R_RISCV_PCREL_LO12_S
        | object::elf::R_RISCV_GOT_HI20 => 4,
        _ => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rv32_vendor_text_does_not_treat_one_instruction_as_an_eight_byte_pointer() {
        assert_eq!(
            unresolved_relocation_width(object::elf::R_RISCV_64, 64, true),
            4
        );
        assert_eq!(
            unresolved_relocation_width(object::elf::R_RISCV_64, 64, false),
            8
        );
    }
}
