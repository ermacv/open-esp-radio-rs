//! Independent consumption of immutable HIL run bundles.

mod checks;
mod chips;
mod closure;
mod decision;
mod measurement;
mod observer;
mod procedure;
mod provenance;
pub(crate) mod shard;
mod snapshot;
mod subject;

pub(crate) use decision::{EvidenceDecision, EvidenceStatus, ObservationCounts};

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Component, Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::Result;

// The bundle's documents, their structural validation and the seals are the
// run bundle's own, read through its one reader: a field or variant the
// producer learns is known here at once, never split into two copies.
// Independence comes from verifying the seals and hashing every file again
// at admission, through that shared code, never from a second reader.
use oer_hil_run_bundle_format::RunBundle;
use oer_hil_run_bundle_format::run::FirmwareArtifact;
use oer_hil_run_bundle_format::run::PlannedFirmware;
use oer_hil_run_bundle_format::run::RUN_SCHEMA;
use oer_hil_run_bundle_format::run::RepositoryProvenance;
use oer_hil_run_bundle_format::run::RunManifest;
use oer_hil_run_bundle_format::run::SuiteResult;
use oer_hil_run_bundle_format::run::validation::safe_relative;
use oer_hil_run_bundle_format::run::validation::valid_sha256;
use oer_hil_run_bundle_format::run::validation::validate_suite;

#[derive(Clone, Debug)]
pub(crate) struct RepositoryState {
    pub(crate) commit: String,
    pub(crate) dirty: bool,
}

