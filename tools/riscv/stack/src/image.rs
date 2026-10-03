//! Functions, compiler frame records and value-analysis observations of one
//! static RV32 image.
use crate::TransferKind;
use object::{Object, ObjectSection, ObjectSymbol, SymbolKind};
use oer_riscv_analysis::{FunctionInput, PreparedReferences, research};
use oer_riscv_lift::RiscvDecoder;
use oer_riscv_model::*;
use oer_riscv_program::ProgramView;
use std::collections::BTreeMap;

/// Working capacity of one image's analysis.
const MEMORY_LIMIT: u64 = 1 << 32;
const SP: u8 = 2;

/// A defined code symbol and its aliases.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Function {
    pub address: u32,
    pub size: u32,
    /// Every symbol name at `address`, sorted.
    pub names: Vec<String>,
    /// Section index in the ELF section table.
    pub section: u32,
}

impl Function {
    pub fn label(&self) -> String {
        match self.names.first() {
            Some(name) => name.clone(),
            None => format!("{:#010x}", self.address),
        }
    }
}

fn invalid(message: impl Into<String>) -> Error {
    Error::new(ErrorCode::Integrity, message)
}

fn parse(elf: &[u8]) -> Result<object::File<'_>> {
    let file = object::File::parse(elf).map_err(|_| invalid("invalid ELF"))?;
    if file.kind() != object::ObjectKind::Executable
        || file.architecture() != object::Architecture::Riscv32
        || !file.is_little_endian()
    {
        return Err(invalid(
            "stack analysis requires a static little-endian RV32 executable",
        ));
    }
    Ok(file)
}

