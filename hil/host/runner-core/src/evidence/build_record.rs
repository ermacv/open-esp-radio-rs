//! Immutable build subjects without fabricated hardware observations.
use super::{
    build::{self, BuildFileMaterial, SourceMaterial, SourceRebuildStatus},
    firmware,
    run::{FirmwareArtifact, RepositoryProvenance, atomic_json, sha256_file},
};
use crate::{
    Result,
    image::{Artifacts, ImageClass, snapshot::FrozenSources},
};
use serde::Serialize;
use std::{
    fs,
    path::{Path, PathBuf},
};

pub(super) fn archive_snapshot(
    directory: &Path,
    output: &Path,
    target: &Path,
) -> Result<(FrozenSources, Vec<SourceMaterial>, Vec<BuildFileMaterial>)> {
    let destination = output.join("source/snapshot");
    let mut files: Vec<BuildFileMaterial> = Vec::new();
    for (name, filename) in [
        ("source-snapshot-metadata", "snapshot.json"),
        ("source-snapshot-manifest", "manifest.json"),
        ("source-snapshot-archive", "sources.tar"),
    ] {
        let relative = PathBuf::from("source/snapshot").join(filename);
        let archived = build::archive_content_addressed(
            &directory.join(filename),
            &output.join(&relative),
            target,
        )?;
        files.push(build::archived_file_material(
            name,
            Path::new(filename),
            relative,
            &archived,
        ));
    }
    let frozen = FrozenSources::open(&destination)?;
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
                workspace_sha256: source.identity()?,
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

#[derive(Serialize)]
struct Record {
    schema: u16,
    kind: &'static str,
    target: &'static str,
    repository: RepositoryProvenance,
    firmware: Vec<FirmwareArtifact>,
}

/// Publish atomically after construction. The content identity covers the entire
/// inventory; build records contain no scenario, outcome or repetition count.
pub fn publish(
    root: &Path,
    snapshot: &Path,
    class: ImageClass,
    artifacts: &Artifacts,
) -> Result<PathBuf> {
    let target = root.join("target/hil/esp32s31");
    let builds = target.join("builds");
    fs::create_dir_all(&builds)?;
    let staging = tempfile::Builder::new()
        .prefix(".build-")
        .tempdir_in(&builds)?;
    let directory = staging.path();
    let (frozen, sources, materials) = archive_snapshot(snapshot, directory, &target)?;
    frozen.verify_unchanged()?;
    let (artifact, _) = firmware::archive(
        firmware::Context {
            directory,
            target_directory: &target,
            source_root: &frozen.repository(),
            source_materials: &sources,
            snapshot_materials: &materials,
        },
        firmware::Inputs {
            selection: (class, artifacts.network),
            application: &artifacts.application_image,
            runtime_elf: &artifacts.runtime_elf,
            runtime_bin: &artifacts.runtime_bin,
            bootstrap_elf: &artifacts.bootstrap_elf,
            effective_locks: (
                &artifacts.effective_embedded_lock,
                &artifacts.effective_bootstrap_lock,
            ),
        },
    )?;
    let source = sources
        .first()
        .filter(|s| s.name == "repository")
        .ok_or("build snapshot has no primary repository")?;
    atomic_json(
        &directory.join("build.json"),
        &Record {
            schema: 1,
            kind: "open-esp-radio-build",
            target: "esp32s31",
            repository: RepositoryProvenance {
                commit: source.commit.clone(),
                dirty: source.dirty,
                workspace_sha256: source.workspace_sha256.clone(),
            },
            firmware: vec![artifact],
        },
    )?;
    super::run::write_integrity_index(directory, "build-only")?;
    let id = sha256_file(&directory.join("integrity.json"))?;
    let destination = builds.join(id);
    if destination.exists() {
        if !fs::symlink_metadata(&destination)?.file_type().is_dir()
            || sha256_file(&destination.join("integrity.json"))?
                != sha256_file(&directory.join("integrity.json"))?
            || serde_json::to_value(super::run::collect_integrity_files(&destination)?)?
                != serde_json::to_value(super::run::collect_integrity_files(directory)?)?
        {
            return Err("published build record does not match its identity".into());
        }
        return Ok(destination);
    }
    fs::rename(directory, &destination)?;
    Ok(destination)
}
