//! Tracked summaries of qualifying HIL observations, bound to sources.
//!
//! A run bundle stays in ignored output and qualifies only for the checkout
//! it was produced from. When an observation qualifies on the current
//! checkout, `cargo qualification hil-evidence` records it as a shard: its
//! outcome, repetitions and measurements, completion seal, subject and
//! executed scenario document, plus the digests of every source the observed
//! firmware and observer were built from. A shard supports qualification
//! while those sources are unchanged, whatever else the repository changed.
use super::*;
use crate::model::scenario_evidence::{SourceDigest, digest_directory};
use serde::Serialize;
use serde_json::Value;

/// Shard format.
const SCHEMA: u16 = 1;
/// Extension of a shard file; its stem is the scenario identifier.
const EXTENSION: &str = "json";

/// Files, beyond the package directories, that the firmware and observer
/// builds read: lockfiles, the stack policy, the partition table and the
/// observer's input registry.
const BUILD_FILES: &[&str] = &[
    "Cargo.lock",
    "hil/targets/esp32s31/Cargo.lock",
    "hil/targets/esp32s31/stack.toml",
    "platform/esp32s31/Cargo.lock",
    "platform/esp32s31/partitions/applications.csv",
    "hil/schema/observer-inputs.json",
    "rust-toolchain.toml",
];
/// Workspaces whose path packages compose the firmware image.
const FIRMWARE_WORKSPACES: &[&str] = &[
    "hil/targets/esp32s31/Cargo.toml",
    "platform/esp32s31/Cargo.toml",
];

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub(super) struct Shard {
    schema: u16,
    target: String,
    scenario: String,
    run_id: String,
    started_unix_millis: u64,
    outcome: Outcome,
    failure: Option<Value>,
    repetitions: Vec<Repetition>,
    completion_seal: CompletionSeal,
    subject: subject::ObservationSubject,
    /// The executed scenario document.
    procedure: Option<Value>,
    /// Digests of the package directories and build files of the observed
    /// firmware and observer, ascending by path.
    sources: Vec<SourceDigest>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct Repetition {
    outcome: Outcome,
    failure: Option<Value>,
    measurements: Vec<Value>,
}

fn digest(root: &Path, path: &Path) -> Result<String> {
    let full = root.join(path);
    if full.is_file() {
        sha256_file(&full)
    } else {
        digest_directory(root, path).map_err(|e| e.to_string().into())
    }
}

fn current(root: &Path, sources: &[SourceDigest]) -> bool {
    !sources.is_empty()
        && sources
            .iter()
            .all(|s| digest(root, &s.path).is_ok_and(|d| d == s.sha256))
}

impl Shard {
    /// The observation this shard records.
    pub(super) fn observation(&self) -> ScenarioEvidence {
        ScenarioEvidence {
            run_id: self.run_id.clone(),
            started_unix_millis: self.started_unix_millis,
            outcome: self.outcome,
            repetition_outcomes: self.repetitions.iter().map(|r| r.outcome).collect(),
            exclusions: Vec::new(),
            repetitions: self.repetitions.len(),
            measurements: self
                .repetitions
                .iter()
                .map(|r| r.measurements.clone())
                .collect(),
            completion_seal: Some(self.completion_seal.clone()),
            subject: Some(self.subject.clone()),
            failure: self.failure.clone(),
            repetition_failures: self.repetitions.iter().map(|r| r.failure.clone()).collect(),
            run_directory: None,
            review: None,
            resolution: None,
            procedure_document: self.procedure.clone(),
            source_bound: true,
        }
    }

    pub(super) fn scenario(&self) -> &str {
        &self.scenario
    }
}

/// Every shard of `directory` with whether its sources are current. A
/// missing directory holds none.
pub(super) fn load(root: &Path, directory: &Path, target: &str) -> Result<Vec<(Shard, bool)>> {
    if directory.as_os_str().is_empty() {
        return Ok(vec![]);
    }
    let entries = match fs::read_dir(root.join(directory)) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(error) => return Err(error.into()),
    };
    let mut paths = entries
        .map(|entry| Ok(entry?.path()))
        .collect::<Result<Vec<_>>>()?;
    paths.sort();
    let mut shards = vec![];
    for path in paths {
        if path.extension().and_then(|e| e.to_str()) != Some(EXTENSION) {
            return Err(format!("unexpected HIL evidence file {}", path.display()).into());
        }
        let shard: Shard = read_json(&path)?;
        if shard.schema != SCHEMA
            || shard.target != target
            || path.file_stem().and_then(|s| s.to_str()) != Some(shard.scenario.as_str())
            || !valid_sha256(&shard.completion_seal.sha256)
            || shard.outcome != Outcome::Passed
            || shard.sources.windows(2).any(|w| w[0].path >= w[1].path)
            || shard
                .sources
                .iter()
                .any(|s| !safe_relative(&s.path) || !valid_sha256(&s.sha256))
        {
            return Err(format!("invalid HIL evidence shard {}", path.display()).into());
        }
        let current = current(root, &shard.sources);
        shards.push((shard, current));
    }
    Ok(shards)
}

