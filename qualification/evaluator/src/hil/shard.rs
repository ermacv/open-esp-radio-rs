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
use oer_hil_run_bundle_format::build::BuildParameters;
use oer_hil_run_bundle_format::build::BuildProvenance;
use oer_hil_run_bundle_format::observer::store as observer_store;
use oer_vendor_evidence_shard::{SourceDigest, digest_directory};
use serde::Serialize;
use serde_json::Value;

/// Shard format.
/// Schema 2 records the observer build by digest in `observers/`.
const SCHEMA: u16 = 2;
/// Extension of a shard file; its stem is the scenario identifier.
const EXTENSION: &str = "json";

/// Files the observer's own build reads beside its manifest directories: the
/// root lock and toolchain it is built with and the inputs it validates.
const OBSERVER_FILES: &[&str] = &[
    "Cargo.lock",
    "rust-toolchain.toml",
    "hil/schema/observer-inputs.json",
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
        Ok(crate::digests()
            .sha256_file(&full)
            .map_err(|error| error.to_string())?)
    } else {
        digest_directory(root, path).map_err(|e| e.to_string().into())
    }
}

/// The repository files a run's source snapshot captured, with their bytes.
struct Snapshot(BTreeMap<PathBuf, Vec<u8>>);

impl Snapshot {
    /// The snapshot `run` captured, its files read from the store's source
    /// objects, each checked against its digest; `None` for a bundle recorded
    /// before source snapshots.
    fn load(run: &Path) -> Result<Option<Self>> {
        let manifest = run.join("source/snapshot/manifest.json");
        if !manifest.is_file() {
            return Ok(None);
        }
        let manifest: oer_hil_schema::snapshot::Manifest = read_json(&manifest)?;
        let objects = oer_hil_schema::snapshot::objects_of_run(run)?;
        let mut files = BTreeMap::new();
        for source in manifest.sources.iter().filter(|s| s.name == "repository") {
            for file in &source.files {
                let stored = oer_hil_schema::snapshot::object(&objects, &file.sha256);
                let bytes = fs::read(&stored).map_err(|error| {
                    format!("source object {} is missing: {error}", stored.display())
                })?;
                if oer_durable::sha256_bytes(&bytes) != file.sha256 {
                    return Err(format!(
                        "source object {} does not hash to its name",
                        stored.display()
                    )
                    .into());
                }
                files.insert(file.path.clone(), bytes);
            }
        }
        Ok(Some(Self(files)))
    }

    /// The digest [`digest`] computes in a checkout holding exactly these
    /// files: a file's SHA-256, or [`digest_directory`] over a directory.
    fn digest(&self, path: &Path) -> Result<String> {
        use sha2::{Digest as _, Sha256};
        if let Some(bytes) = self.0.get(path) {
            return Ok(oer_durable::sha256_bytes(bytes));
        }
        let mut hash = Sha256::new();
        let mut any = false;
        for (file, bytes) in self.0.range(path.to_path_buf()..) {
            let Ok(relative) = file.strip_prefix(path) else {
                break;
            };
            let skipped_directory = relative
                .parent()
                .into_iter()
                .flat_map(Path::components)
                .any(|component| {
                    let name = component.as_os_str().to_string_lossy();
                    name == "target" || name.starts_with('.')
                });
            if skipped_directory || file.to_string_lossy().ends_with(".md") {
                continue;
            }
            any = true;
            let name = file.to_string_lossy();
            hash.update((name.len() as u64).to_le_bytes());
            hash.update(name.as_bytes());
            hash.update((bytes.len() as u64).to_le_bytes());
            hash.update(bytes);
        }
        if !any {
            return Err(format!("source {} is not in the run's snapshot", path.display()).into());
        }
        Ok(format!("{:x}", hash.finalize()))
    }
}

