//! Firmware subjects, source materials and build provenance.

use std::{
    env,
    fs::{self, File},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::Result;
use oer_durable::{atomic_write, sha256_file};
use oer_hil_image_class::ImageClass;
use oer_hil_source_snapshot::FrozenSources;
use oer_process::lock::{FileLock, Mode};

pub const BUILD_PROVENANCE_SCHEMA: u16 = 1;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SourceRebuildStatus {
    SourceSnapshot,
    CleanCommit,
    TrackedPatch,
    Incomplete,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SourceLimitation {
    RepositoryStateNotCaptured,
    SourceRemoteUnavailable,
    UntrackedContentNotArchived,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SourceFileIdentity {
    pub path: PathBuf,
    pub size_bytes: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SourceMaterial {
    pub name: String,
    pub checkout_path: PathBuf,
    pub remote: Option<String>,
    pub commit: String,
    pub dirty: bool,
    pub workspace_sha256: String,
    pub rebuild_status: SourceRebuildStatus,
    pub tracked_patch_path: Option<PathBuf>,
    pub tracked_patch_size_bytes: Option<u64>,
    pub tracked_patch_sha256: Option<String>,
    pub untracked_files: Vec<SourceFileIdentity>,
    pub limitations: Vec<SourceLimitation>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BuildFileMaterial {
    pub name: String,
    pub path: PathBuf,
    pub archive_path: Option<PathBuf>,
    pub size_bytes: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BuildParameters {
    pub image: ImageClass,
    /// None is retained when decoding older bundles with no recorded selection.
    #[serde(default)]
    pub network: Option<String>,
    pub runtime_profile: String,
    pub target: String,
    pub runtime_features: String,
    /// The seed the runtime was linked with; absent for the natural order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout_seed: Option<std::num::NonZeroU32>,
    /// Runtime features added to or removed from the class's own; an image
    /// built with any is an experiment's, never its class's.
    #[serde(
        default,
        skip_serializing_if = "oer_hil_image_class::FeatureDelta::is_empty"
    )]
    pub features: oer_hil_image_class::FeatureDelta,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum BuildSubjectRole {
    Application,
    BootstrapElf,
    RuntimeBin,
    RuntimeElf,
    Bootloader,
    PartitionTable,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BuildSubject {
    pub role: BuildSubjectRole,
    pub path: PathBuf,
    pub size_bytes: u64,
    pub sha256: String,
}

/// One host tool that took part in a firmware build.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BuildTool {
    pub name: String,
    pub program: String,
    pub version: Option<String>,
}

/// Host tools and inherited settings under which a firmware image was built.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BuildEnvironment {
    pub tools: Vec<BuildTool>,
    pub inherited_rustflags: Option<String>,
    pub inherited_encoded_rustflags: Option<String>,
    pub cargo_incremental: String,
    pub source_date_epoch: Option<String>,
}

impl BuildEnvironment {
    /// Query the tools the image builder runs. Call it when the build runs,
    /// so the record names the tools that produced the image.
    pub fn capture() -> Self {
        Self {
            tools: oer_toolchain::versions()
                .into_iter()
                .map(|tool| BuildTool {
                    name: tool.tool.name().to_owned(),
                    program: tool.program,
                    version: tool.version,
                })
                .collect(),
            inherited_rustflags: env::var("RUSTFLAGS").ok(),
            inherited_encoded_rustflags: env::var("CARGO_ENCODED_RUSTFLAGS").ok(),
            cargo_incremental: String::from("0"),
            source_date_epoch: env::var("SOURCE_DATE_EPOCH").ok(),
        }
    }

    /// A fixed environment in which every recorded tool reports a version,
    /// independent of the tools installed on the test host.
    #[cfg(any(test, feature = "test-support"))]
    pub fn synthetic() -> Self {
        Self {
            tools: oer_toolchain::Tool::ALL
                .into_iter()
                .map(|tool| BuildTool {
                    name: tool.name().to_owned(),
                    program: tool.name().to_owned(),
                    version: Some(format!("{} synthetic-test-version", tool.name())),
                })
                .collect(),
            inherited_rustflags: None,
            inherited_encoded_rustflags: None,
            cargo_incremental: String::from("0"),
            source_date_epoch: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum BuildReproducibility {
    Unverified,
    Verified,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BuildProvenance {
    pub schema: u16,
    pub build_id: String,
    pub build_type: String,
    pub parameters: BuildParameters,
    pub sources: Vec<SourceMaterial>,
    pub files: Vec<BuildFileMaterial>,
    pub environment: BuildEnvironment,
    pub subjects: Vec<BuildSubject>,
    pub source_reconstructable: bool,
    pub reproducibility: BuildReproducibility,
}

#[derive(Clone)]
pub struct ArchivedFile {
    pub size_bytes: u64,
    pub sha256: String,
}

struct GitSourceState {
    commit: String,
    status: String,
    tracked_diff: Vec<u8>,
    untracked_files: Vec<SourceFileIdentity>,
    workspace_sha256: String,
}

pub fn archive_content_addressed(
    source: &Path,
    destination: &Path,
    target_directory: &Path,
) -> Result<ArchivedFile> {
    let source_metadata = fs::symlink_metadata(source)?;
    if !source_metadata.file_type().is_file() {
        return Err(format!(
            "firmware artifact is not a regular file: {}",
            source.display()
        )
        .into());
    }
    if destination.try_exists()? {
        return Err(format!(
            "firmware artifact is already archived: {}",
            destination.display()
        )
        .into());
    }
    let size_bytes = source_metadata.len();
    let sha256 = sha256_file(source)?;
    // Collection deletes objects only while no archive uses the store.
    let _store = object_store_lock(target_directory, Mode::Shared)?;
    let object = target_directory
        .join("objects/sha256")
        .join(&sha256[..2])
        .join(&sha256);
    if object.try_exists()? {
        require_archive_identity(&object, size_bytes, &sha256)?;
    } else {
        copy_regular_file(source, &object)?;
        require_archive_identity(&object, size_bytes, &sha256)?;
    }
    make_read_only(&object)?;
    let destination_parent = destination.parent().ok_or_else(|| {
        format!(
            "firmware archive path has no parent: {}",
            destination.display()
        )
    })?;
    fs::create_dir_all(destination_parent)?;
    if fs::hard_link(&object, destination).is_err() {
        copy_regular_file(&object, destination)?;
    }
    require_archive_identity(destination, size_bytes, &sha256)?;
    make_read_only(destination)?;
    File::open(destination_parent)?.sync_all()?;
    Ok(ArchivedFile { size_bytes, sha256 })
}

/// The lock that orders archiving into a checkout's object store before its
/// collection: archives hold it shared, collection exclusively.
fn object_store_lock(target_directory: &Path, mode: Mode) -> Result<FileLock> {
    FileLock::acquire(&target_directory.join("objects/lock"), mode)
}

/// What [`collect_objects`] deleted.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CollectedObjects {
    pub objects: usize,
    pub bytes: u64,
}

/// Delete the objects of the store below `target_directory` that no file
/// links to any more, and temporary files an interrupted archive left.
///
/// Every archived subject is a hard link to its object, or a copy when
/// linking failed, so an object with a single link serves no run: it only
/// saves a later identical archive a copy. Collection holds the store's lock
/// exclusively, so no archive reuses an object while it is deleted.
pub fn collect_objects(target_directory: &Path) -> Result<CollectedObjects> {
    use std::os::unix::fs::MetadataExt as _;
    let root = target_directory.join("objects/sha256");
    let mut collected = CollectedObjects::default();
    if !root.is_dir() {
        return Ok(collected);
    }
    let _store = object_store_lock(target_directory, Mode::Exclusive)?;
    for prefix in fs::read_dir(&root)? {
        let prefix = prefix?;
        if !prefix.file_type()?.is_dir() {
            continue;
        }
        for entry in fs::read_dir(prefix.path())? {
            let entry = entry?;
            let metadata = entry.metadata()?;
            let temporary = entry
                .file_name()
                .to_string_lossy()
                .starts_with(".firmware-artifact.tmp-");
            if metadata.is_file() && (metadata.nlink() == 1 || temporary) {
                fs::remove_file(entry.path())?;
                collected.objects += usize::from(!temporary);
                collected.bytes += metadata.len();
            }
        }
    }
    Ok(collected)
}

fn make_read_only(path: &Path) -> Result<()> {
    let mut permissions = fs::metadata(path)?.permissions();
    permissions.set_readonly(true);
    fs::set_permissions(path, permissions)?;
    Ok(())
}

fn require_archive_identity(path: &Path, expected_size: u64, expected_sha256: &str) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_file()
        || metadata.len() != expected_size
        || sha256_file(path)? != expected_sha256
    {
        return Err(format!(
            "content-addressed firmware artifact has the wrong identity: {}",
            path.display()
        )
        .into());
    }
    Ok(())
}

fn copy_regular_file(source: &Path, destination: &Path) -> Result<()> {
    oer_durable::atomic_write(destination, &fs::read(source)?)
}

pub fn capture_sources(root: &Path, run_directory: &Path) -> Result<Vec<SourceMaterial>> {
    let mut materials = vec![capture_source_material(
        "repository",
        root,
        run_directory,
        Path::new("source/repository.patch"),
    )?];
    for (name, path) in external_source_override_paths()? {
        let patch_path = PathBuf::from("source/overrides").join(format!("{name}.patch"));
        materials.push(capture_source_material(
            &name,
            &path,
            run_directory,
            &patch_path,
        )?);
    }
    Ok(materials)
}

pub fn capture_source_material(
    name: &str,
    root: &Path,
    run_directory: &Path,
    patch_path: &Path,
) -> Result<SourceMaterial> {
    let checkout_path = fs::canonicalize(root).unwrap_or_else(|_| root.to_owned());
    let state = match capture_git_source_state(root)? {
        Some(state) => state,
        None => {
            return Ok(SourceMaterial {
                name: name.to_owned(),
                checkout_path,
                remote: None,
                commit: String::new(),
                dirty: true,
                workspace_sha256: oer_durable::sha256_bytes(&[]),
                rebuild_status: SourceRebuildStatus::Incomplete,
                tracked_patch_path: None,
                tracked_patch_size_bytes: None,
                tracked_patch_sha256: None,
                untracked_files: Vec::new(),
                limitations: vec![SourceLimitation::RepositoryStateNotCaptured],
            });
        }
    };
    let (tracked_patch_path, tracked_patch_size_bytes, tracked_patch_sha256) =
        if state.tracked_diff.is_empty() {
            (None, None, None)
        } else {
            atomic_write(&run_directory.join(patch_path), &state.tracked_diff)?;
            (
                Some(patch_path.to_owned()),
                Some(u64::try_from(state.tracked_diff.len())?),
                Some(oer_durable::sha256_bytes(&state.tracked_diff)),
            )
        };
    let mut limitations = Vec::new();
    if !state.untracked_files.is_empty() {
        limitations.push(SourceLimitation::UntrackedContentNotArchived);
    }
    if !state.status.is_empty() && state.tracked_diff.is_empty() && state.untracked_files.is_empty()
    {
        limitations.push(SourceLimitation::RepositoryStateNotCaptured);
    }
    let remote = oer_process::git::text(root, ["remote", "get-url", "origin"])
        .ok()
        .filter(|remote| !remote.is_empty())
        .map(sanitize_git_remote);
    if remote.is_none() {
        limitations.push(SourceLimitation::SourceRemoteUnavailable);
    }
    let rebuild_status = if !limitations.is_empty() {
        SourceRebuildStatus::Incomplete
    } else if state.status.is_empty() {
        SourceRebuildStatus::CleanCommit
    } else {
        SourceRebuildStatus::TrackedPatch
    };
    Ok(SourceMaterial {
        name: name.to_owned(),
        checkout_path,
        remote,
        commit: state.commit,
        dirty: !state.status.is_empty(),
        workspace_sha256: state.workspace_sha256,
        rebuild_status,
        tracked_patch_path,
        tracked_patch_size_bytes,
        tracked_patch_sha256,
        untracked_files: state.untracked_files,
        limitations,
    })
}

fn capture_git_source_state(root: &Path) -> Result<Option<GitSourceState>> {
    let commit = match oer_process::git::text(root, ["rev-parse", "HEAD"]) {
        Ok(commit) => commit,
        Err(_) => return Ok(None),
    };
    let status = oer_process::git::text(
        root,
        ["status", "--porcelain=v1", "--untracked-files=normal"],
    )?;
    let tracked_diff = oer_process::git::output(root, ["diff", "--binary", "HEAD", "--"])?;
    let untracked =
        oer_process::git::output(root, ["ls-files", "--others", "--exclude-standard", "-z"])?;
    let mut digest = Sha256::new();
    digest.update(status.as_bytes());
    digest.update(&tracked_diff);
    let mut untracked_files = Vec::new();
    for path in untracked
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
    {
        digest.update(path);
        let path = String::from_utf8(path.to_vec())?;
        let absolute = root.join(&path);
        if !fs::symlink_metadata(&absolute)?.file_type().is_file() {
            return Err(format!(
                "untracked HIL source identity is not a regular file: {}",
                absolute.display()
            )
            .into());
        }
        let contents = fs::read(&absolute)?;
        digest.update(&contents);
        untracked_files.push(SourceFileIdentity {
            path: PathBuf::from(path),
            size_bytes: u64::try_from(contents.len())?,
            sha256: oer_durable::sha256_bytes(&contents),
        });
    }
    untracked_files.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(Some(GitSourceState {
        commit,
        status,
        tracked_diff,
        untracked_files,
        workspace_sha256: format!("{:x}", digest.finalize()),
    }))
}

fn external_source_override_paths() -> Result<Vec<(String, PathBuf)>> {
    let current_directory = env::current_dir()?;
    let mut overrides = Vec::new();
    for (name, variable) in [
        ("esp-hal", "ESP_HAL_ROOT"),
        ("embassy", "EMBASSY_ROOT"),
        ("xarxa", "OPEN_RADIO_XARXA_ROOT"),
    ] {
        let Some(path) = env::var_os(variable).map(PathBuf::from) else {
            continue;
        };
        let path = if path.is_absolute() {
            path
        } else {
            current_directory.join(path)
        };
        overrides.push((name.to_owned(), fs::canonicalize(&path).unwrap_or(path)));
    }
    Ok(overrides)
}

fn sanitize_git_remote(remote: String) -> String {
    let Some((scheme, remainder)) = remote.split_once("://") else {
        return remote;
    };
    let authority_end = remainder.find('/').unwrap_or(remainder.len());
    let authority = &remainder[..authority_end];
    let sanitized_authority = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    format!(
        "{scheme}://{sanitized_authority}{}",
        &remainder[authority_end..]
    )
}

pub fn build_id(subjects: &[BuildSubject]) -> String {
    let mut digest = Sha256::new();
    digest.update(b"open-esp-radio-hil-build-v1\0");
    for subject in subjects {
        digest.update(format!("{:?}", subject.role).as_bytes());
        digest.update([0]);
        digest.update(subject.path.as_os_str().as_encoded_bytes());
        digest.update([0]);
        digest.update(subject.sha256.as_bytes());
        digest.update(subject.size_bytes.to_le_bytes());
    }
    format!("{:x}", digest.finalize())
}

/// The material of the repository file `path`, named `name`.
pub fn build_file_material(root: &Path, name: &str, path: &Path) -> Result<BuildFileMaterial> {
    let absolute = root.join(path);
    let metadata = fs::symlink_metadata(&absolute)?;
    if !metadata.file_type().is_file() {
        return Err(format!(
            "build material is not a regular file: {}",
            absolute.display()
        )
        .into());
    }
    Ok(BuildFileMaterial {
        name: name.to_owned(),
        path: path.to_owned(),
        archive_path: None,
        size_bytes: metadata.len(),
        sha256: sha256_file(&absolute)?,
    })
}

pub fn archived_file_material(
    name: &str,
    path: &Path,
    archive_path: PathBuf,
    archived: &ArchivedFile,
) -> BuildFileMaterial {
    BuildFileMaterial {
        name: name.to_owned(),
        path: path.to_owned(),
        archive_path: Some(archive_path),
        size_bytes: archived.size_bytes,
        sha256: archived.sha256.clone(),
    }
}

/// Archive the snapshot in `directory` into the bundle at `output`, with its
/// objects in the store below `target`, and open the archived copy in a
/// free build workspace of `build_slots`.
pub fn archive_snapshot(
    directory: &Path,
    output: &Path,
    target: &Path,
    build_slots: &Path,
) -> Result<(FrozenSources, Vec<SourceMaterial>, Vec<BuildFileMaterial>)> {
    let destination = output.join("source/snapshot");
    let mut files: Vec<BuildFileMaterial> = Vec::new();
    for (name, filename) in [
        ("source-snapshot-metadata", "snapshot.json"),
        ("source-snapshot-manifest", "manifest.json"),
        ("source-snapshot-archive", "sources.tar"),
    ] {
        let relative = PathBuf::from("source/snapshot").join(filename);
        let archived =
            archive_content_addressed(&directory.join(filename), &output.join(&relative), target)?;
        files.push(archived_file_material(
            name,
            Path::new(filename),
            relative,
            &archived,
        ));
    }
    // A persistent host build slot keeps unchanged sources, so Cargo
    // rebuilds only the packages whose sources changed on each run.
    let frozen = FrozenSources::open_in_free_workspace(&destination, build_slots)?;
    let sources = frozen
        .sources()
        .iter()
        .map(|source| {
            Ok(SourceMaterial {
                name: source.name.clone(),
                checkout_path: PathBuf::from(&source.name),
                remote: None,
                commit: source.commit.clone(),
                dirty: source.dirty,
                workspace_sha256: oer_hil_source_snapshot::identity(source)?,
                rebuild_status: SourceRebuildStatus::SourceSnapshot,
                tracked_patch_path: None,
                tracked_patch_size_bytes: None,
                tracked_patch_sha256: None,
                untracked_files: Vec::new(),
                limitations: Vec::new(),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok((frozen, sources, files))
}

#[cfg(test)]
mod tests;
