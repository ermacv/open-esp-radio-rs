//! Functions, compiler frame records and value-analysis observations of one
//! static RV32 image.
use crate::TransferKind;
use oer_riscv_analysis::{FunctionInput, KnownJump, PreparedReferences, research};
use oer_riscv_lift::RiscvDecoder;
use oer_riscv_model::*;
use oer_riscv_program::ProgramView;
use std::collections::BTreeMap;

/// Working capacity of one image's analysis.
const MEMORY_LIMIT: u64 = 1 << 32;
const SP: u8 = 2;

/// A defined code symbol and every alias at its address.
pub use oer_elf::Function;

fn invalid(message: impl Into<String>) -> Error {
    Error::new(ErrorCode::Integrity, message)
}

pub(crate) fn parse(elf: &[u8]) -> Result<oer_elf::Elf<'_>> {
    oer_elf::Elf::executable(elf)
        .map_err(|_| invalid("stack analysis requires a static little-endian RV32 executable"))
}

/// The defined code symbols of `elf` as functions, ascending by address
/// ([`oer_elf::Elf::functions`]).
pub fn functions(elf: &[u8]) -> Result<Vec<Function>> {
    parse(elf)?
        .functions()
        .map_err(|error| invalid(error.to_string()))
}

/// The little-endian word at `address` of an allocated section with file
/// contents that is neither writable nor executable, such as `.rodata`: code
/// loaded into RAM may hold tables the program rewrites, such as a vector
/// table in an `AX` section.
pub(crate) fn read_only_word(file: &oer_elf::Elf<'_>, address: u32) -> Option<u32> {
    let address = u64::from(address);
    file.sections().find_map(|section| {
        let readonly = section.allocated && !section.writable && !section.executable;
        if !readonly || address < section.address || address + 4 > section.address + section.size {
            return None;
        }
        section.word(address)
    })
}

/// The words of the data object that starts at `address` in an unwritable
/// section: a table whose every entry an index the language bounds-checks
/// against the object's length may load.
pub(crate) fn table(elf: &[u8], address: u32) -> Result<Option<Vec<u32>>> {
    let file = parse(elf)?;
    let Some(symbol) = file.symbols().find(|symbol| {
        symbol.kind == oer_elf::SymbolKind::Data
            && symbol.address == u64::from(address)
            && symbol.size > 0
            && symbol.size % 4 == 0
    }) else {
        return Ok(None);
    };
    let words: Option<Vec<u32>> = (0..symbol.size / 4)
        .map(|i| read_only_word(&file, address.wrapping_add(4 * i as u32)))
        .collect();
    Ok(words)
}

/// Address ranges of the executable sections of `elf`.
pub(crate) fn executable_ranges(elf: &[u8]) -> Result<Vec<(u32, u32)>> {
    let file = parse(elf)?;
    let mut ranges = Vec::new();
    for section in file.sections().filter(|section| section.executable) {
        let start = u32::try_from(section.address).map_err(|_| invalid("section beyond RV32"))?;
        let end = u32::try_from(section.address + section.size)
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
    let bytes = section.data;
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
pub(crate) fn function_bytes<'a>(file: &oer_elf::Elf<'a>, function: &Function) -> Result<&'a [u8]> {
    let section = file
        .section(function.section)
        .map_err(|_| invalid("function in a missing section"))?;
    let start = u64::from(function.address)
        .checked_sub(section.address)
        .ok_or_else(|| invalid("function before its section"))? as usize;
    section
        .data
        .get(start..start + function.size as usize)
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
    /// The exact integer registers at each transfer the analysis reached.
    pub site_registers: BTreeMap<u32, [Option<u32>; 32]>,
}

#[derive(Default)]
struct Observer {
    deepest: i64,
    dynamic: bool,
    unexpanded: Vec<u32>,
    site_depths: BTreeMap<u32, u64>,
    site_registers: BTreeMap<u32, [Option<u32>; 32]>,
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
                target,
                call,
            } => {
                let kind = if *call {
                    TransferKind::Call
                } else {
                    TransferKind::Tail
                };
                // Every alternative must be an address for the site to count
                // as resolved.
                let addresses: Option<Vec<u32>> = match target {
                    AbstractValue::ImageAddress { address } => Some(vec![*address]),
                    AbstractValue::Alternatives { values } => values
                        .values()
                        .iter()
                        .map(|value| match value {
                            ValueAlternative::ImageAddress { address } => Some(*address),
                            _ => None,
                        })
                        .collect(),
                    _ => None,
                };
                for address in addresses.into_iter().flatten() {
                    self.transfers.push((*offset as u32, address, kind));
                }
            }
            FunctionRecord::SemanticGap {
                offset,
                reason: SemanticGapReason::UnexpandedControlFlow,
            } => self.unexpanded.push(*offset as u32),
            FunctionRecord::CallInputs { offset, registers } => {
                if let Some(AbstractValue::EntryStack { offset: sp }) = registers.get(SP as usize) {
                    self.site_depths.insert(*offset as u32, sp.unsigned_abs());
                }
                let mut exact = [None; 32];
                for (slot, value) in exact.iter_mut().zip(registers) {
                    *slot = match value {
                        AbstractValue::Constant { value: v }
                        | AbstractValue::ImageAddress { address: v } => Some(*v),
                        _ => None,
                    };
                }
                self.site_registers.insert(*offset as u32, exact);
            }
            _ => {}
        }
        Ok(())
    }
}

/// Run `body` with an observer of `elf`'s functions: `observe(function,
/// jumps)` runs the bounded value analysis over one function, its control-
/// flow graph following the indirect jumps whose targets `jumps` names.
pub(crate) fn observing<R>(
    elf: &[u8],
    body: impl FnOnce(&mut dyn FnMut(&Function, &[KnownJump]) -> Result<Observation>) -> Result<R>,
) -> Result<R> {
    let file = parse(elf)?;
    let memory = WorkingMemory::new(MEMORY_LIMIT)?;
    let view = ProgramView::new(elf, file.object(), &memory, &mut || Ok(()))?;
    let mut observe = |function: &Function, jumps: &[KnownJump]| -> Result<Observation> {
        let mut control = || Ok(());
        let code = function_bytes(&file, function)?;
        let references = PreparedReferences::new(
            &[],
            function.section as u32,
            &RiscvDecoder,
            &memory,
            &mut control,
        )?;
        let mut observer = Observer::default();
        let summary = research(
            FunctionInput {
                image: Some(&view),
                section: function.section as u32,
                extent: CodeRange {
                    start: u64::from(function.address),
                    length: u64::from(function.size),
                },
                bytes: code,
                relocations: &references,
                data_ranges: &[],
                jumps,
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
        Ok(Observation {
            depth,
            complete,
            resolved: observer.transfers,
            local_jumps: observer.unexpanded,
            site_depths: observer.site_depths,
            site_registers: observer.site_registers,
        })
    };
    body(&mut observe)
}