/// Whether every source `shard` binds still has its recorded digest. A
/// source that no longer exists is an error, not a stale shard: the shard
/// is recorded again or deleted.
fn current(root: &Path, shard: &Path, sources: &[SourceDigest]) -> Result<bool> {
    if sources.is_empty() {
        return Ok(false);
    }
    for source in sources {
        if fs::symlink_metadata(root.join(&source.path)).is_err() {
            return Err(format!(
                "HIL evidence shard {} binds {}, which does not exist; record the scenario \
                 again or delete the shard",
                shard.display(),
                source.path.display()
            )
            .into());
        }
        if digest(root, &source.path)? != source.sha256 {
            return Ok(false);
        }
    }
    Ok(true)
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
            procedure_document: self.procedure.clone(),
            source_bound: true,
            stale_snapshot: false,
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
        if path.file_name() == Some(observer_store::DIRECTORY.as_ref()) && path.is_dir() {
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) != Some(EXTENSION) {
            return Err(format!("unexpected HIL evidence file {}", path.display()).into());
        }
        let mut shard: Shard = read_json(&path)?;
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
        shard.subject.resolve_observer(&root.join(directory))?;
        let current = current(root, &path, &shard.sources)?;
        shards.push((shard, current));
    }
    Ok(shards)
}

/// The sources the observed builds read: the path packages of the firmware
/// workspaces, the manifest directories of the observers of `index`'s
/// qualifying run observations and the build files.
pub(crate) fn tracked_sources(root: &Path, index: &HilEvidenceIndex) -> Result<Vec<PathBuf>> {
    let mut paths = BTreeSet::new();
    let chips = super::chips::all(root)?;
    let model = oer_repo::Model::load(&oer_repo::Repo::load(root)?)?;
    let workspaces = chips
        .iter()
        .flat_map(|chip| chip.packages.iter().map(|(workspace, _)| workspace))
        .collect::<BTreeSet<_>>();
    for workspace in workspaces {
        let workspace = workspace.to_string_lossy();
        let members: Vec<_> = model.members(&workspace).collect();
        for package in model.closure(
            &members,
            oer_repo::closure::Edges::All,
            None,
            &oer_repo::closure::Features::All,
        )? {
            paths.insert(PathBuf::from(&package.directory));
        }
    }
    paths.extend(observer_directories(&observers(index))?);
    paths.extend(OBSERVER_FILES.iter().map(PathBuf::from));
    paths.extend(chips.into_iter().flat_map(|chip| chip.build_files));
    paths.remove(Path::new(""));
    Ok(paths.into_iter().collect())
}

/// The manifest directories of the observers' builds.
fn observer_directories(observers: &[&subject::ObserverReference]) -> Result<BTreeSet<PathBuf>> {
    use std::{
        collections::HashMap,
        sync::{Mutex, OnceLock},
    };
    // A build is named by the digest of its bytes, so its directories are
    // remembered by that name: each build is read once, not once per
    // observation, and only the directories are kept.
    type Directories = HashMap<(String, String), BTreeSet<PathBuf>>;
    static DIRECTORIES: OnceLock<Mutex<Directories>> = OnceLock::new();
    let mut paths = BTreeSet::new();
    for observer in observers {
        let identity = observer.identity();
        let known = DIRECTORIES
            .get_or_init(Default::default)
            .lock()
            .map_err(|_| "observer directory cache poisoned")?
            .get(&identity)
            .cloned();
        let directories = match known {
            Some(directories) => directories,
            None => {
                let directories = manifest_directories(&observer.proof()?)?;
                DIRECTORIES
                    .get_or_init(Default::default)
                    .lock()
                    .map_err(|_| "observer directory cache poisoned")?
                    .insert(identity, directories.clone());
                directories
            }
        };
        paths.extend(directories);
    }
    Ok(paths)
}

/// The manifest directories of `proof`'s build.
fn manifest_directories(proof: &Value) -> Result<BTreeSet<PathBuf>> {
    let mut paths = BTreeSet::new();
    let manifests = proof["build"]["resolved"]["manifests"]
        .as_object()
        .ok_or("observer manifests missing")?;
    for manifest in manifests.keys() {
        let path = Path::new(manifest);
        if !safe_relative(path) {
            return Err("unsafe observer manifest path".into());
        }
        paths.insert(path.parent().map(Path::to_path_buf).unwrap_or_default());
    }
    Ok(paths)
}

/// The sources of one observation: the repository files its firmware was
/// built from, which the run bundle records per image as
/// `firmware/<image>/source-inputs.json`, its own observer's manifest
/// directories, the build files and the Cargo configuration the builds read.
/// `None` when a firmware image records no inputs, such as a replay or an
/// older bundle, or when its list lacks a package the repository model's
/// closure of the image (its target and features) independently finds.
///
/// The digests of these sources are taken from the current tree when the
/// shard is recorded. That is sound only because `distill` records
/// observations that qualify on the current checkout; keep that condition.
fn observation_sources(
    root: &Path,
    observation: &ScenarioEvidence,
) -> Result<Option<Vec<PathBuf>>> {
    let (Some(run), Some(subject)) = (&observation.run_directory, &observation.subject) else {
        return Ok(None);
    };
    recorded_sources(
        run,
        &subject.firmware,
        subject.observer.as_ref(),
        &|provenance| image_packages(root, &provenance.parameters),
    )
}

