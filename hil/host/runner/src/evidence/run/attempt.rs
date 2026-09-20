//! Durable completion of one scenario's whole repetition set.
//!
//! A seal references immutable material in the enclosing run without copying
//! firmware. Publication is last and atomic; a missing seal makes no claim.
//! Closing an attempt neither closes the campaign nor releases fixture leases.

use super::*;
use crate::scenario::Scenario;
use serde::Serialize;

#[derive(Serialize)]
struct AttemptSeal {
    schema: u16,
    manifest: RunManifest,
    suite: SuiteResult,
    files: Vec<IntegrityFile>,
}

impl RunSession {
    pub(crate) fn seal_scenario(
        &mut self,
        scenario: &Scenario,
        result: &ScenarioResult,
    ) -> Result<()> {
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
        let mut files = Vec::new();
        let mut roots = vec![
            PathBuf::from("scenarios").join(&scenario.id),
            PathBuf::from("source"),
        ];
        if !manifest.firmware.is_empty() {
            let firmware = PathBuf::from("firmware").join(scenario.image.id());
            if !self.directory.join(&firmware).try_exists()? {
                return Err("attempt firmware material is missing".into());
            }
            roots.push(firmware);
        }
        for relative in roots {
            let path = self.directory.join(&relative);
            if path.try_exists()? {
                if !fs::symlink_metadata(&path)?.file_type().is_dir() {
                    return Err("attempt material root must be a regular directory".into());
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
            let path = self.directory.join(relative);
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
