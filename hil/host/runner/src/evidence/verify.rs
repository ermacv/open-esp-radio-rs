//! Offline integrity verification for immutable HIL run bundles.

use std::{
    collections::BTreeSet,
    fs,
    path::{Component, Path, PathBuf},
};

use serde::Serialize;

use crate::Result;
use crate::evidence::{
    build::{
        BUILD_PROVENANCE_SCHEMA, BuildProvenance, BuildReproducibility, BuildSubject,
        BuildSubjectRole, SourceMaterial, SourceRebuildStatus, build_id,
    },
    run::{
        IntegrityIndex, RUN_SCHEMA, RunManifest, RunState, SuiteResult, collect_integrity_files,
        sha256_file,
        validation::{read_json, validate_manifest, validate_suite},
    },
};
use crate::lab::provenance::LabProvenance;

#[derive(Debug, Serialize)]
pub(crate) struct VerificationCompletion {
    pub(crate) schema: u16,
    pub(crate) target: String,
    pub(crate) status: &'static str,
    pub(crate) runs: usize,
    pub(crate) attachments: usize,
    pub(crate) firmware_artifacts: usize,
    pub(crate) verified_run_ids: Vec<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct ArchivedFirmware {
    pub(crate) run_id: String,
    pub(crate) image: crate::image::ImageClass,
    pub(crate) application_path: PathBuf,
    pub(crate) application_sha256: String,
    pub(crate) build_id: Option<String>,
    pub(super) source_directory: PathBuf,
    pub(super) integrity_sha256: String,
    pub(super) repository: super::run::RepositoryProvenance,
    pub(super) artifact: super::run::FirmwareArtifact,
    pub(super) build_provenance: Option<BuildProvenance>,
}

pub(crate) fn verify(
    root: &Path,
    target: &str,
    run_id: Option<&str>,
) -> Result<VerificationCompletion> {
    verify_at(&root.join("target/hil").join(target), target, run_id)
}

pub(crate) fn archived_firmware(
    root: &Path,
    target: &str,
    run_id: &str,
    image: crate::image::ImageClass,
) -> Result<ArchivedFirmware> {
    verify(root, target, Some(run_id))?;
    let run_directory = root
        .join("target/hil")
        .join(target)
        .join("runs")
        .join(run_id);
    let manifest: RunManifest = read_json(&run_directory.join("manifest.json"))?;
    let artifact = manifest
        .firmware
        .iter()
        .find(|artifact| artifact.image == image)
        .ok_or_else(|| {
            format!(
                "HIL run `{run_id}` has no archived `{}` firmware",
                image.id()
            )
        })?;
    let build_provenance = artifact
        .build_provenance_path
        .as_ref()
        .map(|path| read_json(&run_directory.join(path)))
        .transpose()?;
    Ok(ArchivedFirmware {
        run_id: run_id.to_owned(),
        image,
        application_path: run_directory.join(&artifact.application_path),
        application_sha256: artifact.application_sha256.clone(),
        build_id: artifact.build_id.clone(),
        source_directory: run_directory.clone(),
        integrity_sha256: sha256_file(&run_directory.join("integrity.json"))?,
        repository: manifest.repository,
        artifact: artifact.clone(),
        build_provenance,
    })
}

pub(crate) fn verify_at(
    target_directory: &Path,
    target: &str,
    run_id: Option<&str>,
) -> Result<VerificationCompletion> {
    let runs_directory = target_directory.join("runs");
    let run_directories = select_run_directories(&runs_directory, run_id)?;
    let mut attachments = 0;
    let mut firmware_artifacts = 0;
    let mut verified_run_ids = Vec::with_capacity(run_directories.len());

    for run_directory in run_directories {
        let manifest_path = run_directory.join("manifest.json");
        require_regular_file(&manifest_path)?;
        let manifest: RunManifest = read_json(&manifest_path)?;
        validate_manifest(&manifest, target, &run_directory)?;
        if manifest.state == RunState::Running {
            return Err(format!(
                "HIL run `{}` is still running and has no immutable integrity seal",
                manifest.run_id
            )
            .into());
        }
        validate_lab_provenance(&run_directory, &manifest)?;
        validate_firmware(&run_directory, &manifest)?;
        firmware_artifacts += manifest.firmware.len();

        if manifest.state == RunState::Completed {
            let suite_path = run_directory.join("suite.json");
            require_regular_file(&suite_path)?;
            let suite: SuiteResult = read_json(&suite_path)?;
            validate_suite(&suite, &manifest)?;
            attachments += validate_attachments(&run_directory, &suite)?;
        }
        validate_integrity_index(&run_directory, &manifest)?;
        verified_run_ids.push(manifest.run_id);
    }

    Ok(VerificationCompletion {
        schema: RUN_SCHEMA,
        target: target.to_owned(),
        status: "verified",
        runs: verified_run_ids.len(),
        attachments,
        firmware_artifacts,
        verified_run_ids,
    })
}

fn validate_lab_provenance(run_directory: &Path, manifest: &RunManifest) -> Result<()> {
    let Some(path) = manifest.lab_provenance_path.as_deref() else {
        // Schema-2 bundles created before lab provenance remain valid history.
        return Ok(());
    };
    let expected = Path::new("lab-provenance.json");
    if path != expected {
        return Err(format!(
            "HIL run `{}` has a non-canonical lab provenance path `{}`",
            manifest.run_id,
            path.display()
        )
        .into());
    }
    require_regular_file_below(run_directory, path)?;
    let provenance: LabProvenance = read_json(&run_directory.join(path))?;
    if provenance.scope == crate::lab::provenance::ObservationScope::System {
        let plan: crate::evidence::run::RunPlan = read_json(&run_directory.join("plan.json"))?;
        let selected: Vec<_> = plan
            .entries
            .iter()
            .filter(|entry| entry.disposition == crate::evidence::run::PlanDisposition::Selected)
            .collect();
        if plan.run_id != manifest.run_id
            || plan.schema != RUN_SCHEMA
            || selected.is_empty()
            || selected.iter().any(|entry| {
                entry.requirements.is_none_or(|required| {
                    required != crate::lab::requirements::Requirements::default()
                })
            })
        {
            return Err(
                "system-only lab provenance requires an explicit plan with no network dependencies"
                    .into(),
            );
        }
        for entry in selected {
            let snapshot = PathBuf::from("scenarios")
                .join(&entry.scenario)
                .join("scenario.json");
            validate_relative_path(&snapshot, "scenario snapshot")?;
            require_regular_file_below(run_directory, &snapshot)?;
            let scenario: crate::scenario::Scenario = read_json(&run_directory.join(snapshot))?;
            if scenario.id != entry.scenario
                || scenario.image != entry.image
                || scenario.repetitions != entry.repetitions
                || Some(crate::lab::requirements::Requirements::for_scenario(
                    &scenario,
                )) != entry.requirements
            {
                return Err(
                    "system-only lab provenance disagrees with the selected scenario snapshot"
                        .into(),
                );
            }
        }
    }
    provenance.validate_binding(
        &manifest.cell.cell_id,
        &manifest.cell.device_id,
        manifest.started_unix_millis,
        manifest.finished_unix_millis,
    )
}

fn validate_integrity_index(run_directory: &Path, manifest: &RunManifest) -> Result<()> {
    let path = run_directory.join("integrity.json");
    require_regular_file(&path)?;
    let index: IntegrityIndex = read_json(&path)?;
    if index.schema != RUN_SCHEMA || index.run_id != manifest.run_id {
        return Err(format!(
            "HIL integrity index `{}` does not match its manifest",
            path.display()
        )
        .into());
    }
    let mut indexed_paths = BTreeSet::new();
    for file in &index.files {
        validate_relative_path(&file.path, "integrity index")?;
        validate_sha256(
            &file.sha256,
            "integrity index",
            &file.path.display().to_string(),
        )?;
        if file.path == Path::new("integrity.json") || !indexed_paths.insert(&file.path) {
            return Err(format!(
                "HIL integrity index `{}` has a reserved or duplicate path `{}`",
                path.display(),
                file.path.display()
            )
            .into());
        }
    }
    let actual_files = collect_integrity_files(run_directory)?;
    if index.files != actual_files {
        return Err(format!(
            "HIL run `{}` does not match its sealed file inventory",
            manifest.run_id
        )
        .into());
    }
    Ok(())
}

fn select_run_directories(runs_directory: &Path, run_id: Option<&str>) -> Result<Vec<PathBuf>> {
    require_directory(runs_directory)?;
    if let Some(run_id) = run_id {
        if !is_single_normal_component(Path::new(run_id)) {
            return Err(format!("invalid HIL run ID `{run_id}`").into());
        }
        let directory = runs_directory.join(run_id);
        require_directory(&directory)?;
        return Ok(vec![directory]);
    }

    let mut entries = fs::read_dir(runs_directory)?.collect::<std::io::Result<Vec<_>>>()?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    let mut directories = Vec::with_capacity(entries.len());
    for entry in entries {
        if !entry.file_type()?.is_dir() {
            return Err(format!(
                "HIL runs directory contains a non-directory entry: {}",
                entry.path().display()
            )
            .into());
        }
        directories.push(entry.path());
    }
    Ok(directories)
}

fn validate_firmware(run_directory: &Path, manifest: &RunManifest) -> Result<()> {
    let mut images = BTreeSet::new();
    let mut paths = BTreeSet::new();
    for artifact in &manifest.firmware {
        validate_replay_origin(manifest, artifact)?;
        let expected_path = PathBuf::from("firmware")
            .join(artifact.image.id())
            .join("application.bin");
        if artifact.application_path != expected_path
            || !images.insert(artifact.image.id())
            || !paths.insert(artifact.application_path.clone())
        {
            return Err(format!(
                "HIL run `{}` has non-canonical firmware provenance for `{}`",
                manifest.run_id,
                artifact.image.id()
            )
            .into());
        }
        validate_sha256(
            &artifact.runtime_elf_sha256,
            "runtime ELF",
            &manifest.run_id,
        )?;
        validate_optional_firmware_file(
            run_directory,
            &mut paths,
            artifact.runtime_elf_path.as_deref(),
            artifact.runtime_elf_size_bytes,
            &artifact.runtime_elf_sha256,
            &PathBuf::from("firmware")
                .join(artifact.image.id())
                .join("runtime.elf"),
            "runtime ELF",
        )?;
        validate_sha256(
            &artifact.runtime_bin_sha256,
            "runtime binary",
            &manifest.run_id,
        )?;
        validate_optional_firmware_file(
            run_directory,
            &mut paths,
            artifact.runtime_bin_path.as_deref(),
            artifact.runtime_bin_size_bytes,
            &artifact.runtime_bin_sha256,
            &PathBuf::from("firmware")
                .join(artifact.image.id())
                .join("runtime.bin"),
            "runtime binary",
        )?;
        validate_sha256(
            &artifact.bootstrap_elf_sha256,
            "bootstrap ELF",
            &manifest.run_id,
        )?;
        validate_optional_firmware_file(
            run_directory,
            &mut paths,
            artifact.bootstrap_elf_path.as_deref(),
            artifact.bootstrap_elf_size_bytes,
            &artifact.bootstrap_elf_sha256,
            &PathBuf::from("firmware")
                .join(artifact.image.id())
                .join("bootstrap.elf"),
            "bootstrap ELF",
        )?;
        verify_indexed_file(
            run_directory,
            &artifact.application_path,
            artifact.application_size_bytes,
            &artifact.application_sha256,
            "firmware application",
        )?;
        match (&artifact.build_id, &artifact.build_provenance_path) {
            (None, None) => {}
            (Some(build_id), Some(path)) => {
                let expected = PathBuf::from("firmware")
                    .join(artifact.image.id())
                    .join("build-provenance.json");
                if path != &expected || !paths.insert(path.clone()) {
                    return Err(format!(
                        "HIL run `{}` has non-canonical build provenance for `{}`",
                        manifest.run_id,
                        artifact.image.id()
                    )
                    .into());
                }
                validate_build_provenance(run_directory, manifest, artifact, build_id, path)?;
            }
            _ => {
                return Err(format!(
                    "HIL run `{}` has incomplete build provenance reference",
                    manifest.run_id
                )
                .into());
            }
        }
    }
    Ok(())
}

fn validate_optional_firmware_file(
    run_directory: &Path,
    paths: &mut BTreeSet<PathBuf>,
    path: Option<&Path>,
    size_bytes: Option<u64>,
    sha256: &str,
    expected_path: &Path,
    kind: &str,
) -> Result<()> {
    match (path, size_bytes) {
        (None, None) => Ok(()),
        (Some(path), Some(size_bytes))
            if path == expected_path && paths.insert(path.to_owned()) =>
        {
            verify_indexed_file(run_directory, path, size_bytes, sha256, kind)
        }
        _ => Err(format!(
            "HIL {kind} has incomplete, duplicate or non-canonical archive provenance"
        )
        .into()),
    }
}

fn validate_build_provenance(
    run_directory: &Path,
    manifest: &RunManifest,
    artifact: &super::run::FirmwareArtifact,
    expected_build_id: &str,
    path: &Path,
) -> Result<()> {
    validate_relative_path(path, "build provenance")?;
    require_regular_file_below(run_directory, path)?;
    let provenance: BuildProvenance = read_json(&run_directory.join(path))?;
    if provenance.schema != BUILD_PROVENANCE_SCHEMA
        || provenance.build_id != expected_build_id
        || provenance.build_id != build_id(&provenance.subjects)
        || provenance.build_type != "open-esp-radio-hil-firmware/v1"
        || provenance.parameters.image != artifact.image
        || provenance.parameters.runtime_profile != artifact.image.runtime_profile()
        || provenance.parameters.runtime_features != artifact.image.runtime_features()
        || provenance.parameters.target != crate::image::TARGET
        || provenance.reproducibility != BuildReproducibility::Unverified
    {
        return Err(format!(
            "HIL run `{}` has build provenance inconsistent with `{}`",
            manifest.run_id,
            artifact.image.id()
        )
        .into());
    }
    let expected_subjects = vec![
        BuildSubject {
            role: BuildSubjectRole::Application,
            path: artifact.application_path.clone(),
            size_bytes: artifact.application_size_bytes,
            sha256: artifact.application_sha256.clone(),
        },
        BuildSubject {
            role: BuildSubjectRole::BootstrapElf,
            path: artifact
                .bootstrap_elf_path
                .clone()
                .ok_or("build provenance requires an archived bootstrap ELF")?,
            size_bytes: artifact
                .bootstrap_elf_size_bytes
                .ok_or("build provenance requires a bootstrap ELF size")?,
            sha256: artifact.bootstrap_elf_sha256.clone(),
        },
        BuildSubject {
            role: BuildSubjectRole::RuntimeBin,
            path: artifact
                .runtime_bin_path
                .clone()
                .ok_or("build provenance requires an archived runtime binary")?,
            size_bytes: artifact
                .runtime_bin_size_bytes
                .ok_or("build provenance requires a runtime binary size")?,
            sha256: artifact.runtime_bin_sha256.clone(),
        },
        BuildSubject {
            role: BuildSubjectRole::RuntimeElf,
            path: artifact
                .runtime_elf_path
                .clone()
                .ok_or("build provenance requires an archived runtime ELF")?,
            size_bytes: artifact
                .runtime_elf_size_bytes
                .ok_or("build provenance requires a runtime ELF size")?,
            sha256: artifact.runtime_elf_sha256.clone(),
        },
    ];
    if provenance.subjects != expected_subjects || provenance.sources.is_empty() {
        return Err(format!(
            "HIL run `{}` has inconsistent build subjects or no source materials",
            manifest.run_id
        )
        .into());
    }
    for subject in &provenance.subjects {
        verify_indexed_file(
            run_directory,
            &subject.path,
            subject.size_bytes,
            &subject.sha256,
            "build subject",
        )?;
    }
    let mut source_names = BTreeSet::new();
    for source in &provenance.sources {
        if source.name.is_empty() || !source_names.insert(&source.name) {
            return Err(format!(
                "HIL run `{}` has an invalid or duplicate source material",
                manifest.run_id
            )
            .into());
        }
        validate_source_material(run_directory, manifest, source)?;
    }
    let primary = &provenance.sources[0];
    let expected_repository = artifact
        .replayed_from
        .as_ref()
        .map(|origin| &origin.firmware_repository)
        .unwrap_or(&manifest.repository);
    if primary.name != "repository"
        || primary.commit != expected_repository.commit
        || primary.dirty != expected_repository.dirty
        || primary.workspace_sha256 != expected_repository.workspace_sha256
    {
        return Err(format!(
            "HIL run `{}` has primary source material inconsistent with its manifest",
            manifest.run_id
        )
        .into());
    }
    let source_reconstructable = provenance
        .sources
        .iter()
        .all(|source| source.rebuild_status != SourceRebuildStatus::Incomplete);
    if provenance.source_reconstructable != source_reconstructable {
        return Err(format!(
            "HIL run `{}` has inconsistent source reconstructability",
            manifest.run_id
        )
        .into());
    }
    let mut file_names = BTreeSet::new();
    let mut file_paths = BTreeSet::new();
    for file in &provenance.files {
        validate_relative_path(&file.path, "build file material")?;
        validate_sha256(&file.sha256, "build file material", &file.name)?;
        if file.name.is_empty() || !file_names.insert(&file.name) || !file_paths.insert(&file.path)
        {
            return Err(format!(
                "HIL run `{}` has an invalid or duplicate build file material",
                manifest.run_id
            )
            .into());
        }
        if let Some(archive_path) = &file.archive_path {
            verify_indexed_file(
                run_directory,
                archive_path,
                file.size_bytes,
                &file.sha256,
                "archived build file material",
            )?;
        }
    }
    let expected_lock_archive = PathBuf::from("firmware")
        .join(artifact.image.id())
        .join("effective-Cargo.lock");
    if provenance
        .files
        .iter()
        .find(|file| file.name == "embedded-lock")
        .and_then(|file| {
            file.archive_path
                .as_ref()
                .filter(|path| *path == &expected_lock_archive)
        })
        .is_none()
    {
        return Err(format!(
            "HIL run `{}` does not archive its effective embedded lock file",
            manifest.run_id
        )
        .into());
    }
    let mut tools = BTreeSet::new();
    if provenance.environment.cargo_incremental != "0"
        || provenance.environment.tools.iter().any(|tool| {
            tool.name.is_empty() || tool.program.is_empty() || !tools.insert(&tool.name)
        })
    {
        return Err(format!(
            "HIL run `{}` has invalid build environment provenance",
            manifest.run_id
        )
        .into());
    }
    Ok(())
}

fn validate_replay_origin(
    manifest: &RunManifest,
    artifact: &super::run::FirmwareArtifact,
) -> Result<()> {
    let Some(origin) = &artifact.replayed_from else {
        return Ok(());
    };
    if origin.source_run_id == manifest.run_id
        || !is_single_normal_component(Path::new(&origin.source_run_id))
        || origin.source_build_id != artifact.build_id
    {
        return Err(format!(
            "HIL run `{}` has inconsistent replay origin for `{}`",
            manifest.run_id,
            artifact.image.id()
        )
        .into());
    }
    validate_sha256(
        &origin.source_integrity_sha256,
        "source run integrity",
        &origin.source_run_id,
    )
}

fn validate_source_material(
    run_directory: &Path,
    manifest: &RunManifest,
    source: &SourceMaterial,
) -> Result<()> {
    match (
        source.tracked_patch_path.as_deref(),
        source.tracked_patch_size_bytes,
        source.tracked_patch_sha256.as_deref(),
    ) {
        (None, None, None) => {}
        (Some(path), Some(size_bytes), Some(sha256)) => verify_indexed_file(
            run_directory,
            path,
            size_bytes,
            sha256,
            "tracked source patch",
        )?,
        _ => {
            return Err(format!(
                "HIL run `{}` has incomplete tracked source patch provenance",
                manifest.run_id
            )
            .into());
        }
    }
    let mut untracked_paths = BTreeSet::new();
    validate_sha256(&source.workspace_sha256, "source workspace", &source.name)?;
    for file in &source.untracked_files {
        validate_relative_path(&file.path, "untracked source identity")?;
        validate_sha256(
            &file.sha256,
            "untracked source identity",
            &file.path.display().to_string(),
        )?;
        if !untracked_paths.insert(&file.path) {
            return Err(format!(
                "HIL run `{}` has a duplicate untracked source path `{}`",
                manifest.run_id,
                file.path.display()
            )
            .into());
        }
    }
    let state_is_consistent = match source.rebuild_status {
        SourceRebuildStatus::CleanCommit => {
            !source.dirty
                && source.tracked_patch_path.is_none()
                && source.untracked_files.is_empty()
                && source.limitations.is_empty()
                && source.remote.is_some()
                && !source.commit.is_empty()
        }
        SourceRebuildStatus::TrackedPatch => {
            source.dirty
                && source.tracked_patch_path.is_some()
                && source.untracked_files.is_empty()
                && source.limitations.is_empty()
                && source.remote.is_some()
                && !source.commit.is_empty()
        }
        SourceRebuildStatus::Incomplete => !source.limitations.is_empty(),
    };
    if !state_is_consistent {
        return Err(format!(
            "HIL run `{}` has inconsistent source rebuild provenance",
            manifest.run_id
        )
        .into());
    }
    Ok(())
}

fn validate_attachments(run_directory: &Path, suite: &SuiteResult) -> Result<usize> {
    let mut paths = BTreeSet::new();
    let mut count = 0;
    for scenario in &suite.scenarios {
        for repetition in &scenario.repetitions {
            validate_relative_path(&repetition.artifact_directory, "artifact directory")?;
            for attachment in &repetition.attachments {
                if attachment.media_type.is_empty()
                    || !attachment.path.starts_with(&repetition.artifact_directory)
                    || !paths.insert(&attachment.path)
                {
                    return Err(format!(
                        "HIL run `{}` has an invalid or duplicate attachment `{}`",
                        suite.run_id,
                        attachment.path.display()
                    )
                    .into());
                }
                verify_indexed_file(
                    run_directory,
                    &attachment.path,
                    attachment.size_bytes,
                    &attachment.sha256,
                    "attachment",
                )?;
                count += 1;
            }
        }
    }
    Ok(count)
}

fn verify_indexed_file(
    run_directory: &Path,
    relative_path: &Path,
    expected_size: u64,
    expected_sha256: &str,
    kind: &str,
) -> Result<()> {
    validate_relative_path(relative_path, kind)?;
    validate_sha256(expected_sha256, kind, &relative_path.display().to_string())?;
    let path = run_directory.join(relative_path);
    require_regular_file_below(run_directory, relative_path)?;
    let actual_size = fs::metadata(&path)?.len();
    if actual_size != expected_size {
        return Err(format!(
            "HIL {kind} `{}` has size {actual_size}, expected {expected_size}",
            path.display()
        )
        .into());
    }
    let actual_sha256 = sha256_file(&path)?;
    if actual_sha256 != expected_sha256 {
        return Err(format!(
            "HIL {kind} `{}` has SHA-256 {actual_sha256}, expected {expected_sha256}",
            path.display()
        )
        .into());
    }
    Ok(())
}

fn validate_relative_path(path: &Path, kind: &str) -> Result<()> {
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(format!(
            "HIL {kind} path is not a safe relative path: {}",
            path.display()
        )
        .into());
    }
    Ok(())
}