/// The inherited compiler flags an observation's firmware was built with, as
/// its build provenance records them. Such an image depends on the builder's
/// environment, which no source binding covers, so it is never recorded.
fn inherited_flags(observation: &ScenarioEvidence) -> Result<Option<&'static str>> {
    let (Some(run), Some(subject)) = (&observation.run_directory, &observation.subject) else {
        return Ok(None);
    };
    images_inherited_flags(run, &subject.firmware)
}

fn images_inherited_flags(
    run: &Path,
    images: &[subject::FirmwareIdentity],
) -> Result<Option<&'static str>> {
    for image in images.iter().filter_map(|f| f.image.as_deref()) {
        let path = run
            .join("firmware")
            .join(image)
            .join("build-provenance.json");
        let Some(provenance) = read_optional_json::<BuildProvenance>(&path)? else {
            continue;
        };
        let environment = &provenance.environment;
        if environment.inherited_rustflags.is_some() {
            return Ok(Some("RUSTFLAGS"));
        }
        if environment.inherited_encoded_rustflags.is_some() {
            return Ok(Some("CARGO_ENCODED_RUSTFLAGS"));
        }
    }
    Ok(None)
}

/// Repository package directories of an image's runtime and bootstrap, with
/// the runtime features its build provenance records: the path packages
/// their normal and build dependencies reach for the image's target, as the
/// repository model resolves the features (the HIL agent without its
/// default features, the bootstrap with them).
fn image_packages(root: &Path, parameters: &BuildParameters) -> Result<Option<BTreeSet<PathBuf>>> {
    let (features, target) = (
        parameters.runtime_features.as_str(),
        parameters.target.as_str(),
    );
    let root = root.canonicalize()?;
    // The image's chip is the one whose firmware compiles for its target.
    let Some(chip) = super::chips::all(&root)?
        .into_iter()
        .find(|chip| chip.rust_target == target)
    else {
        return Ok(None);
    };
    let model = oer_repo::Model::load(&oer_repo::Repo::load(&root)?)?;
    let triple = oer_repo::closure::Target::new(target)?;
    let mut packages = BTreeSet::new();
    for (index, (workspace, name)) in chip.packages.iter().enumerate() {
        let workspace = workspace.to_string_lossy();
        let package = model
            .members(&workspace)
            .find(|package| package.name == *name)
            .ok_or_else(|| format!("{workspace} has no package {name}"))?;
        // The runtime features apply to the HIL agent, listed first.
        let selection = if index == 0 {
            oer_repo::closure::Features::Resolved {
                default: false,
                features: features
                    .split(',')
                    .filter(|feature| !feature.is_empty())
                    .map(str::to_owned)
                    .collect(),
            }
        } else {
            oer_repo::closure::Features::Resolved {
                default: true,
                features: Vec::new(),
            }
        };
        for package in model.closure(
            &[package],
            oer_repo::closure::Edges::Build,
            Some(&triple),
            &selection,
        )? {
            packages.insert(PathBuf::from(&package.directory));
        }
    }
    Ok(Some(packages))
}

fn recorded_sources(
    run: &Path,
    images: &[subject::FirmwareIdentity],
    observer: Option<&subject::ObserverReference>,
    packages: &dyn Fn(&BuildProvenance) -> Result<Option<BTreeSet<PathBuf>>>,
) -> Result<Option<Vec<PathBuf>>> {
    if images.is_empty() {
        return Ok(None);
    }
    let mut paths = BTreeSet::new();
    for firmware in images {
        let Some(image) = firmware.image.as_deref().filter(|_| !firmware.replayed) else {
            return Ok(None);
        };
        let directory = run.join("firmware").join(image);
        let Some(inputs) = super::closure::SourceInputs::read(&directory)? else {
            return Ok(None);
        };
        let mut files = BTreeSet::new();
        for path in inputs.files {
            if !safe_relative(&path) {
                return Err(format!("unsafe source input {}", path.display()).into());
            }
            files.insert(path);
        }
        // A list that silently lost a package would make a stale shard look
        // current: every package the image's closure holds must be listed.
        let Some(provenance) =
            read_optional_json::<BuildProvenance>(&directory.join("build-provenance.json"))?
        else {
            return Ok(None);
        };
        let Some(expected) = packages(&provenance)? else {
            return Ok(None);
        };
        if expected
            .iter()
            .any(|package| !files.contains(&package.join("Cargo.toml")))
        {
            return Ok(None);
        }
        paths.extend(files);
    }
    let observers = observer.into_iter().collect::<Vec<_>>();
    paths.extend(observer_directories(&observers)?);
    paths.extend(OBSERVER_FILES.iter().map(PathBuf::from));
    paths.remove(Path::new(""));
    Ok(Some(paths.into_iter().collect()))
}

