//! Durable completion of one scenario's whole repetition set.
//!
//! A seal references immutable material in the enclosing run without copying
//! firmware. Publication is last and atomic; a missing seal makes no claim.
//! Closing an attempt neither closes the campaign nor releases fixture leases.

use super::*;
use crate::scenario::Scenario;
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize)]
struct AttemptSeal {
    schema: u16,
    manifest: RunManifest,
    suite: SuiteResult,
    files: Vec<IntegrityFile>,
}

impl RunSession {
    pub fn seal_scenario(&mut self, scenario: &Scenario, result: &ScenarioResult) -> Result<()> {
        if result.scenario != scenario.id
            || result.image != scenario.image
            || result.required_repetitions != scenario.repetitions
            || scenario.id.is_empty()
            || !scenario
                .id
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        {
            return Err("attempt result does not match its selected scenario".into());
        }
        let seal_path = self
            .directory
            .join("attempts")
            .join(format!("{}.json", scenario.id));
        if seal_path.try_exists()? {
            return Err("a sealed scenario attempt cannot be replaced".into());
        }
        let mut manifest = self.manifest.clone();
        manifest.state = RunState::Completed;
        manifest.finished_unix_millis = Some(unix_millis()?);
        manifest.duration_millis = Some(duration_millis(self.started.elapsed()));
        // Other images may be prepared later. They are not this experiment's
        // subject, and their mutable manifest must not invalidate this seal.
        manifest
            .firmware
            .retain(|artifact| artifact.image == scenario.image);
        let suite = SuiteResult {
            schema: RUN_SCHEMA,
            run_id: manifest.run_id.clone(),
            target: manifest.target.clone(),
            outcome: if result.outcome.is_passed() {
                Outcome::Passed
            } else {
                Outcome::Failed
            },
            started_unix_millis: manifest.started_unix_millis,
            finished_unix_millis: manifest.finished_unix_millis.unwrap(),
            duration_millis: manifest.duration_millis.unwrap(),
            counts: SuiteCounts::from_results(std::slice::from_ref(result)),
            scenarios: vec![result.clone()],
        };
        validation::validate_suite(&suite, &manifest)?;
        let output = self.scenario_directory(&scenario.id);
        atomic_json(&output.join("scenario.json"), scenario)?;
        atomic_json(&output.join("result.json"), result)?;
        let files = material_files(&self.directory, &manifest, &scenario.id, scenario.image)?;
        atomic_json(
            &seal_path,
            &AttemptSeal {
                schema: 1,
                manifest,
                suite,
                files,
            },
        )
    }
}

