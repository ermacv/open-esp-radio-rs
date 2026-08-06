//! Offline review of evidence emitted by a protected verification run.

use std::path::PathBuf;

use super::super::*;

fn set_path(slot: &mut Option<PathBuf>, value: String, option: &str) -> Result<()> {
    if slot.replace(PathBuf::from(value)).is_some() {
        return Err(format!("duplicate {option}").into());
    }
    Ok(())
}

pub(super) fn run(arguments: Vec<String>) -> Result<bool> {
    let mut report = None;
    let mut baseline = None;
    let mut candidate = None;
    let mut arguments = arguments.into_iter();
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--report" => set_path(
                &mut report,
                take_value(&mut arguments, "--report")?,
                "--report",
            )?,
            "--evidence-baseline" => set_path(
                &mut baseline,
                take_value(&mut arguments, "--evidence-baseline")?,
                "--evidence-baseline",
            )?,
            "--candidate" => set_path(
                &mut candidate,
                take_value(&mut arguments, "--candidate")?,
                "--candidate",
            )?,
            "--no-evidence-baseline" => {}
            _ => return Err(format!("unknown verify evidence option: {argument}").into()),
        }
    }
    let report = report.ok_or("verify evidence requires --report")?;
    let baseline = baseline.ok_or("verify evidence requires --evidence-baseline")?;
    let evidence = load_evidence_report(&report)?;
    let report_sha256 = artifact_sha256(&report)?;
    let expected = load_evidence_baseline(&baseline)?;
    let passed = check_evidence_baseline(&expected, &evidence);
    if let Some(candidate) = candidate.as_deref() {
        write_evidence_candidate(
            candidate,
            &[
                ("accepted baseline", &baseline),
                ("verification report", &report),
            ],
            &evidence,
        )?;
    }
    println!(
        "EVIDENCE-REVIEW\t{}\tbaseline={}\treport={}\treport-sha256={}\texpected={}\tactual={}",
        if passed { "PASS" } else { "FAIL" },
        baseline.display(),
        report.display(),
        report_sha256,
        expected.len(),
        evidence.len()
    );
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
            run(vec![
                "--report".to_owned(),
                report.display().to_string(),
                "--evidence-baseline".to_owned(),
                baseline.display().to_string(),
                "--candidate".to_owned(),
                candidate.display().to_string(),
            ])
            .unwrap()
        );
        assert_eq!(
            fs::read_to_string(&candidate).unwrap(),
            "evidence rom leaf symbolic\n"
        );

        let error = run(vec![
            "--report".to_owned(),
            report.display().to_string(),
            "--evidence-baseline".to_owned(),
            baseline.display().to_string(),
            "--candidate".to_owned(),
            report.display().to_string(),
        ])
        .unwrap_err();
        assert!(error.to_string().contains("verification report"));
        assert!(fs::read_to_string(&report).unwrap().starts_with('{'));

        fs::remove_file(report).unwrap();
        fs::remove_file(baseline).unwrap();
        fs::remove_file(candidate).unwrap();
    }
}