/// Record the latest qualifying observation of every scenario of `index` in
/// `directory`, bound to the digests of the sources its bundle records its
/// firmware was built from, else to `sources`. With `runs`, only observations
/// of those runs are recorded, so only the shards of scenarios they observed
/// change. A run that archived its source snapshot binds the digests of the
/// sources as that snapshot holds them, so one run records the same bytes
/// from any checkout. Returns the scenarios recorded.
pub(crate) fn distill(
    root: &Path,
    index: &HilEvidenceIndex,
    directory: &Path,
    target: &str,
    sources: &[PathBuf],
    runs: Option<&BTreeSet<String>>,
) -> Result<Vec<String>> {
    let mut known = BTreeMap::<PathBuf, String>::new();
    let mut checkout_digests = |paths: &[PathBuf]| -> Result<Vec<SourceDigest>> {
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
    // The observer builds stored beside the shards so far: each is read and
    // stored once, however many shards name it.
    let mut stored = BTreeSet::new();
    for (scenario, observations) in &index.scenarios {
        let mut qualifying = Vec::new();
        for o in observations {
            if o.source_bound
                || o.outcome != Outcome::Passed
                || o.run_directory.is_none()
                || !runs.is_none_or(|runs| runs.contains(&o.run_id))
            {
                continue;
            }
            // A requested run is recorded by the rule that later judges its
            // shard current: every source the shard binds matches the run's
            // snapshot, whatever else the tree changed since.
            let recordable = o.exclusions.is_empty()
                || (runs.is_some()
                    && o.stale_snapshot
                    && o.exclusions
                        .iter()
                        .all(decision::Exclusion::is_tree_binding)
                    && differing_sources(root, o, sources)?.is_some_and(|d| d.is_empty()));
            if recordable {
                qualifying.push(o);
            }
        }
        qualifying.sort_by_key(|o| std::cmp::Reverse(o.started_unix_millis));
        let mut chosen = None;
        for observation in qualifying {
            match inherited_flags(observation)? {
                Some(flags) => eprintln!(
                    "hil-evidence: {scenario}: run {} is not recorded; its firmware was built \
                     with inherited {flags}, which no source binding covers",
                    observation.run_id
                ),
                None => {
                    chosen = Some(observation);
                    break;
                }
            }
        }
        let Some(observation) = chosen else {
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
            sources: {
                let paths = observation_sources(root, observation)?;
                let paths = paths.as_deref().unwrap_or(sources);
                match Snapshot::load(run)? {
                    Some(snapshot) => paths
                        .iter()
                        .map(|path| {
                            Ok(SourceDigest {
                                sha256: snapshot.digest(path)?,
                                path: path.clone(),
                            })
                        })
                        .collect::<Result<Vec<_>>>()?,
                    None => checkout_digests(paths)?,
                }
            },
        };
        if let Some(observer) = &subject.observer
            && stored.insert(observer.identity())
        {
            observer_store::store(&root.join(directory), &observer.proof()?["build"])
                .map_err(|error| error.to_string())?;
        }
        let mut bytes = serde_json::to_vec_pretty(&shard)?;
        bytes.push(b'\n');
        fs::write(
            root.join(directory).join(format!("{scenario}.{EXTENSION}")),
            bytes,
        )?;
        recorded.push(scenario.clone());
    }
    collect_observers(&root.join(directory))?;
    Ok(recorded)
}

/// What recording did with one scenario of one requested run, and why.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum RunVerdict {
    /// Its observation is now the scenario's shard.
    Recorded,
    /// A newer qualifying run of the scenario was recorded instead.
    NewerRunRecorded { run: String },
    /// Only passed observations are recorded.
    NotPassed { outcome: Outcome },
    /// Its firmware was built with inherited compiler flags.
    InheritedFlags { flags: &'static str },
    /// The observation does not qualify on this checkout. For a source
    /// exclusion, `differing` names the observation's recorded sources whose
    /// checkout bytes differ from the run's snapshot; `None` without a
    /// snapshot to compare.
    Excluded {
        exclusion: decision::Exclusion,
        differing: Option<Vec<PathBuf>>,
    },
    /// The run is not in the store, or observed no scenario.
    NotFound,
}

impl RunVerdict {
    /// The verdict's kebab-case identifier.
    pub(crate) fn id(&self) -> &'static str {
        match self {
            Self::Recorded => "recorded",
            Self::NewerRunRecorded { .. } => "newer-run-recorded",
            Self::NotPassed { .. } => "not-passed",
            Self::InheritedFlags { .. } => "inherited-flags",
            Self::Excluded { exclusion, .. } => exclusion.id(),
            Self::NotFound => "not-found",
        }
    }

    /// The verdict's detail, empty when it has none.
    pub(crate) fn detail(&self) -> String {
        match self {
            Self::NewerRunRecorded { run } => format!("recorded run {run}"),
            Self::NotPassed { outcome } => format!("outcome {}", outcome.id()),
            Self::InheritedFlags { flags } => format!("built with inherited {flags}"),
            Self::Excluded {
                differing: Some(differing),
                ..
            } if differing.is_empty() => String::from(
                "its recorded sources match the checkout; files outside them changed since the run",
            ),
            Self::Excluded {
                differing: Some(differing),
                ..
            } => format!(
                "run inputs differ from the checkout in: {}",
                differing
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            _ => String::new(),
        }
    }
}

/// Explain, for every scenario each of `runs` observed, what recording did
/// with it; `recorded` are the scenarios [`distill`] wrote.
pub(crate) fn explain(
    root: &Path,
    index: &HilEvidenceIndex,
    runs: &BTreeSet<String>,
    recorded: &[String],
    directory: &Path,
    sources: &[PathBuf],
) -> Result<Vec<(String, String, RunVerdict)>> {
    let mut verdicts = Vec::new();
    for run in runs {
        let mut observed = false;
        for (scenario, observations) in &index.scenarios {
            for observation in observations.iter().filter(|o| &o.run_id == run) {
                observed = true;
                let shard_run = recorded
                    .contains(scenario)
                    .then(|| {
                        read_json::<Value>(
                            &root.join(directory).join(format!("{scenario}.{EXTENSION}")),
                        )
                    })
                    .transpose()?
                    .and_then(|shard| shard["run-id"].as_str().map(str::to_owned));
                let verdict = if observation.outcome != Outcome::Passed {
                    RunVerdict::NotPassed {
                        outcome: observation.outcome,
                    }
                } else if shard_run.as_deref() == Some(run.as_str()) {
                    RunVerdict::Recorded
                } else if let Some(exclusion) = observation.exclusions.first() {
                    RunVerdict::Excluded {
                        exclusion: exclusion.clone(),
                        differing: differing_sources(root, observation, sources)?,
                    }
                } else if let Some(flags) = inherited_flags(observation)? {
                    RunVerdict::InheritedFlags { flags }
                } else if let Some(id) = shard_run {
                    RunVerdict::NewerRunRecorded { run: id }
                } else {
                    RunVerdict::NotFound
                };
                verdicts.push((run.clone(), scenario.clone(), verdict));
            }
        }
        if !observed {
            verdicts.push((run.clone(), String::new(), RunVerdict::NotFound));
        }
    }
    Ok(verdicts)
}

/// The observation's recorded sources whose checkout bytes differ from its
/// run's snapshot, at most [`DIFFERING_SHOWN`]; `None` without a snapshot.
fn differing_sources(
    root: &Path,
    observation: &ScenarioEvidence,
    sources: &[PathBuf],
) -> Result<Option<Vec<PathBuf>>> {
    let Some(run) = observation.run_directory.as_ref() else {
        return Ok(None);
    };
    // The same sources a shard of this observation would bind.
    let paths = observation_sources(root, observation)?.unwrap_or_else(|| sources.to_vec());
    let Some(snapshot) = Snapshot::load(run)? else {
        return Ok(None);
    };
    let mut differing = Vec::new();
    for path in paths {
        let checkout = if root.join(&path).exists() {
            Some(digest(root, &path)?)
        } else {
            None
        };
        if checkout.as_deref() != snapshot.digest(&path).ok().as_deref() {
            differing.push(path);
            if differing.len() == DIFFERING_SHOWN {
                break;
            }
        }
    }
    Ok(Some(differing))
}

/// Differing sources named per observation.
const DIFFERING_SHOWN: usize = 10;

/// Remove the observer builds of `directory` that none of its shards names,
/// so replacing or removing a shard leaves no orphaned build behind.
fn collect_observers(directory: &Path) -> Result<()> {
    let mut named = BTreeSet::new();
    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()) == Some(EXTENSION) {
            let shard: Value = read_json(&path)?;
            if let Some(digest) = shard["subject"]["observer"]["build_sha256"].as_str() {
                named.insert(digest.to_owned());
            }
        }
    }
    observer_store::collect_garbage(directory, &named, std::time::Duration::ZERO)
        .map_err(|error| error.to_string())?;
    Ok(())
}

