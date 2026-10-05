//! The files of a checkout that can change a HIL observation.
//!
//! A run's source snapshot binds the whole repository, but a firmware image
//! and the host runner read only a part of it: the path packages the firmware
//! workspaces build, the path packages the runner depends on, the workspace
//! and toolchain files that configure both builds, the platform's placement
//! inputs and the scenario catalog. A snapshot that matches the checkout in
//! every such file is current even when unrelated files (documentation,
//! other crates) changed since the run. The closure is taken from the
//! checkout being evaluated, so a package that joined it since the run has
//! no files in the snapshot and makes the snapshot stale.

use super::*;
use oer_repo::closure::{Edges, Features};

/// The host package that executes scenarios and records the observation.
const RUNNER: &str = "oer-hil-runner";

/// Directories read whole besides each chip's HIL agent and platform
/// directories: the scenario catalog and Cargo's configuration.
const DIRECTORIES: [&str; 2] = ["hil/scenarios", ".cargo"];

/// Workspace files that configure every build.
const FILES: [&str; 4] = [
    "Cargo.toml",
    "Cargo.lock",
    "rust-toolchain.toml",
    "rust-toolchain",
];

/// The host packages of a checkout that bear on its runs: the runner's path
/// packages, and the stand operation packages among the HIL packages
/// (`open-radio.hil = "operation"`), which never shape a passed
/// observation and stay out of every closure, so stand work does not stale
/// the evidence.
#[derive(Default)]
pub(super) struct Host {
    pub(super) runner: BTreeSet<PathBuf>,
    pub(super) operation: BTreeSet<PathBuf>,
}

impl Host {
    /// The runner and stand operation packages of the checkout at `root`.
    pub(super) fn of(root: &Path) -> Result<Self> {
        Self::from_model(&oer_repo::Model::load(&oer_repo::Repo::load(root)?)?)
    }

    fn from_model(model: &oer_repo::Model) -> Result<Self> {
        let runner = model.package(RUNNER)?;
        let runner = model
            .closure(&[runner], Edges::Build, None, &Features::All)?
            .into_iter()
            .map(|package| PathBuf::from(&package.directory))
            .collect();
        let mut operation = BTreeSet::new();
        for package in model.packages() {
            if model.classification(package)?.hil == Some(oer_repo::Hil::Operation) {
                operation.insert(PathBuf::from(&package.directory));
            }
        }
        Ok(Self { runner, operation })
    }
}

/// Whether `path` tests or documents a HIL host package rather than shaping
/// an observation.
fn is_test_or_prose(path: &Path) -> bool {
    path.starts_with("hil/host")
        && (path.extension().is_some_and(|extension| extension == "md")
            || path.file_name().is_some_and(|name| name == "tests.rs")
            || path.components().any(|component| {
                matches!(component.as_os_str().to_str(), Some("tests" | "testdata"))
            }))
}

pub(super) struct Closure {
    directories: BTreeSet<PathBuf>,
    /// Single files, besides the directories read whole.
    files: BTreeSet<PathBuf>,
    /// Directories inside those that cannot shape this closure's
    /// observation: the HIL protocol modules a run exchanged no message of.
    excluded: BTreeSet<PathBuf>,
    /// The stand operation packages, never part of a closure.
    operation: BTreeSet<PathBuf>,
}

/// The HIL protocol package and its wire lock.
const PROTOCOL: &str = "hil/protocol";
const PROTOCOL_LOCK: &str = "hil/protocol/messages.lock";

