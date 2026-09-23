//! Borrowed RV32 static ELF image. No loading, relocation or environment models.
use blobray_domain::*;
use object::{Object, ObjectSection, ObjectSegment};

pub(crate) struct ProgramView<'a> {
    bytes: &'a [u8],
    segments: Vec<ImageSegment>,
    _capacity: MemoryReservation<'a>,
}
fn invalid(message: &str) -> Error {
    Error::new(ErrorCode::Integrity, message)
}
impl<'a> ProgramView<'a> {
    pub fn new(
        bytes: &'a [u8],
        file: &object::File<'_>,
        memory: &'a WorkingMemory,
        c: &mut dyn RunControl,
    ) -> Result<Self> {
        use object::read::elf::{ProgramHeader, SectionHeader};
        let object::File::Elf32(elf) = file else {
            return Err(invalid("image is not ELF32"));
        };
        for header in elf.elf_program_headers() {
            c.checkpoint(1)?;
            if matches!(
                header.p_type(elf.endian()),
                object::elf::PT_DYNAMIC | object::elf::PT_INTERP | object::elf::PT_TLS
            ) {
                return Err(Error::new(
                    ErrorCode::Incompatible,
                    "dynamic loading and TLS are outside the static image profile",
                ));
            }
        }
        crate::image::abi(file).map_err(|e| Error::new(ErrorCode::Incompatible, e.message))?;
        let capacity = memory.reserve(
            1024 * std::mem::size_of::<ImageSegment>() as u64,
            c.position(),
        )?;
        let mut segments: Vec<ImageSegment> = Vec::new();
        segments.try_reserve_exact(1024).map_err(|_| {
            Error::new(
                ErrorCode::ResourceLimited,
                "image segment allocation refused",
            )
        })?;
        for segment in file.segments() {
            c.checkpoint(1)?;
            if segments.len() == 1024 {
                return Err(Error::new(
                    ErrorCode::ResourceLimited,
                    "image exceeds 1024 load segments",
                ));
            }
            let object::SegmentFlags::Elf { p_flags } = segment.flags() else {
                return Err(invalid("non-ELF load segment"));
            };
            let (offset, size) = segment.file_range();
            let end = segment
                .address()
                .checked_add(segment.size())
                .filter(|v| *v <= 1u64 << 32)
                .ok_or_else(|| invalid("segment exceeds RV32 address space"))?;
            if size > segment.size()
                || offset
                    .checked_add(size)
                    .is_none_or(|v| v > bytes.len() as u64)
                || p_flags & (object::elf::PF_W | object::elf::PF_X)
                    == (object::elf::PF_W | object::elf::PF_X)
            {
                return Err(invalid("invalid segment bounds or writable code"));
            }
            for previous in &segments {
                c.checkpoint(1)?;
                if segment.size() != 0
                    && previous.memory_size != 0
                    && segment.address() < previous.address + previous.memory_size
                    && previous.address < end
                {
                    return Err(invalid("overlapping load segments"));
                }
            }
            segments.push(ImageSegment {
                address: segment.address(),
                file_offset: offset,
                file_size: size,
                memory_size: segment.size(),
                flags: p_flags,
            });
        }
        if segments.is_empty() {
            return Err(invalid("image has no load segments"));
        }
        for section in file.sections() {
            c.checkpoint(1)?;
            let header = elf
                .elf_section_table()
                .section(section.index())
                .map_err(|_| invalid("invalid ELF section index"))?;
            if matches!(
                section.kind(),
                object::SectionKind::Tls | object::SectionKind::UninitializedTls
            ) || header.sh_type(elf.endian()) == object::elf::SHT_DYNAMIC
            {
                return Err(Error::new(
                    ErrorCode::Incompatible,
                    "dynamic loading and TLS are outside the static image profile",
                ));
            }
            if header.sh_flags(elf.endian()) & object::elf::SHF_ALLOC != 0
                && matches!(
                    header.sh_type(elf.endian()),
                    object::elf::SHT_REL | object::elf::SHT_RELA
                )
            {
                return Err(Error::new(
                    ErrorCode::Incompatible,
                    "runtime relocation sections are outside the static image profile",
                ));
            }
        }
        Ok(Self {
            bytes,
            segments,
            _capacity: capacity,
        })
    }
    /// Verify that a section's captured bytes are the file-backed bytes loaded at its VMA.
    pub fn data_range(
        &self,
        address: u64,
        file_offset: u64,
        length: u64,
        c: &mut dyn RunControl,
    ) -> Result<bool> {
        let mut consumed = 0;
        let mut writable = false;
        while consumed < length {
            c.checkpoint(1)?;
            let at = address
                .checked_add(consumed)
                .ok_or_else(|| invalid("data address overflow"))?;
            let mut found = None;
            for segment in &self.segments {
                c.checkpoint(1)?;
                if at >= segment.address && at - segment.address < segment.file_size {
                    found = Some(segment);
                    break;
                }
            }
            let segment =
                found.ok_or_else(|| invalid("data range has no file-backed load mapping"))?;
            writable |= segment.flags & object::elf::PF_W != 0;
            let offset = at - segment.address;
            if segment.file_offset.checked_add(offset) != file_offset.checked_add(consumed) {
                return Err(invalid("section data differs from loaded segment mapping"));
            }
            consumed += (segment.file_size - offset).min(length - consumed);
        }
        Ok(writable)
    }
    pub fn code(&self, address: u64, code: &[u8], c: &mut dyn RunControl) -> Result<()> {
        for segment in &self.segments {
            c.checkpoint(1)?;
            if segment.flags & object::elf::PF_X != 0 && address >= segment.address {
                let offset = address - segment.address;
                if offset
                    .checked_add(code.len() as u64)
                    .is_some_and(|end| end <= segment.file_size)
                {
                    c.checkpoint(code.len() as u64)?;
                    let start = (segment.file_offset + offset) as usize;
                    if self.bytes[start..start + code.len()] == *code {
                        return Ok(());
                    }
                    return Err(invalid("section code differs from loaded segment bytes"));
                }
            }
        }
        Err(invalid("function has no executable file-backed load range"))
    }
}
impl ImageMemory for ProgramView<'_> {
    fn read_constant(
        &self,
        address: u32,
        bytes: &mut [u8],
        c: &mut dyn RunControl,
    ) -> Result<bool> {
        let address = u64::from(address);
        for segment in &self.segments {
            c.checkpoint(1)?;
            if segment.flags & (object::elf::PF_R | object::elf::PF_W) != object::elf::PF_R
                || address < segment.address
            {
                continue;
            }
            let offset = address - segment.address;
            if offset
                .checked_add(bytes.len() as u64)
                .is_some_and(|end| end <= segment.file_size)
            {
                let start = (segment.file_offset + offset) as usize;
                let length = bytes.len();
                for (to, from) in bytes
                    .chunks_mut(4096)
                    .zip(self.bytes[start..start + length].chunks(4096))
                {
                    c.checkpoint(1)?;
                    to.copy_from_slice(from);
                }
                return Ok(true);
            }
        }
        Ok(false)
    }
}

