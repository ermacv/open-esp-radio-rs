//! Strict validation and summary loading for a stored navigation index.

use std::{collections::BTreeSet, fs, path::Path};

use super::model::{
    IDENTITY_SCHEME, NavigationDocument, SCHEMA_VERSION, SymbolDocument, SymbolKey, address,
};
use crate::{Result, artifact_path_sha256};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct StoredNavigationSummary {
    pub(crate) artifacts: usize,
    pub(crate) symbols: usize,
    pub(crate) linked_ir_functions: usize,
    pub(crate) interface_callers: usize,
    pub(crate) interface_roots: usize,
    pub(crate) unmatched_interface_roots: usize,
    pub(crate) project_call_links: usize,
    pub(crate) unique_project_calls: usize,
    pub(crate) ambiguous_project_calls: usize,
    pub(crate) unresolved_project_calls: usize,
}

pub(crate) fn inspect_report(path: &Path) -> Result<StoredNavigationSummary> {
    let document: NavigationDocument = serde_json::from_str(&fs::read_to_string(path)?)?;
    validate_header(&document)?;
    validate_inputs(&document, path.parent().unwrap_or_else(|| Path::new(".")))?;
    let artifact_ids = validate_artifacts(&document)?;
    validate_symbols(&document.symbols, &artifact_ids)?;
    validate_project_calls(&document)?;
    validate_summary(&document)?;
    Ok(StoredNavigationSummary {
        artifacts: document.summary.artifacts,
        symbols: document.summary.symbols,
        linked_ir_functions: document.summary.linked_ir_functions,
        interface_callers: document.summary.interface_callers,
        interface_roots: document.summary.interface_roots,
        unmatched_interface_roots: document.summary.unmatched_interface_roots,
        project_call_links: document.summary.project_call_links,
        unique_project_calls: document.summary.unique_project_calls,
        ambiguous_project_calls: document.summary.ambiguous_project_calls,
        unresolved_project_calls: document.summary.unresolved_project_calls,
    })
}

fn validate_project_calls(document: &NavigationDocument) -> Result<()> {
    let identities = document
        .symbols
        .iter()
        .flat_map(|symbol| symbol.linked_ir.iter().map(|item| item.identity.as_str()))
        .collect::<BTreeSet<_>>();
    for call in &document.project_calls {
        if call.caller.is_empty()
            || call.symbol.is_empty()
            || call.linker_resolution_claim
            || !identities.contains(call.caller.as_str())
            || call
                .candidates
                .iter()
                .any(|candidate| !identities.contains(candidate.as_str()))
        {
            return Err(crate::Error::invalid(
                "navigation project call has invalid identity or claim",
            ));
        }
        let expected = match call.candidates.len() {
            0 => "unresolved",
            1 => "unique",
            _ => "ambiguous",
        };
        if call.status != expected {
            return Err(crate::Error::invalid(
                "navigation project call status does not match its candidates",
            ));
        }
    }
    Ok(())
}

fn validate_header(document: &NavigationDocument) -> Result<()> {
    if document.schema_version != SCHEMA_VERSION
        || document.command != "project navigation"
        || document.identity_scheme != IDENTITY_SCHEME
    {
        return Err(crate::Error::invalid(format!(
            "navigation index requires project navigation schema_version {SCHEMA_VERSION}"
        )));
    }
    if document.semantic_claim || document.linker_resolution_claim {
        return Err(crate::Error::invalid(
            "navigation index must not claim semantics or linker resolution",
        ));
    }
    Ok(())
}

fn validate_inputs(document: &NavigationDocument, base: &Path) -> Result<()> {
    let mut identities = BTreeSet::new();
    for input in &document.inputs {
        if input.kind.is_empty() || input.id.is_empty() || input.path.is_empty() {
            return Err(crate::Error::invalid(
                "navigation input kind, id and path must be non-empty",
            ));
        }
        if !identities.insert((input.kind.as_str(), input.id.as_str())) {
            return Err(crate::Error::invalid(format!(
                "duplicate navigation input identity {}:{}",
                input.kind, input.id
            )));
        }
        let input_path = Path::new(&input.path);
        if input_path.is_absolute() {
            return Err(crate::Error::invalid(format!(
                "navigation input path must be relative to the index: {}",
                input.path
            )));
        }
        let resolved = base.join(input_path);
        let actual = artifact_path_sha256(&resolved)
            .map_err(|error| {
                format!(
                    "cannot authenticate navigation input {}: {error}",
                    input.path
                )
            })
            .map_err(crate::Error::invalid)?;
        if actual != input.sha256 {
            return Err(crate::Error::invalid(format!(
                "navigation input changed since indexing: {}",
                input.path
            )));
        }
    }
    Ok(())
}

fn validate_artifacts(document: &NavigationDocument) -> Result<BTreeSet<&str>> {
    let mut ids = BTreeSet::new();
    for artifact in &document.artifacts {
        if artifact.sha256.is_empty() || artifact.paths.is_empty() {
            return Err(crate::Error::invalid(
                "navigation artifact requires sha256 and at least one path",
            ));
        }
        if !ids.insert(artifact.sha256.as_str()) {
            return Err(crate::Error::invalid(format!(
                "duplicate navigation artifact {}",
                artifact.sha256
            )));
        }
    }
    Ok(ids)
}

fn validate_symbols(symbols: &[SymbolDocument], artifact_ids: &BTreeSet<&str>) -> Result<()> {
    let mut ids = BTreeSet::new();
    for (index, symbol) in symbols.iter().enumerate() {
        if !artifact_ids.contains(symbol.artifact_sha256.as_str()) {
            return Err(crate::Error::invalid(format!(
                "navigation symbol {index} refers to unknown artifact {}",
                symbol.artifact_sha256
            )));
        }
        let key = SymbolKey {
            artifact_sha256: symbol.artifact_sha256.clone(),
            member: symbol.member.clone(),
            name: symbol.name.clone(),
            object_address: address(&symbol.object_address, "navigation symbol")?,
        };
        if symbol.id != key.id() {
            return Err(crate::Error::invalid(format!(
                "navigation symbol {index} has an invalid stable id"
            )));
        }
        if !ids.insert(symbol.id.as_str()) {
            return Err(crate::Error::invalid(format!(
                "duplicate navigation symbol id {:?}",
                symbol.id
            )));
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
    let actual_unique = document
        .project_calls
        .iter()
        .filter(|call| call.status == "unique")
        .count();
    let actual_ambiguous = document
        .project_calls
        .iter()
        .filter(|call| call.status == "ambiguous")
        .count();
    let actual_unresolved = document
        .project_calls
        .iter()
        .filter(|call| call.status == "unresolved")
        .count();
    if expected.artifacts != document.artifacts.len()
        || expected.symbols != document.symbols.len()
        || expected.inventory_symbols != actual_inventory
        || expected.linked_ir_functions != actual_ir
        || expected.interface_callers != actual_callers
        || expected.interface_roots != actual_roots
        || expected.project_call_links != document.project_calls.len()
        || expected.unique_project_calls != actual_unique
        || expected.ambiguous_project_calls != actual_ambiguous
        || expected.unresolved_project_calls != actual_unresolved
    {
        return Err(crate::Error::invalid(
            "navigation index summary does not match its typed contents",
        ));
    }
    Ok(())
}
