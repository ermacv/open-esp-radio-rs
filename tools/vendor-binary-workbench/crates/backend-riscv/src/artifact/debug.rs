//! Rust symbol and optional DWARF source enrichment for linked ELF artifacts.
//!
//! These facts are navigation evidence. They never participate in instruction
//! semantics, linker selection, or verification trace equivalence.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use object::{Object as _, ObjectSymbol as _};

use crate::Result;

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ArtifactDebugSymbol {
    pub raw_name: String,
    pub demangled_name: String,
    pub address: u64,
    pub size: u64,
    pub source_file: Option<String>,
    pub source_line: Option<u32>,
    pub source_column: Option<u32>,
}

pub fn inspect_rust_debug_symbols(path: &Path) -> Result<Vec<ArtifactDebugSymbol>> {
    let bytes = std::fs::read(path)?;
    let object = object::File::parse(bytes.as_slice())?;
    let loader = addr2line::Loader::new(path).ok();
    let text_symbols = object
        .symbols()
        .chain(object.dynamic_symbols())
        .filter(|symbol| {
            symbol.is_definition()
                && symbol.kind() == object::SymbolKind::Text
                && symbol.address() != 0
        })
        .filter_map(|symbol| {
            Some((
                symbol.name().ok()?.to_owned(),
                symbol.address(),
                symbol.size(),
            ))
        })
        .collect::<Vec<_>>();
    let mut symbols =
        BTreeMap::<(String, Option<String>, Option<u32>, Option<u32>), ArtifactDebugSymbol>::new();
    for (raw_name, address, size) in &text_symbols {
        let Some(demangled_name) = demangle_rust_symbol(raw_name) else {
            continue;
        };
        let location = loader
            .as_ref()
            .and_then(|loader| loader.find_location(*address).ok().flatten());
        insert_symbol(
            &mut symbols,
            ArtifactDebugSymbol {
                raw_name: raw_name.clone(),
                demangled_name,
                address: *address,
                size: *size,
                source_file: location
                    .as_ref()
                    .and_then(|location| location.file)
                    .map(str::to_owned),
                source_line: location.as_ref().and_then(|location| location.line),
                source_column: location.as_ref().and_then(|location| location.column),
            },
        );
    }
    if let Some(loader) = &loader {
        let scan_ranges = text_symbols
            .iter()
            .map(|(_, address, size)| (*address, *size))
            .collect::<BTreeSet<_>>();
        for (address, size) in scan_ranges {
            let end = address.saturating_add(size.max(2));
            for probe in (address..end).step_by(2) {
                let Ok(mut frames) = loader.find_frames(probe) else {
                    continue;
                };
                while let Ok(Some(frame)) = frames.next() {
                    let Some(function) = frame.function else {
                        continue;
                    };
                    let Ok(demangled_name) = function.demangle() else {
                        continue;
                    };
                    if !demangled_name.contains("::") {
                        continue;
                    }
                    let raw_name = function
                        .raw_name()
                        .map_or_else(|_| "<dwarf>".to_owned(), |name| name.into_owned());
                    let location = frame.location;
                    insert_symbol(
                        &mut symbols,
                        ArtifactDebugSymbol {
                            raw_name,
                            demangled_name: demangled_name.into_owned(),
                            address: probe,
                            size,
                            source_file: location
                                .as_ref()
                                .and_then(|location| location.file)
                                .map(str::to_owned),
                            source_line: location.as_ref().and_then(|location| location.line),
                            source_column: location.as_ref().and_then(|location| location.column),
                        },
                    );
                }
            }
        }
    }
    Ok(symbols.into_values().collect())
}

fn demangle_rust_symbol(raw_name: &str) -> Option<String> {
    if !raw_name.starts_with("_R") && !raw_name.starts_with("_ZN") {
        return None;
    }
    let demangled = rustc_demangle::demangle(raw_name);
    Some(format!("{demangled:#}"))
}

fn insert_symbol(
    symbols: &mut BTreeMap<(String, Option<String>, Option<u32>, Option<u32>), ArtifactDebugSymbol>,
    symbol: ArtifactDebugSymbol,
) {
    let key = (
        symbol.demangled_name.clone(),
        symbol.source_file.clone(),
        symbol.source_line,
        symbol.source_column,
    );
    match symbols.get_mut(&key) {
        Some(existing) if symbol.address < existing.address => *existing = symbol,
        Some(_) => {}
        None => {
            symbols.insert(key, symbol);
        }
    }
}
