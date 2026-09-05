//! Reviewed source-path descriptions attached to existing qualification roots.
//!
//! These values are reference metadata, never another readiness axis.

use super::{BTreeSet, Path, PathBuf, Result, fs, slug, validate_relative_path};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum SourceComposition {
    /// A production owner composes the operation within the declared scope.
    Production,
    /// A diagnostic owner composes the operation; this is not production support.
    Diagnostic,
    /// No composition implements the scope; hardware feasibility is not implied.
    Unimplemented,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub(crate) struct SourceContract {
    id: String,
    composition: SourceComposition,
    scope: String,
    limits: String,
    source_paths: Vec<PathBuf>,
}

pub(super) fn validate(contracts: &[SourceContract], root: &Path) -> Result<()> {
    let root = fs::canonicalize(root)?;
    let mut ids = BTreeSet::new();
    for contract in contracts {
        let id = slug(&contract.id, "source contract")?;
        if !ids.insert(id.clone()) {
            return Err(format!("duplicate source contract {id}").into());
        }
        if contract.scope.trim().is_empty() || contract.limits.trim().is_empty() {
            return Err(format!("source contract {id} requires scope and limits").into());
        }
        if contract.source_paths.is_empty() {
            return Err(format!("source contract {id} requires source-paths").into());
        }
        let mut paths = BTreeSet::new();
        for relative in &contract.source_paths {
            validate_relative_path(relative)?;
            if !paths.insert(relative) {
                return Err(format!("source contract {id} repeats {}", relative.display()).into());
            }
            let path = root.join(relative);
            if !fs::symlink_metadata(&path)?.file_type().is_file()
                || !fs::canonicalize(&path)?.starts_with(&root)
            {
                return Err(format!(
                    "source contract {id} reference must be a regular repository file: {}",
                    relative.display()
                )
                .into());
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
