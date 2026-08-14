//! Presentation-neutral semantic verification results shared across provider boundaries.

use std::path::PathBuf;

use serde::Serialize;

use crate::{EquivalenceMode, EquivalenceVerdict};

/// One immutable input whose identity participates in a semantic verification run.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SemanticVerificationArtifact {
    pub role: &'static str,
    pub path: PathBuf,
    pub sha256: String,
}

/// First concrete disagreement retained for manual inspection.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct SemanticVerificationDifference {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub index: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vendor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rust: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub vendor_events: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub rust_events: Vec<String>,
}

/// Reviewed state-footprint coverage observed by one scenario.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct SemanticVerificationStateFootprint {
    pub read_bytes: usize,
    pub written_bytes: usize,
    pub classified_ranges: usize,
}

/// Typed scenario row shared by human and machine renderers.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SemanticVerificationCase {
    pub name: String,
    pub verdict: EquivalenceVerdict,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub events: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub steps: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch_outcomes: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch_events: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub calls: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub call_events: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<SemanticVerificationStateFootprint>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub difference: Option<SemanticVerificationDifference>,
}

/// Aggregate coverage totals for one named semantic verification contract.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SemanticVerificationSummary {
    pub scenarios: usize,
    pub matched: usize,
    pub mismatched: usize,
    pub incomplete: usize,
    pub failed: usize,
    pub steps: u64,
    pub branch_outcomes: usize,
    pub calls: usize,
}

/// Complete result returned by a compiled platform provider.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SemanticVerificationReport {
    pub schema: u32,
    pub mode: EquivalenceMode,
    pub contract: &'static str,
    pub vendor_symbol: &'static str,
    pub verdict: EquivalenceVerdict,
    pub matched: bool,
    pub artifacts: Vec<SemanticVerificationArtifact>,
    pub cases: Vec<SemanticVerificationCase>,
    pub summary: SemanticVerificationSummary,
}
