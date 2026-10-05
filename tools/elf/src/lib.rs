//! The one view of ELF files and static archives: symbols with their
//! aliases by address, sections, relocations with the one RV32 relocation
//! table ([`rv32`]), the functions of a relocatable object with their
//! relocation sites, and source locations from DWARF ([`dwarf`]).
//!
//! Every host tool that reads an ELF reads it through this crate instead of
//! parsing `llvm-nm` or `llvm-objdump` output or walking `object` itself.
//! It depends on no other repository package, so Blobray's standalone
//! extraction takes it by path.

#![forbid(unsafe_code)]

use object::read::archive::ArchiveFile;
use object::{
    Object, ObjectSection, ObjectSegment, ObjectSymbol, RelocationFlags, RelocationTarget,
};
use std::collections::BTreeMap;
use std::fmt;

pub mod dwarf;
pub mod rv32;

/// Why a file could not be read.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Error(String);

impl Error {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

impl From<object::Error> for Error {
    fn from(error: object::Error) -> Self {
        Self(error.to_string())
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// What a symbol names.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SymbolKind {
    Text,
    Data,
    Section,
    File,
    Tls,
    Unknown,
}

/// One entry of the static symbol table.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Symbol<'data> {
    /// Index in the symbol table.
    pub index: usize,
    pub name: &'data str,
    pub address: u64,
    pub size: u64,
    pub kind: SymbolKind,
    /// Defined in this file: neither undefined nor common. Section, file
    /// and absolute linker symbols are defined.
    pub defined: bool,
    pub global: bool,
    /// Index of the section that defines it.
    pub section: Option<usize>,
}

impl Symbol<'_> {
    /// The name demangled, without Rust's legacy hash.
    pub fn demangled(&self) -> String {
        demangle(self.name)
    }
}

/// A Rust (legacy or v0) name demangled without hashes; any other name as is.
pub fn demangle(name: &str) -> String {
    addr2line::demangle_auto(name.into(), None).into_owned()
}

/// One section header.
#[derive(Clone, Debug)]
pub struct Section<'data> {
    pub index: usize,
    pub name: &'data str,
    pub address: u64,
    pub size: u64,
    /// `SHF_ALLOC`: occupies memory at run time.
    pub allocated: bool,
    /// `SHF_WRITE`.
    pub writable: bool,
    /// `SHF_EXECINSTR`.
    pub executable: bool,
    /// `SHF_TLS`.
    pub tls: bool,
    /// `SHT_NOBITS`: no bytes in the file.
    pub nobits: bool,
    /// The section's bytes in the file; empty for `NOBITS`.
    pub data: &'data [u8],
}

impl Section<'_> {
    /// Whether `address` lies in the section's address range.
    pub fn contains(&self, address: u64) -> bool {
        self.address <= address && address - self.address < self.size
    }

    /// The little-endian word at `address`, when the file holds it.
    pub fn word(&self, address: u64) -> Option<u32> {
        let at = usize::try_from(address.checked_sub(self.address)?).ok()?;
        let bytes = self.data.get(at..at.checked_add(4)?)?;
        Some(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }
}

/// One loadable program segment.
#[derive(Clone, Debug)]
pub struct Segment<'data> {
    pub address: u64,
    /// Size in memory.
    pub size: u64,
    /// `PF_X`.
    pub executable: bool,
    /// The segment's bytes in the file, which may be fewer than `size`.
    pub data: &'data [u8],
}

impl<'data> Segment<'data> {
    /// The `length` file bytes at `address`, when the segment holds them all.
    pub fn bytes(&self, address: u64, length: usize) -> Option<&'data [u8]> {
        let at = usize::try_from(address.checked_sub(self.address)?).ok()?;
        self.data.get(at..at.checked_add(length)?)
    }
}

/// What a relocation refers to.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Target {
    /// The symbol at this symbol-table index.
    Symbol(usize),
    /// The start of the section at this index.
    Section(usize),
    /// No symbol: the addend is the value.
    Absolute,
    /// A form an RV32 ELF does not use.
    Other,
}

