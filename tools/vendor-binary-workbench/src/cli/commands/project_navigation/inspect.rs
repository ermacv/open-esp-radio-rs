//! Strict validation and summary loading for a stored navigation index.

use std::{collections::BTreeSet, fs, path::Path};

use super::{
    Result,
    model::{
        IDENTITY_SCHEME, NavigationDocument, SCHEMA_VERSION, SymbolDocument, SymbolKey, address,
    },
};
use crate::artifact_sha256;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct StoredNavigationSummary {
    pub(crate) artifacts: usize,
    pub(crate) symbols: usize,
    pub(crate) linked_ir_functions: usize,
    pub(crate) interface_callers: usize,
    pub(crate) interface_roots: usize,
    pub(crate) unmatched_interface_roots: usize,
}

pub(crate) fn inspect_report(path: &Path) -> Result<StoredNavigationSummary> {
    let document: NavigationDocument = serde_json::from_str(&fs::read_to_string(path)?)?;
    validate_header(&document)?;
    validate_inputs(&document)?;
    let artifact_ids = validate_artifacts(&document)?;
    validate_symbols(&document.symbols, &artifact_ids)?;
    validate_summary(&document)?;
    Ok(StoredNavigationSummary {
        artifacts: document.summary.artifacts,
        symbols: document.summary.symbols,
        linked_ir_functions: document.summary.linked_ir_functions,
        interface_callers: document.summary.interface_callers,
        interface_roots: document.summary.interface_roots,
        unmatched_interface_roots: document.summary.unmatched_interface_roots,
    })
}

fn validate_header(document: &NavigationDocument) -> Result<()> {
    if document.schema_version != SCHEMA_VERSION
        || document.command != "project navigation"
        || document.identity_scheme != IDENTITY_SCHEME
    {
        return Err("navigation index requires project navigation schema_version 1".into());
    }
    if document.semantic_claim || document.linker_resolution_claim {
        return Err("navigation index must not claim semantics or linker resolution".into());
    }
    Ok(())
}

fn validate_inputs(document: &NavigationDocument) -> Result<()> {
    let mut identities = BTreeSet::new();
    for input in &document.inputs {
        if input.kind.is_empty() || input.id.is_empty() || input.path.is_empty() {
            return Err("navigation input kind, id and path must be non-empty".into());
        }
        if !identities.insert((input.kind.as_str(), input.id.as_str())) {
            return Err(format!(
                "duplicate navigation input identity {}:{}",
                input.kind, input.id
            )
            .into());
        }
        let actual = artifact_sha256(Path::new(&input.path)).map_err(|error| {
            format!(
                "cannot authenticate navigation input {}: {error}",
                input.path
            )
        })?;
        if actual != input.sha256 {
            return Err(format!("navigation input changed since indexing: {}", input.path).into());
        }
    }
    Ok(())
}

fn validate_artifacts(document: &NavigationDocument) -> Result<BTreeSet<&str>> {
    let mut ids = BTreeSet::new();
    for artifact in &document.artifacts {
        if artifact.sha256.is_empty() || artifact.paths.is_empty() {
            return Err("navigation artifact requires sha256 and at least one path".into());
        }
        if !ids.insert(artifact.sha256.as_str()) {
            return Err(format!("duplicate navigation artifact {}", artifact.sha256).into());
        }
    }
    Ok(ids)
}

fn validate_symbols(symbols: &[SymbolDocument], artifact_ids: &BTreeSet<&str>) -> Result<()> {
    let mut ids = BTreeSet::new();
    for (index, symbol) in symbols.iter().enumerate() {
        if !artifact_ids.contains(symbol.artifact_sha256.as_str()) {
            return Err(format!(
                "navigation symbol {index} refers to unknown artifact {}",
                symbol.artifact_sha256
            )
            .into());
        }
        let key = SymbolKey {
            artifact_sha256: symbol.artifact_sha256.clone(),
            member: symbol.member.clone(),
            name: symbol.name.clone(),
            object_address: address(&symbol.object_address, "navigation symbol")?,
        };
        if symbol.id != key.id() {
            return Err(format!("navigation symbol {index} has an invalid stable id").into());
        }
        if !ids.insert(symbol.id.as_str()) {
            return Err(format!("duplicate navigation symbol id {:?}", symbol.id).into());
        }
    }
    Ok(())
}

fn validate_summary(document: &NavigationDocument) -> Result<()> {
    let expected = &document.summary;
    let actual_inventory = document
        .symbols
        .iter()
        .filter(|symbol| !symbol.inventory.is_empty())
        .count();
    let actual_ir = document
        .symbols
        .iter()
        .filter(|symbol| !symbol.linked_ir.is_empty())
        .count();
    let actual_callers = document
        .symbols
        .iter()
        .filter(|symbol| !symbol.interface_calls.is_empty())
        .count();
    let actual_roots = document
        .symbols
        .iter()
        .filter(|symbol| !symbol.interface_roots.is_empty())
        .count();
    if expected.artifacts != document.artifacts.len()
        || expected.symbols != document.symbols.len()
        || expected.inventory_symbols != actual_inventory
        || expected.linked_ir_functions != actual_ir
        || expected.interface_callers != actual_callers
        || expected.interface_roots != actual_roots
    {
        return Err("navigation index summary does not match its typed contents".into());
    }
    Ok(())
}
