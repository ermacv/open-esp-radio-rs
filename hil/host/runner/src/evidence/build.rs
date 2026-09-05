//! Firmware subjects, source materials and build provenance.

use oer_process::CommandExt as _;
use std::{
    env,
    fs::{self, File, OpenOptions},
    io::Write as _,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::evidence::run::{atomic_write, sha256_file};
use crate::{Result, image::ImageClass};

pub(super) const BUILD_PROVENANCE_SCHEMA: u16 = 1;
static ARCHIVE_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum SourceRebuildStatus {
    CleanCommit,
    TrackedPatch,
    Incomplete,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum SourceLimitation {
    RepositoryStateNotCaptured,
    SourceRemoteUnavailable,
    UntrackedContentNotArchived,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(super) struct SourceFileIdentity {
    pub(super) path: PathBuf,
    pub(super) size_bytes: u64,
    pub(super) sha256: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(super) struct SourceMaterial {
    pub(super) name: String,
    pub(super) checkout_path: PathBuf,
    pub(super) remote: Option<String>,
    pub(super) commit: String,
    pub(super) dirty: bool,
    pub(super) workspace_sha256: String,
    pub(super) rebuild_status: SourceRebuildStatus,
    pub(super) tracked_patch_path: Option<PathBuf>,
    pub(super) tracked_patch_size_bytes: Option<u64>,
    pub(super) tracked_patch_sha256: Option<String>,
    pub(super) untracked_files: Vec<SourceFileIdentity>,
    pub(super) limitations: Vec<SourceLimitation>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(super) struct BuildFileMaterial {
    pub(super) name: String,
    pub(super) path: PathBuf,
    pub(super) archive_path: Option<PathBuf>,
    pub(super) size_bytes: u64,
    pub(super) sha256: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(super) struct BuildParameters {
    pub(super) image: ImageClass,
    pub(super) runtime_profile: String,
    pub(super) target: String,
    pub(super) runtime_features: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum BuildSubjectRole {
    Application,
    BootstrapElf,
    RuntimeBin,
    RuntimeElf,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(super) struct BuildSubject {
    pub(super) role: BuildSubjectRole,
    pub(super) path: PathBuf,
    pub(super) size_bytes: u64,
    pub(super) sha256: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(super) struct BuildTool {
    pub(super) name: String,
    pub(super) program: String,
    pub(super) version: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(super) struct BuildEnvironment {
    pub(super) tools: Vec<BuildTool>,
    pub(super) inherited_rustflags: Option<String>,
    pub(super) inherited_encoded_rustflags: Option<String>,
    pub(super) cargo_incremental: String,
    pub(super) source_date_epoch: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum BuildReproducibility {
    Unverified,
    Verified,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(super) struct BuildProvenance {
    pub(super) schema: u16,
    pub(super) build_id: String,
    pub(super) build_type: String,
    pub(super) parameters: BuildParameters,
    pub(super) sources: Vec<SourceMaterial>,
    pub(super) files: Vec<BuildFileMaterial>,
    pub(super) environment: BuildEnvironment,
    pub(super) subjects: Vec<BuildSubject>,
    pub(super) source_reconstructable: bool,
    pub(super) reproducibility: BuildReproducibility,
}

pub(super) struct ArchivedFile {
    pub(super) size_bytes: u64,
    pub(super) sha256: String,
}

struct GitSourceState {
    commit: String,
    status: String,
    tracked_diff: Vec<u8>,
    untracked_files: Vec<SourceFileIdentity>,
    workspace_sha256: String,
}

pub(super) fn archive_content_addressed(
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
    let parent = destination.parent().ok_or_else(|| {
        format!(
            "firmware archive path has no parent: {}",
            destination.display()
        )
    })?;
    fs::create_dir_all(parent)?;
    let counter = ARCHIVE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let temporary = parent.join(format!(
        ".firmware-artifact.tmp-{}-{counter}",
        std::process::id()
    ));
    let result = (|| -> Result<()> {
        let mut input = File::open(source)?;
        let mut output = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)?;
        std::io::copy(&mut input, &mut output)?;
        output.flush()?;
        output.sync_all()?;
        fs::rename(&temporary, destination)?;
        File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

pub(super) fn capture_sources(root: &Path, run_directory: &Path) -> Result<Vec<SourceMaterial>> {
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

pub(super) fn verify_sources_unchanged(root: &Path, expected: &[SourceMaterial]) -> Result<()> {
    let repository = fs::canonicalize(root).unwrap_or_else(|_| root.to_owned());
    let mut current_paths = vec![(String::from("repository"), repository)];
    current_paths.extend(external_source_override_paths()?);
    if current_paths.len() != expected.len()
        || current_paths
            .iter()
            .zip(expected)
            .any(|((name, path), source)| name != &source.name || path != &source.checkout_path)
    {
        return Err("HIL source override set changed while firmware was being built".into());
    }
    for source in expected {
        verify_source_material_unchanged(source)?;
    }
    Ok(())
}

fn verify_source_material_unchanged(expected: &SourceMaterial) -> Result<()> {
    let current = capture_git_source_state(&expected.checkout_path)?;
    match current {
        Some(current)
            if current.commit == expected.commit
                && current.workspace_sha256 == expected.workspace_sha256 =>
        {
            Ok(())
        }
        None if expected.commit.is_empty() => Ok(()),
        _ => Err(format!(
            "HIL source material `{}` changed while firmware was being built",
            expected.name
        )
        .into()),
    }
}

pub(super) fn capture_source_material(
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
                workspace_sha256: sha256_bytes(&[]),
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
                Some(sha256_bytes(&state.tracked_diff)),
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
    let remote = git_output(root, &["remote", "get-url", "origin"])
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
    let commit = match git_output(root, &["rev-parse", "HEAD"]) {
        Ok(commit) => commit,
        Err(_) => return Ok(None),
    };
    let status = git_output(
        root,
        &["status", "--porcelain=v1", "--untracked-files=normal"],
    )?;
    let tracked_diff = git_output_bytes(root, &["diff", "--binary", "HEAD", "--"])?;
    let untracked = git_output_bytes(root, &["ls-files", "--others", "--exclude-standard", "-z"])?;
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
            sha256: sha256_bytes(&contents),
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

pub(super) fn build_id(subjects: &[BuildSubject]) -> String {
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

pub(super) fn create_provenance(
    root: &Path,
    image: ImageClass,
    build_id: String,
    sources: Vec<SourceMaterial>,
    subjects: Vec<BuildSubject>,
    effective_locks: Vec<BuildFileMaterial>,
) -> Result<BuildProvenance> {
    let mut files = [
        ("workspace-lock", "Cargo.lock"),
        ("embedded-workspace", "hil/targets/esp32s31/Cargo.toml"),
        ("stack-policy", "hil/targets/esp32s31/stack.toml"),
        (
            "partition-table",
            "platform/esp32s31/partitions/applications.csv",
        ),
    ]
    .into_iter()
    .map(|(name, path)| build_file_material(root, name, Path::new(path)))
    .collect::<Result<Vec<_>>>()?;
    files.extend(effective_locks);
    files.sort_by(|left, right| left.name.cmp(&right.name));
    let environment = BuildEnvironment {
        tools: [
            ("rustc", "RUSTC", "rustc"),
            ("cargo", "CARGO", "cargo"),
            ("llvm-objcopy", "LLVM_OBJCOPY", "llvm-objcopy"),
            ("llvm-nm", "LLVM_NM", "llvm-nm"),
            ("espflash", "ESPFLASH", "espflash"),
        ]
        .into_iter()
        .map(|(name, variable, fallback)| build_tool(name, variable, fallback))
        .collect(),
        inherited_rustflags: env::var("RUSTFLAGS").ok(),
        inherited_encoded_rustflags: env::var("CARGO_ENCODED_RUSTFLAGS").ok(),
        cargo_incremental: String::from("0"),
        source_date_epoch: env::var("SOURCE_DATE_EPOCH").ok(),
    };
    Ok(BuildProvenance {
        schema: BUILD_PROVENANCE_SCHEMA,
        build_id,
        build_type: String::from("open-esp-radio-hil-firmware/v1"),
        parameters: BuildParameters {
            image,
            runtime_profile: image.runtime_profile().to_owned(),
            target: crate::image::TARGET.to_owned(),
            runtime_features: image.runtime_features().to_owned(),
        },
        source_reconstructable: sources
            .iter()
            .all(|source| source.rebuild_status != SourceRebuildStatus::Incomplete),
        sources,
        files,
        environment,
        subjects,
        reproducibility: BuildReproducibility::Unverified,
    })
}

fn build_file_material(root: &Path, name: &str, path: &Path) -> Result<BuildFileMaterial> {
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

pub(super) fn archived_file_material(
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

fn build_tool(name: &str, variable: &str, fallback: &str) -> BuildTool {
    let program = env::var_os(variable)
        .unwrap_or_else(|| fallback.into())
        .to_string_lossy()
        .into_owned();
    BuildTool {
        name: name.to_owned(),
        version: command_version(
            &program,
            match name {
                "rustc" => &["-vV"],
                "cargo" => &["-Vv"],
                _ => &["--version"],
            },
        ),
        program,
    }
}

fn sha256_bytes(bytes: &[u8]) -> String {
    let mut digest = Sha256::new();
    digest.update(bytes);
    format!("{:x}", digest.finalize())
}

fn git_output(root: &Path, arguments: &[&str]) -> Result<String> {
    Ok(String::from_utf8(git_output_bytes(root, arguments)?)?
        .trim()
        .to_owned())
}

fn git_output_bytes(root: &Path, arguments: &[&str]) -> Result<Vec<u8>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(arguments)
        .supervised_output()?;
    if !output.status.success() {
        return Err(format!(
            "git {} failed with status {}",
            arguments.join(" "),
            output.status
        )
        .into());
    }
    Ok(output.stdout)
}

fn command_version(program: &str, arguments: &[&str]) -> Option<String> {
    let output = Command::new(program)
        .args(arguments)
        .supervised_output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

#[cfg(test)]
mod tests;
