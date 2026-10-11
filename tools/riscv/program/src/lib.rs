//! Borrowed RV32 static ELF image. No loading, relocation or environment models.
pub mod mapping;
use oer_elf::{ProgramHeaderKind, SectionKind, rv32};
use oer_riscv_model::*;

/// `PF_X`, `PF_W` and `PF_R` of an [`ImageSegment`]'s raw ELF permissions.
const PF_X: u32 = 1;
const PF_W: u32 = 2;
const PF_R: u32 = 4;

pub struct ProgramView<'a> {
    bytes: &'a [u8],
    segments: Vec<ImageSegment>,
    _capacity: MemoryReservation<'a>,
}
fn bad(message: impl Into<String>) -> Error {
    Error::new(ErrorCode::LinkBlocked, message)
}
/// The ELF-declared floating-point calling convention of an RV32 image.
pub fn abi(file: &oer_elf::Elf<'_>) -> Result<RiscvAbi> {
    let flags = rv32::header_flags(file.header_flags());
    if flags.rve {
        return Err(bad("RV32E is outside the RV32 integer-register profile"));
    }
    match flags.float_abi {
        rv32::FloatAbi::Soft => Ok(RiscvAbi::Ilp32),
        rv32::FloatAbi::Single => Ok(RiscvAbi::Ilp32f),
        rv32::FloatAbi::Double => Ok(RiscvAbi::Ilp32d),
        rv32::FloatAbi::Quad => Err(bad("unsupported ELF floating-point ABI")),
    }
}
fn invalid(message: &str) -> Error {
    Error::new(ErrorCode::Integrity, message)
}
impl<'a> ProgramView<'a> {
    pub fn executable_bytes(
        &self,
        address: u32,
        length: u8,
        c: &mut dyn RunControl,
    ) -> Result<&'a [u8]> {
        if !matches!(length, 2 | 4) || address & 1 != 0 {
            return Err(invalid("invalid instruction prefix"));
        }
        for segment in &self.segments {
            c.checkpoint(1)?;
            if segment.flags & PF_X != 0
                && u64::from(address) >= segment.address
                && u64::from(address) + u64::from(length) <= segment.address + segment.file_size
            {
                let start = (segment.file_offset + u64::from(address) - segment.address) as usize;
                return self
                    .bytes
                    .get(start..start + usize::from(length))
                    .ok_or_else(|| invalid("instruction outside captured image"));
            }
        }
        Err(invalid("instruction outside executable captured mapping"))
    }

    pub fn new(
        bytes: &'a [u8],
        file: &oer_elf::Elf<'_>,
        memory: &'a WorkingMemory,
        c: &mut dyn RunControl,
    ) -> Result<Self> {
        let headers = file.program_headers();
        for header in &headers {
            c.checkpoint(1)?;
            if matches!(
                header.kind,
                ProgramHeaderKind::Dynamic
                    | ProgramHeaderKind::Interpreter
                    | ProgramHeaderKind::Tls
            ) {
                return Err(Error::new(
                    ErrorCode::Incompatible,
                    "dynamic loading and TLS are outside the static image profile",
                ));
            }
        }
        abi(file).map_err(|e| Error::new(ErrorCode::Incompatible, e.message))?;
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
        for segment in headers
            .iter()
            .filter(|header| header.kind == ProgramHeaderKind::Load)
        {
            c.checkpoint(1)?;
            if segments.len() == 1024 {
                return Err(Error::new(
                    ErrorCode::ResourceLimited,
                    "image exceeds 1024 load segments",
                ));
            }
            let (offset, size) = (segment.file_offset, segment.file_size);
            let end = segment
                .address
                .checked_add(segment.memory_size)
                .filter(|v| *v <= 1u64 << 32)
                .ok_or_else(|| invalid("segment exceeds RV32 address space"))?;
            if size > segment.memory_size
                || offset
                    .checked_add(size)
                    .is_none_or(|v| v > bytes.len() as u64)
                || (segment.writable() && segment.executable())
            {
                return Err(invalid("invalid segment bounds or writable code"));
            }
            for previous in &segments {
                c.checkpoint(1)?;
                if segment.memory_size != 0
                    && previous.memory_size != 0
                    && segment.address < previous.address + previous.memory_size
                    && previous.address < end
                {
                    return Err(invalid("overlapping load segments"));
                }
            }
            segments.push(ImageSegment {
                address: segment.address,
                file_offset: offset,
                file_size: size,
                memory_size: segment.memory_size,
                flags: segment.flags,
            });
        }
        if segments.is_empty() {
            return Err(invalid("image has no load segments"));
        }
        for section in file.sections() {
            c.checkpoint(1)?;
            if section.tls || section.kind == SectionKind::Dynamic {
                return Err(Error::new(
                    ErrorCode::Incompatible,
                    "dynamic loading and TLS are outside the static image profile",
                ));
            }
            if section.allocated && matches!(section.kind, SectionKind::Rel | SectionKind::Rela) {
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
            writable |= segment.flags & PF_W != 0;
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
            if segment.flags & PF_X != 0 && address >= segment.address {
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
            // Only a readable segment that is neither writable nor executable
            // holds constants: code loaded into RAM may carry tables the
            // program rewrites, such as a vector table in an `AX` section,
            // whose file bytes are only the initial contents.
            if segment.flags & (PF_R | PF_W | PF_X) != PF_R || address < segment.address {
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
    let file = oer_elf::Elf::executable(&bytes)
        .map_err(|_| invalid("execution requires static little-endian RV32 ELF"))?;
    let view = ProgramView::new(&bytes, &file, memory, control)?;
    let boot = boot_data(&file, &bytes, control)?;
    for segment in &view.segments {
        control.checkpoint(1)?;
        if segment.memory_size == 0 {
            continue;
        }
        let file_bytes = &bytes
            [segment.file_offset as usize..(segment.file_offset + segment.file_size) as usize];
        let zero_start = segment.address + segment.file_size;
        let zero_end = segment.address + segment.memory_size;
        let contained: Vec<_> = boot
            .iter()
            .filter(|&&(address, data)| {
                address >= zero_start && address + data.len() as u64 <= zero_end
            })
            .collect();
        if contained.is_empty() {
            consume(segment, file_bytes, control)?;
            continue;
        }
        // The initialized prefix ends at the last boot-data section.
        let end = contained
            .iter()
            .map(|&&(address, data)| address + data.len() as u64 - segment.address)
            .max()
            .unwrap() as usize;
        let mut image = memory.bytes(end, control.position())?;
        image[..file_bytes.len()].copy_from_slice(file_bytes);
        for &&(address, data) in &contained {
            control.checkpoint(1)?;
            let offset = (address - segment.address) as usize;
            image[offset..offset + data.len()].copy_from_slice(data);
        }
        consume(segment, &image, control)?;
    }
    Ok(())
}

/// Address of the defined code symbol at `index` of the static symbol table
/// in section `table_section` of a static RV32 executable: a text or untyped
/// symbol at an even address inside an executable section.
pub fn code_symbol_at(
    source: &dyn ByteSource,
    table_section: u32,
    index: u64,
    memory: &WorkingMemory,
    control: &mut dyn RunControl,
) -> Result<u32> {
    let mut bytes = memory.bytes(
        usize::try_from(source.len()).map_err(|_| invalid("ELF size overflow"))?,
        control.position(),
    )?;
    source.read_at(0, &mut bytes, control)?;
    let file = oer_elf::Elf::executable(&bytes)
        .map_err(|_| invalid("code symbols require a static RV32 executable"))?;
    let physical = file
        .symbol_table_section()
        .ok_or_else(|| invalid("executable has no static symbol table"))?;
    if physical != table_section as usize {
        return Err(invalid("symbol belongs to another physical table"));
    }
    let symbol = file
        .symbol_at(usize::try_from(index).map_err(|_| invalid("symbol index overflow"))?)
        .map_err(|_| invalid("symbol index outside the table"))?;
    let section = symbol
        .section
        .and_then(|i| file.section(i).ok())
        .ok_or_else(|| invalid("execution boundary is undefined/absolute"))?;
    let address = symbol.address;
    if !symbol.defined
        || !matches!(
            symbol.kind,
            oer_elf::SymbolKind::Text | oer_elf::SymbolKind::Unknown
        )
        || !section.executable
        || address < section.address
        || address - section.address >= section.size
        || address & 1 != 0
    {
        return Err(invalid("execution symbol is outside executable bytes"));
    }
    u32::try_from(address).map_err(|_| invalid("symbol address exceeds RV32"))
}

/// Defined, named code symbols of a static RV32 executable as (address, name),
/// ascending by address and then name. Section and mapping symbols are omitted.
pub fn code_symbols(
    source: &dyn ByteSource,
    memory: &WorkingMemory,
    control: &mut dyn RunControl,
) -> Result<Vec<(u32, String)>> {
    let mut bytes = memory.bytes(
        usize::try_from(source.len()).map_err(|_| invalid("ELF size overflow"))?,
        control.position(),
    )?;
    source.read_at(0, &mut bytes, control)?;
    let file = oer_elf::Elf::executable(&bytes)
        .map_err(|_| invalid("code symbols require a static RV32 executable"))?;
    let mut symbols = Vec::new();
    for symbol in file.symbols() {
        control.checkpoint(1)?;
        if !symbol.defined
            || symbol.kind != oer_elf::SymbolKind::Text
            || symbol.name.is_empty()
            || symbol.name.starts_with('$')
        {
            continue;
        }
        let address =
            u32::try_from(symbol.address).map_err(|_| invalid("symbol address exceeds RV32"))?;
        symbols
            .try_reserve(1)
            .map_err(|_| Error::new(ErrorCode::ResourceLimited, "symbol allocation refused"))?;
        symbols.push((address, symbol.name.to_owned()));
    }
    symbols.sort_unstable();
    symbols.dedup();
    Ok(symbols)
}

/// Boot-initialized data: writable `PROGBITS` sections whose bytes the file
/// carries although their load segment is zero-filled at that address. A ROM
/// copies them there at start-up; execution begins from that state. Sections
/// that overlap with content, or exceed the file, are rejected.
fn boot_data<'b>(
    file: &oer_elf::Elf<'_>,
    bytes: &'b [u8],
    control: &mut dyn RunControl,
) -> Result<Vec<(u64, &'b [u8])>> {
    let mut result: Vec<(u64, &'b [u8])> = Vec::new();
    for section in file.sections() {
        control.checkpoint(1)?;
        let size = section.size;
        if section.kind != SectionKind::Program
            || !section.writable
            || section.executable
            || section.tls
            || size == 0
        {
            continue;
        }
        let offset = section.file_offset;
        let address = section.address;
        let data = offset
            .checked_add(size)
            .filter(|end| *end <= bytes.len() as u64)
            .and_then(|end| bytes.get(offset as usize..end as usize))
            .ok_or_else(|| invalid("boot data exceeds the file"))?;
        if address.checked_add(size).is_none_or(|end| end > 1u64 << 32) {
            return Err(invalid("boot data exceeds RV32"));
        }
        if result
            .iter()
            .any(|&(start, other)| start < address + size && address < start + other.len() as u64)
        {
            return Err(invalid("boot data sections overlap"));
        }
        result.push((address, data));
    }
    Ok(result)
}

/// Visit every executable section, including code without function symbols.
/// Section-less files fail rather than reporting a vacuous clean result.
pub fn executable_sections(
    bytes: &[u8],
    memory: &WorkingMemory,
    control: &mut dyn RunControl,
    consume: &mut impl FnMut(ExecutableSectionView<'_>, &mut dyn RunControl) -> Result<()>,
) -> Result<()> {
    let file = oer_elf::Elf::executable(bytes)
        .map_err(|_| invalid("audit requires static little-endian RV32 ELF"))?;
    let _view = ProgramView::new(bytes, &file, memory, control)?;
    let mut found = false;
    for section in file.sections() {
        control.checkpoint(1)?;
        if !section.executable {
            continue;
        }
        let data = section.data;
        // A section whose file range leaves the file reads as empty; that is
        // a malformed image, never an empty section to skip.
        if !section.nobits
            && section
                .file_offset
                .checked_add(section.size)
                .is_none_or(|end| end > bytes.len() as u64)
        {
            return Err(invalid("invalid executable section bytes"));
        }
        if data.is_empty() {
            continue;
        }
        let address =
            u32::try_from(section.address).map_err(|_| invalid("section address exceeds RV32"))?;
        if section
            .address
            .checked_add(data.len() as u64)
            .is_none_or(|v| v > 1u64 << 32)
            || address & 1 != 0
        {
            return Err(invalid("invalid executable section extent"));
        }
        found = true;
        _view.code(u64::from(address), data, control)?;
        let mappings = crate::mapping::data_ranges(&file, section.index, memory, control)?;
        consume(
            ExecutableSectionView {
                section: section.index as u32,
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
            bytes.copy_from_slice(&self.0[offset as usize..offset as usize + bytes.len()]);
            Ok(())
        }
    }
    fn put16(b: &mut Vec<u8>, v: u16) {
        b.extend_from_slice(&v.to_le_bytes());
    }
    fn put32(b: &mut Vec<u8>, v: u32) {
        b.extend_from_slice(&v.to_le_bytes());
    }
    /// ELF32 RV32 executable: one RW load at 0x2000 with 16 zero-filled bytes
    /// and a writable PROGBITS section at `section_address` whose four bytes
    /// the file carries at offset 84.
    fn elf(section_address: u32) -> Vec<u8> {
        segment_elf(section_address, 0, 6)
    }
    /// `elf`, its load segment carrying `file_size` bytes from offset 84 and
    /// the ELF permissions `flags`.
    fn segment_elf(section_address: u32, file_size: u32, flags: u32) -> Vec<u8> {
        let mut b = vec![0x7f, b'E', b'L', b'F', 1, 1, 1, 0];
        b.resize(16, 0);
        put16(&mut b, 2); // ET_EXEC
        put16(&mut b, 243); // EM_RISCV
        put32(&mut b, 1);
        put32(&mut b, 0x2000); // entry
        put32(&mut b, 52); // phoff
        put32(&mut b, 100); // shoff
        put32(&mut b, 0); // flags: soft-float RV32
        put16(&mut b, 52);
        put16(&mut b, 32);
        put16(&mut b, 1);
        put16(&mut b, 40);
        put16(&mut b, 3);
        put16(&mut b, 2);
        // PT_LOAD, 16 bytes of memory.
        for v in [1, 84, 0x2000, 0x2000, file_size, 16, flags, 4] {
            put32(&mut b, v);
        }
        b.extend_from_slice(&[0x11, 0x22, 0x33, 0x44]); // offset 84
        b.extend_from_slice(b"\0.dat\0.shs\0\0"); // offset 88, 12 bytes
        b.resize(100, 0);
        b.extend_from_slice(&[0; 40]);
        // .dat: PROGBITS, SHF_WRITE | SHF_ALLOC.
        for v in [1, 1, 3, section_address, 84, 4, 0, 0, 4, 0] {
            put32(&mut b, v);
        }
        // .shs: STRTAB.
        for v in [6, 3, 0, 0, 88, 12, 0, 0, 1, 0] {
            put32(&mut b, v);
        }
        b
    }
    fn load(bytes: Vec<u8>) -> Result<Vec<(u64, Vec<u8>)>> {
        let memory = WorkingMemory::new(1 << 20).unwrap();
        let mut result = Vec::new();
        execution_segments(&Source(bytes), &memory, &mut || Ok(()), &mut |s, b, _| {
            result.push((s.address, b.to_vec()));
            Ok(())
        })?;
        Ok(result)
    }
    #[test]
    fn boot_data_initializes_its_zero_filled_load_address() {
        assert_eq!(
            load(elf(0x2008)).unwrap(),
            [(0x2000, vec![0, 0, 0, 0, 0, 0, 0, 0, 0x11, 0x22, 0x33, 0x44])]
        );
        // Outside every zero-filled range, boot data is not mapped at all.
        assert_eq!(load(elf(0x3000)).unwrap(), [(0x2000, vec![])]);
    }
    #[test]
    fn an_executable_section_beyond_the_file_fails_instead_of_being_skipped() {
        // `.dat` becomes `AX` (SHF_ALLOC | SHF_EXECINSTR) at the executable
        // segment's address, its size reaching past the end of the file.
        let mut bytes = segment_elf(0x2000, 4, 5);
        let header = 140;
        bytes[header + 8..header + 12].copy_from_slice(&6u32.to_le_bytes());
        bytes[header + 20..header + 24].copy_from_slice(&0x1000u32.to_le_bytes());
        let memory = WorkingMemory::new(1 << 20).unwrap();
        let error = executable_sections(&bytes, &memory, &mut || Ok(()), &mut |_, _| {
            panic!("a truncated section is never visited")
        })
        .unwrap_err();
        assert_eq!(error.message, "invalid executable section bytes");
    }
    #[test]
    fn constants_come_only_from_segments_neither_writable_nor_executable() {
        let constant = |flags| {
            let bytes = segment_elf(0x3000, 4, flags);
            let file = oer_elf::Elf::parse(bytes.as_slice()).unwrap();
            let memory = WorkingMemory::new(1 << 20).unwrap();
            let view = ProgramView::new(&bytes, &file, &memory, &mut || Ok(())).unwrap();
            let mut word = [0; 4];
            let known = view
                .read_constant(0x2000, &mut word, &mut || Ok(()))
                .unwrap();
            known.then_some(word)
        };
        let (r, w, x) = (PF_R, PF_W, PF_X);
        assert_eq!(constant(r), Some([0x11, 0x22, 0x33, 0x44]));
        assert_eq!(constant(r | w), None);
        // A vector table in RAM code: the program rewrites what the file
        // carries as its initial contents.
        assert_eq!(constant(r | x), None);
    }
}
