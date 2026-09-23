//! One owner for ELF mapping-symbol interpretation and admitted interval storage.
use blobray_domain::*;
use object::{Object, ObjectSection, ObjectSymbol};

pub(crate) struct DataRanges<'a> {
    pub ranges: Vec<CodeRange>,
    _capacity: MemoryReservation<'a>,
}
fn invalid(message: &str) -> Error {
    Error::new(ErrorCode::Integrity, message)
}
fn allocation() -> Error {
    Error::new(
        ErrorCode::ResourceLimited,
        "mapping symbol allocation refused",
    )
}
// https://riscv-non-isa.github.io/riscv-elf-psabi-doc/#_mapping_symbol
// ISA-qualified markers delimit code too. They do not expand decoder support.
fn kind(name: &[u8]) -> Result<Option<bool>> {
    if name == b"$d" || name.starts_with(b"$d.") {
        Ok(Some(true))
    } else if name == b"$x" || name.starts_with(b"$x.") || name.starts_with(b"$xrv32") {
        Ok(Some(false))
    } else if name.starts_with(b"$xrv") {
        Err(Error::new(
            ErrorCode::Incompatible,
            "mapping ISA is outside RV32",
        ))
    } else {
        Ok(None)
    }
}
pub(crate) fn data_ranges<'a>(
    file: &object::File<'_>,
    section_index: object::SectionIndex,
    memory: &'a WorkingMemory,
    control: &mut dyn RunControl,
) -> Result<DataRanges<'a>> {
    let section = file
        .section_by_index(section_index)
        .map_err(|_| invalid("invalid mapping section"))?;
    let end = section
        .address()
        .checked_add(section.size())
        .ok_or_else(|| invalid("mapping section extent overflow"))?;
    let mut count = 0usize;
    for symbol in file.symbols() {
        control.checkpoint(1)?;
        if symbol.section_index() == Some(section_index)
            && kind(
                symbol
                    .name_bytes()
                    .map_err(|_| invalid("invalid mapping symbol name"))?,
            )?
            .is_some()
        {
            count = count
                .checked_add(1)
                .ok_or_else(|| invalid("mapping count overflow"))?;
        }
    }
    let capacity = memory.reserve(
        (count as u64)
            .checked_mul(
                (std::mem::size_of::<(u64, bool)>() + std::mem::size_of::<CodeRange>()) as u64,
            )
            .ok_or_else(|| invalid("mapping capacity overflow"))?,
        control.position(),
    )?;
    let mut mappings = Vec::new();
    mappings
        .try_reserve_exact(count)
        .map_err(|_| allocation())?;
    let mut ranges = Vec::new();
    ranges.try_reserve_exact(count).map_err(|_| allocation())?;
    for symbol in file.symbols() {
        control.checkpoint(1)?;
        if symbol.section_index() != Some(section_index) {
            continue;
        }
        let Some(is_data) = kind(
            symbol
                .name_bytes()
                .map_err(|_| invalid("invalid mapping symbol name"))?,
        )?
        else {
            continue;
        };
        if !symbol.is_local()
            || symbol.kind() != object::SymbolKind::Unknown
            || symbol.size() != 0
            || symbol.address() < section.address()
            || symbol.address() > end
            || (!is_data && symbol.address() & 1 != 0)
        {
            return Err(invalid("invalid ELF mapping symbol"));
        }
        mappings.push((symbol.address(), is_data));
    }
    // In-place, bounded storage; charge the comparison bound before sorting.
    control.checkpoint(
        (count as u64).saturating_mul(u64::from(usize::BITS - count.leading_zeros())),
    )?;
    mappings.sort_unstable();
    mappings.dedup();
    for (i, &(start, is_data)) in mappings.iter().enumerate() {
        control.checkpoint(1)?;
        let next = mappings.get(i + 1).map_or(end, |m| m.0);
        if next == start && i + 1 < mappings.len() {
            return Err(invalid("conflicting ELF mapping symbols"));
        }
        if is_data && next > start {
            ranges.push(CodeRange {
                start,
                length: next - start,
            });
        }
    }
    Ok(DataRanges {
        ranges,
        _capacity: capacity,
    })
}
