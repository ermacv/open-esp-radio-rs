//! Persistent evidence produced by one fail-closed multi-phase replay.

use std::path::Path;

use serde::{Deserialize, Serialize};

use super::REPLAY_EVIDENCE;
use crate::{Result, artifact_sha256, execution, execution_model};

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct ReplayArtifactIdentity {
    pub(crate) path: String,
    pub(crate) sha256: String,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct ReplayEvidenceDocument {
    pub(crate) schema_version: u32,
    pub(crate) command: &'static str,
    pub(crate) manifest: ReplayArtifactIdentity,
    pub(crate) artifact: ReplayArtifactIdentity,
    pub(crate) diagnostic_contracts: crate::DiagnosticContractsReport,
    pub(crate) phases: Vec<ReplayPhaseDocument>,
    pub(crate) complete: bool,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct ReplayPhaseDocument {
    pub(crate) name: String,
    pub(crate) symbol: String,
    pub(crate) completion: ReplayCompletionDocument,
    pub(crate) steps: u64,
    pub(crate) calls: Vec<ReplayCallDocument>,
    pub(crate) fifo_lifecycle: Vec<execution_model::FifoLifecycleEvent>,
    pub(crate) memory_observations: Vec<ReplayMemoryObservationDocument>,
    pub(crate) register_observations: Vec<ReplayRegisterObservation>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReplayRegisterObservation {
    pub(crate) sequence: usize,
    pub(crate) access: String,
    pub(crate) address: u32,
    pub(crate) width: u8,
    pub(crate) value: u32,
    pub(crate) pc: Option<u32>,
    pub(crate) region: String,
    pub(crate) register: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct ReplayMemoryObservationDocument {
    pub(crate) id: String,
    pub(crate) symbol: String,
    pub(crate) address: u32,
    pub(crate) width: u8,
    pub(crate) before: u32,
    pub(crate) after: u32,
    pub(crate) writes: Vec<ReplayMemoryWriteDocument>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct ReplayMemoryWriteDocument {
    pub(crate) site: u32,
    pub(crate) value: u32,
}

pub(crate) struct ReplayPhaseEvidence {
    pub(crate) execution: execution::ExecutionPhaseResult,
    pub(crate) memory_observations: Vec<ReplayMemoryObservationDocument>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub(crate) enum ReplayCompletionDocument {
    Returned,
    GoalReached {
        goal: execution_model::ExecutionGoal,
    },
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct ReplayCallDocument {
    pub(crate) site: u32,
    pub(crate) symbol: String,
    pub(crate) arguments: [u32; 8],
}

pub(crate) fn build_replay_evidence(
    manifest: &Path,
    artifact: &Path,
    diagnostic_contracts: crate::DiagnosticContractsReport,
    phases: Vec<ReplayPhaseEvidence>,
) -> Result<ReplayEvidenceDocument> {
    Ok(ReplayEvidenceDocument {
        schema_version: REPLAY_EVIDENCE.version,
        command: REPLAY_EVIDENCE.command,
        manifest: identity(manifest)?,
        artifact: identity(artifact)?,
        diagnostic_contracts,
        phases: phases
            .into_iter()
            .map(|phase| ReplayPhaseDocument {
                register_observations: phase
                    .execution
                    .result
                    .events
                    .iter()
                    .enumerate()
                    .filter_map(|(sequence, event)| {
                        let (access, width, address, value, region, register) = match event {
                            execution::ExecutionEvent::Read {
                                width,
                                address,
                                value,
                                region,
                                register,
                            } => ("read", width, address, value, region, register),
                            execution::ExecutionEvent::Write {
                                width,
                                address,
                                value,
                                region,
                                register,
                            } => ("write", width, address, value, region, register),
                            _ => return None,
                        };
                        Some(ReplayRegisterObservation {
                            sequence,
                            access: access.to_owned(),
                            width: *width,
                            address: *address,
                            value: *value,
                            pc: None,
                            region: region.clone(),
                            register: register.clone(),
                        })
                    })
                    .collect(),
                name: phase.execution.name,
                symbol: phase.execution.symbol,
                completion: match phase.execution.result.completion {
                    execution::ExecutionCompletion::Returned => ReplayCompletionDocument::Returned,
                    execution::ExecutionCompletion::GoalReached(goal) => {
                        ReplayCompletionDocument::GoalReached { goal }
                    }
                },
                steps: phase.execution.result.steps,
                calls: phase
                    .execution
                    .result
                    .ordered_calls
                    .into_iter()
                    .map(|call| ReplayCallDocument {
                        site: call.site,
                        symbol: call.symbol,
                        arguments: call.arguments,
                    })
                    .collect(),
                fifo_lifecycle: phase.execution.result.fifo_lifecycle,
                memory_observations: phase.memory_observations,
            })
            .collect(),
        complete: true,
    })
}

pub(crate) fn render_replay_evidence(document: &ReplayEvidenceDocument) -> Result<String> {
    Ok(format!("{}\n", serde_json::to_string(document)?))
}

fn identity(path: &Path) -> Result<ReplayArtifactIdentity> {
    let canonical = std::fs::canonicalize(path)
        .map_err(|error| crate::Error::read("replay input", path, error))?;
    Ok(ReplayArtifactIdentity {
        path: canonical.display().to_string(),
        sha256: artifact_sha256(&canonical)?,
    })
}
