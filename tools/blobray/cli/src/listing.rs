//! Readable views of one function's records: its instruction listing with
//! the symbols its relocations name, and the references it makes to named
//! symbols.
use oer_riscv_model::{FunctionRecord, ReferenceKind};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// One instruction line: the function offset, the decoded text and the
/// symbols the relocations at that offset name (`name+addend`).
pub fn listing(records: &[FunctionRecord]) -> Vec<String> {
    let mut symbols: BTreeMap<u64, Vec<String>> = BTreeMap::new();
    for record in records {
        if let FunctionRecord::Reference { raw, .. } = record {
            let name = String::from_utf8_lossy(&raw.target.name).into_owned();
            let name = match raw.addend {
                Some(addend) if addend != 0 => format!("{name}{addend:+}"),
                _ => name,
            };
            symbols.entry(raw.offset).or_default().push(name);
        }
    }
    records
        .iter()
        .filter_map(|record| match record {
            FunctionRecord::Instruction {
                offset, decoded, ..
            } => {
                let names = symbols.get(offset).map(|names| names.join(" "));
                Some(match names {
                    Some(names) => format!("{offset:6x}  {:<40} {names}", decoded.text),
                    None => format!("{offset:6x}  {}", decoded.text),
                })
            }
            _ => None,
        })
        .collect()
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
