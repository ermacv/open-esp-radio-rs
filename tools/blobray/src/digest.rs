//! Content identity shared by discovery reports and verification evidence.

use std::{
    fs,
    path::{Path, PathBuf},
};

use sha2::{Digest, Sha256};

use crate::Result;

pub(crate) fn artifact_sha256(path: &Path) -> Result<String> {
    let bytes = fs::read(path)?;
    Ok(format!("{:x}", Sha256::digest(&bytes)))
}

/// Content identity for either one artifact or a deterministic generated tree.
/// Directory entries are ordered and their relative paths are part of the
/// digest, so moving the tree does not change its identity while renaming a
/// member does.
pub(crate) fn artifact_path_sha256(path: &Path) -> Result<String> {
    if path.is_file() {
        return artifact_sha256(path);
    }
    if !path.is_dir() {
        return Err(crate::Error::invalid(format!(
            "artifact path does not exist: {}",
            path.display()
        )));
    }
    let mut files = Vec::new();
    collect_files(path, path, &mut files)?;
    files.sort();
    let mut digest = Sha256::new();
    digest.update(b"blobray-artifact-tree-v1\0");
    for relative in files {
        digest.update(relative.to_string_lossy().as_bytes());
        digest.update([0]);
        let bytes = fs::read(path.join(&relative))?;
        digest.update((bytes.len() as u64).to_le_bytes());
        digest.update(bytes);
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn collect_files(root: &Path, directory: &Path, output: &mut Vec<PathBuf>) -> Result<()> {
    let mut entries = fs::read_dir(directory)?.collect::<std::io::Result<Vec<_>>>()?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            collect_files(root, &path, output)?;
        } else if path.is_file() {
            output.push(
                path.strip_prefix(root)
                    .map_err(|_| crate::Error::invalid("artifact tree escaped its root"))?
                    .to_owned(),
            );
        }
    }
    Ok(())
}
