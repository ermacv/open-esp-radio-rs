//! Freshness-checked hardware evidence for statically qualified features.

use std::{
    fs,
    path::{Path, PathBuf},
};

use serde::Deserialize;

use super::{FeatureHardwareReport, FeatureHardwareSpec};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredHardwareEvidence {
    schema: u32,
    command: String,
    features: Vec<StoredHardwareFeature>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredHardwareFeature {
    id: String,
    passed: bool,
    successful_runs: usize,
    observations: Vec<String>,
    artifacts: Vec<StoredHardwareArtifact>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredHardwareArtifact {
    id: String,
    path: PathBuf,
    sha256: String,
}

pub(super) fn evaluate_hardware(
    feature_id: &str,
    spec: &FeatureHardwareSpec,
    evidence_path: Option<&Path>,
) -> FeatureHardwareReport {
    let mut report = FeatureHardwareReport {
        status: "missing".to_owned(),
        successful_runs: 0,
        minimum_successful_runs: spec.minimum_successful_runs,
        observations: Vec::new(),
        required_observations: spec.required_observations.clone(),
        artifacts: Vec::new(),
        required_artifacts: spec.required_artifacts.clone(),
        blockers: Vec::new(),
    };
    let Some(path) = evidence_path else {
        report
            .blockers
            .push("project [qualification] does not configure hardware-evidence".to_owned());
        return report;
    };
    let input = match fs::read_to_string(path) {
        Ok(input) => input,
        Err(error) => {
            report.blockers.push(format!(
                "cannot read hardware evidence {}: {error}",
                path.display()
            ));
            return report;
        }
    };
    let evidence: StoredHardwareEvidence = match serde_json::from_str(&input) {
        Ok(evidence) => evidence,
        Err(error) => {
            report.status = "invalid".to_owned();
            report.blockers.push(format!(
                "invalid hardware evidence {}: {error}",
                path.display()
            ));
            return report;
        }
    };
    if evidence.schema != 1 || evidence.command != "project hardware evidence" {
        report.status = "invalid".to_owned();
        report.blockers.push(format!(
            "hardware evidence {} requires schema 1 and command \"project hardware evidence\"",
            path.display()
        ));
        return report;
    }
    let candidates = evidence
        .features
        .iter()
        .filter(|feature| feature.id == feature_id)
        .collect::<Vec<_>>();
    let Some(feature) = (candidates.len() == 1).then(|| candidates[0]) else {
        report.blockers.push(if candidates.is_empty() {
            format!("hardware evidence has no result for feature {feature_id:?}")
        } else {
            format!("hardware evidence repeats feature {feature_id:?}")
        });
        return report;
    };
    report.successful_runs = feature.successful_runs;
    report.observations = feature.observations.clone();
    report.artifacts = feature
        .artifacts
        .iter()
        .map(|artifact| artifact.id.clone())
        .collect();
    if !feature.passed {
        report
            .blockers
            .push("hardware scenario did not pass".to_owned());
    }
    if feature.successful_runs < spec.minimum_successful_runs {
        report.blockers.push(format!(
            "hardware evidence has {} successful run(s), requires {}",
            feature.successful_runs, spec.minimum_successful_runs
        ));
    }
    for observation in &spec.required_observations {
        if !feature.observations.contains(observation) {
            report.blockers.push(format!(
                "hardware evidence lacks required observation {observation:?}"
            ));
        }
    }
    let base = path.parent().unwrap_or_else(|| Path::new("."));
    for artifact_id in &spec.required_artifacts {
        let candidates = feature
            .artifacts
            .iter()
            .filter(|artifact| artifact.id == *artifact_id)
            .collect::<Vec<_>>();
        let Some(artifact) = (candidates.len() == 1).then(|| candidates[0]) else {
            report.blockers.push(if candidates.is_empty() {
                format!("hardware evidence lacks required artifact {artifact_id:?}")
            } else {
                format!("hardware evidence repeats artifact {artifact_id:?}")
            });
            continue;
        };
        let artifact_path = if artifact.path.is_absolute() {
            artifact.path.clone()
        } else {
            base.join(&artifact.path)
        };
        match crate::artifact_path_sha256(&artifact_path) {
            Ok(current) if current == artifact.sha256 => {}
            Ok(current) => report.blockers.push(format!(
                "hardware artifact {artifact_id:?} is stale: recorded {}, current {}",
                artifact.sha256, current
            )),
            Err(error) => report.blockers.push(format!(
                "cannot verify hardware artifact {artifact_id:?} at {}: {error}",
                artifact_path.display()
            )),
        }
    }
    report.status = if report.blockers.is_empty() {
        "passed"
    } else if report
        .blockers
        .iter()
        .any(|blocker| blocker.contains("stale"))
    {
        "stale"
    } else {
        "failed"
    }
    .to_owned();
    report
}
