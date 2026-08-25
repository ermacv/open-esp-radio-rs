//! Offline integrity verification for immutable HIL run bundles.

use std::{
    collections::BTreeSet,
    fs,
    path::{Component, Path, PathBuf},
};

use serde::Serialize;

use super::{
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

pub(crate) fn verify(
    root: &Path,
    target: &str,
    run_id: Option<&str>,
) -> Result<VerificationCompletion> {
    verify_at(&root.join("target/hil").join(target), target, run_id)
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
            || !paths.insert(&artifact.application_path)
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
        validate_sha256(
            &artifact.runtime_bin_sha256,
            "runtime binary",
            &manifest.run_id,
        )?;
        validate_sha256(
            &artifact.bootstrap_elf_sha256,
            "bootstrap ELF",
            &manifest.run_id,
        )?;
        verify_indexed_file(
            run_directory,
            &artifact.application_path,
            artifact.application_size_bytes,
            &artifact.application_sha256,
            "firmware application",
        )?;
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
