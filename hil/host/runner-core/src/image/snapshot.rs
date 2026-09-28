//! Explicit, content-addressed source input capture, independent of firmware builds.
//!
//! Tracked files are included automatically. Every nonignored untracked file
//! must be named explicitly before any content is archived, or, with
//! `--include-untracked`, lie inside a path package of the firmware
//! workspaces, the packages an image build reads, or inside the HIL host
//! packages and scenarios, which the run reads. The manifest lists every
//! untracked file it archived and why. This is a source
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

pub use oer_hil_schema::snapshot::SourceInput;
use oer_hil_schema::snapshot::{
    FileInput, MANIFEST_SCHEMA, Manifest, UntrackedInput, UntrackedReason,
};

/// The firmware workspaces whose path packages an image build reads.
const FIRMWARE_WORKSPACES: [&str; 2] = [
    "hil/targets/esp32s31/Cargo.toml",
    "platform/esp32s31/bootstrap/Cargo.toml",
];

/// Repository directories of the HIL host packages and scenarios, whose
/// untracked files `--include-untracked` also archives.
const HIL_HOST_INPUTS: [&str; 3] = ["hil/host", "hil/schema", "hil/scenarios"];

/// Repository-relative directories of the path packages the firmware
/// workspaces build, from Cargo's locked metadata.
pub fn image_packages(root: &Path) -> Result<Vec<PathBuf>> {
    #[derive(Deserialize)]
    struct Metadata {
        packages: Vec<Package>,
    }
    #[derive(Deserialize)]
    struct Package {
        source: Option<String>,
        manifest_path: PathBuf,
    }
    let root = root.canonicalize()?;
    let mut directories = BTreeSet::new();
    for workspace in FIRMWARE_WORKSPACES {
        let output = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
            .args(["metadata", "--format-version", "1", "--offline", "--locked"])
            .arg("--manifest-path")
            .arg(root.join(workspace))
            .supervised_output()?;
        if !output.status.success() {
            return Err(format!(
                "cannot list the packages of {workspace}: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )
            .into());
        }
        let metadata: Metadata = serde_json::from_slice(&output.stdout)?;
        for package in metadata.packages.into_iter().filter(|p| p.source.is_none()) {
            let directory = package
                .manifest_path
                .parent()
                .ok_or("package manifest has no directory")?
                .canonicalize()?;
            if let Ok(relative) = directory.strip_prefix(&root) {
                directories.insert(relative.to_owned());
            }
        }
    }
    Ok(directories.into_iter().collect())
}

#[derive(Deserialize, Serialize)]
pub struct Snapshot {
    schema: u16,
    snapshot_id: String,
    directory: PathBuf,
    archive_sha256: String,
    files: usize,
}

impl Snapshot {
    pub fn directory(&self) -> &Path {
        &self.directory
    }

    /// The snapshot captured earlier into `directory`, whose archive and
    /// manifest are verified before they are used.
    pub fn load(directory: &Path) -> Result<Self> {
        let directory = directory.canonicalize()?;
        let snapshot: Self = serde_json::from_slice(&fs::read(directory.join("snapshot.json"))?)?;
        if snapshot.directory.canonicalize()? != directory {
            return Err(format!(
                "{} holds the record of another snapshot directory",
                directory.display()
            )
            .into());
        }
        FrozenSources::open(&directory)?;
        Ok(snapshot)
    }
}

/// A source's identity: the SHA-256 of its manifest entry.
pub fn identity(source: &SourceInput) -> Result<String> {
    Ok(digest(&serde_json::to_vec(source)?))
}

/// Owns a verified private checkout. No build falls back to the live repository.
pub struct FrozenSources {
    checkout: Checkout,
    snapshot: Snapshot,
    manifest: Manifest,
}

/// Where a snapshot is materialized.
enum Checkout {
    /// A fresh temporary directory, for readers of the sources.
    Temporary(tempfile::TempDir),
    /// A fixed build workspace held under an exclusive lock. A stable path
    /// keeps Cargo's unit identities stable in the shared compile cache, and
    /// unchanged files keep their modification times, so Cargo rebuilds only
    /// the packages a snapshot actually changes.
    Workspace { path: PathBuf, _lock: WorkspaceLock },
}

/// The exclusive lock of one build workspace, released explicitly on drop.
///
/// Closing this descriptor alone does not release the `flock` while a child
/// that another thread forked, and that has not yet executed its program,
/// still holds the shared open file description; the workspace would then
/// look busy to the next build for that moment.
struct WorkspaceLock(fs::File);

impl Drop for WorkspaceLock {
    fn drop(&mut self) {
        if let Err(error) = fs2::FileExt::unlock(&self.0) {
            eprintln!("release the source build workspace lock: {error}");
        }
    }
}

impl Checkout {
    fn path(&self) -> &Path {
        match self {
            Self::Temporary(directory) => directory.path(),
            Self::Workspace { path, .. } => path,
        }
    }
}

/// Persistent build workspaces per checkout: one per build that can run at
/// the same time before later ones fall back to a temporary directory.
const WORKSPACE_SLOTS: usize = 3;

impl FrozenSources {
    pub fn open(directory: &Path) -> Result<Self> {
        let checkout = Checkout::Temporary(
            tempfile::Builder::new()
                .prefix("oer-source-build-")
                .tempdir()?,
        );
        Self::materialize(directory, checkout)
    }

    /// Materialize into `workspace` once no other build holds it. The
    /// snapshot is verified in a staging directory; the workspace then keeps
    /// every file whose bytes and mode are unchanged, receives the rest and
    /// loses anything the snapshot does not contain.
    pub fn open_in_workspace(directory: &Path, workspace: &Path) -> Result<Self> {
        use fs2::FileExt as _;
        let lock = Self::workspace_lock(workspace)?;
        lock.lock_exclusive()?;
        Self::open_locked(directory, workspace, lock)
    }

    /// Materialize into the first free slot of the per-checkout build
    /// workspaces `<base>-<n>`, without waiting. The slots persist, so an
    /// unchanged source file keeps its bytes and modification time and Cargo
    /// rebuilds only the packages whose sources changed; a run that holds its
    /// slot for its whole session never blocks another build of the same
    /// checkout. When every slot is busy the sources go to a temporary
    /// directory, as a cold build.
    pub fn open_in_free_workspace(directory: &Path, base: &Path) -> Result<Self> {
        use fs2::FileExt as _;
        let name = base
            .file_name()
            .ok_or("source build workspace has no name")?
            .to_string_lossy()
            .into_owned();
        for slot in 0..WORKSPACE_SLOTS {
            let workspace = base.with_file_name(format!("{name}-{slot}"));
            let lock = Self::workspace_lock(&workspace)?;
            if lock.try_lock_exclusive().is_ok() {
                return Self::open_locked(directory, &workspace, lock);
            }
        }
        Self::open(directory)
    }

    fn workspace_lock(workspace: &Path) -> Result<fs::File> {
        let parent = workspace
            .parent()
            .ok_or("source build workspace has no parent")?;
        fs::create_dir_all(parent)?;
        Ok(fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(workspace.with_extension("lock"))?)
    }

    fn open_locked(directory: &Path, workspace: &Path, lock: fs::File) -> Result<Self> {
        let staging = workspace.with_extension("staging");
        if fs::symlink_metadata(&staging).is_ok() {
            fs::remove_dir_all(&staging)?;
        }
        fs::create_dir(&staging)?;
        let (snapshot, manifest) = materialize::materialize(directory, &staging)?;
        if !fs::symlink_metadata(workspace).is_ok_and(|metadata| metadata.is_dir()) {
            if fs::symlink_metadata(workspace).is_ok() {
                fs::remove_file(workspace)?;
            }
            fs::create_dir(workspace)?;
        }
        synchronize(&staging, workspace)?;
        fs::remove_dir_all(&staging)?;
        let sources = Self {
            checkout: Checkout::Workspace {
                path: workspace.to_owned(),
                _lock: WorkspaceLock(lock),
            },
            snapshot,
            manifest,
        };
        sources.verify_unchanged()?;
        Ok(sources)
    }

    fn materialize(directory: &Path, checkout: Checkout) -> Result<Self> {
        let (snapshot, manifest) = materialize::materialize(directory, checkout.path())?;
        Ok(Self {
            checkout,
            snapshot,
            manifest,
        })
    }

    pub fn repository(&self) -> PathBuf {
        self.checkout.path().join("repository")
    }

    pub fn sources(&self) -> &[SourceInput] {
        &self.manifest.sources
    }

    pub fn verify_unchanged(&self) -> Result<()> {
        for source in &self.manifest.sources {
            for file in &source.files {
                let path = self.checkout.path().join(&source.name).join(&file.path);
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

/// Make `destination` hold exactly the regular files of `source`, leaving
/// files with equal bytes and mode untouched.
fn synchronize(source: &Path, destination: &Path) -> Result<()> {
    let mut entries = fs::read_dir(destination)?.collect::<std::io::Result<Vec<_>>>()?;
    entries.sort_by_key(fs::DirEntry::file_name);
    for entry in entries {
        let expected = source.join(entry.file_name());
        let kind = entry.file_type()?;
        let keep = match fs::symlink_metadata(&expected) {
            Ok(metadata) if metadata.is_dir() => kind.is_dir(),
            Ok(metadata) if metadata.is_file() => kind.is_file(),
            _ => false,
        };
        if !keep {
            if kind.is_dir() {
                fs::remove_dir_all(entry.path())?;
            } else {
                fs::remove_file(entry.path())?;
            }
        }
    }
    let mut entries = fs::read_dir(source)?.collect::<std::io::Result<Vec<_>>>()?;
    entries.sort_by_key(fs::DirEntry::file_name);
    for entry in entries {
        let target = destination.join(entry.file_name());
        let kind = entry.file_type()?;
        if kind.is_dir() {
            if !target.is_dir() {
                fs::create_dir(&target)?;
            }
            synchronize(&entry.path(), &target)?;
        } else if !kind.is_file() {
            return Err("materialized snapshot holds a non-regular entry".into());
        } else if !same_file(&entry.path(), &target)? {
            // Replace atomically so a partly written file is never kept.
            let temporary =
                destination.join(format!(".{}.sync", entry.file_name().to_string_lossy()));
            fs::copy(entry.path(), &temporary)?;
            fs::rename(&temporary, &target)?;
        }
    }
    Ok(())
}

fn same_file(left: &Path, right: &Path) -> Result<bool> {
    let (left_metadata, Ok(right_metadata)) =
        (fs::symlink_metadata(left)?, fs::symlink_metadata(right))
    else {
        return Ok(false);
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        if left_metadata.permissions().mode() != right_metadata.permissions().mode() {
            return Ok(false);
        }
    }
    Ok(right_metadata.is_file()
        && left_metadata.len() == right_metadata.len()
        && fs::read(left)? == fs::read(right)?)
}

struct Selection {
    name: String,
    root: PathBuf,
    commit: String,
    dirty: bool,
    tracked: BTreeSet<PathBuf>,
    untracked: BTreeSet<PathBuf>,
}

/// Capture the sources at `root`, with the untracked files `include` names
/// and, when `include_untracked`, every untracked file inside an image
/// package.
pub fn capture(root: &Path, include: &[String], include_untracked: bool) -> Result<Snapshot> {
    let overrides = crate::experiment::Dependency::ALL
        .into_iter()
        .filter_map(|dependency| {
            std::env::var_os(dependency.root_env()).map(|path| (dependency, PathBuf::from(path)))
        })
        .collect::<Vec<_>>();
    capture_with_overrides(root, include, include_untracked, &overrides)
}

/// [`capture`] with the local dependency checkouts named by `overrides`
/// instead of the environment's.
pub fn capture_with_overrides(
    root: &Path,
    include: &[String],
    include_untracked: bool,
    overrides: &[(crate::experiment::Dependency, PathBuf)],
) -> Result<Snapshot> {
    let scopes = if include_untracked {
        untracked_scopes(image_packages(root)?)
    } else {
        Vec::new()
    };
    let mut roots = vec![("repository".to_owned(), root.canonicalize()?)];
    for (dependency, path) in overrides {
        roots.push((dependency.id().into(), path.canonicalize()?));
    }
    capture_roots(
        &roots,
        include,
        &scopes,
        &root.join("target/hil/esp32s31/source-snapshots"),
    )
}

/// The repository directories whose untracked files `--include-untracked`
/// archives, each with the reason it records: the image `packages`, then the
/// HIL host inputs.
fn untracked_scopes(packages: Vec<PathBuf>) -> Vec<(PathBuf, UntrackedReason)> {
    packages
        .into_iter()
        .map(|package| (package, UntrackedReason::ImagePackage))
        .chain(
            HIL_HOST_INPUTS
                .into_iter()
                .map(|input| (PathBuf::from(input), UntrackedReason::HilHost)),
        )
        .collect()
}

/// The `--source-include` arguments that add each of `unresolved` (as
/// `role:path`) to a run, ready to paste.
fn source_include_arguments(unresolved: &[(&str, &Path)]) -> String {
    unresolved
        .iter()
        .map(|(role, path)| match *role {
            "repository" => format!("--source-include {}", path.display()),
            role => format!("--source-include {role}:{}", path.display()),
        })
        .collect::<Vec<_>>()
        .join(" ")
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

/// Repository directories whose untracked files are recorded outputs, which a
/// snapshot neither requires nor archives.
const EVIDENCE_OUTPUTS: &[&str] = &["hil/evidence"];

fn select(name: &str, root: &Path) -> Result<Selection> {
    let top = String::from_utf8(git(root, &["rev-parse", "--show-toplevel"])?)?;
    if Path::new(top.trim()).canonicalize()? != root.canonicalize()? {
        return Err(format!("snapshot source {name} must name its repository root").into());
    }
    let commit = String::from_utf8(git(root, &["rev-parse", "HEAD"])?)?
        .trim()
        .to_owned();
    let tracked = paths(&git(root, &["ls-files", "--cached", "-z"])?)?;
    let mut untracked = paths(&git(
        root,
        &["ls-files", "--others", "--exclude-standard", "-z"],
    )?)?;
    // Recorded evidence shards are outputs of earlier runs, not build inputs.
    untracked.retain(|path| {
        !EVIDENCE_OUTPUTS
            .iter()
            .any(|output| path.starts_with(output))
    });
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

/// A source that changed while it was read; the capture is taken again.
#[derive(Debug)]
struct ChangedDuringCapture(String);

impl std::fmt::Display for ChangedDuringCapture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "source {} changed during snapshot capture", self.0)
    }
}

impl std::error::Error for ChangedDuringCapture {}

/// Captures taken before a source that keeps changing fails the snapshot.
const CAPTURE_ATTEMPTS: u32 = 5;
/// The pause before the next capture, for an edit in progress to finish.
const CAPTURE_RETRY_PAUSE: std::time::Duration = std::time::Duration::from_secs(2);

/// `scopes` are repository-relative directories whose untracked files are
/// archived without being named, each with the reason recorded for them. A
/// source edited while it is read is captured again, so a snapshot is always
/// one consistent state of every source.
fn capture_roots(
    roots: &[(String, PathBuf)],
    include: &[String],
    scopes: &[(PathBuf, UntrackedReason)],
    output: &Path,
) -> Result<Snapshot> {
    until_unchanged(CAPTURE_ATTEMPTS, CAPTURE_RETRY_PAUSE, || {
        capture_roots_once(roots, include, scopes, output)
    })
}

/// Run `capture` again after a pause while it fails only because a source
/// changed under it, at most `attempts` times; any other failure, or the
/// last change, is returned.
fn until_unchanged<T>(
    attempts: u32,
    pause: std::time::Duration,
    mut capture: impl FnMut() -> Result<T>,
) -> Result<T> {
    let mut attempt = 1;
    loop {
        match capture() {
            Err(error) if attempt < attempts && error.is::<ChangedDuringCapture>() => {
                eprintln!("hil: {error}; capturing again");
                attempt += 1;
                oer_process::sleep(pause)?;
            }
            result => return result,
        }
    }
}

fn capture_roots_once(
    roots: &[(String, PathBuf)],
    include: &[String],
    scopes: &[(PathBuf, UntrackedReason)],
    output: &Path,
) -> Result<Snapshot> {
    let selections = roots
        .iter()
        .map(|(name, root)| select(name, root))
        .collect::<Result<Vec<_>>>()?;
    let mut accepted = BTreeMap::<String, BTreeMap<PathBuf, UntrackedReason>>::new();
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
        if accepted
            .entry(role.into())
            .or_default()
            .insert(path, UntrackedReason::SourceInclude)
            .is_some()
        {
            return Err(format!("duplicate --source-include: {role}:{file}").into());
        }
    }
    if let Some(repository) = selections.iter().find(|s| s.name == "repository") {
        let accepted = accepted.entry(repository.name.clone()).or_default();
        for path in &repository.untracked {
            if let Some((_, reason)) = scopes.iter().find(|(scope, _)| path.starts_with(scope)) {
                accepted.entry(path.clone()).or_insert(*reason);
            }
        }
    }
    let unresolved = selections
        .iter()
        .flat_map(|s| {
            s.untracked
                .iter()
                .filter(|p| !accepted.get(&s.name).is_some_and(|v| v.contains_key(*p)))
                .map(|p| (s.name.as_str(), p.as_path()))
        })
        .collect::<Vec<_>>();
    if !unresolved.is_empty() {
        return Err(format!(
            "source snapshot blocked by untracked files, which a build could read:\n{}\n\
             Commit, remove or ignore each one, or add them all to this run with\n  {}\n\
             `--include-untracked` adds those inside the packages an image builds, the HIL \
             host packages and hil/scenarios.",
            unresolved
                .iter()
                .map(|(role, path)| format!("{role}:{}", path.display()))
                .collect::<Vec<_>>()
                .join("\n"),
            source_include_arguments(&unresolved)
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
            return Err(ChangedDuringCapture(selection.name.clone()).into());
        }
    }
    for source in &mut sources {
        source.untracked = accepted
            .remove(&source.name)
            .unwrap_or_default()
            .into_iter()
            .map(|(path, by)| UntrackedInput { path, by })
            .collect();
    }
    let manifest = Manifest {
        schema: MANIFEST_SCHEMA,
        sources,
    };
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
        untracked: Vec::new(),
    })
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

// Snapshot fixtures also serve the runner's cross-package tests.
#[cfg(any(test, feature = "test-support"))]
mod tests;
#[cfg(any(test, feature = "test-support"))]
pub use tests::test_snapshot;

mod builder;
mod materialize;
pub use builder::build;
#[cfg(test)]
use materialize::materialize;

#[cfg(any(test, feature = "test-support"))]
pub fn test_capture(root: &Path) -> Snapshot {
    capture_roots(
        &[("repository".into(), root.to_owned())],
        &[],
        &[],
        &root.join("target/snapshots"),
    )
    .unwrap()
}
