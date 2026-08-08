//! Typed aggregate reports produced by source and inventory verification.

use std::path::Path;

use serde::Serialize;

use super::{EvidenceComparison, ExecutionComparisonReport, VerificationCoreReport, VerifySummary};

pub(crate) const VERIFICATION_REPORT_SCHEMA: u32 = 4;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) protocol: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) driver_adapter: Option<String>,
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
            protocol: None,
            driver_adapter: None,
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
pub(crate) struct VerificationCommandReport<'a> {
    pub(crate) schema_version: u32,
    pub(crate) command: &'static str,
    #[serde(flatten)]
    pub(crate) verification: &'a VerificationCoreReport,
    pub(crate) sources: &'a [SourceVerificationReport],
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) inventory: Vec<SourceInventoryReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) protocols: Option<ProtocolInventoryReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) evidence_comparison: Option<&'a EvidenceComparison>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) report: Option<PublishedVerificationReport>,
}

pub(crate) fn render_verification_human(report: &VerificationCommandReport<'_>) {
    outputln!(
        "{}: {}",
        report.command,
        if report.verification.passed {
            "passed"
        } else {
            "failed"
        }
    );
    for source in report.sources {
        outputln!(
            "  {}: {} functions, {} matched, {} mismatched, {} incomplete, {} missing",
            source.source,
            source.summary.vendor_functions,
            source.summary.matched,
            source.summary.mismatched,
            source.summary.incomplete,
            source.summary.missing,
        );
        for function in &source.functions {
            outputln!(
                "    {}: {}",
                function.vendor_symbol,
                function.status.label()
            );
        }
    }
    if let Some(comparison) = report.evidence_comparison {
        outputln!(
            "  evidence baseline: {} ({} expected, {} actual, {} regressions)",
            if comparison.passed {
                "passed"
            } else {
                "failed"
            },
            comparison.expected,
            comparison.actual,
            comparison.regressions.len(),
        );
    }
    if let Some(publication) = &report.report {
        outputln!("  report: {} ({})", publication.path, publication.status);
    }
}

pub(crate) fn render_verification_tsv(report: &VerificationCommandReport<'_>) {
    outputln!(
        "verification\t{}\t{}",
        report.command,
        if report.verification.passed {
            "passed"
        } else {
            "failed"
        }
    );
    for source in report.sources {
        outputln!(
            "source\t{}\tfunctions={}\tmatched={}\tmismatched={}\tincomplete={}\tmissing={}",
            source.source,
            source.summary.vendor_functions,
            source.summary.matched,
            source.summary.mismatched,
            source.summary.incomplete,
            source.summary.missing,
        );
        for function in &source.functions {
            outputln!(
                "function\t{}\t{}\t{}",
                function.source,
                function.vendor_symbol,
                function.status.label()
            );
        }
    }
}

impl FunctionVerificationStatus {
    const fn label(self) -> &'static str {
        match self {
            Self::Match => "match",
            Self::Mismatch => "mismatch",
            Self::Incomplete => "incomplete",
            Self::ImplementedUnqualified => "implemented-unqualified",
            Self::Uncovered => "uncovered",
        }
    }
}
