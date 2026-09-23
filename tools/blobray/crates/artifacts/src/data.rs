//! Borrowed data slices from the same prepared ELF owner as function views.
use super::*;
pub struct DataView<'a> {
    pub span: DataSpan,
    pub bytes: &'a [u8],
    pub relocations: &'a [FunctionRelocation],
}
impl PreparedObject<'_, '_> {
    /// Exact verified object bytes, borrowed until this prepared owner is released.
    pub fn captured_bytes(&self) -> &[u8] {
        let object::File::Elf32(elf) = &self.file else {
            unreachable!()
        };
        elf.data()
    }
    pub fn validate_data_symbol(&self, occurrence: &ObjectId, symbol: &SymbolId) -> Result<()> {
        use object::read::elf::SectionHeader;
        let object::File::Elf32(elf) = &self.file else {
            unreachable!()
        };
        let table = self
            .file
            .section_by_name(".symtab")
            .ok_or_else(|| invalid("missing symbol table"))?
            .index();
        if symbol.object != *occurrence
            || symbol.table != SymbolTableKind::Static
            || symbol.table_section != table.0 as u32
            || elf
                .elf_section_table()
                .section(table)
                .map_err(parse)?
                .sh_type(elf.endian())
                != object::elf::SHT_SYMTAB
        {
            return Err(invalid("data symbol belongs to another object or table"));
        }
        self.file
            .symbol_by_index(object::SymbolIndex(
                usize::try_from(symbol.index).map_err(|_| invalid("symbol index overflow"))?,
            ))
            .map_err(parse)?;
        Ok(())
    }

    pub fn with_data<T>(
        &mut self,
        occurrence: &ObjectId,
        selector: &DataSelector,
        control: &mut dyn RunControl,
        consume: impl FnOnce(DataView<'_>, &mut dyn RunControl) -> Result<T>,
    ) -> Result<T> {
        if self.occurrence.as_ref().is_some_and(|id| id != occurrence) {
            return Err(invalid("prepared data belongs to another occurrence"));
        }
        self.occurrence = Some(occurrence.clone());
        let table = self
            .file
            .section_by_name(".symtab")
            .ok_or_else(|| invalid("missing symbol table"))?
            .index()
            .0 as u32;
        let (index, offset, length) = match selector {
            DataSelector::Section {
                section,
                offset,
                length,
            } => (object::SectionIndex(*section as usize), *offset, *length),
            DataSelector::Symbol { symbol, length } => {
                self.validate_data_symbol(occurrence, symbol)?;
                if symbol.object != *occurrence
                    || symbol.table != SymbolTableKind::Static
                    || symbol.table_section != table
                {
                    return Err(invalid("data symbol belongs to another object or table"));
                }
                let symbol = self
                    .file
                    .symbol_by_index(object::SymbolIndex(
                        usize::try_from(symbol.index)
                            .map_err(|_| invalid("symbol index overflow"))?,
                    ))
                    .map_err(parse)?;
                let index = symbol
                    .section_index()
                    .ok_or_else(|| invalid("data symbol has no defined section"))?;
                let section = self.file.section_by_index(index).map_err(parse)?;
                let offset = symbol
                    .address()
                    .checked_sub(section.address())
                    .ok_or_else(|| invalid("symbol precedes section"))?;
                let length = length.unwrap_or(symbol.size());
                if symbol.size() != 0 && length > symbol.size() {
                    return Err(invalid("requested bytes exceed declared symbol size"));
                }
                (index, offset, length)
            }
            DataSelector::Image { address, length } => {
                if self.file.kind() != object::ObjectKind::Executable {
                    return Err(invalid("image addresses require executable ELF"));
                }
                let end = address
                    .checked_add(*length)
                    .ok_or_else(|| invalid("image range overflow"))?;
                let mut found = None;
                for s in self.file.sections() {
                    control.checkpoint(1)?;
                    if matches!(s.flags(), object::SectionFlags::Elf { sh_flags } if sh_flags & u64::from(object::elf::SHF_ALLOC) != 0)
                        && *address >= s.address()
                        && s.address().checked_add(s.size()).is_some_and(|n| end <= n)
                    {
                        if found.is_some() {
                            return Err(invalid("ambiguous image data range"));
                        }
                        found = Some((s.index(), address - s.address(), *length));
                    }
                }
                found.ok_or_else(|| invalid("image range has no exact section"))?
            }
        };
        if length == 0 {
            return Err(Error::new(
                ErrorCode::NeedsExtent,
                "data requires a nonempty explicit range or sized symbol",
            ));
        }
        let section = self.file.section_by_index(index).map_err(parse)?;
        if matches!(section.flags(), object::SectionFlags::Elf { sh_flags } if sh_flags & u64::from(object::elf::SHF_COMPRESSED) != 0)
        {
            return Err(Error::new(
                ErrorCode::Incompatible,
                "compressed data sections require an explicit decompression profile",
            ));
        }
        let end = offset
            .checked_add(length)
            .filter(|end| *end <= section.size())
            .ok_or_else(|| invalid("data range exceeds section"))?;
        let (file_offset, file_length) = section
            .file_range()
            .ok_or_else(|| invalid("data has no captured file bytes (NOBITS)"))?;
        if end > file_length {
            return Err(invalid("data range exceeds file backing"));
        }
        self.prepare_section(index, occurrence, table, control)?;
        let section = self.file.section_by_index(index).map_err(parse)?;
        let data = section.data().map_err(parse)?;
        let bytes = data
            .get(
                usize::try_from(offset).map_err(|_| invalid("data offset overflow"))?
                    ..usize::try_from(end).map_err(|_| invalid("data end overflow"))?,
            )
            .ok_or_else(|| invalid("missing data bytes"))?;
        let file_start = file_offset
            .checked_add(offset)
            .ok_or_else(|| invalid("file range overflow"))?;
        let prepared = self.sections[index.0].as_ref().unwrap();
        let allocated = matches!(section.flags(), object::SectionFlags::Elf { sh_flags } if sh_flags & u64::from(object::elf::SHF_ALLOC) != 0);
        let mut writable = matches!(section.flags(), object::SectionFlags::Elf { sh_flags } if sh_flags & u64::from(object::elf::SHF_WRITE) != 0);
        if let Some(image) = &self.image
            && allocated
        {
            writable |= image.data_range(
                section
                    .address()
                    .checked_add(offset)
                    .ok_or_else(|| invalid("data address overflow"))?,
                file_start,
                length,
                control,
            )?;
        }
        let span = DataSpan {
            selector: selector.clone(),
            section: index.0 as u32,
            section_range: CodeRange {
                start: offset,
                length,
            },
            file_range: CodeRange {
                start: file_start,
                length,
            },
            image_address: if self.file.kind() == object::ObjectKind::Executable && allocated {
                Some(
                    section
                        .address()
                        .checked_add(offset)
                        .ok_or_else(|| invalid("image address overflow"))?,
                )
            } else {
                None
            },
            writable,
            digest: ArtifactId::of_bytes_controlled(bytes, control)?,
            export_offset: 0,
            section_relocations: prepared.relocations.len() as u64,
        };
        consume(
            DataView {
                span,
                bytes,
                relocations: &prepared.relocations,
            },
            control,
        )
    }
}
