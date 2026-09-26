//! Function extraction from a static archive of RISC-V relocatable objects.

use std::collections::BTreeMap;

use object::{
    Object, ObjectSection, ObjectSymbol, RelocationFlags, RelocationTarget, SectionIndex,
    SymbolKind, read::archive::ArchiveFile,
};
use sha2::{Digest, Sha256};

use crate::body::{Body, Reference, RelocationSite};

/// One archive revision with every defined function it contains.
#[derive(Debug)]
pub struct Revision {
    /// Caller-chosen label, such as a commit identifier.
    pub label: String,
    /// SHA-256 of the complete archive bytes.
    pub sha256: String,
    /// Defined functions in archive member order.
    pub functions: Vec<Function>,
}

/// One defined function and its normalized body.
#[derive(Debug)]
pub struct Function {
    /// Archive member that defines the function.
    pub member: String,
    /// Symbol name as stored in the archive.
    pub name: String,
    /// Relocation-normalized body.
    pub body: Body,
}

pub type Error = Box<dyn std::error::Error>;

/// Read every defined sized function of every relocatable member.
pub fn read_archive(label: &str, bytes: &[u8]) -> Result<Revision, Error> {
    let archive = ArchiveFile::parse(bytes)?;
    let mut functions = Vec::new();
    for member in archive.members() {
        let member = member?;
        let member_name = String::from_utf8_lossy(member.name()).into_owned();
        let data = member.data(bytes)?;
        let Ok(file) = object::File::parse(data) else {
            continue;
        };
        read_object(&file, &member_name, &mut functions)?;
    }
    Ok(Revision {
        label: label.to_owned(),
        sha256: hex(&Sha256::digest(bytes)),
        functions,
    })
}

fn read_object(
    file: &object::File<'_>,
    member: &str,
    functions: &mut Vec<Function>,
) -> Result<(), Error> {
    // Relocations are grouped by section once; functions select their range.
    let mut relocations: BTreeMap<usize, Vec<(u64, object::Relocation)>> = BTreeMap::new();
    for section in file.sections() {
        relocations.insert(section.index().0, section.relocations().collect());
    }
    for symbol in file.symbols() {
        if symbol.kind() != SymbolKind::Text || !symbol.is_definition() || symbol.size() == 0 {
            continue;
        }
        let Some(section_index) = symbol.section_index() else {
            continue;
        };
        let section = file.section_by_index(section_index)?;
        let data = section.data()?;
        let start = usize::try_from(symbol.address())?;
        let size = usize::try_from(symbol.size())?;
        let Some(bytes) = data.get(start..start + size) else {
            return Err(format!("{member}: {} exceeds its section", symbol.name()?).into());
        };
        let mut sites = Vec::new();
        for (offset, relocation) in relocations.get(&section_index.0).into_iter().flatten() {
            let offset = usize::try_from(*offset)?;
            if offset < start || offset >= start + size {
                continue;
            }
            let RelocationFlags::Elf { r_type } = relocation.flags() else {
                continue;
            };
            let reference = reference(file, relocation, section_index, start, size)?;
            sites.push(RelocationSite {
                offset: offset - start,
                r_type,
                reference,
            });
        }
        functions.push(Function {
            member: member.to_owned(),
            name: symbol.name()?.to_owned(),
            body: Body::new(bytes, sites),
        });
    }
    Ok(())
}

fn reference(
    file: &object::File<'_>,
    relocation: &object::Relocation,
    section_index: SectionIndex,
    start: usize,
    size: usize,
) -> Result<Reference, Error> {
    let RelocationTarget::Symbol(index) = relocation.target() else {
        return Ok(Reference::Other);
    };
    let target = file.symbol_by_index(index)?;
    let address = usize::try_from(target.address())?;
    if target.section_index() == Some(section_index)
        && target.kind() != SymbolKind::Text
        && (start..=start + size).contains(&address)
    {
        return Ok(Reference::Label(address - start));
    }
    let name = target.name()?;
    if name.is_empty() || target.kind() == SymbolKind::Section {
        return Ok(Reference::Anonymous);
    }
    Ok(Reference::Symbol(name.to_owned()))
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