fn is_single_normal_component(path: &Path) -> bool {
    let mut components = path.components();
    matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none()
}

fn validate_sha256(value: &str, kind: &str, owner: &str) -> Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(format!("HIL {kind} `{owner}` has an invalid SHA-256 digest").into());
    }
    Ok(())
}

fn require_regular_file_below(root: &Path, relative_path: &Path) -> Result<()> {
    let mut current = root.to_owned();
    let component_count = relative_path.components().count();
    for (index, component) in relative_path.components().enumerate() {
        let Component::Normal(component) = component else {
            return Err(format!(
                "HIL file path is not a safe relative path: {}",
                relative_path.display()
            )
            .into());
        };
        current.push(component);
        let metadata = fs::symlink_metadata(&current)?;
        let last = index + 1 == component_count;
        if (last && !metadata.file_type().is_file()) || (!last && !metadata.file_type().is_dir()) {
            return Err(format!(
                "HIL bundle path has an unexpected file type: {}",
                current.display()
            )
            .into());
        }
    }
    Ok(())
}

fn require_regular_file(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_file() {
        return Err(format!("HIL bundle path is not a regular file: {}", path.display()).into());
    }
    Ok(())
}

fn require_directory(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_dir() {
        return Err(format!("HIL bundle path is not a directory: {}", path.display()).into());
    }
    Ok(())
}

#[cfg(test)]
mod tests;
