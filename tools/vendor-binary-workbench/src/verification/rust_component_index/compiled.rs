use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

use super::{RustArtifactInput, RustCompiledSymbol, RustComponentArtifact};
use crate::{Result, artifact, artifact_sha256};

pub(super) fn compiled_symbols(
    inputs: &[RustArtifactInput],
) -> Result<(
    Vec<RustComponentArtifact>,
    Vec<RustCompiledSymbol>,
    Vec<String>,
)> {
    let mut grouped = BTreeMap::<PathBuf, BTreeSet<String>>::new();
    for input in inputs {
        grouped
            .entry(input.path.clone())
            .or_default()
            .insert(input.suite.clone());
    }
    let mut artifacts = Vec::new();
    let mut compiled = Vec::new();
    let diagnostics = Vec::new();
    for (path, suites) in grouped {
        let symbols = artifact::inspect_rust_debug_symbols(&path)?;
        let artifact_path = path.display().to_string();
        let dwarf_locations = symbols
            .iter()
            .filter(|symbol| symbol.source_file.is_some())
            .count();
        compiled.extend(symbols.iter().map(|symbol| RustCompiledSymbol {
            artifact: artifact_path.clone(),
            demangled: symbol.demangled_name.clone(),
            address: format!("{:#x}", symbol.address),
            size: symbol.size,
            source_file: symbol.source_file.clone(),
            source_line: symbol.source_line,
            source_column: symbol.source_column,
        }));
        artifacts.push(RustComponentArtifact {
            path: artifact_path,
            sha256: artifact_sha256(&path)?,
            suites: suites.into_iter().collect(),
            rust_symbols: symbols.len(),
            dwarf_locations,
        });
    }
    compiled.sort();
    compiled.dedup();
    Ok((artifacts, compiled, diagnostics))
}

pub(super) fn compiled_matches(component: &str, demangled: &str) -> bool {
    if demangled == component
        || demangled
            .strip_prefix(component)
            .is_some_and(|suffix| suffix.starts_with("::") || suffix.starts_with('<'))
    {
        return true;
    }
    let mut cursor = 0usize;
    for segment in component.split("::") {
        let Some(relative) = demangled[cursor..].find(segment) else {
            return false;
        };
        let start = cursor + relative;
        let end = start + segment.len();
        let left = start == 0 || !rust_identifier_byte(demangled.as_bytes()[start - 1]);
        let right = end == demangled.len() || !rust_identifier_byte(demangled.as_bytes()[end]);
        if !left || !right {
            return false;
        }
        cursor = end;
    }
    true
}

const fn rust_identifier_byte(byte: u8) -> bool {
    byte == b'_' || byte.is_ascii_alphanumeric()
}
