//! Structural validation of the canonical HIL run documents.

use super::{
    MeasurementVerdict, Outcome, RUN_SCHEMA, RunManifest, RunState, SuiteCounts, SuiteResult,
    aggregate_outcome,
};
use crate::Result;
use serde::Deserialize;
use std::{collections::BTreeSet, fs, path::Path};

pub(crate) fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T> {
    serde_json::from_slice(&fs::read(path)?)
        .map_err(|error| format!("invalid HIL report `{}`: {error}", path.display()).into())
}

pub(crate) fn validate_manifest(
    manifest: &RunManifest,
    target: &str,
    directory: &Path,
) -> Result<()> {
    if manifest.schema != RUN_SCHEMA {
        return Err(format!(
            "HIL manifest `{}` has schema {}, expected {RUN_SCHEMA}",
            directory.display(),
            manifest.schema
        )
        .into());
    }
    if manifest.target != target {
        return Err(format!(
            "HIL manifest `{}` targets `{}`, expected `{target}`",
            directory.display(),
            manifest.target
        )
        .into());
    }
    let timestamps_are_consistent = match manifest.state {
        RunState::Running => {
            manifest.finished_unix_millis.is_none() && manifest.duration_millis.is_none()
        }
        RunState::Completed | RunState::Interrupted => {
            manifest.finished_unix_millis.is_some()
                && manifest.duration_millis.is_some()
                && manifest.finished_unix_millis >= Some(manifest.started_unix_millis)
        }
    };
    if !timestamps_are_consistent {
        return Err(format!(
            "HIL manifest `{}` has timestamps inconsistent with state {:?}",
            directory.display(),
            manifest.state
        )
        .into());
    }
    let directory_id = directory
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("HIL run directory is not UTF-8: {}", directory.display()))?;
    if manifest.run_id != directory_id {
        return Err(format!(
            "HIL manifest run ID `{}` does not match directory `{directory_id}`",
            manifest.run_id
        )
        .into());
    }
    Ok(())
}

pub(crate) fn validate_suite(suite: &SuiteResult, manifest: &RunManifest) -> Result<()> {
    if suite.schema != RUN_SCHEMA {
        return Err(format!(
            "HIL suite `{}` has schema {}, expected {RUN_SCHEMA}",
            suite.run_id, suite.schema
        )
        .into());
    }
    if suite.run_id != manifest.run_id
        || suite.target != manifest.target
        || suite.started_unix_millis != manifest.started_unix_millis
        || Some(suite.finished_unix_millis) != manifest.finished_unix_millis
        || Some(suite.duration_millis) != manifest.duration_millis
    {
        return Err(format!(
            "HIL suite `{}` does not match its manifest",
            manifest.run_id
        )
        .into());
    }
    if suite.counts != SuiteCounts::from_results(&suite.scenarios) {
        return Err(format!("HIL suite `{}` has inconsistent counts", suite.run_id).into());
    }
    let expected_outcome = if suite
        .scenarios
        .iter()
        .all(|scenario| scenario.outcome.is_passed())
    {
        Outcome::Passed
    } else {
        Outcome::Failed
    };
    if suite.outcome != expected_outcome {
        return Err(format!(
            "HIL suite `{}` has an outcome inconsistent with its scenarios",
            suite.run_id
        )
        .into());
    }
    for scenario in &suite.scenarios {
        if scenario.schema != RUN_SCHEMA {
            return Err(format!(
                "HIL scenario `{}` has schema {}, expected {RUN_SCHEMA}",
                scenario.scenario, scenario.schema
            )
            .into());
        }
        if scenario.repetitions.is_empty() {
            if scenario.outcome != Outcome::Blocked || scenario.failure.is_none() {
                return Err(format!(
                    "HIL scenario `{}` has no repetitions without a blocking failure",
                    scenario.scenario
                )
                .into());
            }
            continue;
        }
        if scenario.repetitions.len() != usize::from(scenario.required_repetitions)
            || scenario.outcome
                != aggregate_outcome(scenario.repetitions.iter().map(|entry| entry.outcome))
        {
            return Err(format!(
                "HIL scenario `{}` has inconsistent repetition results",
                scenario.scenario
            )
            .into());
        }
        for (index, repetition) in scenario.repetitions.iter().enumerate() {
            if repetition.schema != RUN_SCHEMA {
                return Err(format!(
                    "HIL scenario `{}` repetition {} has schema {}, expected {RUN_SCHEMA}",
                    scenario.scenario, repetition.repetition, repetition.schema
                )
                .into());
            }
            if usize::from(repetition.repetition) != index + 1 {
                return Err(format!(
                    "HIL scenario `{}` has a non-canonical repetition sequence",
                    scenario.scenario
                )
                .into());
            }
            if (repetition.outcome == Outcome::Passed && repetition.failure.is_some())
                || (matches!(
                    repetition.outcome,
                    Outcome::Failed | Outcome::Broken | Outcome::Blocked | Outcome::Interrupted
                ) && repetition.failure.is_none())
            {
                return Err(format!(
                    "HIL scenario `{}` repetition {} has an inconsistent failure record",
                    scenario.scenario, repetition.repetition
                )
                .into());
            }
            let mut names = BTreeSet::new();
            for measurement in &repetition.measurements {
                if measurement.name.is_empty()
                    || measurement.name.len() > 128
                    || !measurement.name.bytes().all(|byte| {
                        byte.is_ascii_lowercase()
                            || byte.is_ascii_digit()
                            || matches!(byte, b'.' | b'-')
                    })
                    || !names.insert(&measurement.name)
                    || !measurement.is_consistent()
                {
                    return Err(format!(
                        "HIL scenario `{}` repetition {} has invalid measurements",
                        scenario.scenario, repetition.repetition
                    )
                    .into());
                }
            }
            if repetition
                .measurements
                .iter()
                .any(|measurement| measurement.verdict == Some(MeasurementVerdict::Failed))
                && repetition.outcome != Outcome::Failed
            {
                return Err(format!(
                    "HIL scenario `{}` repetition {} passed with a failed measurement verdict",
                    scenario.scenario, repetition.repetition
                )
                .into());
            }
        }
    }
    Ok(())
}
