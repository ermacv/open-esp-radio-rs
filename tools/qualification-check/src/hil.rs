//! Independent consumption of immutable HIL run bundles.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Component, Path, PathBuf},
    process::Command,
};

use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::Result;

const HIL_RUN_SCHEMA: u16 = 2;

#[derive(Clone, Debug)]
pub(crate) struct RepositoryState {
    pub(crate) commit: String,
    pub(crate) dirty: bool,
}

impl RepositoryState {
    pub(crate) fn read(root: &Path) -> Result<Self> {
        let commit = git_output(root, &["rev-parse", "HEAD"])?;
        let status = git_output(
            root,
            &["status", "--porcelain=v1", "--untracked-files=normal"],
        )?;
        Ok(Self {
            commit,
            dirty: !status.is_empty(),
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct HilRequirement {
    pub(crate) scenario: String,
    pub(crate) minimum_repetitions: u8,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct ScenarioCatalog {
    repetitions: BTreeMap<String, u8>,
}

impl ScenarioCatalog {
    pub(crate) fn load(root: &Path, catalog: &Path) -> Result<Self> {
        let directory = root.join(catalog);
        if !directory.is_dir() {
            return Err(format!(
                "HIL scenario catalog is not a directory: {}",
                directory.display()
            )
            .into());
        }
        let mut entries = fs::read_dir(&directory)?.collect::<std::io::Result<Vec<_>>>()?;
        entries.sort_by_key(std::fs::DirEntry::file_name);
        let mut repetitions = BTreeMap::new();
        for entry in entries {
            if !entry.file_type()?.is_file()
                || entry.path().extension().and_then(|value| value.to_str()) != Some("toml")
            {
                return Err(format!(
                    "HIL scenario catalog contains a non-TOML entry: {}",
                    entry.path().display()
                )
                .into());
            }
            let document: ScenarioDocument =
                toml_edit::de::from_str(&fs::read_to_string(entry.path())?)?;
            if document.schema != 3
                || document.id
                    != entry
                        .path()
                        .file_stem()
                        .and_then(|value| value.to_str())
                        .unwrap_or_default()
                || !(1..=20).contains(&document.repetitions)
            {
                return Err(format!(
                    "invalid HIL scenario catalog entry: {}",
                    entry.path().display()
                )
                .into());
            }
            if repetitions
                .insert(document.id.clone(), document.repetitions)
                .is_some()
            {
                return Err(format!("duplicate HIL scenario id {}", document.id).into());
            }
        }
        Ok(Self { repetitions })
    }

    pub(crate) fn validate_requirement(&self, requirement: &HilRequirement) -> Result<()> {
        let repetitions = self
            .repetitions
            .get(&requirement.scenario)
            .ok_or_else(|| format!("unknown HIL scenario {}", requirement.scenario))?;
        if requirement.minimum_repetitions > *repetitions {
            return Err(format!(
                "HIL requirement {} needs {} repetitions but its scenario declares {}",
                requirement.scenario, requirement.minimum_repetitions, repetitions
            )
            .into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct HilEvidenceIndex {
    scenarios: BTreeMap<String, Vec<ScenarioEvidence>>,
    summary: HilEvidenceSummary,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct HilEvidenceSummary {
    pub(crate) directories: usize,
    pub(crate) bundles: usize,
    pub(crate) incomplete: usize,
    pub(crate) completed: usize,
    pub(crate) passing: usize,
    pub(crate) current_clean_producer: usize,
    pub(crate) qualifying: usize,
    pub(crate) evaluator_dirty: bool,
}

#[derive(Clone, Debug)]
struct ScenarioEvidence {
    run_id: String,
    repetitions: usize,
}

#[derive(Deserialize)]
struct ScenarioDocument {
    schema: u16,
    id: String,
    #[serde(default = "one_repetition")]
    repetitions: u8,
}

const fn one_repetition() -> u8 {
    1
}

impl HilEvidenceIndex {
    pub(crate) fn load(
        root: &Path,
        runs: &Path,
        target: &str,
        repository: &RepositoryState,
    ) -> Result<Self> {
        let directory = root.join(runs);
        if !directory.try_exists()? {
            return Ok(Self {
                summary: HilEvidenceSummary {
                    evaluator_dirty: repository.dirty,
                    ..HilEvidenceSummary::default()
                },
                ..Self::default()
            });
        }
        if !directory.is_dir() {
            return Err(format!(
                "HIL evidence path is not a directory: {}",
                directory.display()
            )
            .into());
        }
        let mut entries = fs::read_dir(&directory)?.collect::<std::io::Result<Vec<_>>>()?;
        entries.sort_by_key(std::fs::DirEntry::file_name);
        let mut scenarios = BTreeMap::<String, Vec<ScenarioEvidence>>::new();
        let mut summary = HilEvidenceSummary {
            evaluator_dirty: repository.dirty,
            ..HilEvidenceSummary::default()
        };
        for entry in entries {
            if !entry.file_type()?.is_dir() {
                return Err(format!(
                    "HIL runs directory contains a non-directory entry: {}",
                    entry.path().display()
                )
                .into());
            }
            let run_directory = entry.path();
            summary.directories += 1;
            let Some(manifest): Option<RunManifest> =
                read_optional_json(&run_directory.join("manifest.json"))?
            else {
                // HIL producers can create their output directory before the
                // first durable run document is published. Such a directory
                // makes no evidence claim yet, so report it as incomplete and
                // keep evaluating the target. Once a manifest exists its state
                // and immutable-bundle contract remain fail-closed below.
                summary.incomplete += 1;
                continue;
            };
            summary.bundles += 1;
            if manifest.schema != HIL_RUN_SCHEMA {
                return Err(format!(
                    "HIL run {} has unsupported manifest schema {}",
                    run_directory.display(),
                    manifest.schema
                )
                .into());
            }
            if manifest.run_id
                != run_directory
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or_default()
                || manifest.target != target
            {
                return Err(format!(
                    "HIL manifest does not match run directory or configured target: {}",
                    run_directory.display()
                )
                .into());
            }
            // A process may be killed before `RunSession::drop` can turn its
            // live directory into a sealed interrupted bundle. Running
            // directories are mutable execution state, never qualification
            // evidence, so do not require an integrity inventory from them.
            // Completed and interrupted bundles are immutable and must still
            // fail closed if their seal is absent or inconsistent.
            if manifest.state == RunState::Running {
                continue;
            }
            verify_integrity(&run_directory)?;
            if manifest.state != RunState::Completed {
                continue;
            }
            summary.completed += 1;
            let suite: SuiteResult = read_json(&run_directory.join("suite.json"))?;
            validate_suite(&suite, &manifest, &run_directory)?;
            if !valid_sha256(&manifest.repository.workspace_sha256) {
                return Err(format!(
                    "HIL run has an invalid workspace digest: {}",
                    run_directory.display()
                )
                .into());
            }
            if suite.outcome == Outcome::Passed {
                summary.passing += 1;
            }
            let artifact_replays_firmware = manifest
                .firmware
                .iter()
                .any(|artifact| artifact.replayed_from.is_some());
            let plan_replays_firmware =
                read_optional_json::<RunPlanProvenance>(&run_directory.join("plan.json"))?
                    .and_then(|plan| plan.firmware)
                    .is_some_and(|firmware| firmware.source == PlannedFirmwareSource::Replay);
            let replays_firmware = artifact_replays_firmware || plan_replays_firmware;
            let current_clean_producer = !replays_firmware
                && !manifest.repository.dirty
                && manifest.repository.commit == repository.commit;
            if current_clean_producer {
                summary.current_clean_producer += 1;
            }
            if repository.dirty || !current_clean_producer || suite.outcome != Outcome::Passed {
                continue;
            }
            summary.qualifying += 1;
            let mut seen = BTreeSet::new();
            for scenario in suite.scenarios {
                if !seen.insert(scenario.scenario.clone()) {
                    return Err(format!(
                        "HIL run {} repeats scenario {}",
                        manifest.run_id, scenario.scenario
                    )
                    .into());
                }
                scenarios
                    .entry(scenario.scenario)
                    .or_default()
                    .push(ScenarioEvidence {
                        run_id: manifest.run_id.clone(),
                        repetitions: scenario.repetitions.len(),
                    });
            }
        }
        Ok(Self { scenarios, summary })
    }

    pub(crate) fn evidence_for(&self, requirement: &HilRequirement) -> Option<String> {
        self.scenarios
            .get(&requirement.scenario)?
            .iter()
            .filter(|evidence| evidence.repetitions >= usize::from(requirement.minimum_repetitions))
            .max_by(|left, right| left.run_id.cmp(&right.run_id))
            .map(|evidence| {
                format!(
                    "hil:{}/{}:repetitions={}",
                    evidence.run_id, requirement.scenario, evidence.repetitions
                )
            })
    }

    pub(crate) fn summary(&self) -> &HilEvidenceSummary {
        &self.summary
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
enum RunState {
    Running,
    Completed,
    Interrupted,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
enum Outcome {
    Passed,
    Failed,
    Broken,
    Skipped,
    Blocked,
    Interrupted,
}

#[derive(Deserialize)]
struct RepositoryProvenance {
    commit: String,
    dirty: bool,
    workspace_sha256: String,
}

#[derive(Deserialize)]
struct RunManifest {
    schema: u16,
    run_id: String,
    target: String,
    state: RunState,
    started_unix_millis: u64,
    finished_unix_millis: Option<u64>,
    duration_millis: Option<u64>,
    repository: RepositoryProvenance,
    #[serde(default)]
    firmware: Vec<FirmwareArtifactProvenance>,
}

#[derive(Deserialize)]
struct FirmwareArtifactProvenance {
    #[serde(default)]
    replayed_from: Option<serde::de::IgnoredAny>,
}

#[derive(Deserialize)]
struct RunPlanProvenance {
    #[serde(default)]
    firmware: Option<PlannedFirmwareProvenance>,
}

#[derive(Deserialize)]
struct PlannedFirmwareProvenance {
    source: PlannedFirmwareSource,
}

#[derive(Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
enum PlannedFirmwareSource {
    BuildCurrent,
    Replay,
}

#[derive(Deserialize)]
struct SuiteResult {
    schema: u16,
    run_id: String,
    target: String,
    outcome: Outcome,
    started_unix_millis: u64,
    finished_unix_millis: u64,
    duration_millis: u64,
    counts: SuiteCounts,
    scenarios: Vec<ScenarioResult>,
}

#[derive(Deserialize, Eq, PartialEq)]
struct SuiteCounts {
    scenarios: usize,
    passed: usize,
    failed: usize,
    broken: usize,
    skipped: usize,
    blocked: usize,
    interrupted: usize,
}

impl SuiteCounts {
    fn from_scenarios(scenarios: &[ScenarioResult]) -> Self {
        let mut counts = Self {
            scenarios: scenarios.len(),
            passed: 0,
            failed: 0,
            broken: 0,
            skipped: 0,
            blocked: 0,
            interrupted: 0,
        };
        for scenario in scenarios {
            match scenario.outcome {
                Outcome::Passed => counts.passed += 1,
                Outcome::Failed => counts.failed += 1,
                Outcome::Broken => counts.broken += 1,
                Outcome::Skipped => counts.skipped += 1,
                Outcome::Blocked => counts.blocked += 1,
                Outcome::Interrupted => counts.interrupted += 1,
            }
        }
        counts
    }
}

#[derive(Deserialize)]
struct ScenarioResult {
    schema: u16,
    scenario: String,
    outcome: Outcome,
    required_repetitions: u8,
    repetitions: Vec<RepetitionResult>,
    failure: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct RepetitionResult {
    schema: u16,
    repetition: u8,
    outcome: Outcome,
    failure: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct IntegrityIndex {
    schema: u16,
    run_id: String,
    files: Vec<IntegrityFile>,
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd)]
struct IntegrityFile {
    path: PathBuf,
    size_bytes: u64,
    sha256: String,
}

fn validate_suite(suite: &SuiteResult, manifest: &RunManifest, directory: &Path) -> Result<()> {
    if suite.schema != HIL_RUN_SCHEMA
        || suite.run_id != manifest.run_id
        || suite.target != manifest.target
        || suite.started_unix_millis != manifest.started_unix_millis
        || Some(suite.finished_unix_millis) != manifest.finished_unix_millis
        || Some(suite.duration_millis) != manifest.duration_millis
        || suite.counts != SuiteCounts::from_scenarios(&suite.scenarios)
    {
        return Err(format!(
            "HIL suite does not match its manifest or counts: {}",
            directory.display()
        )
        .into());
    }
    let expected_suite = if suite
        .scenarios
        .iter()
        .all(|scenario| scenario.outcome == Outcome::Passed)
    {
        Outcome::Passed
    } else {
        Outcome::Failed
    };
    if suite.outcome != expected_suite {
        return Err(format!(
            "HIL suite outcome is inconsistent with its scenarios: {}",
            directory.display()
        )
        .into());
    }
    let mut seen = BTreeSet::new();
    for scenario in &suite.scenarios {
        if scenario.schema != HIL_RUN_SCHEMA
            || !valid_id(&scenario.scenario)
            || !(1..=20).contains(&scenario.required_repetitions)
            || !seen.insert(&scenario.scenario)
        {
            return Err(format!(
                "invalid or duplicate HIL scenario in {}",
                directory.display()
            )
            .into());
        }
        if scenario.repetitions.is_empty() {
            if scenario.outcome != Outcome::Blocked || scenario.failure.is_none() {
                return Err(format!(
                    "HIL scenario {} has no repetitions without a blocking failure",
                    scenario.scenario
                )
                .into());
            }
            continue;
        }
        if scenario.repetitions.len() != usize::from(scenario.required_repetitions)
            || scenario.outcome
                != aggregate_outcome(
                    scenario
                        .repetitions
                        .iter()
                        .map(|repetition| repetition.outcome),
                )
        {
            return Err(format!(
                "HIL scenario {} has inconsistent repetitions",
                scenario.scenario
            )
            .into());
        }
        for (index, repetition) in scenario.repetitions.iter().enumerate() {
            if repetition.schema != HIL_RUN_SCHEMA
                || usize::from(repetition.repetition) != index + 1
                || (repetition.outcome == Outcome::Passed && repetition.failure.is_some())
                || (matches!(
                    repetition.outcome,
                    Outcome::Failed | Outcome::Broken | Outcome::Blocked | Outcome::Interrupted
                ) && repetition.failure.is_none())
            {
                return Err(format!(
                    "HIL scenario {} has an invalid repetition sequence",
                    scenario.scenario
                )
                .into());
            }
        }
    }
    Ok(())
}

fn aggregate_outcome(outcomes: impl IntoIterator<Item = Outcome>) -> Outcome {
    let observed = outcomes.into_iter().collect::<Vec<_>>();
    if !observed.is_empty() && observed.iter().all(|outcome| *outcome == Outcome::Passed) {
        Outcome::Passed
    } else if observed.contains(&Outcome::Interrupted) {
        Outcome::Interrupted
    } else if observed.contains(&Outcome::Broken) {
        Outcome::Broken
    } else if observed.contains(&Outcome::Failed) {
        Outcome::Failed
    } else if observed.contains(&Outcome::Blocked) {
        Outcome::Blocked
    } else {
        Outcome::Skipped
    }
}

fn verify_integrity(run_directory: &Path) -> Result<()> {
    let path = run_directory.join("integrity.json");
    let index: IntegrityIndex = read_json(&path)?;
    if index.schema != HIL_RUN_SCHEMA
        || index.run_id
            != run_directory
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default()
    {
        return Err(format!("invalid HIL integrity identity: {}", path.display()).into());
    }
    let mut declared = index.files;
    declared.sort();
    let mut unique = BTreeSet::new();
    for file in &declared {
        if !safe_relative(&file.path)
            || file.path == Path::new("integrity.json")
            || !valid_sha256(&file.sha256)
            || !unique.insert(file.path.clone())
        {
            return Err(format!("invalid HIL integrity entry: {}", file.path.display()).into());
        }
        let actual = run_directory.join(&file.path);
        let metadata = fs::symlink_metadata(&actual)?;
        if !metadata.file_type().is_file()
            || metadata.len() != file.size_bytes
            || sha256_file(&actual)? != file.sha256
        {
            return Err(format!("HIL integrity mismatch: {}", actual.display()).into());
        }
    }
    let actual = collect_integrity_files(run_directory)?;
    if declared != actual {
        return Err(format!(
            "HIL run does not match its sealed inventory: {}",
            run_directory.display()
        )
        .into());
    }
    Ok(())
}

fn collect_integrity_files(directory: &Path) -> Result<Vec<IntegrityFile>> {
    fn visit(root: &Path, directory: &Path, output: &mut Vec<IntegrityFile>) -> Result<()> {
        let mut entries = fs::read_dir(directory)?.collect::<std::io::Result<Vec<_>>>()?;
        entries.sort_by_key(std::fs::DirEntry::file_name);
        for entry in entries {
            let file_type = entry.file_type()?;
            if file_type.is_dir() {
                visit(root, &entry.path(), output)?;
            } else if file_type.is_file() {
                let relative = entry.path().strip_prefix(root)?.to_owned();
                if relative == Path::new("integrity.json") {
                    continue;
                }
                output.push(IntegrityFile {
                    path: relative,
                    size_bytes: entry.metadata()?.len(),
                    sha256: sha256_file(&entry.path())?,
                });
            } else {
                return Err(format!(
                    "HIL run contains a symlink or special file: {}",
                    entry.path().display()
                )
                .into());
            }
        }
        Ok(())
    }

    let mut output = Vec::new();
    visit(directory, directory, &mut output)?;
    output.sort();
    Ok(output)
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T> {
    let input = fs::read(path)
        .map_err(|error| format!("cannot read HIL evidence {}: {error}", path.display()))?;
    serde_json::from_slice(&input)
        .map_err(|error| format!("cannot parse HIL evidence {}: {error}", path.display()).into())
}

fn read_optional_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<Option<T>> {
    let input = match fs::read(path) {
        Ok(input) => input,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(format!("cannot read HIL evidence {}: {error}", path.display()).into());
        }
    };
    serde_json::from_slice(&input)
        .map(Some)
        .map_err(|error| format!("cannot parse HIL evidence {}: {error}", path.display()).into())
}

fn safe_relative(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn valid_id(value: &str) -> bool {
    value
        .bytes()
        .next()
        .is_some_and(|byte| byte.is_ascii_lowercase())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && !value.ends_with('-')
        && !value.contains("--")
}

fn sha256_file(path: &Path) -> Result<String> {
    Ok(format!("{:x}", Sha256::digest(fs::read(path)?)))
}

fn git_output(root: &Path, arguments: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(arguments)
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "git {} failed with status {}",
            arguments.join(" "),
            output.status
        )
        .into());
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn seal(run: &Path) {
        let mut names = vec!["manifest.json", "suite.json"];
        if run.join("plan.json").is_file() {
            names.push("plan.json");
        }
        let files = names
            .into_iter()
            .map(|name| {
                let path = run.join(name);
                json!({
                    "path": name,
                    "size_bytes": fs::metadata(&path).unwrap().len(),
                    "sha256": sha256_file(&path).unwrap(),
                })
            })
            .collect::<Vec<_>>();
        fs::write(
            run.join("integrity.json"),
            serde_json::to_vec_pretty(&json!({
                "schema": 2,
                "run_id": "run-1",
                "files": files,
            }))
            .unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn rejects_parent_paths_in_integrity_entries() {
        assert!(!safe_relative(Path::new("../suite.json")));
        assert!(!safe_relative(Path::new("/suite.json")));
        assert!(safe_relative(Path::new("scenarios/smoke/uart.log")));
    }

    #[test]
    fn digest_requires_canonical_lowercase_hex() {
        assert!(valid_sha256(&"ab".repeat(32)));
        assert!(!valid_sha256(&"AB".repeat(32)));
        assert!(!valid_sha256("abc"));
    }

    #[test]
    fn current_sealed_run_qualifies_and_tampering_fails_closed() {
        let root = std::env::temp_dir().join(format!(
            "open-radio-qualification-hil-{}",
            std::process::id()
        ));
        if root.exists() {
            fs::remove_dir_all(&root).unwrap();
        }
        let run = root.join("runs/run-1");
        fs::create_dir_all(&run).unwrap();
        let digest = "00".repeat(32);
        fs::write(
            run.join("manifest.json"),
            serde_json::to_vec_pretty(&json!({
                "schema": 2,
                "run_id": "run-1",
                "target": "esp32s31",
                "state": "completed",
                "started_unix_millis": 100,
                "finished_unix_millis": 200,
                "duration_millis": 100,
                "repository": {
                    "commit": "abc123",
                    "dirty": false,
                    "workspace_sha256": digest,
                }
            }))
            .unwrap(),
        )
        .unwrap();
        let mut suite = json!({
            "schema": 2,
            "run_id": "run-1",
            "target": "esp32s31",
            "outcome": "passed",
            "started_unix_millis": 100,
            "finished_unix_millis": 200,
            "duration_millis": 100,
            "counts": {
                "scenarios": 1,
                "passed": 1,
                "failed": 0,
                "broken": 0,
                "skipped": 0,
                "blocked": 0,
                "interrupted": 0,
            },
            "scenarios": [{
                "schema": 2,
                "scenario": "station-reconnect",
                "outcome": "passed",
                "required_repetitions": 2,
                "repetitions": [
                    {"schema": 2, "repetition": 1, "outcome": "passed", "failure": null},
                    {"schema": 2, "repetition": 2, "outcome": "passed", "failure": null}
                ],
                "failure": null,
            }]
        });
        fs::write(
            run.join("suite.json"),
            serde_json::to_vec_pretty(&suite).unwrap(),
        )
        .unwrap();
        seal(&run);

        let repository = RepositoryState {
            commit: "abc123".to_owned(),
            dirty: false,
        };
        let index =
            HilEvidenceIndex::load(&root, Path::new("runs"), "esp32s31", &repository).unwrap();
        assert!(
            index
                .evidence_for(&HilRequirement {
                    scenario: "station-reconnect".to_owned(),
                    minimum_repetitions: 2,
                })
                .is_some()
        );

        let mut manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(run.join("manifest.json")).unwrap()).unwrap();
        manifest["firmware"] = json!([{
            "replayed_from": {
                "source_run_id": "older-run"
            }
        }]);
        fs::write(
            run.join("manifest.json"),
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .unwrap();
        seal(&run);
        let replayed =
            HilEvidenceIndex::load(&root, Path::new("runs"), "esp32s31", &repository).unwrap();
        assert_eq!(replayed.summary().current_clean_producer, 0);
        assert_eq!(replayed.summary().qualifying, 0);

        manifest["firmware"] = json!([]);
        fs::write(
            run.join("manifest.json"),
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .unwrap();
        fs::write(
            run.join("plan.json"),
            serde_json::to_vec_pretty(&json!({
                "firmware": {
                    "source": "replay",
                    "source_run_id": "older-run"
                }
            }))
            .unwrap(),
        )
        .unwrap();
        seal(&run);
        let planned_replay =
            HilEvidenceIndex::load(&root, Path::new("runs"), "esp32s31", &repository).unwrap();
        assert_eq!(planned_replay.summary().current_clean_producer, 0);
        assert_eq!(planned_replay.summary().qualifying, 0);

        fs::remove_file(run.join("plan.json")).unwrap();
        seal(&run);

        fs::write(run.join("suite.json"), b"{}").unwrap();
        assert!(
            HilEvidenceIndex::load(&root, Path::new("runs"), "esp32s31", &repository,).is_err()
        );

        suite["counts"]["passed"] = json!(0);
        fs::write(
            run.join("suite.json"),
            serde_json::to_vec_pretty(&suite).unwrap(),
        )
        .unwrap();
        seal(&run);
        assert!(
            HilEvidenceIndex::load(&root, Path::new("runs"), "esp32s31", &repository,).is_err()
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unsealed_running_run_is_mutable_state_not_evidence() {
        let root = std::env::temp_dir().join(format!(
            "open-radio-qualification-hil-running-{}",
            std::process::id()
        ));
        if root.exists() {
            fs::remove_dir_all(&root).unwrap();
        }
        let run = root.join("runs/run-1");
        fs::create_dir_all(&run).unwrap();
        fs::write(
            run.join("manifest.json"),
            serde_json::to_vec_pretty(&json!({
                "schema": 2,
                "run_id": "run-1",
                "target": "esp32s31",
                "state": "running",
                "started_unix_millis": 100,
                "finished_unix_millis": null,
                "duration_millis": null,
                "repository": {
                    "commit": "abc123",
                    "dirty": false,
                    "workspace_sha256": "00".repeat(32),
                }
            }))
            .unwrap(),
        )
        .unwrap();

        let repository = RepositoryState {
            commit: "abc123".to_owned(),
            dirty: false,
        };
        let index =
            HilEvidenceIndex::load(&root, Path::new("runs"), "esp32s31", &repository).unwrap();
        assert_eq!(index.summary().directories, 1);
        assert_eq!(index.summary().bundles, 1);
        assert_eq!(index.summary().incomplete, 0);
        assert_eq!(index.summary().completed, 0);
        assert_eq!(index.summary().qualifying, 0);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn manifestless_generated_run_is_incomplete_not_an_error() {
        let root = std::env::temp_dir().join(format!(
            "open-radio-qualification-hil-incomplete-{}",
            std::process::id()
        ));
        if root.exists() {
            fs::remove_dir_all(&root).unwrap();
        }
        let run = root.join("runs/run-1");
        fs::create_dir_all(&run).unwrap();
        fs::write(run.join("result.json"), b"{}\n").unwrap();

        let repository = RepositoryState {
            commit: "abc123".to_owned(),
            dirty: false,
        };
        let index =
            HilEvidenceIndex::load(&root, Path::new("runs"), "esp32s31", &repository).unwrap();
        assert_eq!(index.summary().directories, 1);
        assert_eq!(index.summary().bundles, 0);
        assert_eq!(index.summary().incomplete, 1);
        assert_eq!(index.summary().completed, 0);
        assert_eq!(index.summary().qualifying, 0);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn malformed_existing_manifest_still_fails_closed() {
        let root = std::env::temp_dir().join(format!(
            "open-radio-qualification-hil-malformed-{}",
            std::process::id()
        ));
        if root.exists() {
            fs::remove_dir_all(&root).unwrap();
        }
        let run = root.join("runs/run-1");
        fs::create_dir_all(&run).unwrap();
        fs::write(run.join("manifest.json"), b"not-json\n").unwrap();

        let repository = RepositoryState {
            commit: "abc123".to_owned(),
            dirty: false,
        };
        let error =
            HilEvidenceIndex::load(&root, Path::new("runs"), "esp32s31", &repository).unwrap_err();
        assert!(error.to_string().contains("cannot parse HIL evidence"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unsealed_completed_run_still_fails_closed() {
        let root = std::env::temp_dir().join(format!(
            "open-radio-qualification-hil-unsealed-completed-{}",
            std::process::id()
        ));
        if root.exists() {
            fs::remove_dir_all(&root).unwrap();
        }
        let run = root.join("runs/run-1");
        fs::create_dir_all(&run).unwrap();
        fs::write(
            run.join("manifest.json"),
            serde_json::to_vec_pretty(&json!({
                "schema": 2,
                "run_id": "run-1",
                "target": "esp32s31",
                "state": "completed",
                "started_unix_millis": 100,
                "finished_unix_millis": 200,
                "duration_millis": 100,
                "repository": {
                    "commit": "abc123",
                    "dirty": false,
                    "workspace_sha256": "00".repeat(32),
                }
            }))
            .unwrap(),
        )
        .unwrap();

        let repository = RepositoryState {
            commit: "abc123".to_owned(),
            dirty: false,
        };
        let error =
            HilEvidenceIndex::load(&root, Path::new("runs"), "esp32s31", &repository).unwrap_err();
        assert!(error.to_string().contains("integrity.json"));
        fs::remove_dir_all(root).unwrap();
    }
}
