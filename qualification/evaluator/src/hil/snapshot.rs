//! Independent validation of captured source identities and every archived byte.
use super::*;
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

// The producer writes the manifest with the same types, so its fields and
// their order, which a source's identity digests, are one contract.
pub(super) use oer_hil_schema::snapshot::{MANIFEST_SCHEMA, Manifest};

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
    if manifest.schema != MANIFEST_SCHEMA
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
/// The closure of the checkout at `root`, computed once per evaluation;
/// `None` when Cargo cannot list it, and then every file is compared.
fn closure_of(root: &Path) -> Option<std::sync::Arc<super::closure::Closure>> {
    static CLOSURES: std::sync::Mutex<
        BTreeMap<PathBuf, Option<std::sync::Arc<super::closure::Closure>>>,
    > = std::sync::Mutex::new(BTreeMap::new());
    let root = root.canonicalize().ok()?;
    CLOSURES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .entry(root.clone())
        .or_insert_with(|| {
            super::closure::Closure::of(&root)
                .ok()
                .map(std::sync::Arc::new)
        })
        .clone()
}

/// Whether the run's snapshot is the checkout's current state in every file
/// that can change the observation (see [`super::closure`]).
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
    let closure = closure_of(root);
    let relevant = |path: &Path| {
        closure
            .as_ref()
            .is_none_or(|closure| closure.contains(path))
    };
    let tracked = std::str::from_utf8(&output.stdout)?
        .split('\0')
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .filter(|path| relevant(path))
        .collect::<BTreeSet<_>>();
    let captured = repository
        .files
        .iter()
        .filter(|file| relevant(&file.path))
        .collect::<Vec<_>>();
    if tracked != captured.iter().map(|f| f.path.clone()).collect() {
        return Ok(false);
    }
    for file in captured {
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

#[cfg(test)]
mod tests {
    use oer_hil_schema::snapshot::SourceInput;

    /// The source identity is the digest of the reserialized manifest entry,
    /// so an entry must read back to the producer's exact bytes.
    #[test]
    fn a_source_entry_reserializes_to_the_producers_bytes() {
        let file = r#"{"path":"a.rs","size_bytes":1,"sha256":"00","mode":420}"#;
        for entry in [
            format!(r#"{{"name":"repository","commit":"c","dirty":true,"files":[{file}]}}"#),
            format!(
                r#"{{"name":"repository","commit":"c","dirty":true,"files":[{file}],"untracked":[{{"path":"a.rs","by":"source-include"}},{{"path":"b.rs","by":"image-package"}}]}}"#
            ),
        ] {
            let parsed: SourceInput = serde_json::from_str(&entry).unwrap();
            assert_eq!(serde_json::to_string(&parsed).unwrap(), entry);
        }
        let unknown = format!(
            r#"{{"name":"repository","commit":"c","dirty":true,"files":[{file}],"untracked":[{{"path":"a.rs","by":"guess"}}]}}"#
        );
        assert!(serde_json::from_str::<SourceInput>(&unknown).is_err());
    }
}
