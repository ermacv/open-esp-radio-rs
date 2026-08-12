//! Persistent evidence produced by one fail-closed multi-phase replay.

use std::path::Path;

use serde::Serialize;

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
    phases: Vec<execution::ExecutionPhaseResult>,
) -> Result<ReplayEvidenceDocument> {
    Ok(ReplayEvidenceDocument {
        schema_version: REPLAY_EVIDENCE.version,
        command: REPLAY_EVIDENCE.command,
        manifest: identity(manifest)?,
        artifact: identity(artifact)?,
        phases: phases
            .into_iter()
            .map(|phase| ReplayPhaseDocument {
                name: phase.name,
                symbol: phase.symbol,
                completion: match phase.result.completion {
                    execution::ExecutionCompletion::Returned => ReplayCompletionDocument::Returned,
                    execution::ExecutionCompletion::GoalReached(goal) => {
                        ReplayCompletionDocument::GoalReached { goal }
                    }
                },
                steps: phase.result.steps,
                calls: phase
                    .result
                    .ordered_calls
                    .into_iter()
                    .map(|call| ReplayCallDocument {
                        site: call.site,
                        symbol: call.symbol,
                        arguments: call.arguments,
                    })
                    .collect(),
                fifo_lifecycle: phase.result.fifo_lifecycle,
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