/// One relocation of a section.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Relocation {
    /// The section it patches.
    pub section: usize,
    /// `r_offset`: the address it patches in a linked file, its offset in
    /// the section in a relocatable object.
    pub at: u64,
    /// ELF `r_type`; [`rv32::kind`] describes it.
    pub r_type: u32,
    pub target: Target,
    pub addend: i64,
}

/// A code symbol with every other name at its address.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Function {
    pub address: u32,
    pub size: u32,
    /// Every symbol name at `address`, raw and sorted.
    pub names: Vec<String>,
    /// Index of the section that holds it.
    pub section: usize,
}

impl Function {
    /// The first name, or the address when it has none.
    pub fn label(&self) -> String {
        match self.names.first() {
            Some(name) => name.clone(),
            None => format!("{:#010x}", self.address),
        }
    }
}

/// What one relocation inside a relocatable function refers to.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Reference {
    /// A position in the function's own section: the target's offset
    /// (addend included) from the function start. Section symbols and local
    /// labels resolve here.
    Local(i64),
    /// A named symbol elsewhere.
    Symbol(String),
    /// A section or unnamed symbol of another section.
    Anonymous,
    /// A non-symbol target.
    Other,
}

/// One relocation inside a relocatable function.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Site {
    /// Byte offset from the function start.
    pub offset: usize,
    /// ELF `r_type`.
    pub r_type: u32,
    pub reference: Reference,
}

/// A defined, sized function of a relocatable object (or of an ELF image)
/// with its bytes and the relocations inside them, by offset.
#[derive(Clone, Debug)]
pub struct Code<'data> {
    /// Archive member, or empty for a standalone file.
    pub member: String,
    pub name: &'data str,
    pub bytes: &'data [u8],
    pub sites: Vec<Site>,
}

/// One parsed ELF file.
pub struct Elf<'data> {
    file: object::File<'data>,
}

/// Whether `bytes` are a static archive or an ELF file.
pub fn is_binary(bytes: &[u8]) -> bool {
    bytes.starts_with(b"!<arch>\n") || bytes.starts_with(b"\x7fELF")
}

/// Every member of the static archive `bytes`, by name, in archive order.
/// A thin archive, whose members live elsewhere, is refused.
pub fn members(bytes: &[u8]) -> Result<Vec<(String, &[u8])>> {
    let archive = ArchiveFile::parse(bytes)?;
    if archive.is_thin() {
        return Err(Error::new("thin archives hold no members"));
    }
    let mut members = Vec::new();
    for member in archive.members() {
        let member = member?;
        let name = String::from_utf8_lossy(member.name()).into_owned();
        members.push((name, member.data(bytes)?));
    }
    Ok(members)
}

/// The ELF objects of `bytes`: each member of a static archive that parses
/// as one, by member name, or the single ELF file with an empty name.
pub fn objects(bytes: &[u8]) -> Result<Vec<(String, Elf<'_>)>> {
    if bytes.starts_with(b"!<arch>\n") {
        return Ok(members(bytes)?
            .into_iter()
            .filter_map(|(name, data)| Elf::parse(data).ok().map(|elf| (name, elf)))
            .collect());
    }
    Ok(vec![(String::new(), Elf::parse(bytes)?)])
}

impl<'data> Elf<'data> {
    /// Parse an ELF file.
    pub fn parse(bytes: &'data [u8]) -> Result<Self> {
        let file = object::File::parse(bytes)?;
        if file.format() != object::BinaryFormat::Elf {
            return Err(Error::new("not an ELF file"));
        }
        Ok(Self { file })
    }

    /// Parse a static little-endian RV32 executable.
    pub fn executable(bytes: &'data [u8]) -> Result<Self> {
        let elf = Self::parse(bytes)?;
        if elf.file.kind() != object::ObjectKind::Executable
            || elf.file.architecture() != object::Architecture::Riscv32
            || !elf.file.is_little_endian()
        {
            return Err(Error::new("not a static little-endian RV32 executable"));
        }
        Ok(elf)
    }

