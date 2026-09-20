//! Independent validation of captured source identities and every archived byte.
use super::*;
use serde::Serialize;
use serde_json::Value;

#[derive(Deserialize, PartialEq)]
pub(super) struct Source {
    pub(super) name: String,
    pub(super) commit: String,
    pub(super) dirty: bool,
    pub(super) workspace_sha256: String,
    pub(super) rebuild_status: String,
    pub(super) limitations: Vec<Value>,
    pub(super) untracked_files: Vec<Value>,
    pub(super) tracked_patch_path: Option<PathBuf>,
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

// Serialization order is the producer's v1 snapshot identity contract.
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct FileInput {
    pub(super) path: PathBuf,
    pub(super) size_bytes: u64,
    pub(super) sha256: String,
    pub(super) mode: u32,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SourceInput {
    pub(super) name: String,
    commit: String,
    dirty: bool,
    pub(super) files: Vec<FileInput>,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Manifest {
    schema: u16,
    pub(super) sources: Vec<SourceInput>,
}
#[derive(Deserialize)]
struct Snapshot {
    schema: u16,
    snapshot_id: String,
    archive_sha256: String,
    files: usize,
}

pub(super) fn verified(directory: &Path, sources: &[Source]) -> Result<Option<Manifest>> {
    let manifest: Manifest = read_json(&directory.join("manifest.json"))?;
    let snapshot: Snapshot = read_json(&directory.join("snapshot.json"))?;
    if manifest.schema != 1
        || snapshot.schema != 1
        || digest(&serde_json::to_vec(&manifest)?) != snapshot.snapshot_id
        || sha256_file(&directory.join("sources.tar"))? != snapshot.archive_sha256
        || sources.len() != manifest.sources.len()
    {
        return Ok(None);
    }
    let mut expected = BTreeMap::new();
    for (captured, source) in manifest.sources.iter().zip(sources) {
        if captured.name != source.name
            || captured.commit != source.commit
            || captured.dirty != source.dirty
            || digest(&serde_json::to_vec(captured)?) != source.workspace_sha256
        {
            return Ok(None);
        }
        for file in &captured.files {
            if !safe_relative(&file.path)
                || !valid_sha256(&file.sha256)
                || !matches!(file.mode, 0o644 | 0o755)
                || expected
                    .insert(PathBuf::from(&captured.name).join(&file.path), file)
                    .is_some()
            {
                return Ok(None);
            }
        }
    }
    if snapshot.files != expected.len() {
        return Ok(None);
    }
    // Check all archived bytes without extracting anything into the filesystem.
    let mut archive = tar::Archive::new(fs::File::open(directory.join("sources.tar"))?);
    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.into_owned();
        let Some(file) = expected.remove(&path) else {
            return Ok(None);
        };
        if !entry.header().entry_type().is_file()
            || entry.size() != file.size_bytes
            || entry.header().mode()? != file.mode
        {
            return Ok(None);
        }
        let mut hash = Sha256::new();
        std::io::copy(&mut entry, &mut hash)?;
        if format!("{:x}", hash.finalize()) != file.sha256 {
            return Ok(None);
        }
    }
    Ok(expected.is_empty().then_some(manifest))
}

/// A direct observation binds the complete current source selection. Property
/// reviews deliberately bind a narrower, explicitly reviewed set elsewhere.
pub(super) fn current(root: &Path, run: &Path, sources: &[Source]) -> Result<bool> {
    let directory = run.join("source/snapshot");
    // Reject known differences before scanning the archive a second time. The
    // outer seal is already checked; acceptance still validates every byte below.
    let manifest: Manifest = read_json(&directory.join("manifest.json"))?;
    let Some(repository) = manifest.sources.first() else {
        return Ok(false);
    };
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args([
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "-z",
        ])
        .output()?;
    if !output.status.success() {
        return Ok(false);
    }
    let tracked = std::str::from_utf8(&output.stdout)?
        .split('\0')
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .collect::<BTreeSet<_>>();
    if tracked != repository.files.iter().map(|f| f.path.clone()).collect() {
        return Ok(false);
    }
    for file in &repository.files {
        let Some(identity) = super::subject::file(root, &file.path)? else {
            return Ok(false);
        };
        if identity.size_bytes != file.size_bytes || identity.sha256 != file.sha256 {
            return Ok(false);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = if fs::metadata(root.join(&file.path))?.permissions().mode() & 0o111 == 0 {
                0o644
            } else {
                0o755
            };
            if mode != file.mode {
                return Ok(false);
            }
        }
    }
    Ok(verified(&directory, sources)?.is_some())
}
