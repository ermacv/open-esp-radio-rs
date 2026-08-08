//! Typed semantic-contract qualification and CLI-only presentation.

use super::super::*;
#[cfg(feature = "esp32s31-harness")]
use crate::harnesses::{QualificationCase, QualificationDifference, QualificationReport};

#[cfg(feature = "esp32s31-harness")]
pub(super) fn run(
    arguments: VerifyContractArgs,
    svd: &MmioMap,
    harness: &str,
    contract: &str,
) -> Result<bool> {
    let vendor_artifact = arguments
        .vendor_artifact
        .ok_or("missing --vendor-artifact")
        .map_err(crate::Error::invalid)?;
    let vendor_companion = arguments
        .vendor_companion
        .ok_or("missing --vendor-companion")
        .map_err(crate::Error::invalid)?;
    let report = crate::harnesses::verify_named_contract(
        harness,
        contract,
        svd,
        &vendor_artifact,
        &vendor_companion,
    )?;
    let matched = report.matched;
    crate::cli::output::render_report(&report, || render_human(&report));
    Ok(matched)
}

#[cfg(not(feature = "esp32s31-harness"))]
pub(super) fn run(
    _arguments: VerifyContractArgs,
    _svd: &MmioMap,
    harness: &str,
    _contract: &str,
) -> Result<bool> {
    Err(crate::Error::invalid(format!(
        "platform harness {harness:?} is not compiled into this neutral workbench build"
    )))
}

#[cfg(feature = "esp32s31-harness")]
fn render_human(report: &QualificationReport) {
    outputln!(
        "Semantic qualification: {} — {}",
        report.contract,
        report.verdict.label()
    );
    outputln!("  vendor symbol: {}", report.vendor_symbol);
    for artifact in &report.artifacts {
        outputln!(
            "  {}: {} (sha256:{})",
            artifact.role,
            artifact.path.display(),
            artifact.sha256
        );
    }
    for case in &report.cases {
        render_case_human(case);
    }
    outputln!(
        "Summary: scenarios={} matched={} mismatched={} incomplete={} steps={} branch-outcomes={} calls={}",
        report.summary.scenarios,
        report.summary.matched,
        report.summary.mismatched,
        report.summary.incomplete,
        report.summary.steps,
        report.summary.branch_outcomes,
        report.summary.calls,
    );
}

#[cfg(feature = "esp32s31-harness")]
fn render_case_human(case: &QualificationCase) {
    outputln!("  {}: {}", case.name, case.verdict.label());
    if let Some(events) = case.events {
        outputln!(
            "    events={events} steps={} branch-outcomes={} calls={}",
            case.steps.unwrap_or_default(),
            case.branch_outcomes.unwrap_or_default(),
            case.calls.unwrap_or_default(),
        );
    }
    if let Some(difference) = case.difference.as_ref() {
        render_difference_human(difference);
    }
}

#[cfg(feature = "esp32s31-harness")]
fn render_difference_human(difference: &QualificationDifference) {
    if let Some(reason) = difference.reason.as_deref() {
        outputln!("    difference: {reason}");
    }
    if let Some(index) = difference.index {
        outputln!(
            "    first difference at {index}: vendor={} rust={}",
            difference.vendor.as_deref().unwrap_or("<missing>"),
            difference.rust.as_deref().unwrap_or("<missing>"),
        );
    }
    for (index, event) in difference.vendor_events.iter().enumerate() {
        outputln!("    vendor[{index}]: {event}");
    }
    for (index, event) in difference.rust_events.iter().enumerate() {
        outputln!("    rust[{index}]: {event}");
    }
}

#[cfg(all(test, feature = "esp32s31-harness"))]
mod tests {
    use super::*;
    use open_radio_vendor_harness_esp32s31_semantic::verification::QualificationSummary;

    #[test]
    fn qualification_report_is_a_stable_typed_document() {
        let report = QualificationReport {
            schema: 2,
            mode: EquivalenceMode::Semantic,
            contract: "fixture",
            vendor_symbol: "fixture_symbol",
            verdict: EquivalenceVerdict::Match,
            matched: true,
            artifacts: Vec::new(),
            cases: vec![QualificationCase {
                name: "cold".to_owned(),
                verdict: EquivalenceVerdict::Match,
                events: Some(3),
                steps: Some(7),
                branch_outcomes: Some(2),
                branch_events: Some(2),
                calls: Some(1),
                call_events: Some(1),
                state: None,
                difference: None,
            }],
            summary: QualificationSummary {
                scenarios: 1,
                matched: 1,
                mismatched: 0,
                incomplete: 0,
                failed: 0,
                steps: 7,
                branch_outcomes: 2,
                calls: 1,
            },
        };
        let document = serde_json::to_value(report).unwrap();
        assert_eq!(document["schema"], 2);
        assert_eq!(document["cases"][0]["verdict"], "match");
        assert_eq!(document["summary"]["matched"], 1);
    }
}
