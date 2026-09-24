//! Borrowed ELF function views; no linker or ISA semantics.
use blobray_domain::*;
use object::{Object, ObjectSection, ObjectSymbol, ObjectSymbolTable};

pub struct FunctionView<'a> {
    pub abi: RiscvAbi,
    pub address_space: CodeAddressSpace,
    pub image: Option<&'a dyn ImageMemory>,
    pub section: u32,
    pub extent: CodeRange,
    pub user_extent: bool,
    pub code: &'a [u8],
    pub relocations: &'a [FunctionRelocation],
    pub data_ranges: &'a [CodeRange],
}
fn invalid(message: impl Into<String>) -> Error {
    Error::new(ErrorCode::InvalidRequest, message)
}
fn parse(error: object::Error) -> Error {
    Error::new(ErrorCode::Integrity, error.to_string())
}
pub struct PreparedObject<'a, 'm> {
    file: object::File<'a>,
    occurrence: Option<ObjectId>,
    image: Option<crate::program::ProgramView<'a>>,
    abi: RiscvAbi,
    memory: &'m WorkingMemory,
    targets: Vec<Option<std::sync::Arc<ReferenceTarget>>>,
    target_capacity: Vec<MemoryReservation<'m>>,
    sections: Vec<Option<PreparedSection<'m>>>,
    _capacity: MemoryReservation<'m>,
}
struct PreparedSection<'a> {
    relocations: Vec<FunctionRelocation>,
    mappings: crate::mapping::DataRanges<'a>,
    _capacity: MemoryReservation<'a>,
}
fn reserved<T>(count: usize) -> Result<Vec<T>> {
    let mut v = Vec::new();
    v.try_reserve_exact(count).map_err(|_| {
        Error::new(
            ErrorCode::ResourceLimited,
            "object metadata allocation refused",
        )
    })?;
    Ok(v)
}
/// Own captured bytes in this callback scope; parsed views cannot escape it.
pub fn with_prepared_object<T>(
    source: &dyn ByteSource,
    payload: &ArtifactId,
    memory: &WorkingMemory,
    control: &mut dyn RunControl,
    consume: impl FnOnce(&mut PreparedObject<'_, '_>, &mut dyn RunControl) -> Result<T>,
) -> Result<T> {
    control.phase(RunPhase::PrepareObject)?;
    let size =
        usize::try_from(source.len()).map_err(|_| invalid("object exceeds host address space"))?;
    let mut bytes = memory.bytes(size, control.position())?;
    source.read_at(0, &mut bytes, control)?;
    control.measure(WorkMetric::ObjectReadBytes, size as u64);
    let actual = ArtifactId::of_bytes_controlled(&bytes, control)?;
    control.measure(WorkMetric::ObjectHashBytes, size as u64);
    if actual != *payload {
        return Err(Error::new(
            ErrorCode::Integrity,
            "function payload differs from captured inventory",
        ));
    }
    let file = object::File::parse(&*bytes).map_err(parse)?;
    if file.is_64()
        || !file.is_little_endian()
        || file.architecture() != object::Architecture::Riscv32
        || !matches!(
            file.kind(),
            object::ObjectKind::Relocatable | object::ObjectKind::Executable
        )
    {
        return Err(Error::new(
            ErrorCode::Incompatible,
            "function analysis requires little-endian RV32 ET_REL or ET_EXEC",
        ));
    }
    let object::File::Elf32(elf) = &file else {
        unreachable!()
    };
    use object::read::elf::SectionHeader;
    let mut static_tables = 0;
    let mut dynamic_tables = 0;
    for header in elf.elf_section_table().iter() {
        control.checkpoint(1)?;
        match header.sh_type(elf.endian()) {
            object::elf::SHT_SYMTAB => static_tables += 1,
            object::elf::SHT_DYNSYM => dynamic_tables += 1,
            _ => {}
        }
    }
    if static_tables > 1 || dynamic_tables > 1 {
        return Err(invalid("ambiguous physical symbol tables"));
    }
    let image = if file.kind() == object::ObjectKind::Executable {
        Some(crate::program::ProgramView::new(
            &bytes, &file, memory, control,
        )?)
    } else {
        None
    };
    let abi =
        crate::image::abi(&file).map_err(|e| Error::new(ErrorCode::Incompatible, e.message))?;
    let object::File::Elf32(elf) = &file else {
        unreachable!()
    };
    let symbols = elf.elf_symbol_table().symbols().len();
    let section_count = elf.elf_section_table().len();
    let _capacity = memory.reserve(
        (symbols as u64)
            .checked_mul(
                (std::mem::size_of::<Option<std::sync::Arc<ReferenceTarget>>>()
                    + 2 * std::mem::size_of::<MemoryReservation<'_>>()) as u64,
            )
            .and_then(|n| {
                n.checked_add(
                    (section_count as u64)
                        .checked_mul(std::mem::size_of::<Option<PreparedSection<'_>>>() as u64)?,
                )
            })
            .ok_or_else(|| invalid("object metadata overflow"))?,
        control.position(),
    )?;
    let mut targets = reserved(symbols)?;
    targets.resize_with(symbols, || None);
    let mut sections = reserved(section_count)?;
    sections.resize_with(section_count, || None);
    let target_capacity = reserved(
        symbols
            .checked_mul(2)
            .ok_or_else(|| invalid("symbol capacity overflow"))?,
    )?;
    control.measure(WorkMetric::ObjectsPrepared, 1);
    let mut object = PreparedObject {
        file,
        occurrence: None,
        image,
        abi,
        memory,
        targets,
        target_capacity,
        sections,
        _capacity,
    };
    consume(&mut object, control)
}
/// Single-function adapter over the same prepared object owner.
pub fn with_function<T>(
    source: &dyn ByteSource,
    payload: &ArtifactId,
    request: &FunctionRequest,
    memory: &WorkingMemory,
    control: &mut dyn RunControl,
    consume: impl FnOnce(FunctionView<'_>, &mut dyn RunControl) -> Result<T>,
) -> Result<T> {
    with_prepared_object(source, payload, memory, control, |object, c| {
        object.with_function(request, c, consume)
    })
}
impl<'data> PreparedObject<'data, '_> {
    /// Resolve a physical FUNC/NOTYPE symbol at an executable ET_EXEC boundary.
    /// Zero-sized symbols are allowed: this identifies an address, not an extent
    /// or a claim that the following instruction/body is supported.
    pub fn code_symbol_address(
        &self,
        occurrence: &ObjectId,
        id: &SymbolId,
        control: &mut dyn RunControl,
    ) -> Result<u32> {
        if self.file.kind() != object::ObjectKind::Executable {
            return Err(invalid("execution boundary requires a linked executable"));
        }
        let symbol = self.selected_symbol(occurrence, id)?;
        if !matches!(symbol.flags(), object::SymbolFlags::Elf { st_info, .. } if matches!(st_info & 15, 0 | 2))
        {
            return Err(invalid("execution boundary is not a code symbol"));
        }
        let section = self
            .file
            .section_by_index(
                symbol
                    .section_index()
                    .ok_or_else(|| invalid("execution boundary is undefined/absolute"))?,
            )
            .map_err(parse)?;
        let address = symbol.address();
        if !matches!(section.flags(), object::SectionFlags::Elf { sh_flags } if sh_flags & 4 != 0)
            || section.file_range().is_none()
            || address < section.address()
            || address
                .checked_sub(section.address())
                .is_none_or(|offset| offset >= section.size())
            || address & 1 != 0
            || address >= u64::from(u32::MAX - 1)
        {
            return Err(invalid("execution symbol is outside executable bytes"));
        }
        let start = (address - section.address()) as usize;
        let data = section.data().map_err(parse)?;
        let prefix = data
            .get(start..start + 2)
            .ok_or_else(|| invalid("execution boundary has no captured halfword"))?;
        self.image
            .as_ref()
            .ok_or_else(|| invalid("execution boundary has no static mappings"))?
            .code(address, prefix, control)?;
        Ok(address as u32)
    }
    /// Validate physical table kind, section and index without a name lookup or
    /// interpreting one table's index in another table.
    fn selected_symbol(
        &self,
        occurrence: &ObjectId,
        symbol: &SymbolId,
    ) -> Result<object::Symbol<'data, '_>> {
        let object::File::Elf32(elf) = &self.file else {
            unreachable!()
        };
        let (physical, table) = match symbol.table {
            SymbolTableKind::Static => (elf.elf_symbol_table().section(), self.file.symbol_table()),
            SymbolTableKind::Dynamic => (
                elf.elf_dynamic_symbol_table().section(),
                self.file.dynamic_symbol_table(),
            ),
        };
        if symbol.object != *occurrence
            || physical.0 == 0
            || symbol.table_section as usize != physical.0
        {
            return Err(invalid(
                "symbol belongs to another object or physical table",
            ));
        }
        table
            .ok_or_else(|| invalid("missing selected symbol table"))?
            .symbol_by_index(object::SymbolIndex(
                usize::try_from(symbol.index).map_err(|_| invalid("symbol index overflow"))?,
            ))
            .map_err(parse)
    }

    fn prepare_section(
        &mut self,
        section_index: object::SectionIndex,
        occurrence: &ObjectId,
        control: &mut dyn RunControl,
    ) -> Result<()> {
        let file = &self.file;
        let memory = self.memory;
        let section = file.section_by_index(section_index).map_err(parse)?;
        let object::File::Elf32(elf) = file else {
            unreachable!()
        };
        let table_section = elf.elf_symbol_table().section().0 as u32;
        if self.sections[section_index.0].is_none() {
            control.phase(RunPhase::PrepareSection)?;
            let mut count = 0usize;
            for _ in section.relocations() {
                control.checkpoint(1)?;
                count = count
                    .checked_add(1)
                    .ok_or_else(|| invalid("relocation count overflow"))?;
            }
            let amount = (count as u64)
                .checked_mul(std::mem::size_of::<FunctionRelocation>() as u64)
                .ok_or_else(|| invalid("metadata capacity overflow"))?;
            let _metadata = memory.reserve(amount, control.position())?;
            let mut relocations = Vec::new();
            relocations.try_reserve_exact(count).map_err(|_| {
                Error::new(ErrorCode::ResourceLimited, "relocation allocation refused")
            })?;
            // Preserve the physical relocation section and record index, not a virtual address.
            let mut relocation_section = None;
            for s in file.sections() {
                control.checkpoint(1)?;
                if let object::SectionFlags::Elf { .. } = s.flags() {
                    // Raw ELF32 sh_info/sh_link preserve table identity without ISA interpretation.
                    let object::File::Elf32(elf) = &file else {
                        unreachable!()
                    };
                    use object::read::elf::SectionHeader;
                    let header = elf.elf_section_table().section(s.index()).map_err(parse)?;
                    let hdr = (
                        header.sh_type(elf.endian()),
                        header.sh_link(elf.endian()),
                        header.sh_info(elf.endian()),
                    );
                    if matches!(hdr.0, object::elf::SHT_RELA | object::elf::SHT_REL)
                        && hdr.2 == section_index.0 as u32
                    {
                        if hdr.1 != table_section {
                            return Err(Error::new(
                                ErrorCode::Incompatible,
                                "relocations using a non-static symbol table require a dynamic relocation profile",
                            ));
                        }
                        if relocation_section
                            .replace((s.index().0 as u32, hdr.0))
                            .is_some()
                        {
                            return Err(Error::new(
                                ErrorCode::Incompatible,
                                "multiple relocation sections for code section",
                            ));
                        }
                    }
                }
            }
            for (index, (offset, relocation)) in section.relocations().enumerate() {
                control.checkpoint(1)?;
                // ELF relocations with r_sym=0 (notably RELAX/ALIGN) are exposed
                // by object as Absolute. Keep the physical null-symbol index.
                let target = match relocation.target() {
                    object::RelocationTarget::Symbol(index) => index,
                    object::RelocationTarget::Absolute => object::SymbolIndex(0),
                    _ => {
                        return Err(Error::new(
                            ErrorCode::Incompatible,
                            "non-symbol ELF relocation target",
                        ));
                    }
                };
                if target.0 >= self.targets.len() {
                    return Err(invalid("relocation target outside symbol table"));
                }
                let target_info = if let Some(existing) = &self.targets[target.0] {
                    existing.clone()
                } else {
                    let charge = memory.reserve(
                        (std::mem::size_of::<ReferenceTarget>()
                            + 2 * std::mem::size_of::<usize>()
                            + 64) as u64,
                        control.position(),
                    )?;
                    let symbol_id = SymbolId {
                        object: occurrence.clone(),
                        table: SymbolTableKind::Static,
                        table_section,
                        index: target.0 as u64,
                    };
                    let target_info = if target.0 == 0 {
                        use object::read::elf::Sym;
                        let zero = elf
                            .elf_symbol_table()
                            .symbols()
                            .first()
                            .ok_or_else(|| invalid("missing ELF null symbol"))?;
                        if zero.st_name(elf.endian()) != 0
                            || zero.st_value(elf.endian()) != 0
                            || zero.st_size(elf.endian()) != 0
                            || zero.st_info() != 0
                            || zero.st_other() != 0
                            || zero.st_shndx(elf.endian()) != 0
                        {
                            return Err(invalid("invalid ELF null symbol"));
                        }
                        ReferenceTarget {
                            symbol: symbol_id,
                            name: Vec::new(),
                            section: None,
                            offset: 0,
                            binding: 0,
                            definition: SymbolDefinition::Null,
                            symbol_type: 0,
                        }
                    } else {
                        let symbol = file.symbol_by_index(target).map_err(parse)?;
                        let name = symbol.name_bytes().map_err(parse)?;
                        if name.len() > 4096 {
                            return Err(Error::new(
                                ErrorCode::ResourceLimited,
                                "reference name exceeds 4096 bytes",
                            ));
                        }
                        let charge = memory.reserve(name.len() as u64, control.position())?;
                        self.target_capacity.push(charge);
                        let mut owned_name = reserved(name.len())?;
                        owned_name.extend_from_slice(name);
                        ReferenceTarget {
                            symbol: symbol_id,
                            name: owned_name,
                            section: symbol.section_index().map(|s| s.0 as u32),
                            offset: symbol.address(),
                            binding: match symbol.flags() {
                                object::SymbolFlags::Elf { st_info, .. } => st_info >> 4,
                                _ => 0,
                            },
                            definition: match symbol.section() {
                                object::SymbolSection::Undefined => SymbolDefinition::Undefined,
                                object::SymbolSection::Common => SymbolDefinition::Common,
                                object::SymbolSection::Absolute => SymbolDefinition::Absolute,
                                object::SymbolSection::Section(_) => SymbolDefinition::Section,
                                _ => SymbolDefinition::Other,
                            },
                            symbol_type: match symbol.flags() {
                                object::SymbolFlags::Elf { st_info, .. } => st_info & 15,
                                _ => 0,
                            },
                        }
                    };
                    let shared = std::sync::Arc::new(target_info);
                    self.targets[target.0] = Some(shared.clone());
                    self.target_capacity.push(charge);
                    shared
                };
                let object::RelocationFlags::Elf { r_type } = relocation.flags() else {
                    return Err(invalid("non-ELF relocation"));
                };
                let (rel_section, kind) =
                    relocation_section.ok_or_else(|| invalid("relocation section missing"))?;
                relocations.push(FunctionRelocation {
                    section: rel_section,
                    index: index as u64,
                    offset,
                    relocation_type: r_type,
                    addend: if kind == object::elf::SHT_RELA {
                        Some(relocation.addend())
                    } else {
                        None
                    },
                    target: target_info,
                });
            }
            control.checkpoint(
                (count as u64).saturating_mul(u64::from(usize::BITS - count.leading_zeros())),
            )?;
            relocations.sort_unstable_by_key(|r| (r.offset, r.section, r.index));
            let mappings = crate::mapping::data_ranges(file, section_index, memory, control)?;
            self.sections[section_index.0] = Some(PreparedSection {
                relocations,
                mappings,
                _capacity: _metadata,
            });
            control.measure(WorkMetric::SectionsPrepared, 1);
        }
        Ok(())
    }

    pub fn with_function<T>(
        &mut self,
        request: &FunctionRequest,
        control: &mut dyn RunControl,
        consume: impl FnOnce(FunctionView<'_>, &mut dyn RunControl) -> Result<T>,
    ) -> Result<T> {
        let occurrence = request.selector.object();
        if self.occurrence.as_ref().is_some_and(|id| id != occurrence) {
            return Err(invalid("prepared object belongs to another occurrence"));
        }
        self.occurrence = Some(occurrence.clone());
        let file = &self.file;
        let (section_index, extent, required_start, user_extent) = match &request.selector {
            FunctionSelector::Symbol { symbol } => {
                let symbol = self.selected_symbol(occurrence, symbol)?;
                let section = symbol
                    .section_index()
                    .ok_or_else(|| invalid("function symbol has no defined section"))?;
                if request.extent.is_none() && symbol.size() == 0 {
                    return Err(Error::new(
                        ErrorCode::NeedsExtent,
                        "symbol size is unknown; supply an explicit extent starting at the selected symbol",
                    ));
                }
                (
                    section,
                    request.extent.unwrap_or(CodeRange {
                        start: symbol.address(),
                        length: symbol.size(),
                    }),
                    symbol.address(),
                    request.extent.is_some(),
                )
            }
            FunctionSelector::Range {
                section, extent, ..
            } => {
                if request.extent.is_some() {
                    return Err(invalid(
                        "range selection cannot have a symbol extent override",
                    ));
                }
                (
                    object::SectionIndex(*section as usize),
                    *extent,
                    extent.start,
                    true,
                )
            }
        };
        let section = file.section_by_index(section_index).map_err(parse)?;
        if !matches!(section.flags(),object::SectionFlags::Elf{sh_flags} if sh_flags&u64::from(object::elf::SHF_EXECINSTR)!=0)
        {
            return Err(invalid("function selection is not in executable code"));
        }
        let end = extent
            .start
            .checked_add(extent.length)
            .ok_or_else(|| invalid("function extent overflow"))?;
        if extent.start != required_start
            || !extent.start.is_multiple_of(2)
            || extent.length == 0
            || extent.start < section.address()
            || section
                .address()
                .checked_add(section.size())
                .is_none_or(|limit| end > limit)
        {
            return Err(invalid(
                "function extent must start at the selected aligned address and stay in its section",
            ));
        }
        self.prepare_section(section_index, occurrence, control)?;
        let section = self.file.section_by_index(section_index).map_err(parse)?;
        let image = &self.image;
        let data = section.data().map_err(parse)?;
        let code = data
            .get(
                usize::try_from(extent.start - section.address())
                    .map_err(|_| invalid("extent overflow"))?
                    ..usize::try_from(end - section.address())
                        .map_err(|_| invalid("extent overflow"))?,
            )
            .ok_or_else(|| invalid("function has no file-backed bytes"))?;
        if let Some(image) = &image {
            image.code(extent.start, code, control)?;
        }
        let prepared = self.sections[section_index.0].as_ref().unwrap();
        control.phase(RunPhase::AnalyzeFunction)?;
        consume(
            FunctionView {
                abi: self.abi,
                address_space: if image.is_some() {
                    CodeAddressSpace::Image
                } else {
                    CodeAddressSpace::Section
                },
                image: image.as_ref().map(|v| v as &dyn ImageMemory),
                section: section_index.0 as u32,
                extent,
                user_extent,
                code,
                relocations: &prepared.relocations,
                data_ranges: &prepared.mappings.ranges,
            },
            control,
        )
    }
}

#[path = "data.rs"]
mod data;
pub use data::DataView;