fn material_files(
    run: &Path,
    manifest: &RunManifest,
    scenario: &str,
    image: ImageClass,
) -> Result<Vec<IntegrityFile>> {
    let mut files = Vec::new();
    let mut roots = vec![
        PathBuf::from("scenarios").join(scenario),
        PathBuf::from("source"),
    ];
    if !manifest.firmware.is_empty() {
        let firmware = PathBuf::from("firmware").join(image.id());
        if !run.join(&firmware).try_exists()? {
            return Err("attempt firmware material is missing".into());
        }
        roots.push(firmware);
    }
    for relative in roots {
        let path = run.join(&relative);
        if path.try_exists()? {
            let mut ancestor = run.to_owned();
            for component in relative.components() {
                ancestor.push(component);
                if !fs::symlink_metadata(&ancestor)?.file_type().is_dir() {
                    return Err(
                        "attempt material root must have regular directory components".into(),
                    );
                }
            }
            for file in collect_attachments(&path, &relative)? {
                files.push(IntegrityFile {
                    path: file.path,
                    size_bytes: file.size_bytes,
                    sha256: file.sha256,
                });
            }
        }
    }
    for relative in ["plan.json", "lab-provenance.json"] {
        let path = run.join(relative);
        if path.try_exists()? {
            let metadata = fs::symlink_metadata(&path)?;
            if !metadata.file_type().is_file() {
                return Err("attempt material must be a regular file".into());
            }
            files.push(IntegrityFile {
                path: relative.into(),
                size_bytes: metadata.len(),
                sha256: sha256_file(&path)?,
            });
        }
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
}

/// Read independently completed attempts using the producer's seal boundary.
/// A present attempts directory supersedes the enclosing suite, avoiding duplicates.
pub fn completed_attempts(run: &Path, parent: &RunManifest) -> Result<Option<Vec<SuiteResult>>> {
    let directory = run.join("attempts");
    if !directory.try_exists()? {
        return Ok(None);
    }
    if !fs::symlink_metadata(&directory)?.file_type().is_dir() {
        return Err("attempts must be a regular directory".into());
    }
    let mut entries = fs::read_dir(directory)?.collect::<std::io::Result<Vec<_>>>()?;
    entries.sort_by_key(fs::DirEntry::file_name);
    let mut suites = Vec::new();
    for entry in entries {
        let name = entry.file_name();
        let name = name.to_str().ok_or("invalid attempt filename")?;
        if name.starts_with('.') && name.contains(".tmp-") {
            continue;
        }
        if !entry.file_type()?.is_file() {
            return Err("attempt must be a regular file".into());
        }
        let mut seal: AttemptSeal = serde_json::from_slice(&fs::read(entry.path())?)?;
        if seal.schema != 1
            || seal.manifest.run_id != parent.run_id
            || seal.manifest.target != parent.target
            || seal.manifest.state != RunState::Completed
            || seal.suite.scenarios.len() != 1
            || seal.manifest.firmware.len() > 1
        {
            return Err("attempt completion identity is inconsistent".into());
        }
        validation::validate_suite(&seal.suite, &seal.manifest)?;
        let result = &seal.suite.scenarios[0];
        if name != format!("{}.json", result.scenario)
            || result.scenario.is_empty()
            || !result
                .scenario
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        {
            return Err("invalid attempt scenario identity".into());
        }
        let files = material_files(run, &seal.manifest, &result.scenario, result.image)?;
        seal.files.sort_by(|a, b| a.path.cmp(&b.path));
        for artifact in &seal.manifest.firmware {
            for (path, size, hash) in [
                (
                    Some(&artifact.application_path),
                    Some(artifact.application_size_bytes),
                    &artifact.application_sha256,
                ),
                (
                    artifact.runtime_elf_path.as_ref(),
                    artifact.runtime_elf_size_bytes,
                    &artifact.runtime_elf_sha256,
                ),
                (
                    artifact.runtime_bin_path.as_ref(),
                    artifact.runtime_bin_size_bytes,
                    &artifact.runtime_bin_sha256,
                ),
                (
                    artifact.bootstrap_elf_path.as_ref(),
                    artifact.bootstrap_elf_size_bytes,
                    &artifact.bootstrap_elf_sha256,
                ),
            ] {
                if let Some(path) = path
                    && !files
                        .iter()
                        .any(|f| &f.path == path && Some(f.size_bytes) == size && &f.sha256 == hash)
                {
                    return Err("attempt firmware identity is outside sealed material".into());
                }
            }
            if artifact
                .build_provenance_path
                .as_ref()
                .is_some_and(|p| !files.iter().any(|f| &f.path == p))
            {
                return Err("attempt build provenance is outside sealed material".into());
            }
        }
        let output = run.join("scenarios").join(&result.scenario);
        let scenario: Scenario = serde_json::from_slice(&fs::read(output.join("scenario.json"))?)?;
        let recorded: ScenarioResult =
            serde_json::from_slice(&fs::read(output.join("result.json"))?)?;
        if &recorded != result
            || scenario.id != result.scenario
            || scenario.image != result.image
            || scenario.repetitions != result.required_repetitions
            || seal
                .manifest
                .firmware
                .iter()
                .any(|f| f.image != result.image)
            || files != seal.files
        {
            return Err("attempt material does not match its seal".into());
        }
        suites.push(seal.suite);
    }
    Ok(Some(suites))
}