impl RepositoryState {
    pub(crate) fn read(root: &Path) -> Result<Self> {
        let commit = oer_process::git::text(root, ["rev-parse", "HEAD"])
            .map_err(|error| error.to_string())?;
        let status = oer_process::git::text(
            root,
            ["status", "--porcelain=v1", "--untracked-files=normal"],
        )
        .map_err(|error| error.to_string())?;
        Ok(Self {
            commit,
            dirty: !status.is_empty(),
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct HilRequirement {
    pub(crate) scenario: String,
    pub(crate) checks: Vec<String>,
    pub(crate) minimum_repetitions: u8,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct ScenarioCatalog {
    repetitions: BTreeMap<String, u8>,
    roles: BTreeMap<String, ScenarioRole>,
    checks: BTreeMap<String, BTreeMap<String, checks::Contract>>,
    definitions: BTreeMap<String, serde_json::Value>,
}

impl ScenarioCatalog {
    /// The catalog at `catalog` below the trusted repository `root`,
    /// discovered and validated by the scenario catalog's one reader
    /// ([`oer_hil_scenario_catalog::documents`]); this evaluator keeps each
    /// document as a value and never imports the runner's family types.
    pub(crate) fn load(root: &Path, catalog: &Path) -> Result<Self> {
        if catalog.as_os_str().is_empty()
            || !catalog
                .components()
                .all(|component| matches!(component, Component::Normal(_)))
        {
            return Err("HIL scenario catalog must be a contained relative path".into());
        }
        let documents = oer_hil_scenario_catalog::documents(&root.canonicalize()?.join(catalog))
            .map_err(|error| error.to_string())?;
        let mut repetitions = BTreeMap::new();
        let mut roles = BTreeMap::new();
        let mut definitions = BTreeMap::new();
        for document in documents {
            let id = document.header.id;
            repetitions.insert(id.clone(), document.header.repetitions);
            roles.insert(id.clone(), document.header.role);
            definitions.insert(id, document.value);
        }
        let checks = definitions
            .iter()
            .map(|(id, document)| Ok((id.clone(), checks::contracts(document)?)))
            .collect::<Result<_>>()?;
        Ok(Self {
            repetitions,
            roles,
            checks,
            definitions,
        })
    }

    /// Whether `scenario` is an investigation scenario, which no observation
    /// lets satisfy a qualification program.
    pub(crate) fn investigation(&self, scenario: &str) -> bool {
        self.roles.get(scenario) == Some(&ScenarioRole::Investigation)
    }

    /// Hold every declared role to the programs: a qualification scenario is
    /// one that `referenced` (the scenarios every program of this catalog
    /// requires) names, and every other scenario is an investigation one.
    /// A requirement on an investigation scenario is accepted here and
    /// reported by its decision, which it can never satisfy.
    pub(crate) fn check_roles(&self, referenced: &BTreeSet<String>) -> Result<()> {
        let unreferenced = self
            .roles
            .iter()
            .filter(|(id, role)| **role == ScenarioRole::Qualification && !referenced.contains(*id))
            .map(|(id, _)| id.as_str())
            .collect::<Vec<_>>();
        if !unreferenced.is_empty() {
            return Err(format!(
                "HIL scenarios with role \"qualification\" that no program references: {}; \
                 give them role \"investigation\"",
                unreferenced.join(", ")
            )
            .into());
        }
        Ok(())
    }

    /// The declared reason the current firmware cannot run `scenario`.
    pub(crate) fn unsupported(&self, scenario: &str) -> Option<&str> {
        self.definitions.get(scenario)?.get("unsupported")?.as_str()
    }

    pub(crate) fn validate_requirement(&self, requirement: &HilRequirement) -> Result<()> {
        let mut seen = BTreeSet::new();
        for name in &requirement.checks {
            if !seen.insert(name)
                || !self
                    .checks
                    .get(&requirement.scenario)
                    .is_some_and(|checks| checks.contains_key(name))
            {
                return Err(format!(
                    "HIL requirement {} names duplicate or unsupported check {name}",
                    requirement.scenario
                )
                .into());
            }
        }
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
    pub(crate) observer_configuration_problem: Option<String>,
    pub(crate) directories: usize,
    pub(crate) bundles: usize,
    pub(crate) incomplete: usize,
    pub(crate) completed: usize,
    pub(crate) passing: usize,
    pub(crate) current_source_producer: usize,
    pub(crate) qualifying: usize,
    /// Tracked evidence shards, and those whose sources match the checkout.
    pub(crate) shards: usize,
    pub(crate) current_shards: usize,
    pub(crate) sealed_attempts: usize,
    pub(crate) evaluator_dirty: bool,
    /// Published runs whose bundle or seals fail validation, with the
    /// reason. They contribute no evidence; every other run still counts.
    pub(crate) invalid: Vec<InvalidRun>,
}

/// A published run excluded from evidence because it failed validation.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct InvalidRun {
    pub(crate) run: String,
    pub(crate) reason: String,
}

#[cfg(test)]
impl HilEvidenceIndex {
    /// Why a run store was refused: the load error, or the reason the first
    /// run was excluded from evidence.
    pub(crate) fn rejection(result: Result<Self>) -> Option<String> {
        match result {
            Err(error) => Some(error.to_string()),
            Ok(index) => index.summary.invalid.first().map(|run| run.reason.clone()),
        }
    }
}

/// What one run directory contributes before evidence decisions.
enum LoadedRun {
    /// No manifest yet: the producer has not published the run.
    Unpublished,
    /// Published but not yet completion evidence (still running, or ended
    /// without completing).
    NotEvidence,
    /// Another chip's run: every chip's runs share the store.
    OtherChip,
    /// Validated evidence units and whether they are independent seals.
    Units {
        bundle: Box<RunBundle>,
        units: Vec<Unit>,
        independently_sealed: bool,
    },
}

/// One completion boundary: the manifest and suite a seal fixed, and the
/// seal, relative to its run.
struct Unit {
    manifest: RunManifest,
    suite: SuiteResult,
    seal: PathBuf,
}

/// Validates one run directory's manifest, seals and integrity through the
/// run bundle's reader, every sealed file hashed again; an error means the
/// published bundle cannot be trusted as evidence.
fn load_run(run_directory: &Path, target: &str) -> Result<LoadedRun> {
    let Some(bundle) = RunBundle::open(run_directory).map_err(|error| error.to_string())? else {
        return Ok(LoadedRun::Unpublished);
    };
    let manifest = bundle.manifest();
    if manifest.schema != RUN_SCHEMA {
        return Err(format!("unsupported manifest schema {}", manifest.schema).into());
    }
    if manifest.run_id
        != run_directory
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
    {
        return Err("manifest does not match its run directory".into());
    }
    if manifest.target != target {
        return Ok(LoadedRun::OtherChip);
    }
    bundle.validate().map_err(|error| error.to_string())?;
    let attempts = bundle.attempts().map_err(|error| error.to_string())?;
    let independently_sealed = attempts.is_some();
    let units = if let Some(attempts) = attempts {
        let mut units = Vec::new();
        for attempt in attempts {
            let result = &attempt.suite.scenarios[0];
            // Only a Wi-Fi procedure names its image; other families imply
            // it. An explicit one must be the image the seal fixed.
            let document = bundle
                .scenario_document(&result.scenario)
                .map_err(|error| error.to_string())?;
            if document
                .pointer("/wifi/image")
                .is_some_and(|declared| declared.as_str() != Some(result.image.id()))
            {
                return Err("HIL attempt subject, procedure or result is inconsistent".into());
            }
            units.push(Unit {
                manifest: attempt.manifest,
                suite: attempt.suite,
                seal: attempt.seal,
            });
        }
        units
    } else {
        // Whole-invocation evidence and independently sealed attempts
        // are different completion boundaries in the current format.
        if manifest.state == RunState::Running {
            return Ok(LoadedRun::NotEvidence);
        }
        bundle.integrity().map_err(|error| error.to_string())?;
        if manifest.state != RunState::Completed {
            return Ok(LoadedRun::NotEvidence);
        }
        let suite = bundle
            .suite()
            .map_err(|error| error.to_string())?
            .ok_or_else(|| {
                format!(
                    "completed HIL run has no suite: {}",
                    run_directory.display()
                )
            })?;
        validate_suite(&suite, manifest).map_err(|error| error.to_string())?;
        vec![Unit {
            manifest: manifest.clone(),
            suite,
            seal: PathBuf::from(oer_hil_run_bundle_format::run::INTEGRITY),
        }]
    };
    if units
        .iter()
        .any(|unit| !valid_sha256(&unit.manifest.repository.workspace_sha256))
    {
        return Err("invalid workspace digest".into());
    }
    Ok(LoadedRun::Units {
        bundle: Box::new(bundle),
        units,
        independently_sealed,
    })
}

#[derive(Clone, Debug)]
struct ScenarioEvidence {
    run_id: String,
    started_unix_millis: u64,
    outcome: Outcome,
    repetition_outcomes: Vec<Outcome>,
    exclusions: Vec<decision::Exclusion>,
    repetitions: usize,
    measurements: Vec<Vec<serde_json::Value>>,
    completion_seal: Option<CompletionSeal>,
    subject: Option<subject::ObservationSubject>,
    failure: Option<serde_json::Value>,
    repetition_failures: Vec<Option<serde_json::Value>>,
    run_directory: Option<PathBuf>,
    /// The executed scenario document of an observation recorded in a
    /// tracked shard, which has no run directory to read it from.
    procedure_document: Option<serde_json::Value>,
    /// An observation from a tracked shard whose recorded sources, including
    /// the observer's, match the checkout.
    source_bound: bool,
    /// Its build binds its sources in every respect but one: the checkout's
    /// tree changed since its snapshot.
    stale_snapshot: bool,
}

impl ScenarioEvidence {
    fn applicable(&self) -> bool {
        self.exclusions.is_empty()
    }
    fn observation_id(&self, scenario: &str) -> Option<String> {
        let seal = self.completion_seal.as_ref()?;
        let mut digest = Sha256::new();
        digest.update(b"oer-hil-observation-v1\0");
        for part in [
            &self.run_id,
            scenario,
            &seal.path.to_string_lossy(),
            &seal.sha256,
        ] {
            digest.update(part.as_bytes());
            digest.update([0]);
        }
        Some(format!("{:x}", digest.finalize()))
    }
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, Deserialize)]
struct CompletionSeal {
    path: PathBuf,
    sha256: String,
}

/// What a scenario is for; a scenario is a qualification scenario because a
/// program references it, and [`ScenarioCatalog::check_roles`] holds the
/// declared role to the programs.
pub(crate) use oer_hil_scenario_catalog::Role as ScenarioRole;

impl HilEvidenceIndex {
    #[cfg(test)]
    pub(crate) fn synthetic(entries: &[(&str, usize)]) -> Self {
        Self {
            scenarios: entries
                .iter()
                .map(|(scenario, repetitions)| {
                    (
                        (*scenario).to_owned(),
                        vec![ScenarioEvidence {
                            run_id: format!("synthetic-{scenario}"),
                            started_unix_millis: 0,
                            outcome: Outcome::Passed,
                            completion_seal: None,
                            subject: None,
                            failure: None,
                            repetition_failures: vec![None; *repetitions],
                            run_directory: None,
                            repetition_outcomes: vec![Outcome::Passed; *repetitions],
                            exclusions: Vec::new(),
                            repetitions: *repetitions,
                            measurements: vec![Vec::new(); *repetitions],
                            procedure_document: None,
                            source_bound: false,
                            stale_snapshot: false,
                        }],
                    )
                })
                .collect(),
            summary: HilEvidenceSummary {
                qualifying: entries.len(),
                ..HilEvidenceSummary::default()
            },
        }
    }

    /// Run bundles below `runs`, and the tracked shards below `evidence`
    /// whose sources match the checkout.
    pub(crate) fn load(
        root: &Path,
        runs: &Path,
        evidence: &Path,
        target: &str,
        repository: &RepositoryState,
    ) -> Result<Self> {
        Self::load_selected(root, runs, evidence, target, repository, None)
    }

    /// [`Self::load`] reading only the run bundles named in `only`, when set.
    pub(crate) fn load_selected(
        root: &Path,
        runs: &Path,
        evidence: &Path,
        target: &str,
        repository: &RepositoryState,
        only: Option<&BTreeSet<String>>,
    ) -> Result<Self> {
        let mut index = Self::load_runs(root, runs, target, repository, only)?;
        for (shard, current) in shard::load(root, evidence, target)? {
            index.summary.shards += 1;
            if !current {
                continue;
            }
            index.summary.current_shards += 1;
            let observation = shard.observation();
            let observations = index
                .scenarios
                .entry(shard.scenario().to_owned())
                .or_default();
            if observations.iter().any(|o| o.run_id == observation.run_id) {
                continue;
            }
            observations.push(observation);
        }
        Ok(index)
    }

    fn load_runs(
        root: &Path,
        runs: &Path,
        target: &str,
        repository: &RepositoryState,
        only: Option<&BTreeSet<String>>,
    ) -> Result<Self> {
        let current_observer = observer::Current::load(root);
        // The checkout's run directory, which `cargo hil` links to the store
        // shared by every checkout of this user. The evaluator reads only
        // this path: a temporary or foreign root never pulls in that store.
        let mut directories = vec![root.join(runs)];
        directories.retain(|directory| directory.exists());
        if directories.is_empty() {
            return Ok(Self {
                summary: HilEvidenceSummary {
                    evaluator_dirty: repository.dirty,
                    ..HilEvidenceSummary::default()
                },
                ..Self::default()
            });
        }
        let mut entries = Vec::new();
        let mut names = BTreeSet::new();
        for directory in &directories {
            if !directory.is_dir() {
                return Err(format!(
                    "HIL evidence path is not a directory: {}",
                    directory.display()
                )
                .into());
            }
            for entry in fs::read_dir(directory)? {
                let entry = entry?;
                if only.is_some_and(|only| {
                    entry
                        .file_name()
                        .to_str()
                        .is_none_or(|name| !only.contains(name))
                }) {
                    continue;
                }
                if names.insert(entry.file_name()) {
                    entries.push(entry);
                }
            }
        }
        entries.sort_by_key(std::fs::DirEntry::file_name);
        let mut scenarios = BTreeMap::<String, Vec<ScenarioEvidence>>::new();
        let mut summary = HilEvidenceSummary {
            observer_configuration_problem: current_observer.problem.clone(),
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
            // One untrustworthy bundle is reported and excluded; it cannot
            // count as evidence, and the other runs stay evaluable.
            let (bundle, units) = match load_run(&run_directory, target) {
                Ok(LoadedRun::Unpublished) => {
                    // HIL producers can create their output directory before
                    // the first durable run document is published; such a
                    // directory makes no evidence claim yet.
                    summary.incomplete += 1;
                    continue;
                }
                Ok(LoadedRun::NotEvidence) => {
                    summary.bundles += 1;
                    continue;
                }
                Ok(LoadedRun::OtherChip) => {
                    summary.directories -= 1;
                    continue;
                }
                Ok(LoadedRun::Units {
                    bundle,
                    units,
                    independently_sealed,
                }) => {
                    summary.bundles += 1;
                    if independently_sealed {
                        summary.sealed_attempts += units.len();
                    }
                    (bundle, units)
                }
                Err(error) => {
                    summary.bundles += 1;
                    summary.invalid.push(InvalidRun {
                        run: entry.file_name().to_string_lossy().into_owned(),
                        reason: error.to_string(),
                    });
                    continue;
                }
            };
            let mut current_producer = false;
            let mut qualifying = false;
            // Counters describe enclosing invocations, not individual seals.
            if bundle.manifest().state == RunState::Completed {
                summary.completed += 1;
                if !units.is_empty()
                    && units
                        .iter()
                        .all(|unit| unit.suite.outcome == Outcome::Passed)
                {
                    summary.passing += 1;
                }
            }
            let plan_replays_firmware = matches!(
                bundle
                    .plan()
                    .map_err(|error| error.to_string())?
                    .and_then(|plan| plan.firmware),
                Some(PlannedFirmware::Replay { .. })
            );
            for Unit {
                manifest,
                suite,
                seal,
            } in units
            {
                let artifact_replays_firmware = manifest
                    .firmware
                    .iter()
                    .any(|artifact| artifact.replayed_from.is_some());
                let replays_firmware = artifact_replays_firmware || plan_replays_firmware;
                // Exact snapshot bytes establish identity independently of Git
                // bookkeeping.
                let binding = provenance::current_sources(root, &run_directory, &manifest)?;
                let mut exclusions = Vec::new();
                if replays_firmware {
                    exclusions.push(decision::Exclusion::ReplaySubjectNotBound);
                }
                // A run built from another clean commit still observed this
                // checkout when none of the files it depends on changed since.
                let commit_current = binding == provenance::Binding::Commit
                    && !manifest.repository.dirty
                    && snapshot::unchanged_since(
                        root,
                        &run_directory,
                        &manifest.repository.commit,
                    )?;
                if binding != provenance::Binding::Snapshot {
                    if manifest.repository.dirty {
                        exclusions.push(decision::Exclusion::ProducerDirty);
                    }
                    if manifest.repository.commit != repository.commit && !commit_current {
                        exclusions.push(decision::Exclusion::DifferentCommit);
                    }
                }
                if exclusions.is_empty() && binding == provenance::Binding::Unavailable {
                    exclusions.push(decision::Exclusion::SourceBindingNotEstablished);
                }
                if exclusions.is_empty() && binding == provenance::Binding::StaleSnapshot {
                    exclusions.push(decision::Exclusion::SnapshotDiffersFromCheckout);
                }
                let stale_snapshot = binding == provenance::Binding::StaleSnapshot;
                if exclusions.is_empty() {
                    current_producer = true;
                }
                if repository.dirty && binding != provenance::Binding::Snapshot && !commit_current {
                    exclusions.push(decision::Exclusion::EvaluatorDirty);
                }
                let mut seen = BTreeSet::new();
                for scenario in suite.scenarios {
                    let completion_seal = Some(CompletionSeal {
                        sha256: crate::digests()
                            .sha256_file(&run_directory.join(&seal))
                            .map_err(|error| error.to_string())?,
                        path: seal.clone(),
                    });
                    let subject = Some(subject::ObservationSubject::load(
                        &run_directory,
                        &manifest,
                        &scenario.scenario,
                    )?);
                    if !seen.insert(scenario.scenario.clone()) {
                        return Err(format!(
                            "HIL run {} repeats scenario {}",
                            manifest.run_id, scenario.scenario
                        )
                        .into());
                    }
                    let scenario_id = scenario.scenario.clone();
                    scenarios
                        .entry(scenario.scenario)
                        .or_default()
                        .push(ScenarioEvidence {
                            run_id: manifest.run_id.clone(),
                            completion_seal,
                            subject,
                            run_directory: Some(run_directory.clone()),
                            failure: scenario
                                .failure
                                .as_ref()
                                .map(serde_json::to_value)
                                .transpose()?,
                            repetition_failures: scenario
                                .repetitions
                                .iter()
                                .map(|r| r.failure.as_ref().map(serde_json::to_value).transpose())
                                .collect::<std::result::Result<_, _>>()?,
                            started_unix_millis: suite.started_unix_millis,
                            outcome: scenario.outcome,
                            repetition_outcomes: scenario
                                .repetitions
                                .iter()
                                .map(|r| r.outcome)
                                .collect(),
                            exclusions: exclusions.clone(),
                            repetitions: scenario.repetitions.len(),
                            measurements: scenario
                                .repetitions
                                .iter()
                                .map(|repetition| {
                                    repetition
                                        .measurements
                                        .iter()
                                        .map(serde_json::to_value)
                                        .collect::<std::result::Result<Vec<_>, _>>()
                                })
                                .collect::<std::result::Result<_, _>>()?,
                            procedure_document: None,
                            source_bound: false,
                            stale_snapshot,
                        });
                    let observation = scenarios.get_mut(&scenario_id).unwrap().last_mut().unwrap();
                    if !current_observer.available() {
                        observation
                            .exclusions
                            .push(decision::Exclusion::CurrentObserverConfigurationUnavailable);
                    } else {
                        match observer::assess(root, &current_observer, observation, None)? {
                            observer::Compatibility::Compatible => {}
                            observer::Compatibility::GraphNotProjectable => observation
                                .exclusions
                                .push(decision::Exclusion::ObserverGraphNotProjectable),
                            observer::Compatibility::IdentityDiffers => observation
                                .exclusions
                                .push(decision::Exclusion::ObserverIdentityNotEstablished),
                        }
                    }
                    if observation.exclusions.is_empty() && observation.outcome == Outcome::Passed {
                        qualifying = true;
                    }
                }
            }
            summary.current_source_producer += usize::from(current_producer);
            summary.qualifying += usize::from(qualifying);
        }
        Ok(Self { scenarios, summary })
    }

    pub(crate) fn evidence_for(
        &self,
        requirement: &HilRequirement,
        catalog: &ScenarioCatalog,
    ) -> Option<String> {
        self.decision_for(requirement, catalog).evidence
    }

    pub(crate) fn summary(&self) -> &HilEvidenceSummary {
        &self.summary
    }
}

// The run vocabulary is the runner's own, from the shared schema.
use oer_hil_schema::run::{Outcome, RunState};

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T> {
    oer_hil_run_bundle_format::run::validation::read_json(path)
        .map_err(|error| error.to_string().into())
}

fn read_optional_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<Option<T>> {
    oer_hil_run_bundle_format::run::validation::read_optional_json(path)
        .map_err(|error| error.to_string().into())
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod catalog_tests;

#[cfg(test)]
mod attempt_tests;

#[cfg(test)]
mod provenance_tests;
