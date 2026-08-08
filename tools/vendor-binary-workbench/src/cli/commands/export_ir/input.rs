//! Artifact input parsing and project-mode validation for linked IR export.

use std::{collections::BTreeSet, path::PathBuf};

use crate::{Result, cli::SourcePath, source_id::is_source_id};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct IrArtifactInput {
    pub(super) source: String,
    pub(super) path: PathBuf,
}

#[cfg(test)]
pub(super) fn named_artifact(source: &str, path: &str) -> Result<IrArtifactInput> {
    named_artifact_path(source, PathBuf::from(path))
}

pub(super) fn named_artifact_path(source: &str, path: PathBuf) -> Result<IrArtifactInput> {
    if !is_source_id(source) {
        return Err(format!("invalid artifact source id {source:?}").into());
    }
    if path.as_os_str().is_empty() {
        return Err("artifact path must not be empty".into());
    }
    Ok(IrArtifactInput {
        source: source.to_owned(),
        path,
    })
}

impl From<SourcePath> for IrArtifactInput {
    fn from(value: SourcePath) -> Self {
        Self {
            source: value.source.into_string(),
            path: value.path,
        }
    }
}

pub(super) fn validate_artifact_inputs(
    artifacts: &[IrArtifactInput],
    companions: &[PathBuf],
) -> Result<()> {
    if artifacts.is_empty() {
        return Err("ir export requires at least one --artifact SOURCE=PATH".into());
    }
    if artifacts.len() > 1 && !companions.is_empty() {
        return Err("--companion is only supported with one primary IR artifact".into());
    }
    let mut sources = BTreeSet::new();
    for artifact in artifacts {
        if !sources.insert(artifact.source.clone()) {
            return Err(format!("duplicate artifact source {:?}", artifact.source).into());
        }
    }
    Ok(())
}
