//! Typed aggregate reports produced by source and inventory verification.

use std::path::Path;

use open_radio_vendor_semantics::{DriverAdapterCase, DriverAdapterClaim};
use serde::{Deserialize, Serialize};

use super::EvidenceSet;
use super::{EvidenceComparison, ExecutionComparisonReport, VerificationCoreReport, VerifySummary};

pub(crate) const VERIFICATION_REPORT_SCHEMA: u32 = 8;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum FunctionVerificationStatus {
    Match,
    Mismatch,
    Incomplete,
    ImplementedUnqualified,
    Uncovered,
}

#[derive(Debug, Serialize)]
pub(crate) struct FunctionVerificationReport {
    pub(crate) source: String,
    pub(crate) vendor_symbol: String,
    pub(crate) status: FunctionVerificationStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) rust_symbol: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) rust_component: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) evidence: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) contract: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) profile: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) disposition: Option<String>,
    pub(crate) disposition_reviewed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) protocol: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) driver_adapter: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) claim: Option<DriverAdapterClaim>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) adapter_cases: Vec<DriverAdapterCase>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) hil_evidence: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) qualification_blockers: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) uncovered: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) vendor_events: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) rust_events: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) effects: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) return_compared: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) execution: Option<ExecutionComparisonReport>,
}

impl FunctionVerificationReport {
    pub(crate) fn new(
        source: &str,
        vendor_symbol: &str,
        status: FunctionVerificationStatus,
    ) -> Self {
        Self {
            source: source.to_owned(),
            vendor_symbol: vendor_symbol.to_owned(),
            status,
            rust_symbol: None,
            rust_component: None,
            evidence: None,
            contract: None,
            profile: None,
            disposition: None,
            disposition_reviewed: false,
            protocol: None,
            driver_adapter: None,
            claim: None,
            adapter_cases: Vec::new(),
            hil_evidence: None,
            qualification_blockers: Vec::new(),
            reason: None,
            uncovered: None,
            vendor_events: None,
            rust_events: None,
            effects: None,
            return_compared: None,
            execution: None,
        }
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct SourceVerificationReport {
    pub(crate) source: String,
    pub(crate) summary: VerifySummary,
    pub(crate) functions: Vec<FunctionVerificationReport>,
}

#[derive(Debug, Serialize)]
pub(crate) struct SourceInventoryReport {
    pub(crate) source: String,
    pub(crate) symbols: usize,
}

#[derive(Debug, Serialize)]
pub(crate) struct ProtocolInventoryReport {
    pub(crate) shared: usize,
    pub(crate) wifi: usize,
    pub(crate) bluetooth: usize,
    pub(crate) ble: usize,
    pub(crate) coex: usize,
    pub(crate) ieee802154: usize,
    pub(crate) unknown: usize,
    pub(crate) exact_dispositions: usize,
    pub(crate) executable_bindings: usize,
}

#[derive(Debug, Serialize)]
pub(crate) struct PublishedVerificationReport {
    pub(crate) path: String,
    pub(crate) status: &'static str,
}

impl PublishedVerificationReport {
    pub(crate) fn written(path: &Path) -> Self {
        Self {
            path: path.display().to_string(),
            status: "written",
        }
    }
}

#[derive(Serialize)]
pub(crate) struct VerificationCommandReport {
    pub(crate) schema_version: u32,
    pub(crate) command: &'static str,
    #[serde(flatten)]
    pub(crate) verification: VerificationCoreReport,
    pub(crate) sources: Vec<SourceVerificationReport>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) inventory: Vec<SourceInventoryReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) protocols: Option<ProtocolInventoryReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) evidence_comparison: Option<EvidenceComparison>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) report: Option<PublishedVerificationReport>,
}

impl VerificationCommandReport {
    pub(crate) fn evidence_set(&self) -> EvidenceSet {
        self.verification
            .evidence
            .iter()
            .map(|entry| {
                (
                    (entry.source.clone(), entry.symbol.clone()),
                    entry.identity.clone(),
                )
            })
            .collect()
    }
}

impl FunctionVerificationStatus {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Match => "match",
            Self::Mismatch => "mismatch",
            Self::Incomplete => "incomplete",
            Self::ImplementedUnqualified => "implemented-unqualified",
            Self::Uncovered => "uncovered",
        }
    }
}
