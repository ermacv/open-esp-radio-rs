//! An image's interrupt table: for each peripheral source its handler, level
//! and core, read from the link's facts. The table is the slice an exported
//! symbol holds (its pointer word relocated to the entries, then its length);
//! every non-zero handler word carries the relocation that names it, and a
//! zero word is an entry the build left out.
use crate::image::read_only_word;
use crate::relocations::Relocations;
use oer_riscv_model::{Error, ErrorCode, Result};

/// Where one field lies in a table entry, in bytes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Field {
    pub offset: u32,
    pub size: u32,
}

/// The layout of a chip's table entry: the platform's contract, which its
/// own build asserts beside the binding type.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TableLayout {
    /// Bytes per entry.
    pub entry: u32,
    pub source: Field,
    pub level: Field,
    pub core: Field,
    /// Offset of the handler's address word.
    pub handler: u32,
}

/// One entry of the table.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TableEntry {
    pub source: u32,
    pub level: u32,
    pub core: u32,
    /// The handler's address; `None` for an entry the build left out.
    pub handler: Option<u32>,
}

/// The entries of the table `symbol` holds in `elf`.
pub fn interrupt_table(elf: &[u8], symbol: &str, layout: &TableLayout) -> Result<Vec<TableEntry>> {
    let invalid = |message: String| Error::new(ErrorCode::Integrity, message);
    let file = oer_elf::Elf::parse(elf).map_err(|_| invalid("invalid ELF".into()))?;
    let relocations = Relocations::read(elf)?;
    let slice = file
        .symbols()
        .find(|candidate| candidate.name == symbol)
        .ok_or_else(|| Error::new(ErrorCode::NotFound, format!("no `{symbol}` in the image")))?;
    let at = u32::try_from(slice.address).map_err(|_| invalid("table beyond RV32".into()))?;
    let word = |address: u32| {
        read_only_word(&file, address)
            .ok_or_else(|| invalid(format!("{address:#010x} is not read-only data")))
    };
    let first = word(at)?;
    if relocations.word(at) != Some(first) {
        return Err(invalid(format!(
            "`{symbol}`'s pointer at {at:#010x} has no relocation"
        )));
    }
    let length = word(at + 4)?;
    let field = |entry: u32, field: Field| -> Result<u32> {
        let address = entry + field.offset;
        if address % 4 + field.size > 4 {
            return Err(invalid(format!(
                "a field at {address:#010x} spans two words"
            )));
        }
        let value = word(address & !3)? >> (8 * (address % 4));
        Ok(match field.size {
            4 => value,
            size => value & ((1 << (8 * size)) - 1),
        })
    };
    (0..length)
        .map(|index| {
            let entry = first + index * layout.entry;
            let address = entry + layout.handler;
            let handler = word(address)?;
            if handler != 0 && relocations.word(address) != Some(handler) {
                return Err(invalid(format!(
                    "the handler word at {address:#010x} has no relocation"
                )));
            }
            Ok(TableEntry {
                source: field(entry, layout.source)?,
                level: field(entry, layout.level)?,
                core: field(entry, layout.core)?,
                handler: (handler != 0).then_some(handler & !1),
            })
        })
        .collect()
}