/// The module directories of the HIL protocol a run cannot depend on: every
/// module but those whose messages it exchanged (`used`, message paths whose
/// first segment names the module), `base` and their dependencies as the
/// checkout's lock lists them. The framework outside the module directories
/// and the lock itself stay bound.
fn unused_protocol_modules(lock: &str, used: &[String]) -> Result<BTreeSet<PathBuf>> {
    let mut dependencies = BTreeMap::<&str, Vec<&str>>::new();
    let mut section = "";
    for line in lock.lines().map(str::trim) {
        if line.starts_with('[') {
            section = line;
        } else if section == "[modules]" && !line.is_empty() && !line.starts_with('#') {
            let mut words = line.split_whitespace();
            let module = words.next().ok_or("empty module line")?;
            dependencies.insert(module, words.collect());
        }
    }
    if dependencies.is_empty() {
        return Err(format!("{PROTOCOL_LOCK} lists no modules").into());
    }
    let mut needed = BTreeSet::new();
    let mut pending = vec!["base"];
    for path in used {
        let module = path.split('/').next().unwrap_or_default();
        if !dependencies.contains_key(module) {
            return Err(format!("message {path} names no module of {PROTOCOL_LOCK}").into());
        }
        pending.push(module);
    }
    while let Some(module) = pending.pop() {
        if needed.insert(module) {
            pending.extend(dependencies.get(module).into_iter().flatten().copied());
        }
    }
    Ok(dependencies
        .keys()
        .filter(|module| !needed.contains(*module))
        .map(|module| Path::new(PROTOCOL).join("src").join(module))
        .collect())
}

impl Closure {
    /// The closure of the checkout at `root`: every path package of each
    /// chip's firmware workspaces and the runner's path packages.
    pub(super) fn of(root: &Path) -> Result<Self> {
        let root = root.canonicalize()?;
        let model = oer_repo::Model::load(&oer_repo::Repo::load(&root)?)?;
        let mut directories = DIRECTORIES
            .map(PathBuf::from)
            .into_iter()
            .collect::<BTreeSet<_>>();
        let chips = super::chips::all(&root)?;
        directories.extend(
            chips
                .iter()
                .flat_map(|chip| chip.directories.iter().cloned()),
        );
        let workspaces: BTreeSet<String> = chips
            .iter()
            .flat_map(|chip| &chip.packages)
            .map(|(workspace, _)| workspace.to_string_lossy().into_owned())
            .collect();
        for workspace in &workspaces {
            let members: Vec<_> = model.members(workspace).collect();
            for package in model.closure(&members, Edges::All, None, &Features::All)? {
                directories.insert(PathBuf::from(&package.directory));
            }
        }
        let host = Host::from_model(&model)?;
        directories.extend(host.runner);
        Ok(Self {
            directories,
            files: BTreeSet::new(),
            excluded: BTreeSet::new(),
            operation: host.operation,
        })
    }

    /// The closure of one run, narrower than the checkout's: every file its
    /// firmware image builds read, which each build recorded as
    /// `source-inputs.json` (sources, workspace and Cargo configuration,
    /// policies and the image builder), the files of the scenarios it
    /// executed and the runner's packages (`host`, from [`Host::of`]). A
    /// change to another image class, crate or scenario leaves it current.
    /// `None` when an image of the run has no complete record.
    pub(super) fn of_run(root: &Path, run: &Path, host: &Host) -> Result<Option<Self>> {
        let mut files = BTreeSet::new();
        let mut images = 0;
        for image in fs::read_dir(run.join("firmware"))? {
            let image = image?.path();
            if !image.is_dir() {
                continue;
            }
            let Some(inputs) = SourceInputs::read(&image)? else {
                return Ok(None);
            };
            files.extend(inputs.files);
            images += 1;
        }
        if images == 0 {
            return Ok(None);
        }
        let mut scenarios = BTreeSet::new();
        for scenario in fs::read_dir(run.join("scenarios"))? {
            let scenario = scenario?;
            if scenario.path().is_dir() {
                scenarios.insert(scenario.file_name().to_string_lossy().into_owned());
            }
        }
        files.extend(scenario_files(root, &scenarios)?);
        // A run that lists the messages it exchanged binds only the protocol
        // modules they belong to; without that list it binds all of them.
        let bundle = oer_hil_run_bundle::RunBundle::open(run).map_err(|error| error.to_string())?;
        let excluded = match bundle {
            Some(bundle) => unused_protocol_modules(
                &fs::read_to_string(root.join(PROTOCOL_LOCK))?,
                &bundle.manifest().messages_used,
            )?,
            None => BTreeSet::new(),
        };
        Ok(Some(Self {
            directories: host.runner.clone(),
            files,
            excluded,
            operation: host.operation.clone(),
        }))
    }

