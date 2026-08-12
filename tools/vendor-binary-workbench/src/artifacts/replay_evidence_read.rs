//! Strict consumer for schema-v1 concrete replay evidence.

#![allow(
    dead_code,
    reason = "complete stored DTOs enforce every persistent replay field"
)]

use std::path::Path;

use serde::Deserialize;

use crate::{Result, execution_model};

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredReplayEvidence {
    schema_version: u32,
    command: String,
    pub(crate) manifest: StoredReplayIdentity,
    pub(crate) artifact: StoredReplayIdentity,
    pub(crate) phases: Vec<StoredReplayPhase>,
    pub(crate) complete: bool,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredReplayIdentity {
    pub(crate) path: String,
    pub(crate) sha256: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredReplayPhase {
    pub(crate) name: String,
    pub(crate) symbol: String,
    pub(crate) completion: StoredReplayCompletion,
    pub(crate) steps: u64,
    pub(crate) calls: Vec<StoredReplayCall>,
    pub(crate) fifo_lifecycle: Vec<StoredFifoLifecycleEvent>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub(crate) enum StoredReplayCompletion {
    Returned,
    GoalReached {
        goal: execution_model::ExecutionGoal,
    },
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredReplayCall {
    pub(crate) site: u32,
    pub(crate) symbol: String,
    pub(crate) arguments: [u32; 8],
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub(crate) enum StoredFifoLifecycleEvent {
    Enqueued {
        service_id: String,
        site: u32,
        value: u32,
        depth_before: usize,
        depth_after: usize,
        woke_receiver: bool,
    },
    Dequeued {
        service_id: String,
        site: u32,
        value: u32,
        depth_before: usize,
        depth_after: usize,
    },
    Full {
        service_id: String,
        site: u32,
        value: u32,
        depth: usize,
    },
    Empty {
        service_id: String,
        site: u32,
    },
    Length {
        service_id: String,
        site: u32,
        depth: usize,
    },
}

impl StoredReplayEvidence {
    pub(crate) fn phase(&self, name: &str) -> Result<&StoredReplayPhase> {
        let matches = self
            .phases
            .iter()
            .filter(|phase| phase.name == name)
            .collect::<Vec<_>>();
        match matches.as_slice() {
            [phase] => Ok(*phase),
            [] => Err(crate::Error::invalid(format!(
                "replay evidence has no phase {name:?}"
            ))),
            _ => Err(crate::Error::invalid(format!(
                "replay evidence duplicates phase {name:?}"
            ))),
        }
    }

    pub(crate) fn validate_freshness(&self) -> Result<()> {
        validate_identity("manifest", &self.manifest)?;
        validate_identity("artifact", &self.artifact)?;
        if !self.complete {
            return Err(crate::Error::invalid(
                "replay evidence does not make a complete concrete-execution claim",
            ));
        }
        Ok(())
    }
}

pub(crate) fn parse_replay_evidence(input: &str) -> Result<StoredReplayEvidence> {
    super::expect_identity(input, super::REPLAY_EVIDENCE)?;
    let document: StoredReplayEvidence = serde_json::from_str(input)?;
    if document.phases.is_empty() {
        return Err(crate::Error::invalid("replay evidence has no phases"));
    }
    document.validate_freshness()?;
    Ok(document)
}

fn validate_identity(kind: &str, identity: &StoredReplayIdentity) -> Result<()> {
    let path = Path::new(&identity.path);
    let actual = crate::artifact_sha256(path).map_err(|error| {
        crate::Error::invalid(format!(
            "cannot validate replay {kind} {}: {error}",
            path.display()
        ))
    })?;
    if actual != identity.sha256 {
        return Err(crate::Error::invalid(format!(
            "replay {kind} changed since execution: {}",
            path.display()
        )));
    }
    Ok(())
}