    /// Whether the file is an executable.
    pub fn is_executable(&self) -> bool {
        self.file.kind() == object::ObjectKind::Executable
    }

    /// Whether the file is a relocatable object.
    pub fn is_relocatable(&self) -> bool {
        self.file.kind() == object::ObjectKind::Relocatable
    }

    /// Whether the file has a static symbol table.
    pub fn has_symbol_table(&self) -> bool {
        self.file.symbol_table().is_some()
    }

    /// The underlying `object` view, for format details this view does not
    /// name (Blobray's ABI checks).
    pub fn object(&self) -> &object::File<'data> {
        &self.file
    }

    /// Every symbol of the static symbol table, in table order. Symbols
    /// whose name is unreadable are skipped.
    pub fn symbols(&self) -> impl Iterator<Item = Symbol<'data>> + '_ {
        self.file.symbols().filter_map(symbol)
    }

    /// The symbol at `index` of the static symbol table.
    pub fn symbol_at(&self, index: usize) -> Result<Symbol<'data>> {
        let entry = self.file.symbol_by_index(object::SymbolIndex(index))?;
        symbol(entry).ok_or_else(|| Error::new(format!("symbol {index} has no readable name")))
    }

    /// The first defined symbol named `name`.
    pub fn symbol(&self, name: &str) -> Option<Symbol<'data>> {
        self.symbols().find(|s| s.defined && s.name == name)
    }

    /// The address of the first defined symbol named `name`.
    pub fn address(&self, name: &str) -> Option<u64> {
        self.symbol(name).map(|s| s.address)
    }

    /// Every defined, named symbol by name; a name defined twice keeps its
    /// first address.
    pub fn addresses(&self) -> BTreeMap<&'data str, u64> {
        let mut map = BTreeMap::new();
        for s in self.symbols().filter(|s| s.defined && !s.name.is_empty()) {
            map.entry(s.name).or_insert(s.address);
        }
        map
    }

    /// Every loadable segment, in program-header order.
    pub fn segments(&self) -> impl Iterator<Item = Segment<'data>> + '_ {
        self.file.segments().map(|segment| Segment {
            address: segment.address(),
            size: segment.size(),
            executable: matches!(
                segment.flags(),
                object::SegmentFlags::Elf { p_flags } if p_flags & object::elf::PF_X != 0
            ),
            data: segment.data().unwrap_or_default(),
        })
    }

    /// Every section header, in table order.
    pub fn sections(&self) -> impl Iterator<Item = Section<'data>> + '_ {
        self.file.sections().map(|s| section(&s))
    }

    /// The section at `index`.
    pub fn section(&self, index: usize) -> Result<Section<'data>> {
        Ok(section(
            &self.file.section_by_index(object::SectionIndex(index))?,
        ))
    }

    /// The first section named `name`.
    pub fn section_by_name(&self, name: &str) -> Option<Section<'data>> {
        self.file.section_by_name(name).map(|s| section(&s))
    }

    /// The relocations of the section at `index`, in file order.
    pub fn relocations(&self, index: usize) -> Result<Vec<Relocation>> {
        let header = self.file.section_by_index(object::SectionIndex(index))?;
        Ok(header
            .relocations()
            .filter_map(|(at, r)| relocation(index, at, &r))
            .collect())
    }

    /// The value a relocation's symbol, section or absolute target plus its
    /// addend designates: an address in a linked file.
    pub fn target_address(&self, relocation: &Relocation) -> Result<u64> {
        let base = match relocation.target {
            Target::Symbol(index) => self.symbol_at(index)?.address,
            Target::Section(index) => self.section(index)?.address,
            Target::Absolute => 0,
            Target::Other => return Err(Error::new("relocation without a target")),
        };
        Ok(base.wrapping_add_signed(relocation.addend))
    }

    /// The defined code symbols of an executable as functions, ascending by
    /// address: its function symbols, and its global untyped symbols in an
    /// executable section outside every sized function, such as assembly
    /// entries and the ROM's `__call_*` trampolines. Symbols at one address
    /// are one function with all their names. A symbol without a size
    /// extends to the next code symbol of its section, or to the section's
    /// end. Mapping symbols (`$x`, `$d`) and local labels (`.L`) are not
    /// functions.
    pub fn functions(&self) -> Result<Vec<Function>> {
        let sections: BTreeMap<usize, Section<'_>> = self
            .sections()
            .map(|section| (section.index, section))
            .collect();
        let executable = |s: &Symbol<'_>| {
            s.section
                .and_then(|index| sections.get(&index))
                .is_some_and(|section| section.executable)
        };
        let sized: Vec<(u64, u64)> = self
            .symbols()
            .filter(|s| s.kind == SymbolKind::Text && s.size > 0)
            .map(|s| (s.address, s.address + s.size))
            .collect();
        let mut by_address: BTreeMap<u32, (u32, Vec<String>, usize)> = BTreeMap::new();
        for s in self.symbols() {
            let untyped_entry = s.kind == SymbolKind::Unknown
                && s.global
                && executable(&s)
                && !sized
                    .iter()
                    .any(|&(start, end)| s.address > start && s.address < end);
            if !s.defined || !(s.kind == SymbolKind::Text || untyped_entry) {
                continue;
            }
            if s.name.is_empty() || s.name.starts_with('$') || s.name.starts_with(".L") {
                continue;
            }
            let Some(section) = s.section else { continue };
            let address = u32::try_from(s.address).map_err(|_| Error::new("symbol beyond RV32"))?;
            let size = u32::try_from(s.size).map_err(|_| Error::new("symbol size beyond RV32"))?;
            let entry = by_address
                .entry(address)
                .or_insert((0, Vec::new(), section));
            entry.0 = entry.0.max(size);
            entry.1.push(s.name.to_owned());
        }
        let starts: Vec<u32> = by_address.keys().copied().collect();
        let mut functions = Vec::with_capacity(by_address.len());
        for (index, (address, (size, mut names, section))) in by_address.into_iter().enumerate() {
            names.sort();
            names.dedup();
            let size = if size != 0 {
                size
            } else {
                let header = sections
                    .get(&section)
                    .ok_or_else(|| Error::new("symbol in a missing section"))?;
                let end = header.address + header.size;
                let next = starts
                    .get(index + 1)
                    .map_or(end, |&next| u64::from(next).min(end));
                u32::try_from(next.saturating_sub(u64::from(address)))
                    .map_err(|_| Error::new("function extent beyond RV32"))?
            };
            if size == 0 {
                continue;
            }
            functions.push(Function {
                address,
                size,
                names,
                section,
            });
        }
        Ok(functions)
    }

    /// Every defined, sized, named function symbol with its bytes and the
    /// relocations inside them. A relocation targeting the function's own
    /// section through its section symbol or a local symbol is a
    /// [`Reference::Local`] offset, addend included.
    pub fn code(&self, member: &str) -> Result<Vec<Code<'data>>> {
        let mut relocations: BTreeMap<usize, Vec<Relocation>> = BTreeMap::new();
        for s in self.sections() {
            relocations.insert(s.index, self.relocations(s.index)?);
        }
        let mut out = Vec::new();
        for s in self.symbols() {
            if s.kind != SymbolKind::Text || s.size == 0 || !s.defined || s.name.is_empty() {
                continue;
            }
            let Some(index) = s.section else { continue };
            let header = self.section(index)?;
            let start = usize::try_from(s.address - header.address)
                .map_err(|_| Error::new("function offset overflow"))?;
            let end = start
                .checked_add(usize::try_from(s.size).map_err(|_| Error::new("size overflow"))?)
                .ok_or_else(|| Error::new("function extent overflow"))?;
            let Some(bytes) = header.data.get(start..end) else {
                return Err(Error::new(format!(
                    "{member}: {} exceeds its section",
                    s.name
                )));
            };
            let mut sites = Vec::new();
            for r in relocations.get(&index).into_iter().flatten() {
                let Some(offset) =
                    r.at.checked_sub(header.address)
                        .and_then(|o| usize::try_from(o).ok())
                else {
                    continue;
                };
                if offset < start || offset >= end {
                    continue;
                }
                let reference = match r.target {
                    Target::Symbol(target) => {
                        let target = self.symbol_at(target)?;
                        if target.section == Some(index)
                            && (target.kind == SymbolKind::Section || !target.global)
                        {
                            Reference::Local(
                                target.address as i64 - header.address as i64 + r.addend
                                    - start as i64,
                            )
                        } else if target.name.is_empty() || target.kind == SymbolKind::Section {
                            Reference::Anonymous
                        } else {
                            Reference::Symbol(target.name.to_owned())
                        }
                    }
                    Target::Section(target) if target == index => {
                        Reference::Local(r.addend - start as i64)
                    }
                    Target::Section(_) => Reference::Anonymous,
                    Target::Absolute | Target::Other => Reference::Other,
                };
                sites.push(Site {
                    offset: offset - start,
                    r_type: r.r_type,
                    reference,
                });
            }
            sites.sort_by_key(|site| (site.offset, site.r_type));
            out.push(Code {
                member: member.to_owned(),
                name: s.name,
                bytes,
                sites,
            });
        }
        Ok(out)
    }
}

