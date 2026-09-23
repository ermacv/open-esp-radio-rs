//! Borrowed ELF function views; no linker or ISA semantics.
use blobray_domain::*;
use object::{Object, ObjectSection, ObjectSymbol};

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
/// Callback borrows admitted bytes and metadata; none may escape its reservation.
pub fn with_function<T>(
    source: &dyn ByteSource,
    payload: &ArtifactId,
    request: &FunctionRequest,
    memory: &WorkingMemory,
    control: &mut dyn RunControl,
    consume: impl FnOnce(FunctionView<'_>, &mut dyn RunControl) -> Result<T>,
) -> Result<T> {
    let size =
        usize::try_from(source.len()).map_err(|_| invalid("object exceeds host address space"))?;
    let mut bytes = memory.bytes(size, control.position())?;
    source.read_at(0, &mut bytes, control)?;
    if ArtifactId::of_bytes_controlled(&bytes, control)? != *payload {
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
    let image = if file.kind() == object::ObjectKind::Executable {
        Some(crate::program::ProgramView::new(
            &bytes, &file, memory, control,
        )?)
    } else {
        None
    };
    let abi =
        crate::image::abi(&file).map_err(|e| Error::new(ErrorCode::Incompatible, e.message))?;
    if request.symbol.table != SymbolTableKind::Static {
        return Err(invalid("function selector requires static symbol table"));
    }
    let table = file
        .section_by_index(object::SectionIndex(request.symbol.table_section as usize))
        .map_err(parse)?;
    if table.name_bytes().map_err(parse)? != b".symtab" {
        return Err(invalid("function selector does not identify .symtab"));
    }
    let mut static_tables = 0;
    for s in file.sections() {
        control.checkpoint(1)?;
        if s.name_bytes().map_err(parse)? == b".symtab" {
            static_tables += 1;
        }
    }
    if static_tables != 1 {
        return Err(invalid("ambiguous static symbol table"));
    }
    let object::File::Elf32(elf) = &file else {
        unreachable!()
    };
    use object::read::elf::SectionHeader;
    if elf
        .elf_section_table()
        .section(table.index())
        .map_err(parse)?
        .sh_type(elf.endian())
        != object::elf::SHT_SYMTAB
    {
        return Err(invalid("selected table is not SHT_SYMTAB"));
    }
    let symbol = file
        .symbol_by_index(object::SymbolIndex(
            usize::try_from(request.symbol.index).map_err(|_| invalid("symbol index overflow"))?,
        ))
        .map_err(parse)?;
    let section_index = symbol
        .section_index()
        .ok_or_else(|| invalid("function symbol has no defined section"))?;
    let section = file.section_by_index(section_index).map_err(parse)?;
    if !matches!(section.flags(),object::SectionFlags::Elf{sh_flags} if sh_flags&u64::from(object::elf::SHF_EXECINSTR)!=0)
    {
        return Err(invalid("function symbol is not in executable code"));
    }
    if request.extent.is_none() && symbol.size() == 0 {
        return Err(Error::new(
            ErrorCode::NeedsExtent,
            "symbol size is unknown; supply an explicit extent starting at the selected symbol",
        ));
    }
    let extent = request.extent.unwrap_or(CodeRange {
        start: symbol.address(),
        length: symbol.size(),
    });
    let end = extent
        .start
        .checked_add(extent.length)
        .ok_or_else(|| invalid("function extent overflow"))?;
    if extent.start != symbol.address()
        || !extent.start.is_multiple_of(2)
        || extent.length == 0
        || extent.start < section.address()
        || section
            .address()
            .checked_add(section.size())
            .is_none_or(|limit| end > limit)
    {
        return Err(invalid(
            "function extent must start at the selected aligned symbol and stay in its section",
        ));
    }
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
    // Bound all retained names and copies before allocation. The complete section's
    // relocations are needed for nonadjacent PCREL HI/LO pairs outside the extent.
    let mut count = 0usize;
    for _ in section.relocations() {
        control.checkpoint(1)?;
        count = count
            .checked_add(1)
            .ok_or_else(|| invalid("relocation count overflow"))?;
    }
    let mut symbols = 0usize;
    for _ in file.symbols() {
        control.checkpoint(1)?;
        symbols = symbols
            .checked_add(1)
            .ok_or_else(|| invalid("symbol count overflow"))?;
    }
    let amount = (count as u64)
        .checked_mul(24576)
        .and_then(|v| v.checked_add((symbols as u64).checked_mul(32)?))
        .ok_or_else(|| invalid("metadata capacity overflow"))?;
    let _metadata = memory.reserve(amount, control.position())?;
    let mut relocations = Vec::new();
    relocations
        .try_reserve_exact(count)
        .map_err(|_| Error::new(ErrorCode::ResourceLimited, "relocation allocation refused"))?;
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
                if hdr.1 != request.symbol.table_section {
                    return Err(invalid("relocation uses another symbol table"));
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
        let symbol_id = SymbolId {
            object: request.symbol.object.clone(),
            table: SymbolTableKind::Static,
            table_section: request.symbol.table_section,
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
            ReferenceTarget {
                symbol: symbol_id,
                name: name.into(),
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
    let mut data_ranges = Vec::new();
    data_ranges.try_reserve_exact(symbols).map_err(|_| {
        Error::new(
            ErrorCode::ResourceLimited,
            "mapping symbol allocation refused",
        )
    })?;
    // Mapping symbols explicitly distinguish embedded data from code. Ranges end
    // at the next mapping symbol; ordinary labels are not extent heuristics.
    let mut mappings = Vec::new();
    mappings.try_reserve_exact(symbols).map_err(|_| {
        Error::new(
            ErrorCode::ResourceLimited,
            "mapping index allocation refused",
        )
    })?;
    for s in file.symbols() {
        control.checkpoint(1)?;
        if s.section_index() != Some(section_index) {
            continue;
        }
        let n = s.name_bytes().map_err(parse)?;
        if n == b"$d" || n.starts_with(b"$d.") {
            mappings.push((s.address(), true));
        } else if n == b"$x" || n.starts_with(b"$x.") {
            mappings.push((s.address(), false));
        }
    }
    mappings.sort_unstable();
    for (i, (start, is_data)) in mappings.iter().enumerate() {
        control.checkpoint(1)?;
        if *is_data {
            let end = mappings
                .get(i + 1)
                .map_or(section.address() + section.size(), |m| m.0);
            if end > *start {
                data_ranges.push(CodeRange {
                    start: *start,
                    length: end - *start,
                });
            }
        }
    }
    consume(
        FunctionView {
            abi,
            address_space: if image.is_some() {
                CodeAddressSpace::Image
            } else {
                CodeAddressSpace::Section
            },
            image: image.as_ref().map(|v| v as &dyn ImageMemory),
            section: section_index.0 as u32,
            extent,
            user_extent: request.extent.is_some(),
            code,
            relocations: &relocations,
            data_ranges: &data_ranges,
        },
        control,
    )
}