/// The sources the observed builds read: the path packages of the firmware
/// workspaces, the observer's manifest directories and the build files.
pub(crate) fn tracked_sources(root: &Path, observers: &[&Value]) -> Result<Vec<PathBuf>> {
    let mut paths = BTreeSet::new();
    for workspace in FIRMWARE_WORKSPACES {
        let output = Command::new("cargo")
            .current_dir(root)
            .args(["metadata", "--format-version", "1", "--offline", "--locked"])
            .args(["--manifest-path", workspace])
            .output()?;
        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).into_owned().into());
        }
        let metadata: Value = serde_json::from_slice(&output.stdout)?;
        let root_path = root.canonicalize()?;
        for package in metadata["packages"]
            .as_array()
            .ok_or("cargo metadata packages")?
        {
            if !package["source"].is_null() {
                continue;
            }
            let manifest = Path::new(package["manifest_path"].as_str().ok_or("manifest path")?);
            let directory = manifest
                .parent()
                .ok_or("manifest directory")?
                .canonicalize()?;
            paths.insert(directory.strip_prefix(&root_path)?.to_path_buf());
        }
    }
    paths.extend(observer_directories(observers)?);
    paths.extend(BUILD_FILES.iter().map(PathBuf::from));
    paths.remove(Path::new(""));
    Ok(paths.into_iter().collect())
}

/// The manifest directories of the observers' builds.
fn observer_directories(observers: &[&Value]) -> Result<BTreeSet<PathBuf>> {
    let mut paths = BTreeSet::new();
    for observer in observers {
        let manifests = observer["build"]["resolved"]["manifests"]
            .as_object()
            .ok_or("observer manifests missing")?;
        for manifest in manifests.keys() {
            let path = Path::new(manifest);
            if !safe_relative(path) {
                return Err("unsafe observer manifest path".into());
            }
            paths.insert(path.parent().map(Path::to_path_buf).unwrap_or_default());
        }
    }
    Ok(paths)
}

/// The sources of one observation: the repository files its firmware was
/// built from, which the run bundle records per image as
/// `firmware/<image>/source-inputs.json`, its own observer's manifest
/// directories and the build files. `None` when a firmware image records no
/// inputs, such as a replay or an older bundle.
fn observation_sources(observation: &ScenarioEvidence) -> Result<Option<Vec<PathBuf>>> {
    let (Some(run), Some(subject)) = (&observation.run_directory, &observation.subject) else {
        return Ok(None);
    };
    recorded_sources(run, &subject.firmware, subject.observer.as_ref())
}

fn recorded_sources(
    run: &Path,
    images: &[subject::FirmwareIdentity],
    observer: Option<&Value>,
) -> Result<Option<Vec<PathBuf>>> {
    if images.is_empty() {
        return Ok(None);
    }
    let mut paths = BTreeSet::new();
    for firmware in images {
        let Some(image) = firmware.image.as_deref().filter(|_| !firmware.replayed) else {
            return Ok(None);
        };
        let Some(inputs) = read_optional_json::<Value>(
            &run.join("firmware").join(image).join("source-inputs.json"),
        )?
        else {
            return Ok(None);
        };
        if inputs["schema"] != 1 {
            return Err(format!("{image}: unsupported source-inputs schema").into());
        }
        for file in inputs["files"]
            .as_array()
            .ok_or("source-inputs lists no files")?
        {
            let path = PathBuf::from(file.as_str().ok_or("source input is not a path")?);
            if !safe_relative(&path) {
                return Err(format!("unsafe source input {}", path.display()).into());
            }
            paths.insert(path);
        }
    }
    let observers = observer.into_iter().collect::<Vec<_>>();
    paths.extend(observer_directories(&observers)?);
    paths.extend(BUILD_FILES.iter().map(PathBuf::from));
    paths.remove(Path::new(""));
    Ok(Some(paths.into_iter().collect()))
}

