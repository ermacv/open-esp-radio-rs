//! Reviewed evidence baselines, comparisons, and candidate publication.

use std::{
    env, fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use super::{EvidenceSet, VerificationEvidenceDocument, record_evidence};
use crate::{Result, error::WorkbenchError};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BaselineDocument {
    schema: u32,
    evidence: Vec<BaselineEntry>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BaselineEntry {
    source: String,
    symbol: String,
    kind: String,
}

#[tracing::instrument(name = "load_evidence_baseline", fields(path = %path.display()))]
pub(crate) fn load_evidence_baseline(path: &Path) -> Result<EvidenceSet> {
    let text = fs::read_to_string(path)?;
    let document: BaselineDocument = toml_edit::de::from_str(&text).map_err(|error| {
        WorkbenchError::manifest_source("evidence baseline TOML", path, &text, &error, error.span())
    })?;
    finish_evidence_baseline(path, document).map_err(|error| {
        WorkbenchError::manifest_document("evidence baseline TOML", path, &text, error)
    })
}

fn finish_evidence_baseline(path: &Path, document: BaselineDocument) -> Result<EvidenceSet> {
    if document.schema != 1 {
        return Err(crate::Error::invalid(
            "evidence baseline TOML requires schema = 1",
        ));
    }
    let mut evidence = EvidenceSet::new();
    for entry in document.evidence {
        record_evidence(&mut evidence, &entry.source, &entry.symbol, entry.kind)?;
    }
    if evidence.is_empty() {
        return Err(crate::Error::invalid(format!(
            "evidence baseline {} is empty",
            path.display()
        )));
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

fn evidence_path_identity(path: &Path) -> Result<PathBuf> {
    if path.exists() {
        return Ok(fs::canonicalize(path)?);
    }
    let absolute = if path.is_absolute() {
        path.to_owned()
    } else {
        env::current_dir()?.join(path)
    };
    let parent = absolute
        .parent()
        .ok_or_else(|| {
            format!(
                "evidence candidate path {} has no parent directory",
                path.display()
            )
        })
        .map_err(crate::Error::invalid)?;
    let file_name = absolute
        .file_name()
        .ok_or_else(|| {
            format!(
                "evidence candidate path {} has no file name",
                path.display()
            )
        })
        .map_err(crate::Error::invalid)?;
    Ok(fs::canonicalize(parent)?.join(file_name))
}

pub(crate) fn write_evidence_candidate(
    path: &Path,
    protected_inputs: &[(&str, &Path)],
    evidence: &EvidenceSet,
) -> Result<()> {
    if evidence.is_empty() {
        return Err(crate::Error::invalid(
            "refusing to write an empty evidence candidate",
        ));
    }
    let candidate_identity = evidence_path_identity(path)?;
    for (role, protected) in protected_inputs {
        if evidence_path_identity(protected)? == candidate_identity {
            return Err(crate::Error::invalid(format!(
                "evidence candidate must not overwrite {role} {}; choose a separate candidate path",
                protected.display()
            )));
        }
    }
    #[derive(Serialize)]
    struct BaselineDocument<'a> {
        schema: u32,
        evidence: Vec<BaselineEntry<'a>>,
    }

    #[derive(Serialize)]
    struct BaselineEntry<'a> {
        source: &'a str,
        symbol: &'a str,
        kind: &'a str,
    }

    let output = toml_edit::ser::to_string_pretty(&BaselineDocument {
        schema: 1,
        evidence: evidence
            .iter()
            .map(|((source, symbol), kind)| BaselineEntry {
                source,
                symbol,
                kind,
            })
            .collect(),
    })?;
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
        return Err(crate::Error::invalid(format!(
            "verification report schema_version must be {}",
            super::super::VERIFICATION_REPORT_SCHEMA
        )));
    }
    if report.command != "verify inventory" {
        return Err(crate::Error::invalid(
            "evidence review requires a verify inventory JSON report",
        ));
    }
    let mut evidence = EvidenceSet::new();
    for entry in report.evidence {
        record_evidence(&mut evidence, &entry.source, &entry.symbol, entry.kind)?;
    }
    if evidence.is_empty() {
        return Err(crate::Error::invalid(format!(
            "verification report {} has no evidence",
            path.display()
        )));
    }
    Ok(evidence)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_checked_in_esp32s31_baseline_is_valid_toml() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .expect("workbench remains under tools");
        let directory = root.join("verification/vendor/targets/esp32s31/baselines");
        let mut paths = fs::read_dir(directory)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| {
                path.extension()
                    .is_some_and(|extension| extension == "toml")
            })
            .collect::<Vec<_>>();
        paths.sort();
        assert_eq!(paths.len(), 9);
        for path in paths {
            assert!(!load_evidence_baseline(&path).unwrap().is_empty());
        }
    }

    #[test]
    fn malformed_baseline_retains_its_physical_source_line() {
        let input =
            "schema = 1\n\n[[evidence]]\nsource = \"rom\"\nsymbol = 42\nkind = \"symbolic\"\n";
        let path = std::env::temp_dir().join(format!(
            "vendor-workbench-evidence-diagnostic-{}.toml",
            std::process::id()
        ));
        fs::write(&path, input).unwrap();
        let error = load_evidence_baseline(&path).unwrap_err();
        fs::remove_file(&path).unwrap();

        let WorkbenchError::ManifestSource {
            path: reported,
            span,
            ..
        } = error
        else {
            panic!("expected source diagnostic");
        };
        let line_start = input.find("symbol = 42").unwrap();
        assert_eq!(reported, path);
        assert!((line_start..line_start + "symbol = 42".len()).contains(&span.offset()));
    }
}
