//! Durable completion of one scenario's whole repetition set.
//!
//! A seal references immutable material in the enclosing run without copying
//! firmware. Publication is last and atomic; a missing seal makes no claim.
//! Closing an attempt neither closes the campaign nor releases fixture leases.

use super::*;
use oer_hil_scenario::{Header, Scenario, ScenarioFamily};
use serde::{Deserialize, Serialize};

/// The directory of a run's scenario seals.
pub const ATTEMPTS: &str = "attempts";

/// One scenario's sealed completion inside a run: the manifest and the
/// one-scenario suite the seal fixed.
#[derive(Clone, Debug)]
pub struct Attempt {
    pub manifest: RunManifest,
    pub suite: SuiteResult,
    /// The seal, relative to the run directory.
    pub seal: PathBuf,
}

#[derive(Deserialize, Serialize)]
struct AttemptSeal {
    schema: u16,
    manifest: RunManifest,
    suite: SuiteResult,
    files: Vec<IntegrityFile>,
}

impl RunSession {
    pub fn seal_scenario<F: ScenarioFamily>(
        &mut self,
        scenario: &Scenario<F>,
        result: &ScenarioResult,
    ) -> Result<()> {
        let image = scenario.image();
        if result.scenario != scenario.id()
            || result.image != image
            || result.required_repetitions != scenario.repetitions()
        {
            return Err("attempt result does not match its selected scenario".into());
        }
        let seal_path = self
            .directory
            .join(ATTEMPTS)
            .join(format!("{}.json", scenario.id()));
        if seal_path.try_exists()? {
            return Err("a sealed scenario attempt cannot be replaced".into());
        }
        let mut manifest = self.manifest.clone();
        manifest.state = RunState::Completed;
        manifest.finished_unix_millis = Some(unix_millis());
        manifest.duration_millis = Some(duration_millis(self.started.elapsed()));
        // Other images may be prepared later. They are not this experiment's
        // subject, and their mutable manifest must not invalidate this seal.
        manifest.firmware.retain(|artifact| artifact.image == image);
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
        let output = self.scenario_directory(scenario.id());
        atomic_json(&output.join("scenario.json"), scenario)?;
        atomic_json(&output.join("result.json"), result)?;
        let files = material_files(&self.directory, &manifest, scenario.id(), image)?;
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

/// Read and verify the run's scenario seals: each seal's identity, its
/// suite, and every file of its material hashed again against the seal. A
/// present attempts directory supersedes the enclosing suite, avoiding
/// duplicates; `None` when the run sealed no scenario on its own.
pub(crate) fn completed(run: &Path, parent: &RunManifest) -> Result<Option<Vec<Attempt>>> {
    let directory = run.join(ATTEMPTS);
    if !directory.try_exists()? {
        return Ok(None);
    }
    if !fs::symlink_metadata(&directory)?.file_type().is_dir() {
        return Err("attempts must be a regular directory".into());
    }
    let mut entries = fs::read_dir(directory)?.collect::<std::io::Result<Vec<_>>>()?;
    entries.sort_by_key(fs::DirEntry::file_name);
    let mut attempts = Vec::new();
    for entry in entries {
        let name = entry.file_name();
        let name = name.to_str().ok_or("invalid attempt filename")?;
        // Atomic publication may leave an uncommitted temporary after SIGKILL.
        if name.starts_with('.') && name.contains(".tmp-") {
            continue;
        }
        if name
            .strip_suffix(".json")
            .is_none_or(|id| !validation::valid_id(id))
        {
            return Err(format!("invalid HIL attempt seal name `{name}`").into());
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
            let application = (
                Some(&artifact.application_path),
                Some(artifact.application_size_bytes),
                Some(artifact.application_sha256.as_str()),
            );
            for (path, size, hash) in std::iter::once(application).chain(
                artifact
                    .subjects()
                    .map(|subject| (subject.path, subject.size_bytes, subject.sha256)),
            ) {
                if let Some(path) = path
                    && !files.iter().any(|f| {
                        &f.path == path
                            && Some(f.size_bytes) == size
                            && Some(f.sha256.as_str()) == hash
                    })
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
        let scenario = Header::from_snapshot(&fs::read(output.join("scenario.json"))?)?;
        let recorded: ScenarioResult =
            serde_json::from_slice(&fs::read(output.join("result.json"))?)?;
        if &recorded != result
            || scenario.id != result.scenario
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
        attempts.push(Attempt {
            seal: PathBuf::from(ATTEMPTS).join(name),
            manifest: seal.manifest,
            suite: seal.suite,
        });
    }
    Ok(Some(attempts))
}
