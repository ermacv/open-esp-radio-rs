//! Typed projection of project verification evidence for one vendor function.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::{ProjectSpec, Result};

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ReplacementEvidence {
    pub association: &'static str,
    pub report: String,
    pub report_complete_project_run: bool,
    pub report_passed: bool,
    pub freshness_claim: bool,
    pub vendor_source: String,
    pub vendor_symbol: String,
    pub status: String,
    pub reviewed: bool,
    pub disposition: Option<String>,
    pub protocol: Option<String>,
    pub production_component: Option<String>,
    pub verification_probes: Vec<String>,
    pub proofs: Vec<ReplacementProofEvidence>,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct ReplacementProofEvidence {
    pub suite: String,
    pub status: String,
    pub claim: Option<String>,
    pub probe_symbol: Option<String>,
    pub evidence: Option<String>,
    pub contract: Option<String>,
    pub profile: Option<String>,
    pub hil_evidence: Option<String>,
    #[serde(default)]
    pub blockers: Vec<String>,
}

#[derive(Deserialize)]
struct StoredVerificationProjection {
    complete_project_run: bool,
    passed: bool,
    replacement_graph: StoredReplacementGraphProjection,
}

#[derive(Deserialize)]
struct StoredReplacementGraphProjection {
    replacements: Vec<StoredReplacementProjection>,
}

#[derive(Deserialize)]
struct StoredReplacementProjection {
    vendor: StoredVendorFunctionProjection,
    reviewed: bool,
    disposition: Option<String>,
    protocol: Option<String>,
    rust: Option<StoredRustReplacementProjection>,
    status: String,
    proofs: Vec<ReplacementProofEvidence>,
}

#[derive(Deserialize)]
struct StoredVendorFunctionProjection {
    source: String,
    symbol: String,
}

#[derive(Deserialize)]
struct StoredRustReplacementProjection {
    production_component: Option<String>,
    #[serde(default)]
    verification_probes: Vec<String>,
}

pub(super) fn replacement_evidence(
    source: &str,
    symbol: &str,
    project: &ProjectSpec,
) -> Result<Vec<ReplacementEvidence>> {
    let Some(workspace) = project.verification.as_ref() else {
        return Ok(Vec::new());
    };
    if !workspace.report.is_file() {
        return Ok(Vec::new());
    }
    let input = std::fs::read_to_string(&workspace.report)
        .map_err(|source| crate::Error::read("verification report", &workspace.report, source))?;
    parse_replacement_evidence(
        source,
        symbol,
        &workspace.report,
        serde_json::from_str(&input)?,
    )
}

fn parse_replacement_evidence(
    source: &str,
    symbol: &str,
    report: &Path,
    document: StoredVerificationProjection,
) -> Result<Vec<ReplacementEvidence>> {
    let candidates = document
        .replacement_graph
        .replacements
        .into_iter()
        .filter(|edge| edge.vendor.symbol == symbol)
        .collect::<Vec<_>>();
    let has_exact = candidates.iter().any(|edge| edge.vendor.source == source);
    let selected = if has_exact {
        candidates
            .into_iter()
            .filter(|edge| edge.vendor.source == source)
            .collect()
    } else if candidates.len() == 1 {
        candidates
    } else {
        Vec::new()
    };
    selected
        .into_iter()
        .map(|edge| {
            let vendor_source = edge.vendor.source;
            let association = if vendor_source == source {
                "exact-source-symbol"
            } else {
                "unique-symbol-across-replacement-graph"
            };
            Ok(ReplacementEvidence {
                association,
                report: report.display().to_string(),
                report_complete_project_run: document.complete_project_run,
                report_passed: document.passed,
                freshness_claim: false,
                vendor_source,
                vendor_symbol: edge.vendor.symbol,
                status: edge.status,
                reviewed: edge.reviewed,
                disposition: edge.disposition,
                protocol: edge.protocol,
                production_component: edge
                    .rust
                    .as_ref()
                    .and_then(|rust| rust.production_component.clone()),
                verification_probes: edge
                    .rust
                    .map(|rust| rust.verification_probes)
                    .unwrap_or_default(),
                proofs: edge.proofs,
            })
        })
        .collect()
}