    pub(super) fn contains(&self, path: &Path) -> bool {
        if is_test_or_prose(path)
            || self
                .operation
                .iter()
                .any(|package| path.starts_with(package))
            || self
                .excluded
                .iter()
                .any(|directory| path.starts_with(directory))
        {
            return false;
        }
        FILES.iter().any(|file| path == Path::new(file))
            || self.files.contains(path)
            || self
                .directories
                .iter()
                .any(|directory| path.starts_with(directory))
    }

    #[cfg(test)]
    pub(super) fn from_directories(directories: &[&str], operation: &[&str]) -> Self {
        Self {
            directories: directories.iter().map(PathBuf::from).collect(),
            files: BTreeSet::new(),
            excluded: BTreeSet::new(),
            operation: operation.iter().map(PathBuf::from).collect(),
        }
    }
}

/// The `source-inputs.json` schema: every repository file an image build read.
const SOURCE_INPUTS_SCHEMA: u32 = 2;

/// The `source-inputs.json` an image build writes beside its artifacts.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SourceInputs {
    schema: u32,
    pub(super) files: Vec<PathBuf>,
}

impl SourceInputs {
    /// The record in the image directory `image`. `None` when the image has
    /// none (a replay or an image built outside this repository's builder);
    /// a record of another schema is an error, never a weaker binding.
    pub(super) fn read(image: &Path) -> Result<Option<Self>> {
        let path = image.join("source-inputs.json");
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let inputs: Self = serde_json::from_slice(&bytes)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        if inputs.schema != SOURCE_INPUTS_SCHEMA {
            return Err(format!(
                "{} has schema {}; only schema {SOURCE_INPUTS_SCHEMA} is read",
                path.display(),
                inputs.schema
            )
            .into());
        }
        Ok(Some(inputs))
    }
}