/// Lend validated static RV32 segments to an admitted session loader. No relocation.
pub fn execution_segments(
    source: &dyn ByteSource,
    memory: &WorkingMemory,
    control: &mut dyn RunControl,
    consume: &mut impl FnMut(&ImageSegment, &[u8], &mut dyn RunControl) -> Result<()>,
) -> Result<()> {
    let mut bytes = memory.bytes(
        usize::try_from(source.len()).map_err(|_| invalid("ELF size overflow"))?,
        control.position(),
    )?;
    source.read_at(0, &mut bytes, control)?;
    let file = object::File::parse(&*bytes).map_err(|_| invalid("invalid executable ELF"))?;
    if file.kind() != object::ObjectKind::Executable
        || file.architecture() != object::Architecture::Riscv32
        || !file.is_little_endian()
        || file.is_64()
    {
        return Err(invalid("execution requires static little-endian RV32 ELF"));
    }
    let view = ProgramView::new(&bytes, &file, memory, control)?;
    for segment in &view.segments {
        control.checkpoint(1)?;
        if segment.memory_size != 0 {
            consume(
                segment,
                &bytes[segment.file_offset as usize
                    ..(segment.file_offset + segment.file_size) as usize],
                control,
            )?;
        }
    }
    Ok(())
}

/// Visit every executable section, including code without function symbols.
/// Section-less files fail rather than reporting a vacuous clean result.
pub fn executable_sections(
    bytes: &[u8],
    memory: &WorkingMemory,
    control: &mut dyn RunControl,
    consume: &mut impl FnMut(ExecutableSectionView<'_>, &mut dyn RunControl) -> Result<()>,
) -> Result<()> {
    let file = object::File::parse(bytes).map_err(|_| invalid("invalid executable ELF"))?;
    if file.kind() != object::ObjectKind::Executable
        || file.architecture() != object::Architecture::Riscv32
        || !file.is_little_endian()
        || file.is_64()
    {
        return Err(invalid("audit requires static little-endian RV32 ELF"));
    }
    let _view = ProgramView::new(bytes, &file, memory, control)?;
    let mut found = false;
    for section in file.sections() {
        control.checkpoint(1)?;
        if !matches!(section.flags(), object::SectionFlags::Elf { sh_flags } if sh_flags & u64::from(object::elf::SHF_EXECINSTR) != 0)
        {
            continue;
        }
        let data = section
            .data()
            .map_err(|_| invalid("invalid executable section bytes"))?;
        if data.is_empty() {
            continue;
        }
        let address = u32::try_from(section.address())
            .map_err(|_| invalid("section address exceeds RV32"))?;
        if section
            .address()
            .checked_add(data.len() as u64)
            .is_none_or(|v| v > 1u64 << 32)
            || address & 1 != 0
        {
            return Err(invalid("invalid executable section extent"));
        }
        found = true;
        _view.code(u64::from(address), data, control)?;
        let mappings = crate::mapping::data_ranges(&file, section.index(), memory, control)?;
        consume(
            ExecutableSectionView {
                section: section.index().0 as u32,
                address,
                bytes: data,
                data_ranges: &mappings.ranges,
            },
            control,
        )?;
    }
    if !found {
        return Err(invalid("no executable sections: target audit unavailable"));
    }
    Ok(())
}
