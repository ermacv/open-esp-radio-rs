//! The one reader and writer of shard files.
use crate::{Index, Result, SHARD_EXTENSION};
use std::path::{Path, PathBuf};

/// The file of `scenario`'s shard in `directory`.
pub fn path(directory: &Path, scenario: &str) -> PathBuf {
    directory.join(format!("{scenario}.{SHARD_EXTENSION}"))
}

/// Write `index` as its scenario's shard of the index at `directory`,
/// creating the directory; returns the file.
pub fn write(directory: &Path, index: &Index) -> Result<PathBuf> {
    std::fs::create_dir_all(directory)?;
    let path = path(directory, &index.scenario);
    let mut bytes = serde_json::to_vec_pretty(index)?;
    bytes.push(b'\n');
    std::fs::write(&path, bytes)?;
    Ok(path)
}

/// The shard `name` in `directory`, when it exists and parses. A shard git
/// left conflicted, or one of another schema, reads as none.
pub fn read(directory: &Path, name: &str) -> Option<Index> {
    std::fs::read_to_string(path(directory, name))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
}

/// The scenario names of the shard files in `directory`, sorted; a missing
/// directory holds none.
pub fn names(directory: &Path) -> Result<Vec<String>> {
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(error) => return Err(error.into()),
    };
    let mut names = vec![];
    for entry in entries {
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()) != Some(SHARD_EXTENSION) {
            continue;
        }
        names.push(
            path.file_stem()
                .and_then(|s| s.to_str())
                .ok_or("shard without a name")?
                .to_owned(),
        );
    }
    names.sort();
    Ok(names)
}

/// Every shard of `directory` that parses, by name.
pub fn shards(directory: &Path) -> Result<Vec<Index>> {
    Ok(names(directory)?
        .iter()
        .filter_map(|name| read(directory, name))
        .collect())
}

/// Scenario names of the shards in `directory` below `root` that are stale
/// or unreadable: a shard is current only when it parses, names its own
/// file and every recorded source keeps its digest ([`Index::is_current`]).
pub fn stale(root: &Path, directory: &Path) -> Result<Vec<String>> {
    let directory = root.join(directory);
    Ok(names(&directory)?
        .into_iter()
        .filter(|name| {
            !read(&directory, name)
                .is_some_and(|shard| shard.scenario == *name && shard.is_current(root))
        })
        .collect())
}

/// Whether `shard` records one of the `changed` repository paths: a
/// recorded file itself, a file below a recorded directory, or a changed
/// decision file whose decisions that apply to the shard differ from those
/// it recorded.
pub fn records_any(root: &Path, shard: &Index, changed: &[PathBuf]) -> bool {
    shard.sources.iter().any(|source| {
        changed
            .iter()
            .any(|path| path == &source.path || path.starts_with(&source.path))
    }) || shard
        .dependence
        .coverage_decisions
        .as_ref()
        .is_some_and(|decisions| {
            changed.contains(&decisions.path)
                && !crate::CoverageDecisions::read(root, &decisions.path)
                    .is_ok_and(|file| file.applicable_digest(&shard.functions) == decisions.sha256)
        })
}

#[cfg(test)]
pub(crate) mod tests;
