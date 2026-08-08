//! Presentation-neutral results produced by executable platform qualifications.

use std::path::PathBuf;

use open_radio_vendor_semantics::{EquivalenceMode, EquivalenceVerdict};
use serde::Serialize;

/// One immutable input whose identity participates in a qualification run.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct QualificationArtifact {
    pub role: &'static str,
    pub path: PathBuf,
    pub sha256: String,
}

/// First concrete disagreement retained for manual inspection.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct QualificationDifference {
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
pub struct QualificationStateFootprint {
    pub read_bytes: usize,
    pub written_bytes: usize,
    pub classified_ranges: usize,
}

impl From<super::StateFootprintStats> for QualificationStateFootprint {
    fn from(value: super::StateFootprintStats) -> Self {
        Self {
            read_bytes: value.read_bytes,
            written_bytes: value.written_bytes,
            classified_ranges: value.classified_ranges,
        }
    }
}

/// Typed scenario row shared by human and machine renderers.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct QualificationCase {
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
    pub state: Option<QualificationStateFootprint>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub difference: Option<QualificationDifference>,
}

/// Aggregate coverage totals for one named qualification contract.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct QualificationSummary {
    pub scenarios: usize,
    pub matched: usize,
    pub mismatched: usize,
    pub incomplete: usize,
    pub failed: usize,
    pub steps: u64,
    pub branch_outcomes: usize,
    pub calls: usize,
}

/// Complete presentation-neutral result returned across the harness boundary.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct QualificationReport {
    pub schema: u32,
    pub mode: EquivalenceMode,
    pub contract: &'static str,
    pub vendor_symbol: &'static str,
    pub verdict: EquivalenceVerdict,
    pub matched: bool,
    pub artifacts: Vec<QualificationArtifact>,
    pub cases: Vec<QualificationCase>,
    pub summary: QualificationSummary,
}