/// The defined code symbols of `elf` as functions, ascending by address. A
/// symbol without a size extends to the next code symbol of its section, or to
/// the section's end. Mapping symbols (`$x`, `$d`) and local labels (`.L`) are
/// not functions.
pub fn functions(elf: &[u8]) -> Result<Vec<Function>> {
    let file = parse(elf)?;
    let mut by_address: BTreeMap<u32, (u32, Vec<String>, u32)> = BTreeMap::new();
    for symbol in file.symbols() {
        if symbol.is_undefined() || symbol.kind() != SymbolKind::Text {
            continue;
        }
        let Ok(name) = symbol.name() else { continue };
        if name.is_empty() || name.starts_with('$') || name.starts_with(".L") {
            continue;
        }
        let Some(section) = symbol.section_index() else {
            continue;
        };
        let address = u32::try_from(symbol.address()).map_err(|_| invalid("symbol beyond RV32"))?;
        let size = u32::try_from(symbol.size()).map_err(|_| invalid("symbol size beyond RV32"))?;
        let entry = by_address
            .entry(address)
            .or_insert((0, Vec::new(), section.0 as u32));
        entry.0 = entry.0.max(size);
        entry.1.push(name.to_owned());
    }
    let starts: Vec<u32> = by_address.keys().copied().collect();
    let mut functions = Vec::with_capacity(by_address.len());
    for (index, (address, (size, mut names, section))) in by_address.into_iter().enumerate() {
        names.sort();
        names.dedup();
        let size = if size != 0 {
            size
        } else {
            let header = file
                .section_by_index(object::SectionIndex(section as usize))
                .map_err(|_| invalid("symbol in a missing section"))?;
            let end = header.address() + header.size();
            let next = starts
                .get(index + 1)
                .map_or(end, |&next| u64::from(next).min(end));
            u32::try_from(next.saturating_sub(u64::from(address)))
                .map_err(|_| invalid("function extent beyond RV32"))?
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

/// Address ranges of the executable sections of `elf`.
pub(crate) fn executable_ranges(elf: &[u8]) -> Result<Vec<(u32, u32)>> {
    let file = parse(elf)?;
    let mut ranges = Vec::new();
    for section in file.sections() {
        let object::SectionFlags::Elf { sh_flags } = section.flags() else {
            continue;
        };
        if sh_flags & u64::from(object::elf::SHF_EXECINSTR) == 0 {
            continue;
        }
        let start = u32::try_from(section.address()).map_err(|_| invalid("section beyond RV32"))?;
        let end = u32::try_from(section.address() + section.size())
            .map_err(|_| invalid("section beyond RV32"))?;
        ranges.push((start, end));
    }
    Ok(ranges)
}

fn uleb128(bytes: &[u8], at: &mut usize) -> Result<u64> {
    let mut value = 0u64;
    for shift in (0..64).step_by(7) {
        let byte = *bytes
            .get(*at)
            .ok_or_else(|| invalid("truncated .stack_sizes entry"))?;
        *at += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
    }
    Err(invalid("oversized .stack_sizes value"))
}

/// The compiler's frame record of each function: `.stack_sizes` entries of a
/// 32-bit address followed by a ULEB128 size. An image without the section has
/// no records; one function recorded twice with different sizes is an error.
pub fn stack_sizes(elf: &[u8]) -> Result<BTreeMap<u32, u64>> {
    let file = parse(elf)?;
    let mut sizes = BTreeMap::new();
    let Some(section) = file.section_by_name(".stack_sizes") else {
        return Ok(sizes);
    };
    let bytes = section
        .data()
        .map_err(|_| invalid("unreadable .stack_sizes"))?;
    let mut at = 0;
    while at < bytes.len() {
        let address = bytes
            .get(at..at + 4)
            .ok_or_else(|| invalid("truncated .stack_sizes address"))?;
        let address = u32::from_le_bytes(address.try_into().unwrap());
        at += 4;
        let size = uleb128(bytes, &mut at)?;
        if let Some(previous) = sizes.insert(address, size)
            && previous != size
        {
            return Err(invalid(format!(
                "{address:#010x} has .stack_sizes {previous} and {size}"
            )));
        }
    }
    Ok(sizes)
}

/// The bytes of `function` in its section.
pub(crate) fn function_bytes<'a>(file: &object::File<'a>, function: &Function) -> Result<&'a [u8]> {
    let section = file
        .section_by_index(object::SectionIndex(function.section as usize))
        .map_err(|_| invalid("function in a missing section"))?;
    let data = section
        .data()
        .map_err(|_| invalid("unreadable code section"))?;
    let start = u64::from(function.address)
        .checked_sub(section.address())
        .ok_or_else(|| invalid("function before its section"))? as usize;
    data.get(start..start + function.size as usize)
        .ok_or_else(|| invalid(format!("{} exceeds its section", function.label())))
}

/// What the value analysis observed in one function.
pub(crate) struct Observation {
    /// The deepest entry-relative `sp`, unless some `sp` value is not entry-relative.
    pub depth: Option<u64>,
    pub complete: bool,
    /// Transfers whose target the value analysis resolved.
    pub resolved: Vec<(u32, u32, TransferKind)>,
    /// Sites of indirect jumps whose targets the analysis found inside the
    /// function: jump tables the linear sweep already covers.
    pub local_jumps: Vec<u32>,
    /// Bytes below the entry `sp` at each transfer the analysis reached.
    pub site_depths: BTreeMap<u32, u64>,
}

#[derive(Default)]
struct Observer {
    deepest: i64,
    dynamic: bool,
    unexpanded: Vec<u32>,
    site_depths: BTreeMap<u32, u64>,
    transfers: Vec<(u32, u32, TransferKind)>,
}

impl FunctionSink for Observer {
    fn record(&mut self, record: &FunctionRecord, _: &mut dyn RunControl) -> Result<()> {
        match record {
            FunctionRecord::Value {
                register: SP,
                value,
                ..
            } => match value {
                AbstractValue::EntryStack { offset } => self.deepest = self.deepest.min(*offset),
                _ => self.dynamic = true,
            },
            FunctionRecord::Transfer {
                offset,
                target: AbstractValue::ImageAddress { address },
                call,
            } => self.transfers.push((
                *offset as u32,
                *address,
                if *call {
                    TransferKind::Call
                } else {
                    TransferKind::Tail
                },
            )),
            FunctionRecord::SemanticGap {
                offset,
                reason: SemanticGapReason::UnexpandedControlFlow,
            } => self.unexpanded.push(*offset as u32),
            FunctionRecord::CallInputs { offset, registers } => {
                if let Some(AbstractValue::EntryStack { offset: sp }) = registers.get(SP as usize) {
                    self.site_depths.insert(*offset as u32, sp.unsigned_abs());
                }
            }
            _ => {}
        }
        Ok(())
    }
}

/// Run the bounded value analysis over every function of `elf`.
pub(crate) fn observe(elf: &[u8], functions: &[Function]) -> Result<BTreeMap<u32, Observation>> {
    let file = parse(elf)?;
    let memory = WorkingMemory::new(MEMORY_LIMIT)?;
    let mut control = || Ok(());
    let view = ProgramView::new(elf, &file, &memory, &mut control)?;
    let mut observed = BTreeMap::new();
    for function in functions {
        let code = function_bytes(&file, function)?;
        let references =
            PreparedReferences::new(&[], function.section, &RiscvDecoder, &memory, &mut control)?;
        let mut observer = Observer::default();
        let summary = research(
            FunctionInput {
                image: Some(&view),
                section: function.section,
                extent: CodeRange {
                    start: u64::from(function.address),
                    length: u64::from(function.size),
                },
                bytes: code,
                relocations: &references,
                data_ranges: &[],
            },
            &RiscvDecoder,
            &memory,
            &mut control,
            &mut observer,
            // The psABI preserves sp and the saved registers across calls.
            Some(CallAbi::RiscvInteger),
        )?;
        let complete = summary.coverage.decoding
            && summary.coverage.control_flow
            && observer.unexpanded.is_empty()
            && !observer.dynamic;
        let depth = (!observer.dynamic).then_some(observer.deepest.unsigned_abs());
        observed.insert(
            function.address,
            Observation {
                depth,
                complete,
                resolved: observer.transfers,
                local_jumps: observer.unexpanded,
                site_depths: observer.site_depths,
            },
        );
    }
    Ok(observed)
}
