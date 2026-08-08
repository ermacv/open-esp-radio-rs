//! Strict validation and summary loading for a stored navigation index.

use std::{collections::BTreeSet, fs, path::Path};

use super::{
    Result,
    model::{IDENTITY_SCHEME, SCHEMA_VERSION, SymbolKey, address},
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
    let root: serde_json::Value = serde_json::from_str(&fs::read_to_string(path)?)?;
    validate_header(&root)?;
    validate_inputs(&root)?;
    let symbol_values = validate_symbols(&root)?;
    let summary = root
        .get("summary")
        .and_then(serde_json::Value::as_object)
        .ok_or("navigation index has no summary object")?;
    let count = |name: &str| -> Result<usize> {
        summary
            .get(name)
            .and_then(serde_json::Value::as_u64)
            .and_then(|value| usize::try_from(value).ok())
            .ok_or_else(|| format!("navigation index summary has invalid {name:?}").into())
    };
    let stored = StoredNavigationSummary {
        artifacts: count("artifacts")?,
        symbols: count("symbols")?,
        linked_ir_functions: count("linked_ir_functions")?,
        interface_callers: count("interface_callers")?,
        interface_roots: count("interface_roots")?,
        unmatched_interface_roots: count("unmatched_interface_roots")?,
    };
    if stored.symbols != symbol_values.len() {
        return Err("navigation index summary symbol count does not match its symbol array".into());
    }
    Ok(stored)
}

fn validate_header(root: &serde_json::Value) -> Result<()> {
    if root
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
        != Some(u64::from(SCHEMA_VERSION))
        || root.get("command").and_then(serde_json::Value::as_str) != Some("project navigation")
        || root
            .get("identity_scheme")
            .and_then(serde_json::Value::as_str)
            != Some(IDENTITY_SCHEME)
    {
        return Err("navigation index requires project navigation schema_version 1".into());
    }
    if root
        .get("semantic_claim")
        .and_then(serde_json::Value::as_bool)
        != Some(false)
        || root
            .get("linker_resolution_claim")
            .and_then(serde_json::Value::as_bool)
            != Some(false)
    {
        return Err("navigation index must not claim semantics or linker resolution".into());
    }
    Ok(())
}

fn validate_inputs(root: &serde_json::Value) -> Result<()> {
    let inputs = root
        .get("inputs")
        .and_then(serde_json::Value::as_array)
        .ok_or("navigation index has no inputs array")?;
    for (index, input) in inputs.iter().enumerate() {
        let input = input
            .as_object()
            .ok_or_else(|| format!("navigation input {index} is not an object"))?;
        let input_path = input
            .get("path")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| format!("navigation input {index} has no path"))?;
        let expected = input
            .get("sha256")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| format!("navigation input {index} has no sha256"))?;
        let actual = artifact_sha256(Path::new(input_path)).map_err(|error| {
            format!("cannot authenticate navigation input {input_path}: {error}")
        })?;
        if actual != expected {
            return Err(format!("navigation input changed since indexing: {input_path}").into());
        }
    }
    Ok(())
}

fn validate_symbols(root: &serde_json::Value) -> Result<&Vec<serde_json::Value>> {
    let symbols = root
        .get("symbols")
        .and_then(serde_json::Value::as_array)
        .ok_or("navigation index has no symbols array")?;
    let mut ids = BTreeSet::new();
    for (index, value) in symbols.iter().enumerate() {
        let value = value
            .as_object()
            .ok_or_else(|| format!("navigation symbol {index} is not an object"))?;
        let string = |name: &str| -> Result<&str> {
            value
                .get(name)
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| format!("navigation symbol {index} has invalid {name:?}").into())
        };
        let member = match value.get("member") {
            None | Some(serde_json::Value::Null) => None,
            Some(value) => Some(
                value
                    .as_str()
                    .ok_or_else(|| format!("navigation symbol {index} has invalid member"))?
                    .to_owned(),
            ),
        };
        let key = SymbolKey {
            artifact_sha256: string("artifact_sha256")?.to_owned(),
            member,
            name: string("name")?.to_owned(),
            object_address: address(string("object_address")?, "navigation symbol")?,
        };
        let id = string("id")?;
        if id != key.id() {
            return Err(format!("navigation symbol {index} has an invalid stable id").into());
        }
        if !ids.insert(id) {
            return Err(format!("duplicate navigation symbol id {id:?}").into());
        }
    }
    Ok(symbols)
}