/// Record the latest qualifying observation of every scenario of `index` in
/// `directory`, bound to the digests of the sources its bundle records its
/// firmware was built from, else to `sources`. Returns the scenarios
/// recorded.
pub(crate) fn distill(
    root: &Path,
    index: &HilEvidenceIndex,
    directory: &Path,
    target: &str,
    sources: &[PathBuf],
) -> Result<Vec<String>> {
    let mut known = BTreeMap::<PathBuf, String>::new();
    let mut digests = |paths: &[PathBuf]| -> Result<Vec<SourceDigest>> {
        paths
            .iter()
            .map(|path| {
                let sha256 = match known.get(path) {
                    Some(sha256) => sha256.clone(),
                    None => {
                        let sha256 = digest(root, path)?;
                        known.insert(path.clone(), sha256.clone());
                        sha256
                    }
                };
                Ok(SourceDigest {
                    sha256,
                    path: path.clone(),
                })
            })
            .collect()
    };
    fs::create_dir_all(root.join(directory))?;
    let mut recorded = vec![];
    for (scenario, observations) in &index.scenarios {
        let Some(observation) = observations
            .iter()
            .filter(|o| {
                !o.source_bound
                    && o.exclusions.is_empty()
                    && o.outcome == Outcome::Passed
                    && o.run_directory.is_some()
            })
            .max_by_key(|o| o.started_unix_millis)
        else {
            continue;
        };
        let (Some(seal), Some(subject), Some(run)) = (
            &observation.completion_seal,
            &observation.subject,
            &observation.run_directory,
        ) else {
            continue;
        };
        let procedure = subject
            .procedure
            .as_ref()
            .map(|p| read_json::<Value>(&run.join(&p.path)))
            .transpose()?;
        let shard = Shard {
            schema: SCHEMA,
            target: target.to_owned(),
            scenario: scenario.clone(),
            run_id: observation.run_id.clone(),
            started_unix_millis: observation.started_unix_millis,
            outcome: observation.outcome,
            failure: observation.failure.clone(),
            repetitions: observation
                .repetition_outcomes
                .iter()
                .zip(&observation.repetition_failures)
                .zip(&observation.measurements)
                .map(|((outcome, failure), measurements)| Repetition {
                    outcome: *outcome,
                    failure: failure.clone(),
                    measurements: measurements.clone(),
                })
                .collect(),
            completion_seal: seal.clone(),
            subject: subject.clone(),
            procedure,
            sources: match observation_sources(observation)? {
                Some(recorded) => digests(&recorded)?,
                None => digests(sources)?,
            },
        };
        let mut bytes = serde_json::to_vec_pretty(&shard)?;
        bytes.push(b'\n');
        fs::write(
            root.join(directory).join(format!("{scenario}.{EXTENSION}")),
            bytes,
        )?;
        recorded.push(scenario.clone());
    }
    Ok(recorded)
}

/// Observer proofs of the qualifying run observations of `index`.
pub(crate) fn observers(index: &HilEvidenceIndex) -> Vec<&Value> {
    index
        .scenarios
        .values()
        .flatten()
        .filter(|o| !o.source_bound && o.exclusions.is_empty() && o.outcome == Outcome::Passed)
        .filter_map(|o| o.subject.as_ref()?.observer.as_ref())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn image(name: &str, replayed: bool) -> subject::FirmwareIdentity {
        subject::FirmwareIdentity {
            image: Some(name.to_owned()),
            build_id: None,
            application: None,
            build_provenance: None,
            replayed,
        }
    }

    #[test]
    fn a_shard_binds_the_files_its_images_were_built_from() {
        let run = std::env::temp_dir().join(format!("oer-shard-sources-{}", std::process::id()));
        let inputs = |image: &str, files: Value| {
            fs::create_dir_all(run.join("firmware").join(image)).unwrap();
            fs::write(
                run.join("firmware").join(image).join("source-inputs.json"),
                serde_json::to_vec(&json!({"schema": 1, "files": files})).unwrap(),
            )
            .unwrap();
        };
        inputs(
            "correctness",
            json!(["crates/radio/src/lib.rs", "platform/linker/link.x"]),
        );
        let observer = json!({"build": {"resolved": {"manifests": {
            "hil/host/runner/Cargo.toml": {}
        }}}});
        let sources = recorded_sources(&run, &[image("correctness", false)], Some(&observer))
            .unwrap()
            .unwrap();
        for expected in [
            "crates/radio/src/lib.rs",
            "platform/linker/link.x",
            "hil/host/runner",
            "rust-toolchain.toml",
            "Cargo.lock",
        ] {
            assert!(
                sources.contains(&PathBuf::from(expected)),
                "{expected}: {sources:?}"
            );
        }
        assert!(sources.windows(2).all(|w| w[0] < w[1]), "sorted and unique");
        // A replay or an image without recorded inputs keeps the broad set.
        assert!(
            recorded_sources(&run, &[image("correctness", true)], None)
                .unwrap()
                .is_none()
        );
        assert!(
            recorded_sources(&run, &[image("performance", false)], None)
                .unwrap()
                .is_none()
        );
        assert!(recorded_sources(&run, &[], None).unwrap().is_none());
        inputs("performance", json!(["../outside.rs"]));
        assert!(recorded_sources(&run, &[image("performance", false)], None).is_err());
        fs::remove_dir_all(run).unwrap();
    }
}
