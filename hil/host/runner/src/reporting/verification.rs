//! Offline integrity verification for immutable HIL run bundles.

use std::{
    collections::BTreeSet,
    fs,
    path::{Component, Path, PathBuf},
};

use serde::Serialize;

use super::{
    build::{
        BUILD_PROVENANCE_SCHEMA, BuildProvenance, BuildReproducibility, BuildSubject,
        BuildSubjectRole, SourceMaterial, SourceRebuildStatus, build_id,
    },
    history::{read_json, validate_manifest, validate_suite},
    run::{
        IntegrityIndex, RUN_SCHEMA, RunManifest, RunState, SuiteResult, collect_integrity_files,
        sha256_file,
    },
};
use crate::Result;

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
    pub(crate) image: crate::qualification::scenario::ImageClass,
    pub(crate) application_path: PathBuf,
    pub(crate) application_sha256: String,
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
    image: crate::qualification::scenario::ImageClass,
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
    Ok(ArchivedFirmware {
        run_id: run_id.to_owned(),
        image,
        application_path: run_directory.join(&artifact.application_path),
        application_sha256: artifact.application_sha256.clone(),
    })
}

fn verify_at(
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
    if primary.name != "repository"
        || primary.commit != manifest.repository.commit
        || primary.dirty != manifest.repository.dirty
        || primary.workspace_sha256 != manifest.repository.workspace_sha256
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
mod tests {
    use super::*;
    use crate::{
        qualification::scenario::ImageClass,
        reporting::run::{
            Attachment, Measurement, MeasurementUnit, Outcome, RepetitionResult, ScenarioResult,
            SuiteCounts, atomic_json, write_integrity_index,
        },
    };
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn fixture() -> (PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "open-radio-hil-verification-{}-{}",
            std::process::id(),
            TEST_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let run = root.join("target/hil/esp32s31/runs/run-1");
        let artifact_path = PathBuf::from("scenarios/icmp/repetition-001/evidence.log");
        let application_path = PathBuf::from("firmware/correctness/application.bin");
        fs::create_dir_all(run.join(artifact_path.parent().unwrap())).unwrap();
        fs::create_dir_all(run.join(application_path.parent().unwrap())).unwrap();
        fs::write(run.join(&artifact_path), b"serial evidence").unwrap();
        fs::write(run.join(&application_path), b"flashed application").unwrap();
        let attachment_sha256 = sha256_file(&run.join(&artifact_path)).unwrap();
        let application_sha256 = sha256_file(&run.join(&application_path)).unwrap();
        atomic_json(
            &run.join("manifest.json"),
            &serde_json::json!({
                "schema": RUN_SCHEMA,
                "run_id": "run-1",
                "target": "esp32s31",
                "state": "completed",
                "started_unix_millis": 100,
                "finished_unix_millis": 200,
                "duration_millis": 100,
                "invocation": ["cargo", "hil", "run", "icmp"],
                "repository": {
                    "commit": "0123456789abcdef",
                    "dirty": false,
                    "workspace_sha256": "00"
                },
                "runner": {
                    "package": "runner",
                    "version": "1",
                    "protocol_version": 1,
                    "host_os": "linux",
                    "host_arch": "x86_64",
                    "tools": []
                },
                "cell": {
                    "cell_id": "cell-1",
                    "device_id": "dut-1",
                    "serial_device": "/dev/ttyACM0"
                },
                "firmware": [{
                    "image": "correctness",
                    "application_path": application_path,
                    "application_size_bytes": 19,
                    "application_sha256": application_sha256,
                    "runtime_elf_sha256": "00".repeat(32),
                    "runtime_bin_sha256": "11".repeat(32),
                    "bootstrap_elf_sha256": "22".repeat(32)
                }]
            }),
        )
        .unwrap();
        let scenarios = vec![ScenarioResult::from_repetitions(
            String::from("icmp"),
            ImageClass::Correctness,
            1,
            vec![RepetitionResult {
                schema: RUN_SCHEMA,
                repetition: 1,
                outcome: Outcome::Passed,
                started_unix_millis: 100,
                duration_millis: 100,
                artifact_directory: PathBuf::from("scenarios/icmp/repetition-001"),
                attachments: vec![Attachment {
                    path: artifact_path,
                    media_type: String::from("text/plain"),
                    size_bytes: 15,
                    sha256: attachment_sha256,
                }],
                measurements: vec![Measurement::observed(
                    "icmp.replies.received",
                    1,
                    MeasurementUnit::Count,
                )],
                failure: None,
            }],
        )];
        atomic_json(
            &run.join("suite.json"),
            &SuiteResult {
                schema: RUN_SCHEMA,
                run_id: String::from("run-1"),
                target: String::from("esp32s31"),
                outcome: Outcome::Passed,
                started_unix_millis: 100,
                finished_unix_millis: 200,
                duration_millis: 100,
                counts: SuiteCounts::from_results(&scenarios),
                scenarios,
            },
        )
        .unwrap();
        fs::write(run.join("report.html"), b"generated report").unwrap();
        write_integrity_index(&run, "run-1").unwrap();
        (root, run)
    }

    fn add_build_provenance(run: &Path) {
        let runtime_elf_path = PathBuf::from("firmware/correctness/runtime.elf");
        let runtime_bin_path = PathBuf::from("firmware/correctness/runtime.bin");
        let bootstrap_elf_path = PathBuf::from("firmware/correctness/bootstrap.elf");
        let effective_lock_path = PathBuf::from("firmware/correctness/effective-Cargo.lock");
        fs::write(run.join(&runtime_elf_path), b"runtime elf").unwrap();
        fs::write(run.join(&runtime_bin_path), b"runtime bin").unwrap();
        fs::write(run.join(&bootstrap_elf_path), b"bootstrap elf").unwrap();
        fs::write(run.join(&effective_lock_path), b"effective lock").unwrap();
        let mut manifest: RunManifest = read_json(&run.join("manifest.json")).unwrap();
        manifest.repository.workspace_sha256 = "00".repeat(32);
        let artifact = &mut manifest.firmware[0];
        artifact.runtime_elf_path = Some(runtime_elf_path.clone());
        artifact.runtime_elf_size_bytes = Some(11);
        artifact.runtime_elf_sha256 = sha256_file(&run.join(&runtime_elf_path)).unwrap();
        artifact.runtime_bin_path = Some(runtime_bin_path.clone());
        artifact.runtime_bin_size_bytes = Some(11);
        artifact.runtime_bin_sha256 = sha256_file(&run.join(&runtime_bin_path)).unwrap();
        artifact.bootstrap_elf_path = Some(bootstrap_elf_path.clone());
        artifact.bootstrap_elf_size_bytes = Some(13);
        artifact.bootstrap_elf_sha256 = sha256_file(&run.join(&bootstrap_elf_path)).unwrap();
        let subjects = vec![
            BuildSubject {
                role: BuildSubjectRole::Application,
                path: artifact.application_path.clone(),
                size_bytes: artifact.application_size_bytes,
                sha256: artifact.application_sha256.clone(),
            },
            BuildSubject {
                role: BuildSubjectRole::BootstrapElf,
                path: bootstrap_elf_path,
                size_bytes: 13,
                sha256: artifact.bootstrap_elf_sha256.clone(),
            },
            BuildSubject {
                role: BuildSubjectRole::RuntimeBin,
                path: runtime_bin_path,
                size_bytes: 11,
                sha256: artifact.runtime_bin_sha256.clone(),
            },
            BuildSubject {
                role: BuildSubjectRole::RuntimeElf,
                path: runtime_elf_path,
                size_bytes: 11,
                sha256: artifact.runtime_elf_sha256.clone(),
            },
        ];
        let build_id = super::super::build::build_id(&subjects);
        let provenance_path = PathBuf::from("firmware/correctness/build-provenance.json");
        artifact.build_id = Some(build_id.clone());
        artifact.build_provenance_path = Some(provenance_path.clone());
        atomic_json(&run.join("manifest.json"), &manifest).unwrap();
        atomic_json(
            &run.join(&provenance_path),
            &BuildProvenance {
                schema: BUILD_PROVENANCE_SCHEMA,
                build_id,
                build_type: String::from("open-esp-radio-hil-firmware/v1"),
                parameters: super::super::build::BuildParameters {
                    image: ImageClass::Correctness,
                    runtime_profile: ImageClass::Correctness.runtime_profile().to_owned(),
                    target: crate::image::TARGET.to_owned(),
                    runtime_features: ImageClass::Correctness.runtime_features().to_owned(),
                },
                sources: vec![SourceMaterial {
                    name: String::from("repository"),
                    checkout_path: PathBuf::from("/build/source"),
                    remote: Some(String::from("https://example.invalid/repository.git")),
                    commit: manifest.repository.commit.clone(),
                    dirty: manifest.repository.dirty,
                    workspace_sha256: manifest.repository.workspace_sha256.clone(),
                    rebuild_status: SourceRebuildStatus::CleanCommit,
                    tracked_patch_path: None,
                    tracked_patch_size_bytes: None,
                    tracked_patch_sha256: None,
                    untracked_files: Vec::new(),
                    limitations: Vec::new(),
                }],
                files: vec![super::super::build::BuildFileMaterial {
                    name: String::from("embedded-lock"),
                    path: PathBuf::from("hil/targets/esp32s31/Cargo.lock"),
                    archive_path: Some(effective_lock_path.clone()),
                    size_bytes: 14,
                    sha256: sha256_file(&run.join(&effective_lock_path)).unwrap(),
                }],
                environment: super::super::build::BuildEnvironment {
                    tools: Vec::new(),
                    inherited_rustflags: None,
                    inherited_encoded_rustflags: None,
                    cargo_incremental: String::from("0"),
                    source_date_epoch: None,
                },
                subjects,
                source_reconstructable: true,
                reproducibility: super::super::build::BuildReproducibility::Unverified,
            },
        )
        .unwrap();
        write_integrity_index(run, "run-1").unwrap();
    }

    #[test]
    fn verifies_firmware_and_attachment_content() {
        let (root, _) = fixture();
        let completion = verify(&root, "esp32s31", None).unwrap();
        assert_eq!(completion.status, "verified");
        assert_eq!(completion.runs, 1);
        assert_eq!(completion.attachments, 1);
        assert_eq!(completion.firmware_artifacts, 1);
        assert_eq!(completion.verified_run_ids, ["run-1"]);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn selects_only_verified_archived_firmware_for_replay() {
        let (root, run) = fixture();
        let firmware = archived_firmware(&root, "esp32s31", "run-1", ImageClass::Correctness)
            .expect("select archived firmware");
        assert_eq!(
            firmware.application_path,
            run.join("firmware/correctness/application.bin")
        );
        assert_eq!(firmware.application_sha256.len(), 64);
        let error = archived_firmware(&root, "esp32s31", "run-1", ImageClass::Performance)
            .expect_err("reject absent image class");
        assert!(
            error
                .to_string()
                .contains("no archived `performance` firmware")
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn verifies_complete_build_provenance_and_all_firmware_subjects() {
        let (root, run) = fixture();
        add_build_provenance(&run);
        let completion = verify(&root, "esp32s31", Some("run-1")).unwrap();
        assert_eq!(completion.firmware_artifacts, 1);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_tampered_archived_runtime_elf() {
        let (root, run) = fixture();
        add_build_provenance(&run);
        fs::write(run.join("firmware/correctness/runtime.elf"), b"runtime elF").unwrap();
        let error = verify(&root, "esp32s31", Some("run-1")).unwrap_err();
        assert!(error.to_string().contains("SHA-256"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_tampered_attachment() {
        let (root, run) = fixture();
        fs::write(
            run.join("scenarios/icmp/repetition-001/evidence.log"),
            b"serial evidencE",
        )
        .unwrap();
        let error = verify(&root, "esp32s31", Some("run-1")).unwrap_err();
        assert!(error.to_string().contains("SHA-256"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_paths_escaping_the_run_bundle() {
        let (root, run) = fixture();
        let mut suite: SuiteResult = read_json(&run.join("suite.json")).unwrap();
        suite.scenarios[0].repetitions[0].attachments[0].path = PathBuf::from("../outside");
        atomic_json(&run.join("suite.json"), &suite).unwrap();
        let error = verify(&root, "esp32s31", None).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("invalid or duplicate attachment")
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_unindexed_files() {
        let (root, run) = fixture();
        fs::write(run.join("injected.log"), b"not sealed").unwrap();
        let error = verify(&root, "esp32s31", None).unwrap_err();
        assert!(error.to_string().contains("sealed file inventory"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_tampered_derived_report() {
        let (root, run) = fixture();
        fs::write(run.join("report.html"), b"tampered report").unwrap();
        let error = verify(&root, "esp32s31", None).unwrap_err();
        assert!(error.to_string().contains("sealed file inventory"));
        fs::remove_dir_all(root).unwrap();
    }
}
