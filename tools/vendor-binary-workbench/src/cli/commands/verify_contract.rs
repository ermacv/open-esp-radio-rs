//! Typed semantic-contract qualification and CLI-only presentation.

use super::super::*;
use crate::harnesses::{QualificationCase, QualificationDifference, QualificationReport};

pub(super) fn run(
    arguments: VerifyContractArgs,
    svd: &MmioRegisterMap,
    harness: &str,
    contract: &str,
) -> Result<bool> {
    let vendor_artifact = arguments
        .vendor_artifact
        .ok_or("missing --vendor-artifact")?;
    let vendor_companion = arguments
        .vendor_companion
        .ok_or("missing --vendor-companion")?;
    let report = crate::harnesses::verify_named_contract(
        harness,
        contract,
        svd,
        &vendor_artifact,
        &vendor_companion,
    )?;
    let matched = report.matched;
    crate::cli::output::render_report(&report, || render_human(&report), || render_tsv(&report));
    Ok(matched)
}

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
    if !case.unmapped_mmio.is_empty() {
        outputln!(
            "    unmapped MMIO: {}",
            case.unmapped_mmio
                .iter()
                .map(|address| format!("{address:#010x}"))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    if let Some(difference) = case.difference.as_ref() {
        render_difference_human(difference);
    }
}

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

fn render_tsv(report: &QualificationReport) {
    for artifact in &report.artifacts {
        outputln!(
            "ORACLE\t{}\t{}\tsha256={}",
            artifact.role,
            artifact.path.display(),
            artifact.sha256
        );
    }
    for case in &report.cases {
        outputln!(
            "VERIFICATION-CASE\t{}\t{}\tevents={}\tsteps={}\tbranch-outcomes={}\tcalls={}\tunmapped-mmio={}",
            case.name,
            case.verdict.label(),
            case.events.unwrap_or_default(),
            case.steps.unwrap_or_default(),
            case.branch_outcomes.unwrap_or_default(),
            case.calls.unwrap_or_default(),
            case.unmapped_mmio.len(),
        );
    }
    outputln!(
        "VERIFICATION-SUMMARY\t{}\t{}\tscenarios={}\tmatched={}\tmismatched={}\tincomplete={}\tsteps={}\tbranch-outcomes={}\tcalls={}",
        report.vendor_symbol,
        report.verdict.label(),
        report.summary.scenarios,
        report.summary.matched,
        report.summary.mismatched,
        report.summary.incomplete,
        report.summary.steps,
        report.summary.branch_outcomes,
        report.summary.calls,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use open_radio_vendor_harness_esp32s31_semantic::verification::{
        QualificationSummary, QualificationVerdict,
    };

    #[test]
    fn qualification_report_is_a_stable_typed_document() {
        let report = QualificationReport {
            schema: 1,
            contract: "fixture",
            vendor_symbol: "fixture_symbol",
            verdict: QualificationVerdict::Match,
            matched: true,
            artifacts: Vec::new(),
            cases: vec![QualificationCase {
                name: "cold".to_owned(),
                verdict: QualificationVerdict::Match,
                events: Some(3),
                steps: Some(7),
                branch_outcomes: Some(2),
                branch_events: Some(2),
                calls: Some(1),
                call_events: Some(1),
                state: None,
                unmapped_mmio: Vec::new(),
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
        assert_eq!(document["schema"], 1);
        assert_eq!(document["cases"][0]["verdict"], "match");
        assert_eq!(document["summary"]["matched"], 1);
    }
}
