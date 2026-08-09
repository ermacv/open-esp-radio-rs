//! Artifact input parsing and project-mode validation for linked IR export.

use std::{collections::BTreeSet, path::PathBuf};

use crate::{Result, source_id::is_source_id};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct IrArtifactInput {
    pub(crate) source: String,
    pub(crate) path: PathBuf,
    pub(crate) reviewed_code: Vec<crate::artifact::ReviewedCodeRange>,
}

#[cfg(test)]
pub(crate) fn named_artifact(source: &str, path: &str) -> Result<IrArtifactInput> {
    named_artifact_path(source, PathBuf::from(path))
}

pub(crate) fn named_artifact_path(source: &str, path: PathBuf) -> Result<IrArtifactInput> {
    if !is_source_id(source) {
        return Err(crate::Error::invalid(format!(
            "invalid artifact source id {source:?}"
        )));
    }
    if path.as_os_str().is_empty() {
        return Err(crate::Error::invalid("artifact path must not be empty"));
    }
    Ok(IrArtifactInput {
        source: source.to_owned(),
        path,
        reviewed_code: Vec::new(),
    })
}

pub(crate) fn validate_artifact_inputs(
    artifacts: &[IrArtifactInput],
    companions: &[PathBuf],
) -> Result<()> {
    if artifacts.is_empty() {
        return Err(crate::Error::invalid(
            "ir export requires at least one --artifact SOURCE=PATH",
        ));
    }
    if artifacts.len() > 1 && !companions.is_empty() {
        return Err(crate::Error::invalid(
            "--companion is only supported with one primary IR artifact",
        ));
    }
    let mut sources = BTreeSet::new();
    for artifact in artifacts {
        if !sources.insert(artifact.source.clone()) {
            return Err(crate::Error::invalid(format!(
                "duplicate artifact source {:?}",
                artifact.source
            )));
        }
    }
    Ok(())
}
