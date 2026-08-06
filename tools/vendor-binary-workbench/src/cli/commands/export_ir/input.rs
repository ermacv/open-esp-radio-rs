//! Artifact input parsing and project-mode validation for linked IR export.

use std::{collections::BTreeSet, path::PathBuf};

use crate::Result;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct IrArtifactInput {
    pub(super) source: String,
    pub(super) path: PathBuf,
}

pub(super) fn valid_source_id(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

pub(super) fn named_artifact(source: &str, path: &str) -> Result<IrArtifactInput> {
    named_artifact_path(source, PathBuf::from(path))
}

pub(super) fn named_artifact_path(source: &str, path: PathBuf) -> Result<IrArtifactInput> {
    if !valid_source_id(source) {
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

pub(super) fn parse_artifact(value: &str) -> Result<IrArtifactInput> {
    let (source, path) = value
        .split_once('=')
        .ok_or("--artifact requires SOURCE=PATH")?;
    named_artifact(source, path)
}

pub(super) fn source_artifact_option(argument: &str) -> Option<&str> {
    argument
        .strip_prefix("--source-artifact:")
        .filter(|source| !source.is_empty())
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