/// Observer proofs of the qualifying run observations of `index`.
fn observers(index: &HilEvidenceIndex) -> Vec<&subject::ObserverReference> {
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
    use oer_hil_run_bundle_format::build::BuildEnvironment;
    use oer_hil_run_bundle_format::build::BuildReproducibility;
    use serde_json::json;

    /// A build provenance of the correctness image for `target` with
    /// `features`, built under `environment`.
    fn provenance(target: &str, features: &str, environment: BuildEnvironment) -> BuildProvenance {
        BuildProvenance {
            schema: oer_hil_run_bundle_format::build::BUILD_PROVENANCE_SCHEMA,
            build_id: String::from("build"),
            build_type: String::from("test"),
            parameters: BuildParameters {
                image: oer_hil_schema::image::ImageClass::Correctness,
                network: None,
                runtime_profile: String::from("profile"),
                target: target.to_owned(),
                runtime_features: features.to_owned(),
                layout_seed: None,
                features: Default::default(),
            },
            sources: Vec::new(),
            files: Vec::new(),
            environment,
            subjects: Vec::new(),
            source_reconstructable: false,
            reproducibility: BuildReproducibility::Unverified,
        }
    }

    fn environment(rustflags: Option<&str>, encoded: Option<&str>) -> BuildEnvironment {
        BuildEnvironment {
            tools: Vec::new(),
            inherited_rustflags: rustflags.map(str::to_owned),
            inherited_encoded_rustflags: encoded.map(str::to_owned),
            cargo_incremental: String::from("0"),
            source_date_epoch: None,
        }
    }

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
        let root = std::env::temp_dir().join(format!("oer-shard-root-{}", std::process::id()));
        fs::create_dir_all(root.join(".cargo")).unwrap();
        fs::write(root.join(".cargo/config.toml"), "").unwrap();
        let run = root.join("run");
        let inputs = |image: &str, files: Value| {
            let directory = run.join("firmware").join(image);
            fs::create_dir_all(&directory).unwrap();
            fs::write(
                directory.join("source-inputs.json"),
                serde_json::to_vec(&json!({"schema": 2, "files": files})).unwrap(),
            )
            .unwrap();
            fs::write(
                directory.join("build-provenance.json"),
                serde_json::to_vec(&provenance("t", "", environment(None, None))).unwrap(),
            )
            .unwrap();
        };
        inputs(
            "correctness",
            json!([
                "crates/radio/Cargo.toml",
                "crates/radio/src/lib.rs",
                "platform/linker/link.x",
                "hil/targets/chip-a/Cargo.toml",
                ".cargo/config.toml"
            ]),
        );
        let build = json!({"resolved": {"manifests": {
            "hil/host/runner/Cargo.toml": {}
        }}});
        let observer = subject::ObserverReference::resolve(
            json!({
                "schema": observer_store::REFERENCED,
                "executable_sha256": "aa".repeat(32),
                "build_sha256": observer_store::store(&root, &build).unwrap(),
            }),
            &root,
        )
        .unwrap();
        let radio = |_: &BuildProvenance| Ok(Some(BTreeSet::from([PathBuf::from("crates/radio")])));
        let sources = recorded_sources(
            &run,
            &[image("correctness", false)],
            Some(&observer),
            &radio,
        )
        .unwrap()
        .unwrap();
        for expected in [
            "crates/radio/src/lib.rs",
            "platform/linker/link.x",
            "hil/host/runner",
            "rust-toolchain.toml",
            "Cargo.lock",
            "hil/targets/chip-a/Cargo.toml",
            ".cargo/config.toml",
        ] {
            assert!(
                sources.contains(&PathBuf::from(expected)),
                "{expected}: {sources:?}"
            );
        }
        assert!(sources.windows(2).all(|w| w[0] < w[1]), "sorted and unique");
        // A package the image closure holds but the list lacks: the broad binding.
        let more = |_: &BuildProvenance| {
            Ok(Some(BTreeSet::from([
                PathBuf::from("crates/radio"),
                PathBuf::from("crates/wifi"),
            ])))
        };
        let recorded =
            |images: &[subject::FirmwareIdentity],
             packages: &dyn Fn(&BuildProvenance) -> Result<Option<BTreeSet<PathBuf>>>| {
                recorded_sources(&run, images, None, packages).unwrap()
            };
        assert!(recorded(&[image("correctness", false)], &more).is_none());
        // A replay or an image without recorded inputs keeps the broad set.
        assert!(recorded(&[image("correctness", true)], &radio).is_none());
        assert!(recorded(&[image("performance", false)], &radio).is_none());
        assert!(recorded(&[], &radio).is_none());
        inputs("performance", json!(["../outside.rs"]));
        assert!(recorded_sources(&run, &[image("performance", false)], None, &radio).is_err());
        // A record of another schema is an error, not a weaker binding.
        fs::write(
            run.join("firmware/correctness/source-inputs.json"),
            serde_json::to_vec(&json!({"schema": 1, "files": ["crates/radio/src/lib.rs"]}))
                .unwrap(),
        )
        .unwrap();
        assert!(
            recorded_sources(&run, &[image("correctness", false)], None, &radio)
                .unwrap_err()
                .to_string()
                .contains("only schema 2")
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn the_model_finds_the_packages_of_one_image() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let image = provenance(
            "riscv32imafc-unknown-none-elf",
            "bluetooth-hil,phy-rx-hot-sram",
            environment(None, None),
        );
        let packages = image_packages(&root, &image.parameters).unwrap().unwrap();
        // The chip of that target: its agent, its Bluetooth driver and its
        // staged boot's bootstrap.
        let chip = oer_repo::chips::profile::Profile::all(&root)
            .unwrap()
            .into_iter()
            .find(|profile| profile.rust_target == "riscv32imafc-unknown-none-elf")
            .unwrap()
            .id;
        for expected in [
            format!("hil/targets/{chip}/agent"),
            format!("crates/hardware/{chip}/driver/bluetooth"),
            format!("platform/{chip}/bootstrap"),
        ] {
            assert!(
                packages.contains(Path::new(&expected)),
                "{expected}: {packages:?}"
            );
        }
        assert!(
            !packages
                .iter()
                .any(|package| package.starts_with("crates/protocols/ieee80211")),
            "{packages:?}"
        );
        // A target no chip compiles for names no packages.
        let foreign = provenance("x86_64-unknown-linux-gnu", "", environment(None, None));
        assert!(
            image_packages(&root, &foreign.parameters)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn firmware_built_with_inherited_flags_is_never_recorded() {
        let run = std::env::temp_dir().join(format!("oer-shard-flags-{}", std::process::id()));
        let write = |environment: BuildEnvironment| {
            fs::create_dir_all(run.join("firmware/correctness")).unwrap();
            fs::write(
                run.join("firmware/correctness/build-provenance.json"),
                serde_json::to_vec(&provenance("t", "", environment)).unwrap(),
            )
            .unwrap();
        };
        let images = [image("correctness", false)];
        write(environment(None, None));
        assert_eq!(images_inherited_flags(&run, &images).unwrap(), None);
        write(environment(Some("-Copt-level=0"), None));
        assert_eq!(
            images_inherited_flags(&run, &images).unwrap(),
            Some("RUSTFLAGS")
        );
        write(environment(None, Some("-g")));
        assert_eq!(
            images_inherited_flags(&run, &images).unwrap(),
            Some("CARGO_ENCODED_RUSTFLAGS")
        );
        fs::remove_dir_all(run).unwrap();
    }
}
