//! Physical external definitions do not acquire a runnable image or its memory view.
use blobray_domain::*;
use object::{Object, ObjectSection, ObjectSegment, ObjectSymbol, read::elf::SectionHeader};

fn invalid(message: &str) -> Error {
    Error::new(ErrorCode::InvalidRequest, message)
}

/// Validate one exact static function symbol and return its RV32 address.
///
/// The selected extent must have one unambiguous executable, file-backed load
/// mapping matching its allocated code section. Non-executable segments are not
/// loaded, including data mappings sharing a virtual address with code.
/// TLS, dynamic metadata and unrelated overlapping mappings do not grant or
/// prevent this address-only capability. Execution and function analysis retain
/// their own whole-image restrictions. Bytes and parser views die before return.
pub fn inspect_link_definition(
    source: &dyn ByteSource,
    selected: &SymbolId,
    memory: &WorkingMemory,
    control: &mut dyn RunControl,
) -> Result<u32> {
    if selected.table != SymbolTableKind::Static
        || selected.object.location != ObjectLocation::Standalone
    {
        return Err(invalid("definition requires a standalone static symbol"));
    }
    control.phase(RunPhase::PrepareObject)?;
    let size =
        usize::try_from(source.len()).map_err(|_| invalid("definition carrier too large"))?;
    let mut bytes = memory.bytes(size, control.position())?;
    source.read_at(0, &mut bytes, control)?;
    if ArtifactId::of_bytes_controlled(&bytes, control)? != selected.object.artifact {
        return Err(Error::new(
            ErrorCode::Integrity,
            "definition carrier digest differs",
        ));
    }
    let file = object::File::parse(&*bytes).map_err(|e| invalid(&e.to_string()))?;
    if file.is_64()
        || !file.is_little_endian()
        || file.architecture() != object::Architecture::Riscv32
        || file.kind() != object::ObjectKind::Executable
    {
        return Err(invalid(
            "definition carrier must be little-endian RV32 ET_EXEC",
        ));
    }
    crate::image::abi(&file)?;
    let object::File::Elf32(elf) = &file else {
        unreachable!()
    };
    let mut table = None;
    for (index, header) in elf.elf_section_table().iter().enumerate() {
        control.checkpoint(1)?;
        if header.sh_type(elf.endian()) == object::elf::SHT_SYMTAB && table.replace(index).is_some()
        {
            return Err(invalid("definition has ambiguous static symbol tables"));
        }
    }
    if table != Some(selected.table_section as usize) {
        return Err(invalid("definition symbol table differs"));
    }
    let symbol = file
        .symbol_by_index(object::SymbolIndex(
            usize::try_from(selected.index)
                .map_err(|_| invalid("definition symbol index overflow"))?,
        ))
        .map_err(|_| invalid("definition symbol absent"))?;
    if !matches!(symbol.flags(), object::SymbolFlags::Elf { st_info, .. }
        if st_info & 15 == object::elf::STT_FUNC)
        || symbol.size() == 0
        || symbol.address() & 1 != 0
    {
        return Err(invalid(
            "definition needs an aligned function with a declared nonzero extent",
        ));
    }
    let section = file
        .section_by_index(
            symbol
                .section_index()
                .ok_or_else(|| invalid("definition has no physical code section"))?,
        )
        .map_err(|_| invalid("definition section absent"))?;
    if !matches!(section.flags(), object::SectionFlags::Elf { sh_flags }
        if sh_flags & u64::from(object::elf::SHF_ALLOC | object::elf::SHF_EXECINSTR)
            == u64::from(object::elf::SHF_ALLOC | object::elf::SHF_EXECINSTR)
            && sh_flags & u64::from(object::elf::SHF_TLS) == 0)
    {
        return Err(invalid("definition is outside allocated executable code"));
    }
    let address = symbol.address();
    let end = address
        .checked_add(symbol.size())
        .filter(|n| *n <= 1u64 << 32)
        .ok_or_else(|| invalid("definition exceeds RV32"))?;
    let offset = address
        .checked_sub(section.address())
        .ok_or_else(|| invalid("definition precedes its code section"))?;
    let (section_file, section_size) = section
        .file_range()
        .ok_or_else(|| invalid("definition code has no captured bytes"))?;
    if offset
        .checked_add(symbol.size())
        .is_none_or(|n| n > section_size)
        || section_file
            .checked_add(section_size)
            .is_none_or(|n| n > bytes.len() as u64)
    {
        return Err(invalid("definition exceeds its captured code section"));
    }
    let mut matched = false;
    for (index, segment) in file.segments().enumerate() {
        control.checkpoint(1)?;
        if index == 1024 {
            return Err(Error::new(
                ErrorCode::ResourceLimited,
                "definition carrier exceeds 1024 load segments",
            ));
        }
        let start = segment.address();
        let segment_end = start
            .checked_add(segment.size())
            .filter(|n| *n <= 1u64 << 32)
            .ok_or_else(|| invalid("definition carrier segment exceeds RV32"))?;
        let (file_offset, file_size) = segment.file_range();
        if file_size > segment.size()
            || file_offset
                .checked_add(file_size)
                .is_none_or(|n| n > bytes.len() as u64)
        {
            return Err(invalid("definition carrier segment exceeds captured bytes"));
        }
        if start < end
            && address < segment_end
            && segment.size() != 0
            && matches!(segment.flags(), object::SegmentFlags::Elf { p_flags }
                if p_flags & object::elf::PF_X != 0)
        {
            if matched
                || address < start
                || end > start + file_size
                || !matches!(segment.flags(), object::SegmentFlags::Elf { p_flags }
                    if p_flags & object::elf::PF_X != 0 && p_flags & object::elf::PF_W == 0)
                || file_offset + (address - start) != section_file + offset
            {
                return Err(invalid(
                    "definition has ambiguous or non-executable physical mapping",
                ));
            }
            matched = true;
        }
    }
    if !matched {
        return Err(invalid("definition has no executable captured mapping"));
    }
    u32::try_from(address).map_err(|_| invalid("definition address exceeds RV32"))
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Source(Vec<u8>);
    impl ByteSource for Source {
        fn len(&self) -> u64 {
            self.0.len() as u64
        }
        fn read_at(&self, offset: u64, bytes: &mut [u8], c: &mut dyn RunControl) -> Result<()> {
            c.bytes(bytes.len())?;
            bytes.copy_from_slice(
                self.0
                    .get(offset as usize..offset as usize + bytes.len())
                    .ok_or_else(|| invalid("short fixture"))?,
            );
            Ok(())
        }
    }
    fn put(bytes: &mut [u8], at: usize, value: u32) {
        bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }
    fn fixture() -> (Source, SymbolId, usize, usize) {
        use object::write::{Object as Writer, Symbol, SymbolSection};
        let mut file = Writer::new(
            object::BinaryFormat::Elf,
            object::Architecture::Riscv32,
            object::Endianness::Little,
        );
        let text = file.add_section(vec![], b".text.helper".to_vec(), object::SectionKind::Text);
        file.append_section_data(text, &[0x13, 0x05, 0x70, 0, 0x67, 0x80, 0, 0], 4);
        file.add_symbol(Symbol {
            name: b"helper".to_vec(),
            value: 0x1000,
            size: 8,
            kind: object::SymbolKind::Text,
            scope: object::SymbolScope::Linkage,
            weak: false,
            section: SymbolSection::Section(text),
            flags: object::SymbolFlags::None,
        });
        let mut bytes = file.write().unwrap();
        let parsed = object::File::parse(bytes.as_slice()).unwrap();
        let text = parsed.section_by_name(".text.helper").unwrap();
        let index = text.index().0;
        let (code, _) = text.file_range().unwrap();
        let table = parsed.section_by_name(".symtab").unwrap();
        let table_index = table.index().0;
        let (table_offset, _) = table.file_range().unwrap();
        let index_symbol = parsed
            .symbols()
            .find(|s| s.name().ok() == Some("helper"))
            .unwrap()
            .index()
            .0;
        let section_headers = u32::from_le_bytes(bytes[32..36].try_into().unwrap()) as usize;
        put(&mut bytes, section_headers + 40 * index + 12, 0x1000);
        bytes[16..18].copy_from_slice(&2u16.to_le_bytes());
        let program = bytes.len();
        put(&mut bytes, 28, program as u32);
        bytes[42..44].copy_from_slice(&32u16.to_le_bytes());
        bytes[44..46].copy_from_slice(&4u16.to_le_bytes());
        for header in [
            [1, code as u32, 0x1000, 0x1000, 8, 8, 5, 4],
            [7, code as u32, 0x3000, 0x3000, 4, 4, 4, 4],
            [1, code as u32, 0x3000, 0x3000, 4, 4, 6, 4],
            [1, code as u32, 0x3000, 0x3000, 4, 4, 6, 4],
        ] {
            for value in header {
                bytes.extend_from_slice(&value.to_le_bytes());
            }
        }
        let selected = SymbolId {
            object: ObjectId {
                artifact: ArtifactId::of_bytes(&bytes),
                location: ObjectLocation::Standalone,
            },
            table: SymbolTableKind::Static,
            table_section: table_index as u32,
            index: index_symbol as u64,
        };
        (
            Source(bytes),
            selected,
            program,
            table_offset as usize + 16 * index_symbol,
        )
    }
    #[test]
    fn definition_does_not_acquire_tls_or_unrelated_overlapping_memory() {
        let (mut source, mut id, program, _) = fixture();
        let memory = WorkingMemory::new(1024 * 1024).unwrap();
        assert_eq!(
            inspect_link_definition(&source, &id, &memory, &mut || Ok(())).unwrap(),
            0x1000
        );
        let file = object::File::parse(source.0.as_slice()).unwrap();
        assert_eq!(
            crate::program::ProgramView::new(&source.0, &file, &memory, &mut || Ok(()))
                .err()
                .unwrap()
                .code,
            ErrorCode::Incompatible
        );
        // An overlapping data address space does not change the selected code bytes.
        put(&mut source.0, program + 64 + 8, 0x1000);
        id.object.artifact = ArtifactId::of_bytes(&source.0);
        assert_eq!(
            inspect_link_definition(&source, &id, &memory, &mut || Ok(())).unwrap(),
            0x1000
        );
        // Even without TLS, overlapping mappings still cannot execute.
        put(&mut source.0, program + 32, 0);
        id.object.artifact = ArtifactId::of_bytes(&source.0);
        assert_eq!(
            inspect_link_definition(&source, &id, &memory, &mut || Ok(())).unwrap(),
            0x1000
        );
        let file = object::File::parse(source.0.as_slice()).unwrap();
        assert_eq!(
            crate::program::ProgramView::new(&source.0, &file, &memory, &mut || Ok(()))
                .err()
                .unwrap()
                .code,
            ErrorCode::Integrity
        );
        assert_eq!(memory.observation().reserved_bytes, 0);
    }
    #[test]
    fn definition_rejects_forged_identity_symbols_and_physical_ranges() {
        for variant in 0..10 {
            let (mut source, mut id, program, symbol) = fixture();
            match variant {
                0 => id.index = u64::MAX,
                1 => id.table = SymbolTableKind::Dynamic,
                2 => id.table_section = 0,
                3 => put(&mut source.0, symbol + 8, 0),
                4 => source.0[symbol + 12] = 0x11, // STT_OBJECT, not STT_FUNC.
                5 => put(&mut source.0, program + 24, 7), // Writable code.
                6 => put(&mut source.0, program + 16, 4), // Extent exceeds file backing.
                7 => {
                    put(&mut source.0, program + 64 + 8, 0x1004);
                    put(&mut source.0, program + 64 + 24, 5);
                } // Partial competing mapping.
                8 => put(&mut source.0, program + 4, 0), // Different physical bytes.
                _ => source.0[0] ^= 1,
            }
            if variant != 9 {
                id.object.artifact = ArtifactId::of_bytes(&source.0);
            }
            let memory = WorkingMemory::new(1024 * 1024).unwrap();
            let error = inspect_link_definition(&source, &id, &memory, &mut || Ok(())).unwrap_err();
            assert_eq!(
                error.code,
                if variant == 9 {
                    ErrorCode::Integrity
                } else {
                    ErrorCode::InvalidRequest
                },
                "variant {variant}"
            );
            assert_eq!(memory.observation().reserved_bytes, 0);
        }
        let (source, id, _, _) = fixture();
        let memory = WorkingMemory::new(1).unwrap();
        assert_eq!(
            inspect_link_definition(&source, &id, &memory, &mut || Ok(()))
                .unwrap_err()
                .code,
            ErrorCode::ResourceLimited
        );
        let memory = WorkingMemory::new(1024 * 1024).unwrap();
        assert_eq!(
            inspect_link_definition(&source, &id, &memory, &mut || Err(Error::new(
                ErrorCode::Cancelled,
                "cancel"
            )))
            .unwrap_err()
            .code,
            ErrorCode::Cancelled
        );
        assert_eq!(memory.observation().reserved_bytes, 0);
    }
}
