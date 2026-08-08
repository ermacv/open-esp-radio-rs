//! Reviewed evidence baselines, comparisons, and candidate publication.

use std::{
    env,
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use super::{EvidenceSet, VerificationEvidenceDocument, record_evidence};
use crate::{Result, error::WorkbenchError};

#[tracing::instrument(name = "load_evidence_baseline", fields(path = %path.display()))]
pub(crate) fn load_evidence_baseline(path: &Path) -> Result<EvidenceSet> {
    let text = fs::read_to_string(path)?;
    parse_evidence_baseline(path, &text)
        .map_err(|error| WorkbenchError::manifest_document("evidence baseline", path, &text, error))
}

fn parse_evidence_baseline(path: &Path, text: &str) -> Result<EvidenceSet> {
    let mut evidence = EvidenceSet::new();
    for (index, raw) in text.lines().enumerate() {
        let line_number = index + 1;
        let line = raw.split_once('#').map_or(raw, |(before, _)| before).trim();
        if line.is_empty() {
            continue;
        }
        (|| {
            let fields = line.split_whitespace().collect::<Vec<_>>();
            let ["evidence", source, symbol, kind] = fields.as_slice() else {
                return Err(
                    "expected evidence baseline directive: evidence SOURCE SYMBOL KIND".into(),
                );
            };
            record_evidence(&mut evidence, source, symbol, *kind)
        })()
        .map_err(|error: WorkbenchError| error.at_line(line_number))?;
    }
    if evidence.is_empty() {
        return Err(format!("evidence baseline {} is empty", path.display()).into());
    }
    Ok(evidence)
}

#[derive(Debug, Serialize)]
pub(crate) struct EvidenceRegression {
    pub(crate) source: String,
    pub(crate) symbol: String,
    pub(crate) expected: String,
    pub(crate) actual: Option<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct EvidenceAddition {
    pub(crate) source: String,
    pub(crate) symbol: String,
    pub(crate) kind: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct EvidenceComparison {
    pub(crate) passed: bool,
    pub(crate) expected: usize,
    pub(crate) actual: usize,
    pub(crate) regressions: Vec<EvidenceRegression>,
    pub(crate) additions: Vec<EvidenceAddition>,
}

pub(crate) fn compare_evidence_baseline(
    expected: &EvidenceSet,
    actual: &EvidenceSet,
) -> EvidenceComparison {
    let mut regressions = Vec::new();
    for ((source, symbol), expected_kind) in expected {
        match actual.get(&(source.clone(), symbol.clone())) {
            Some(actual_kind) if actual_kind == expected_kind => {}
            Some(actual_kind) => regressions.push(EvidenceRegression {
                source: source.clone(),
                symbol: symbol.clone(),
                expected: expected_kind.clone(),
                actual: Some(actual_kind.clone()),
            }),
            None => regressions.push(EvidenceRegression {
                source: source.clone(),
                symbol: symbol.clone(),
                expected: expected_kind.clone(),
                actual: None,
            }),
        }
    }
    let additions = actual
        .iter()
        .filter(|((source, symbol), _)| !expected.contains_key(&(source.clone(), symbol.clone())))
        .map(|((source, symbol), kind)| EvidenceAddition {
            source: source.clone(),
            symbol: symbol.clone(),
            kind: kind.clone(),
        })
        .collect::<Vec<_>>();
    EvidenceComparison {
        passed: regressions.is_empty(),
        expected: expected.len(),
        actual: actual.len(),
        regressions,
        additions,
    }
}

pub(crate) fn print_evidence_comparison(comparison: &EvidenceComparison) {
    for regression in &comparison.regressions {
        outputln!(
            "EVIDENCE-REGRESSION\t{}\t{}\texpected={}\tactual={}",
            regression.source,
            regression.symbol,
            regression.expected,
            regression.actual.as_deref().unwrap_or("missing")
        );
    }
    for addition in &comparison.additions {
        outputln!(
            "EVIDENCE-ADDITION\t{}\t{}\t{}",
            addition.source,
            addition.symbol,
            addition.kind
        );
    }
    outputln!(
        "EVIDENCE-BASELINE\t{}\texpected={}\tactual={}",
        if comparison.passed { "PASS" } else { "FAIL" },
        comparison.expected,
        comparison.actual
    );
}

fn evidence_path_identity(path: &Path) -> Result<PathBuf> {
    if path.exists() {
        return Ok(fs::canonicalize(path)?);
    }
    let absolute = if path.is_absolute() {
        path.to_owned()
    } else {
        env::current_dir()?.join(path)
    };
    let parent = absolute.parent().ok_or_else(|| {
        format!(
            "evidence candidate path {} has no parent directory",
            path.display()
        )
    })?;
    let file_name = absolute.file_name().ok_or_else(|| {
        format!(
            "evidence candidate path {} has no file name",
            path.display()
        )
    })?;
    Ok(fs::canonicalize(parent)?.join(file_name))
}

pub(crate) fn write_evidence_candidate(
    path: &Path,
    protected_inputs: &[(&str, &Path)],
    evidence: &EvidenceSet,
) -> Result<()> {
    if evidence.is_empty() {
        return Err("refusing to write an empty evidence candidate".into());
    }
    let candidate_identity = evidence_path_identity(path)?;
    for (role, protected) in protected_inputs {
        if evidence_path_identity(protected)? == candidate_identity {
            return Err(format!(
                "evidence candidate must not overwrite {role} {}; choose a separate candidate path",
                protected.display()
            )
            .into());
        }
    }
    let mut output = String::new();
    for ((source, symbol), kind) in evidence {
        writeln!(output, "evidence {source} {symbol} {kind}")
            .expect("writing to String cannot fail");
    }
    fs::write(path, output)?;
    Ok(())
}

#[derive(Deserialize)]
struct StoredVerificationReport {
    schema_version: u32,
    command: String,
    evidence: Vec<VerificationEvidenceDocument>,
}

#[tracing::instrument(name = "load_verification_report", fields(path = %path.display()))]
pub(crate) fn load_evidence_report(path: &Path) -> Result<EvidenceSet> {
    let text = fs::read_to_string(path)?;
    parse_evidence_report(path, &text).map_err(|error| {
        WorkbenchError::manifest_document("verification report", path, &text, error)
    })
}

fn parse_evidence_report(path: &Path, text: &str) -> Result<EvidenceSet> {
    let report: StoredVerificationReport = serde_json::from_str(text)?;
    if report.schema_version != super::super::VERIFICATION_REPORT_SCHEMA {
        return Err(format!(
            "verification report schema_version must be {}",
            super::super::VERIFICATION_REPORT_SCHEMA
        )
        .into());
    }
    if report.command != "verify inventory" {
        return Err("evidence review requires a verify inventory JSON report".into());
    }
    let mut evidence = EvidenceSet::new();
    for entry in report.evidence {
        record_evidence(&mut evidence, &entry.source, &entry.symbol, entry.kind)?;
    }
    if evidence.is_empty() {
        return Err(format!("verification report {} has no evidence", path.display()).into());
    }
    Ok(evidence)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malformed_baseline_retains_its_physical_source_line() {
        let error = parse_evidence_baseline(
            Path::new("fixture.evidence"),
            "# reviewed baseline\nevidence incomplete\n",
        )
        .unwrap_err();

        assert!(matches!(error, WorkbenchError::InputLine { line: 2, .. }));
    }
}
