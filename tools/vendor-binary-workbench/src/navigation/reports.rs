//! Minimal typed projections of the independently versioned input reports.

use std::{fs, path::Path};

use serde::Deserialize;

use crate::Result;

#[derive(Deserialize)]
pub(super) struct ArtifactIdentity {
    pub(super) path: String,
    pub(super) sha256: String,
}

#[derive(Deserialize)]
pub(super) struct InventoryArtifact {
    pub(super) index: usize,
    pub(super) artifact: ArtifactIdentity,
    pub(super) sources: Vec<String>,
}

#[derive(Deserialize)]
pub(super) struct InventorySymbol {
    pub(super) artifact: usize,
    pub(super) member: Option<String>,
    pub(super) name: String,
    pub(super) address: String,
    pub(super) table: String,
    pub(super) definition: String,
    pub(super) kind: String,
    pub(super) resolution: String,
}

#[derive(Deserialize)]
pub(super) struct InventoryReport {
    pub(super) schema_version: u32,
    pub(super) command: String,
    pub(super) artifacts: Vec<InventoryArtifact>,
    pub(super) symbols: Vec<InventorySymbol>,
}

#[derive(Deserialize)]
pub(super) struct IrArtifact {
    pub(super) source: String,
    pub(super) artifact: ArtifactIdentity,
}

#[derive(Deserialize)]
pub(super) struct IrFunction {
    pub(super) source: String,
    pub(super) identity: String,
    pub(super) selection: String,
    pub(super) member: Option<String>,
    pub(super) symbol: String,
    pub(super) object_offset: u32,
}

#[derive(Deserialize)]
pub(super) struct IrReport {
    pub(super) schema_version: u32,
    pub(super) command: String,
    pub(super) artifacts: Vec<IrArtifact>,
    pub(super) functions: Vec<IrFunction>,
}

#[derive(Deserialize)]
pub(super) struct InterfaceArtifact {
    pub(super) index: usize,
    pub(super) path: String,
    pub(super) sources: Vec<String>,
    pub(super) sha256: String,
}

#[derive(Deserialize)]
pub(super) struct InterfaceRoot {
    pub(super) kind: String,
    pub(super) member: Option<String>,
    pub(super) symbol: Option<String>,
    pub(super) address: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct InterfaceTarget {
    pub(super) root: InterfaceRoot,
}

#[derive(Deserialize)]
pub(super) struct InterfaceCall {
    pub(super) artifact: usize,
    pub(super) member: Option<String>,
    pub(super) function: String,
    pub(super) function_address: String,
    pub(super) site: String,
    pub(super) kind: String,
    pub(super) target: InterfaceTarget,
}

#[derive(Deserialize)]
pub(super) struct InterfaceReport {
    pub(super) schema_version: u32,
    pub(super) command: String,
    pub(super) artifacts: Vec<InterfaceArtifact>,
    pub(super) calls: Vec<InterfaceCall>,
}

pub(super) fn read<T: for<'de> Deserialize<'de>>(path: &Path, kind: &str) -> Result<T> {
    let input = fs::read_to_string(path)
        .map_err(|error| format!("cannot read {kind} {}: {error}", path.display()))
        .map_err(crate::Error::invalid)?;
    serde_json::from_str(&input).map_err(|error| {
        crate::Error::invalid(format!("cannot parse {kind} {}: {error}", path.display()))
    })
}