/// The catalog files, below `hil/scenarios`, of the scenarios named
/// `scenarios`: each scenario is one file named after it.
fn scenario_files(root: &Path, scenarios: &BTreeSet<String>) -> Result<BTreeSet<PathBuf>> {
    let mut files = BTreeSet::new();
    let mut pending = vec![PathBuf::from("hil/scenarios")];
    while let Some(directory) = pending.pop() {
        let Ok(entries) = fs::read_dir(root.join(&directory)) else {
            continue;
        };
        for entry in entries {
            let entry = entry?;
            let path = directory.join(entry.file_name());
            if entry.file_type()?.is_dir() {
                pending.push(path);
            } else if path
                .extension()
                .is_some_and(|extension| extension == "toml")
                && path
                    .file_stem()
                    .is_some_and(|stem| scenarios.contains(stem.to_string_lossy().as_ref()))
            {
                files.insert(path);
            }
        }
    }
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_build_and_run_inputs_are_in_the_closure() {
        let closure =
            Closure::from_directories(&["crates/hardware/esp32s31/hal", "hil/scenarios"], &[]);
        assert!(closure.contains(Path::new("crates/hardware/esp32s31/hal/src/lib.rs")));
        assert!(closure.contains(Path::new("hil/scenarios/system/boot-smoke.toml")));
        assert!(closure.contains(Path::new("Cargo.lock")));
        assert!(!closure.contains(Path::new("docs/architecture.md")));
        assert!(!closure.contains(Path::new("crates/hardware/esp32s31/hal-extra/src/lib.rs")));
        assert!(!closure.contains(Path::new("tools/blobray/src/main.rs")));
    }

    #[test]
    fn stand_operation_leaves_the_evidence_current_and_observing_code_does_not() {
        let closure = Closure::from_directories(
            &[
                "hil/host/arbiter",
                "hil/host/board",
                "hil/host/lab",
                "hil/host/link",
                "hil/host/run-bundle",
                "hil/host/family/ieee80211",
            ],
            &["hil/host/board"],
        );
        for neutral in [
            "hil/host/board/src/esp_idf.rs",
            "hil/host/link/src/tests.rs",
            "hil/host/family/ieee80211/README.md",
        ] {
            assert!(!closure.contains(Path::new(neutral)), "{neutral}");
        }
        for observing in [
            "hil/host/arbiter/src/spectrum.rs",
            "hil/host/lab/src/recovery.rs",
            "hil/host/link/src/lib.rs",
            "hil/host/run-bundle/src/verify.rs",
            "hil/host/family/ieee80211/src/workload/traffic/rx_traffic.rs",
        ] {
            assert!(closure.contains(Path::new(observing)), "{observing}");
        }
    }

    #[test]
    fn a_run_binds_the_protocol_modules_of_its_messages_and_their_dependencies() {
        let lock = "framing 2\n[modules]\nbase\nbluetooth base system\nnetwork base system wifi\nphy base\nsystem\ntelemetry\nwifi base\n[messages]\nbase/hello 00 topic\n";
        let unused =
            unused_protocol_modules(lock, &["base/hello".into(), "network/session/ready".into()])
                .unwrap();
        assert_eq!(
            unused,
            ["bluetooth", "phy", "telemetry"]
                .iter()
                .map(|module| Path::new(PROTOCOL).join("src").join(module))
                .collect()
        );
        let mut closure = Closure::from_directories(&["hil/protocol"], &[]);
        closure.excluded = unused;
        assert!(
            closure.contains(Path::new("hil/protocol/src/wifi/rx.rs")),
            "a dependency"
        );
        assert!(
            closure.contains(Path::new("hil/protocol/src/framing.rs")),
            "the framework"
        );
        assert!(closure.contains(Path::new("hil/protocol/messages.lock")));
        assert!(!closure.contains(Path::new("hil/protocol/src/phy/fault.rs")));
        assert!(unused_protocol_modules(lock, &["radar/ping".into()]).is_err());
    }

    #[test]
    fn the_protocol_lock_of_this_checkout_parses() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let lock = fs::read_to_string(root.join(PROTOCOL_LOCK)).unwrap();
        let unused = unused_protocol_modules(&lock, &["base/hello".into()]).unwrap();
        for module in &unused {
            assert!(
                root.join(module).is_dir(),
                "{} is no module directory",
                module.display()
            );
        }
        assert!(!unused.contains(&Path::new(PROTOCOL).join("src/base")));
    }

    #[test]
    fn the_runner_closure_follows_path_dependencies_and_roles_name_stand_operation() {
        let root = tempfile::tempdir().unwrap();
        let hil = |name: &str, role: &str, dependencies: &str| {
            format!(
                "[package]\nname = \"{name}\"\n[dependencies]\n{dependencies}\n\
                 [dev-dependencies]\ntest-only = {{ path = \"../test-only\" }}\n\
                 [package.metadata.open-radio]\nlayer = \"hil\"\nplatform = \"host\"\nhil = \"{role}\"\nhost-layer = \"execution\"\n"
            )
        };
        for (directory, text) in [
            (
                "hil/host/runner",
                hil(
                    RUNNER,
                    "orchestration",
                    "core = { path = \"../core\" }\nboard = { path = \"../board\" }\nserde = \"1\"",
                ),
            ),
            ("hil/host/core", hil("core", "observation", "")),
            ("hil/host/board", hil("board", "operation", "")),
            ("hil/host/test-only", hil("test-only", "observation", "")),
            ("tools/other", hil("other", "observation", "")),
        ] {
            fs::create_dir_all(root.path().join(directory)).unwrap();
            fs::write(root.path().join(directory).join("Cargo.toml"), text).unwrap();
        }
        let host = Host::of(root.path()).unwrap();
        assert_eq!(
            host.runner,
            BTreeSet::from([
                PathBuf::from("hil/host/board"),
                PathBuf::from("hil/host/core"),
                PathBuf::from("hil/host/runner")
            ])
        );
        assert_eq!(
            host.operation,
            BTreeSet::from([PathBuf::from("hil/host/board")])
        );
    }
}
