//! Artifact selection and user-facing trace extraction helpers.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use serde::Serialize;

use super::{ReferenceResolver, StructuralPointerContext, trace_binary_symbol};
use crate::{ArtifactSymbolIdentity, FunctionAnalysis, MmioMap, ObservableEvent, Result, artifact};

pub(crate) fn list_code_symbols(
    artifact: &Path,
    prefix: &str,
) -> Result<Vec<ArtifactSymbolIdentity>> {
    Ok(
        artifact::load_code_symbols(artifact, prefix, artifact::CodeSymbolSelection::Exported)?
            .into_iter()
            .map(|symbol| ArtifactSymbolIdentity {
                member: symbol.member,
                name: symbol.name,
            })
            .collect(),
    )
}

#[derive(Clone, Debug)]
pub(crate) struct ArtifactSymbolSelector {
    pub(crate) artifact: PathBuf,
    pub(crate) member: Option<String>,
    pub(crate) symbol: String,
}

#[derive(Serialize)]
pub(crate) struct TraceEventDocument {
    index: usize,
    canonical: String,
}

#[derive(Serialize)]
pub(crate) struct TraceDocument<'a> {
    schema_version: u32,
    symbol: &'a str,
    exact: bool,
    reference_codegen_eligible: bool,
    return_value: String,
    unresolved_branch: bool,
    events: Vec<TraceEventDocument>,
    blockers: &'a [String],
    reference_blockers: &'a [String],
    unmapped_mmio: Vec<u32>,
}

pub(crate) fn trace_document(trace: &FunctionAnalysis) -> TraceDocument<'_> {
    TraceDocument {
        schema_version: 1,
        symbol: &trace.symbol,
        exact: trace.is_exact(),
        reference_codegen_eligible: trace.is_reference_eligible(),
        return_value: trace.return_value.canonical(),
        unresolved_branch: trace.unresolved_branch.is_some(),
        events: trace
            .events
            .iter()
            .enumerate()
            .map(|(index, event)| TraceEventDocument {
                index,
                canonical: event.canonical(),
            })
            .collect(),
        blockers: &trace.blockers,
        reference_blockers: &trace.reference_blockers,
        unmapped_mmio: trace
            .events
            .iter()
            .filter_map(ObservableEvent::unmapped_address)
            .collect(),
    }
}

pub(crate) fn extract(input: &ArtifactSymbolSelector, svd: &MmioMap) -> Result<FunctionAnalysis> {
    let symbols = artifact::load_code_symbols(
        &input.artifact,
        &input.symbol,
        artifact::CodeSymbolSelection::Exported,
    )?;
    let symbol = symbols
        .iter()
        .find(|candidate| {
            candidate.name == input.symbol
                && input
                    .member
                    .as_deref()
                    .is_none_or(|member| candidate.member.as_deref() == Some(member))
        })
        .ok_or_else(|| {
            format!(
                "symbol {} in member {:?} was not found",
                input.symbol, input.member
            )
        })
        .map_err(crate::Error::invalid)?;
    Ok(trace_binary_symbol(
        symbol,
        svd,
        &BTreeMap::new(),
        &StructuralPointerContext::default(),
        None,
    )?)
}

pub(crate) fn extract_reference(
    input: &ArtifactSymbolSelector,
    companions: &[PathBuf],
    harness: &'static crate::RiscvHarnessSpec,
    entry_contract: crate::EntryContractRef,
    svd: &MmioMap,
) -> Result<FunctionAnalysis> {
    Ok(ReferenceResolver::load_with_entry_contract(
        &input.artifact,
        companions,
        harness,
        entry_contract,
    )?
    .trace(input.member.as_deref(), &input.symbol, svd)?)
}

pub(crate) fn returns_equal(left: &FunctionAnalysis, right: &FunctionAnalysis) -> bool {
    left.return_value.is_resolved()
        && right.return_value.is_resolved()
        && left.return_value.canonical() == right.return_value.canonical()
}

pub(crate) fn traces_equal(left: &FunctionAnalysis, right: &FunctionAnalysis) -> bool {
    left.events.len() == right.events.len()
        && left
            .events
            .iter()
            .zip(&right.events)
            .all(|(left, right)| left.equivalent(right))
}
