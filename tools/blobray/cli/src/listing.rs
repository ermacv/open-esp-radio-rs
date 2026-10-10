//! Readable views of one function's records: its instruction listing with
//! the symbols its relocations name, and the references it makes to named
//! symbols.
use oer_riscv_model::{
    AbstractValue, FunctionRecord, MemoryKind, ReferenceKind, SymbolId, ValueAlternative,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

const ZERO: u8 = 0;
const RA: u8 = 1;
const SP: u8 = 2;

/// One instruction line: the function offset, the decoded text, the symbols
/// the relocations at that offset name (`name+addend`) and, after `#`, the
/// exact values the analysis derived there.
///
/// The decoded text stays in the disassembler's form. The annotation shows
/// each register write other than `zero`, `ra` and `sp` as `reg=value`, and each
/// memory access outside the entry stack frame as `[address]`, a store with
/// `<- value`. Values are hexadecimal; an alternative set is `one-of{..}`.
/// Unknown values and expressions are omitted, so an annotation is never a
/// claim the records do not make.
pub fn listing(records: &[FunctionRecord]) -> Vec<String> {
    let mut symbols: BTreeMap<u64, Vec<String>> = BTreeMap::new();
    let mut names: BTreeMap<&SymbolId, String> = BTreeMap::new();
    for record in records {
        if let FunctionRecord::Reference { raw, target, .. } = record {
            let name = String::from_utf8_lossy(&raw.target.name).into_owned();
            names.insert(&target.symbol, name.clone());
            let name = match raw.addend {
                Some(addend) if addend != 0 => format!("{name}{addend:+}"),
                _ => name,
            };
            symbols.entry(raw.offset).or_default().push(name);
        }
    }
    let mut notes: BTreeMap<u64, Vec<String>> = BTreeMap::new();
    for record in records {
        match record {
            FunctionRecord::Value {
                offset,
                register,
                value,
                ..
            } if ![ZERO, RA, SP].contains(register) => {
                if let Some(value) = render(value, &names) {
                    notes.entry(*offset).or_default().push(format!(
                        "{}={value}",
                        oer_riscv_lift::register_name(*register)
                    ));
                }
            }
            FunctionRecord::MemoryAccess {
                offset,
                access,
                address,
                value,
                ..
            } if !matches!(address, AbstractValue::EntryStack { .. }) => {
                if let Some(address) = render(address, &names) {
                    let stored = match access {
                        MemoryKind::Store | MemoryKind::StoreConditional => value
                            .as_ref()
                            .and_then(|value| render(value, &names))
                            .map(|value| format!(" <- {value}")),
                        _ => None,
                    };
                    notes
                        .entry(*offset)
                        .or_default()
                        .push(format!("[{address}]{}", stored.unwrap_or_default()));
                }
            }
            _ => {}
        }
    }
    records
        .iter()
        .filter_map(|record| match record {
            FunctionRecord::Instruction {
                offset, decoded, ..
            } => {
                let symbols = symbols.get(offset).map(|names| names.join(" "));
                let notes = notes.get(offset).map(|notes| notes.join(" "));
                if symbols.is_none() && notes.is_none() {
                    return Some(format!("{offset:6x}  {}", decoded.text));
                }
                let mut line = format!("{offset:6x}  {:<40}", decoded.text);
                if let Some(symbols) = symbols {
                    line = format!("{line} {symbols}");
                }
                if let Some(notes) = notes {
                    line = format!("{line}  # {notes}");
                }
                Some(line)
            }
            _ => None,
        })
        .collect()
}

/// The exact hexadecimal spelling of `value`, or `None` when the records
/// retain no exact value.
fn render(value: &AbstractValue, names: &BTreeMap<&SymbolId, String>) -> Option<String> {
    match value {
        AbstractValue::Unknown | AbstractValue::Expression { .. } => None,
        AbstractValue::Alternatives { values } => {
            let values: Vec<String> = values
                .values()
                .iter()
                .map(|value| alternative(value, names))
                .collect();
            Some(format!("one-of{{{}}}", values.join(" | ")))
        }
        _ => ValueAlternative::from_value(value).map(|value| alternative(&value, names)),
    }
}

fn alternative(value: &ValueAlternative, names: &BTreeMap<&SymbolId, String>) -> String {
    match value {
        ValueAlternative::Constant { value } => format!("{value:#x}"),
        ValueAlternative::ImageAddress { address } => format!("{address:#x}"),
        ValueAlternative::Section { section, offset } => {
            format!("section{section}{}", signed_hex(*offset))
        }
        ValueAlternative::Symbol { symbol, addend } => {
            let name = names
                .get(symbol)
                .cloned()
                .unwrap_or_else(|| format!("symbol{}", symbol.index));
            format!("{name}{}", signed_hex(*addend))
        }
        ValueAlternative::EntryStack { offset } => format!("sp{}", signed_hex(*offset)),
    }
}

/// `+0x10`, `-0x8`, or nothing for zero.
fn signed_hex(value: i64) -> String {
    match value {
        0 => String::new(),
        v if v < 0 => format!("-{:#x}", v.unsigned_abs()),
        v => format!("+{v:#x}"),
    }
}

/// One reference a function makes to a named symbol.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SymbolReference {
    /// The referencing instruction's offset in its function.
    pub offset: u64,
    pub kind: ReferenceKind,
    #[serde(with = "oer_riscv_model::symbol_name")]
    pub target: Vec<u8>,
}

/// The references among `records` to a symbol `targets` names, in offset
/// order. A reference is a relocation: every call or address of another
/// symbol in a relocatable object carries one.
pub fn references_to(records: &[FunctionRecord], targets: &[String]) -> Vec<SymbolReference> {
    let mut found: Vec<_> = records
        .iter()
        .filter_map(|record| match record {
            FunctionRecord::Reference {
                raw,
                reference_kind,
                ..
            } if targets
                .iter()
                .any(|target| target.as_bytes() == raw.target.name.as_slice()) =>
            {
                Some(SymbolReference {
                    offset: raw.offset,
                    kind: *reference_kind,
                    target: raw.target.name.clone(),
                })
            }
            _ => None,
        })
        .collect();
    found.sort_by_key(|reference| reference.offset);
    found.dedup();
    found
}

#[cfg(test)]
mod tests;
