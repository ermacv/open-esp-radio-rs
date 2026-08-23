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
        let (freshness_status, checked_source_files, stale_source_files) =
            artifact_freshness(&path, &symbols);
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
            checked_source_files,
            freshness_status,
            stale_source_files,
        });
    }
    compiled.sort();
    compiled.dedup();
    Ok((artifacts, compiled, diagnostics))
}

fn artifact_freshness(
    artifact: &std::path::Path,
    symbols: &[crate::artifact::ArtifactDebugSymbol],
) -> (&'static str, usize, Vec<String>) {
    let Ok(artifact_time) = std::fs::metadata(artifact).and_then(|metadata| metadata.modified())
    else {
        return ("unknown", 0, Vec::new());
    };
    let source_files = symbols
        .iter()
        .filter_map(|symbol| symbol.source_file.as_deref())
        .map(std::path::PathBuf::from)
        .collect::<BTreeSet<_>>();
    let mut checked = 0;
    let mut stale = Vec::new();
    for source in source_files {
        let Ok(source_time) = std::fs::metadata(&source).and_then(|metadata| metadata.modified())
        else {
            continue;
        };
        checked += 1;
        if source_time > artifact_time {
            stale.push(source.display().to_string());
        }
    }
    let status = if !stale.is_empty() {
        "stale"
    } else if checked != 0 {
        "fresh"
    } else {
        "unknown"
    };
    (status, checked, stale)
}

pub(crate) fn compiled_matches(component: &str, demangled: &str) -> bool {
    if demangled == component
        || demangled
            .strip_prefix(component)
            .is_some_and(|suffix| suffix.starts_with("::") || suffix.starts_with('<'))
    {
        return true;
    }
    if root_reexported_impl_method_matches(component, demangled) {
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

/// Match a method whose source owner lives in a module but whose receiver is
/// re-exported from the crate root.
///
/// Rust source ownership uses `crate::module::Type::method`, while DWARF names
/// the same compiled frame `<crate::Type>::method` when `Type` is defined at or
/// re-exported through the crate root. Keep this fallback deliberately narrow:
/// only the module path may disappear, and the crate, receiver and method must
/// still match exactly.
fn root_reexported_impl_method_matches(component: &str, demangled: &str) -> bool {
    let segments = component.split("::").collect::<Vec<_>>();
    let [crate_name, .., receiver, method] = segments.as_slice() else {
        return false;
    };
    if segments.len() < 4 {
        return false;
    }
    let Some((compiled_receiver, compiled_method)) = demangled
        .strip_prefix('<')
        .and_then(|name| name.split_once(">::"))
    else {
        return false;
    };
    compiled_receiver == format!("{crate_name}::{receiver}")
        && (compiled_method == *method
            || compiled_method
                .strip_prefix(method)
                .is_some_and(|suffix| suffix.starts_with("::") || suffix.starts_with('<')))
}

const fn rust_identifier_byte(byte: u8) -> bool {
    byte == b'_' || byte.is_ascii_alphanumeric()
}
