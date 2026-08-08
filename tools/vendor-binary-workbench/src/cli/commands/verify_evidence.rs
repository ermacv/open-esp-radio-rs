//! Offline review of evidence emitted by a protected verification run.

use super::super::*;

use serde::Serialize;

#[derive(Serialize)]
struct EvidenceReviewReport {
    schema_version: u32,
    command: &'static str,
    baseline: String,
    report: String,
    report_sha256: String,
    candidate: Option<String>,
    comparison: EvidenceComparison,
}

pub(super) fn run(arguments: VerifyEvidenceArgs) -> Result<bool> {
    let report = arguments
        .report
        .ok_or("verify evidence requires --report")?;
    let baseline = arguments
        .evidence_baseline
        .ok_or("verify evidence requires --evidence-baseline")?;
    let evidence = load_evidence_report(&report)?;
    let report_sha256 = artifact_sha256(&report)?;
    let expected = load_evidence_baseline(&baseline)?;
    let comparison = compare_evidence_baseline(&expected, &evidence);
    let passed = comparison.passed;
    if let Some(candidate) = arguments.candidate.as_deref() {
        write_evidence_candidate(
            candidate,
            &[
                ("accepted baseline", &baseline),
                ("verification report", &report),
            ],
            &evidence,
        )?;
    }
    let review = EvidenceReviewReport {
        schema_version: 1,
        command: "verify evidence",
        baseline: baseline.display().to_string(),
        report: report.display().to_string(),
        report_sha256,
        candidate: arguments.candidate.map(|path| path.display().to_string()),
        comparison,
    };
    if !crate::cli::output::structured("evidence-review", &review) {
        print_evidence_comparison(&review.comparison);
        if let Some(candidate) = &review.candidate {
            outputln!(
                "EVIDENCE-CANDIDATE\t{}\tentries={}",
                candidate,
                evidence.len()
            );
        }
        outputln!(
            "EVIDENCE-REVIEW\t{}\tbaseline={}\treport={}\treport-sha256={}\texpected={}\tactual={}",
            if passed { "PASS" } else { "FAIL" },
            review.baseline,
            review.report,
            review.report_sha256,
            review.comparison.expected,
            review.comparison.actual
        );
    }
    Ok(passed)
}

#[cfg(test)]
mod tests {
    use std::{env, fs};

    use super::*;

    #[test]
    fn reviews_a_report_and_writes_only_the_separate_candidate() {
        let suffix = std::process::id();
        let directory = env::temp_dir();
        let report = directory.join(format!("vendor-workbench-review-{suffix}.json"));
        let baseline = directory.join(format!("vendor-workbench-review-{suffix}.evidence"));
        let candidate = directory.join(format!(
            "vendor-workbench-review-{suffix}.candidate.evidence"
        ));
        fs::write(
            &report,
            r#"{"schema_version":3,"command":"verify inventory","evidence":[{"source":"rom","symbol":"leaf","kind":"symbolic"}]}"#,
        )
        .unwrap();
        fs::write(&baseline, "evidence rom leaf symbolic\n").unwrap();

        assert!(
            run(VerifyEvidenceArgs {
                report: Some(report.clone()),
                evidence_baseline: Some(baseline.clone()),
                candidate: Some(candidate.clone()),
                ..Default::default()
            })
            .unwrap()
        );
        assert_eq!(
            fs::read_to_string(&candidate).unwrap(),
            "evidence rom leaf symbolic\n"
        );

        let error = run(VerifyEvidenceArgs {
            report: Some(report.clone()),
            evidence_baseline: Some(baseline.clone()),
            candidate: Some(report.clone()),
            ..Default::default()
        })
        .unwrap_err();
        assert!(error.to_string().contains("verification report"));
        assert!(fs::read_to_string(&report).unwrap().starts_with('{'));

        fs::remove_file(report).unwrap();
        fs::remove_file(baseline).unwrap();
        fs::remove_file(candidate).unwrap();
    }
}
