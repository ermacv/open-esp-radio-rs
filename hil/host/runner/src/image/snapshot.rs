//! Explicit, content-addressed source input capture, independent of firmware builds.
//!
//! Tracked files are included automatically. Every nonignored untracked file
//! must be named explicitly before any content is archived. This is a source
//! snapshot, not a hermetic build or a qualification decision.

use crate::{Result, durable::atomic_json};
use oer_process::CommandExt as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Component, Path, PathBuf},
    process::Command,
};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct FileInput {
    path: PathBuf,
    size_bytes: u64,
    sha256: String,
    mode: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct SourceInput {
    pub(crate) name: String,
    pub(crate) commit: String,
    pub(crate) dirty: bool,
    files: Vec<FileInput>,
}

#[derive(Deserialize, Serialize)]
struct Manifest {
    schema: u16,
    sources: Vec<SourceInput>,
}

#[derive(Deserialize, Serialize)]
pub(crate) struct Snapshot {
    schema: u16,
    snapshot_id: String,
    directory: PathBuf,
    archive_sha256: String,
    files: usize,
}

impl Snapshot {
    pub(crate) fn directory(&self) -> &Path {
        &self.directory
    }
}

impl SourceInput {
    pub(crate) fn identity(&self) -> Result<String> {
        Ok(digest(&serde_json::to_vec(self)?))
    }
}

/// Owns a verified private checkout. No build falls back to the live repository.
pub(crate) struct FrozenSources {
    isolated: tempfile::TempDir,
    snapshot: Snapshot,
    manifest: Manifest,
}

impl FrozenSources {
    pub(crate) fn open(directory: &Path) -> Result<Self> {
        let isolated = tempfile::Builder::new()
            .prefix("oer-source-build-")
            .tempdir()?;
        let (snapshot, manifest) = materialize::materialize(directory, isolated.path())?;
        Ok(Self {
            isolated,
            snapshot,
            manifest,
        })
    }

    pub(crate) fn repository(&self) -> PathBuf {
        self.isolated.path().join("repository")
    }

    pub(crate) fn sources(&self) -> &[SourceInput] {
        &self.manifest.sources
    }

    pub(crate) fn verify_unchanged(&self) -> Result<()> {
        for source in &self.manifest.sources {
            for file in &source.files {
                let path = self.isolated.path().join(&source.name).join(&file.path);
                let metadata = fs::symlink_metadata(&path)?;
                if !metadata.is_file()
                    || metadata.len() != file.size_bytes
                    || crate::durable::sha256_file(&path)? != file.sha256
                {
                    return Err(format!(
                        "frozen build input changed: {}:{}",
                        source.name,
                        file.path.display()
                    )
                    .into());
                }
            }
        }
        Ok(())
    }
}

struct Selection {
    name: String,
    root: PathBuf,
    commit: String,
    dirty: bool,
    tracked: BTreeSet<PathBuf>,
    untracked: BTreeSet<PathBuf>,
}

pub(crate) fn capture(root: &Path, include: &[String]) -> Result<Snapshot> {
    let mut roots = vec![("repository".to_owned(), root.canonicalize()?)];
    for (name, variable) in [
        ("esp-hal", "ESP_HAL_ROOT"),
        ("embassy", "EMBASSY_ROOT"),
        ("xarxa", "OPEN_RADIO_XARXA_ROOT"),
    ] {
        if let Some(path) = std::env::var_os(variable) {
            roots.push((name.into(), PathBuf::from(path).canonicalize()?));
        }
    }
    capture_roots(
        &roots,
        include,
        &root.join("target/hil/esp32s31/source-snapshots"),
    )
}

fn git(root: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .supervised_output()?;
    if !output.status.success() {
        return Err(format!(
            "source snapshot Git query failed in {}: {}",
            root.display(),
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    Ok(output.stdout)
}

fn paths(bytes: &[u8]) -> Result<BTreeSet<PathBuf>> {
    bytes
        .split(|b| *b == 0)
        .filter(|p| !p.is_empty())
        .map(|p| {
            let path = PathBuf::from(std::str::from_utf8(p)?);
            contained(&path)?;
            Ok(path)
        })
        .collect()
}

fn contained(path: &Path) -> Result<()> {
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err(format!(
            "snapshot input must be a contained relative file: {}",
            path.display()
        )
        .into());
    }
    Ok(())
}

fn select(name: &str, root: &Path) -> Result<Selection> {
    let top = String::from_utf8(git(root, &["rev-parse", "--show-toplevel"])?)?;
    if Path::new(top.trim()).canonicalize()? != root.canonicalize()? {
        return Err(format!("snapshot source {name} must name its repository root").into());
    }
    let commit = String::from_utf8(git(root, &["rev-parse", "HEAD"])?)?
        .trim()
        .to_owned();
    let tracked = paths(&git(root, &["ls-files", "--cached", "-z"])?)?;
    let untracked = paths(&git(
        root,
        &["ls-files", "--others", "--exclude-standard", "-z"],
    )?)?;
    let dirty = !git(
        root,
        &["status", "--porcelain=v1", "--untracked-files=normal"],
    )?
    .is_empty();
    Ok(Selection {
        name: name.into(),
        root: root.into(),
        commit,
        dirty,
        tracked,
        untracked,
    })
}

fn capture_roots(
    roots: &[(String, PathBuf)],
    include: &[String],
    output: &Path,
) -> Result<Snapshot> {
    let selections = roots
        .iter()
        .map(|(name, root)| select(name, root))
        .collect::<Result<Vec<_>>>()?;
    let mut accepted = BTreeMap::<String, BTreeSet<PathBuf>>::new();
    for value in include {
        let (role, file) = value.split_once(':').unwrap_or(("repository", value));
        let path = PathBuf::from(file);
        contained(&path)?;
        let source = selections
            .iter()
            .find(|s| s.name == role)
            .ok_or_else(|| format!("unknown snapshot source role {role}"))?;
        if !source.untracked.contains(&path) {
            return Err(format!("--source-include is not an untracked file: {role}:{file}").into());
        }
        if !accepted.entry(role.into()).or_default().insert(path) {
            return Err(format!("duplicate --source-include: {role}:{file}").into());
        }
    }
    let unresolved = selections
        .iter()
        .flat_map(|s| {
            s.untracked
                .iter()
                .filter(|p| !accepted.get(&s.name).is_some_and(|v| v.contains(*p)))
                .map(|p| format!("{}:{}", s.name, p.display()))
        })
        .collect::<Vec<_>>();
    if !unresolved.is_empty() {
        return Err(format!(
            "source snapshot blocked: explicitly include or resolve these untracked files:\n{}",
            unresolved.join("\n")
        )
        .into());
    }
    // No source bytes are archived until selection is complete for every root.
    fs::create_dir_all(output)?;
    let staging = tempfile::Builder::new()
        .prefix(".capture-")
        .tempdir_in(output)?;
    let archive_path = staging.path().join("sources.tar");
    let mut archive = tar::Builder::new(fs::File::create(&archive_path)?);
    let mut sources = Vec::new();
    for selection in &selections {
        sources.push(read_source(selection, |file, bytes| {
            let mut header = tar::Header::new_gnu();
            header.set_size(file.size_bytes);
            header.set_mode(file.mode);
            header.set_uid(0);
            header.set_gid(0);
            header.set_mtime(0);
            header.set_cksum();
            archive.append_data(
                &mut header,
                Path::new(&selection.name).join(&file.path),
                bytes,
            )?;
            Ok(())
        })?);
    }
    archive.finish()?;
    archive.into_inner()?.sync_all()?;
    // Re-read bytes and membership, not just Git diff/stat caching. The archive
    // itself is authoritative; a changed capture is rejected, never relabelled.
    for (selection, captured) in selections.iter().zip(&sources) {
        let after = select(&selection.name, &selection.root)?;
        if after.tracked != selection.tracked
            || after.untracked != selection.untracked
            || read_source(&after, |_, _| Ok(()))? != *captured
        {
            return Err(
                format!("source {} changed during snapshot capture", selection.name).into(),
            );
        }
    }
    let manifest = Manifest { schema: 1, sources };
    let snapshot_id = digest(&serde_json::to_vec(&manifest)?);
    let archive_sha256 = crate::durable::sha256_file(&archive_path)?;
    atomic_json(&staging.path().join("manifest.json"), &manifest)?;
    let snapshot = Snapshot {
        schema: 1,
        directory: output.join(&snapshot_id),
        snapshot_id,
        archive_sha256,
        files: manifest.sources.iter().map(|s| s.files.len()).sum(),
    };
    atomic_json(&staging.path().join("snapshot.json"), &snapshot)?;
    if snapshot.directory.exists() {
        if fs::read(snapshot.directory.join("manifest.json"))?
            != fs::read(staging.path().join("manifest.json"))?
            || fs::read(snapshot.directory.join("snapshot.json"))?
                != fs::read(staging.path().join("snapshot.json"))?
            || crate::durable::sha256_file(&snapshot.directory.join("sources.tar"))?
                != snapshot.archive_sha256
        {
            return Err("existing source snapshot has conflicting or corrupted content".into());
        }
    } else {
        fs::rename(staging.path(), &snapshot.directory)?;
        fs::File::open(output)?.sync_all()?;
    }
    Ok(snapshot)
}

fn read_source(
    selection: &Selection,
    mut consume: impl FnMut(&FileInput, &[u8]) -> Result<()>,
) -> Result<SourceInput> {
    let mut files = Vec::new();
    for relative in selection.tracked.union(&selection.untracked) {
        let mut path = selection.root.clone();
        let mut deleted = false;
        for component in relative.components() {
            path.push(component);
            match fs::symlink_metadata(&path) {
                Err(error)
                    if error.kind() == std::io::ErrorKind::NotFound
                        && selection.tracked.contains(relative) =>
                {
                    deleted = true;
                    break;
                }
                Err(error) => return Err(error.into()),
                Ok(metadata) if metadata.file_type().is_symlink() => {
                    return Err(format!(
                        "snapshot requires review of symlink source: {}:{}",
                        selection.name,
                        relative.display()
                    )
                    .into());
                }
                Ok(_) => {}
            }
        }
        if deleted {
            continue;
        }
        let metadata = fs::symlink_metadata(&path)?;
        if !metadata.is_file() {
            return Err(format!(
                "snapshot input is not a regular file: {}:{}",
                selection.name,
                relative.display()
            )
            .into());
        }
        let bytes = fs::read(&path)?;
        #[cfg(unix)]
        let executable = {
            use std::os::unix::fs::PermissionsExt as _;
            metadata.permissions().mode() & 0o111 != 0
        };
        #[cfg(not(unix))]
        let executable = false;
        let file = FileInput {
            path: relative.clone(),
            size_bytes: bytes.len() as u64,
            sha256: digest(&bytes),
            mode: if executable { 0o755 } else { 0o644 },
        };
        consume(&file, &bytes)?;
        files.push(file);
    }
    Ok(SourceInput {
        name: selection.name.clone(),
        commit: selection.commit.clone(),
        dirty: selection.dirty,
        files,
    })
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg(test)]
mod tests;
#[cfg(test)]
pub(crate) use tests::test_snapshot;

mod builder;
mod materialize;
pub(crate) use builder::build;
#[cfg(test)]
use materialize::materialize;

#[cfg(test)]
pub(crate) fn test_capture(root: &Path) -> Snapshot {
    capture_roots(
        &[("repository".into(), root.to_owned())],
        &[],
        &root.join("target/snapshots"),
    )
    .unwrap()
}