fn symbol<'data>(entry: object::Symbol<'data, '_>) -> Option<Symbol<'data>> {
    let name = entry.name().ok()?;
    Some(Symbol {
        index: entry.index().0,
        name,
        address: entry.address(),
        size: entry.size(),
        kind: match entry.kind() {
            object::SymbolKind::Text => SymbolKind::Text,
            object::SymbolKind::Data => SymbolKind::Data,
            object::SymbolKind::Section => SymbolKind::Section,
            object::SymbolKind::File => SymbolKind::File,
            object::SymbolKind::Tls => SymbolKind::Tls,
            _ => SymbolKind::Unknown,
        },
        defined: !entry.is_undefined() && !entry.is_common(),
        global: entry.is_global(),
        section: entry.section_index().map(|index| index.0),
    })
}

fn section<'data>(header: &object::Section<'data, '_>) -> Section<'data> {
    let flags = match header.flags() {
        object::SectionFlags::Elf { sh_flags } => sh_flags,
        _ => 0,
    };
    // Only `SHT_NOBITS` (and the null section) occupy no file bytes.
    let nobits = header.file_range().is_none();
    Section {
        index: header.index().0,
        name: header.name().unwrap_or_default(),
        address: header.address(),
        size: header.size(),
        allocated: flags & u64::from(object::elf::SHF_ALLOC) != 0,
        writable: flags & u64::from(object::elf::SHF_WRITE) != 0,
        executable: flags & u64::from(object::elf::SHF_EXECINSTR) != 0,
        tls: flags & u64::from(object::elf::SHF_TLS) != 0,
        nobits,
        data: if nobits {
            &[]
        } else {
            header.data().unwrap_or_default()
        },
    }
}

fn relocation(section: usize, at: u64, r: &object::Relocation) -> Option<Relocation> {
    let RelocationFlags::Elf { r_type } = r.flags() else {
        return None;
    };
    Some(Relocation {
        section,
        at,
        r_type,
        target: match r.target() {
            RelocationTarget::Symbol(index) => Target::Symbol(index.0),
            RelocationTarget::Section(index) => Target::Section(index.0),
            RelocationTarget::Absolute => Target::Absolute,
            _ => Target::Other,
        },
        addend: r.addend(),
    })
}

#[cfg(test)]
mod tests;
