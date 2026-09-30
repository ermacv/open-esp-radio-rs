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
use serde_json::Value;
use std::process::Command;

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

/// Runner files that only operate the stand and never shape a passed
/// observation, left out of every closure so that stand work does not stale
/// the evidence: the arbiter's leases, queue and board registry, the flash
/// transactions (a wrong image fails the run's own image check), and the
/// post-mortem, recovery, USB-event and profile reports of failed
/// repetitions. The arbiter's spectrum claims decide the air a run shares
/// and stay in. Tests and prose of runner packages are left out too.
const STAND_OPERATION: [&str; 6] = [
    "hil/host/arbiter",
    "hil/host/board",
    "hil/host/stand/src/post_mortem.rs",
    "hil/host/stand/src/recovery.rs",
    "hil/host/stand/src/usb_events.rs",
    "hil/host/execution/src/profile.rs",
];

/// Files below [`STAND_OPERATION`] that do shape an observation.
const OBSERVING: [&str; 1] = ["hil/host/arbiter/src/spectrum.rs"];

/// Whether `path` operates the stand or tests or documents a package,
/// rather than shaping an observation.
fn is_stand_operation(path: &Path) -> bool {
    if OBSERVING.iter().any(|file| path == Path::new(file)) {
        return false;
    }
    STAND_OPERATION
        .iter()
        .any(|prefix| path.starts_with(prefix))
        || (path.starts_with("hil/host")
            && (path.extension().is_some_and(|extension| extension == "md")
                || path.file_name().is_some_and(|name| name == "tests.rs")
                || path.components().any(|component| {
                    matches!(component.as_os_str().to_str(), Some("tests" | "testdata"))
                })))
}

pub(super) struct Closure {
    directories: BTreeSet<PathBuf>,
    /// Single files, besides the directories read whole.
    files: BTreeSet<PathBuf>,
    /// Directories inside those that cannot shape this closure's
    /// observation: the HIL protocol modules a run exchanged no message of.
    excluded: BTreeSet<PathBuf>,
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
    /// The closure of the checkout at `root`, from Cargo's locked metadata.
    pub(super) fn of(root: &Path) -> Result<Self> {
        let root = root.canonicalize()?;
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
        for (workspace, _) in chips.iter().flat_map(|chip| &chip.packages) {
            let metadata = metadata(&root, &root.join(workspace))?;
            for package in metadata["packages"].as_array().into_iter().flatten() {
                if package["source"].is_null()
                    && let Some(directory) = package_directory(&root, package)
                {
                    directories.insert(directory);
                }
            }
        }
        directories.extend(runner_directories(&root)?);
        Ok(Self {
            directories,
            files: BTreeSet::new(),
            excluded: BTreeSet::new(),
        })
    }

    /// The closure of one run, narrower than the checkout's: every file its
    /// firmware image builds read, which each build recorded as
    /// `source-inputs.json` (sources, workspace and Cargo configuration,
    /// policies and the image builder), the files of the scenarios it
    /// executed and the runner's packages (`runner`, from
    /// [`runner_directories`]). A change to another image class, crate or
    /// scenario leaves it current. `None` when an image of the run has no
    /// complete record.
    pub(super) fn of_run(
        root: &Path,
        run: &Path,
        runner: &BTreeSet<PathBuf>,
    ) -> Result<Option<Self>> {
        let mut files = BTreeSet::new();
        let mut images = 0;
        for image in fs::read_dir(run.join("firmware"))? {
            let image = image?.path();
            if !image.is_dir() {
                continue;
            }
            let Ok(inputs) = fs::read_to_string(image.join("source-inputs.json")) else {
                return Ok(None);
            };
            let inputs: SourceInputs = serde_json::from_str(&inputs)?;
            if inputs.schema != COMPLETE_INPUTS {
                return Ok(None);
            }
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
        let manifest = match fs::read(run.join("manifest.json")) {
            Ok(bytes) => serde_json::from_slice::<Value>(&bytes)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Value::Null,
            Err(error) => return Err(error.into()),
        };
        let excluded = match manifest["messages_used"].as_array() {
            Some(used) => {
                let used = used
                    .iter()
                    .map(|path| path.as_str().map(str::to_owned))
                    .collect::<Option<Vec<_>>>()
                    .ok_or("messages_used holds a non-string path")?;
                unused_protocol_modules(&fs::read_to_string(root.join(PROTOCOL_LOCK))?, &used)?
            }
            None => BTreeSet::new(),
        };
        Ok(Some(Self {
            directories: runner.clone(),
            files,
            excluded,
        }))
    }

    pub(super) fn contains(&self, path: &Path) -> bool {
        if is_stand_operation(path)
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
    pub(super) fn from_directories(directories: &[&str]) -> Self {
        Self {
            directories: directories.iter().map(PathBuf::from).collect(),
            files: BTreeSet::new(),
            excluded: BTreeSet::new(),
        }
    }
}

/// The `source-inputs.json` schema that lists every file an image build
/// read; the earlier schema listed only the compiled sources.
pub(super) const COMPLETE_INPUTS: u32 = 2;

/// The `source-inputs.json` an image build writes beside its artifacts.
#[derive(Deserialize)]
struct SourceInputs {
    schema: u32,
    files: Vec<PathBuf>,
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

/// Directories of the path packages the HIL runner of the checkout at `root`
/// depends on.
pub(super) fn runner_directories(root: &Path) -> Result<BTreeSet<PathBuf>> {
    runner_packages(root, &metadata(root, &root.join("Cargo.toml"))?)
}

fn metadata(root: &Path, manifest: &Path) -> Result<Value> {
    let output = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
        .current_dir(root)
        .args(["metadata", "--format-version", "1", "--offline", "--locked"])
        .arg("--manifest-path")
        .arg(manifest)
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "cannot list the packages of {}: {}",
            manifest.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into());
    }
    Ok(serde_json::from_slice(&output.stdout)?)
}

/// The repository-relative directory of a package inside `root`.
fn package_directory(root: &Path, package: &Value) -> Option<PathBuf> {
    let manifest = Path::new(package["manifest_path"].as_str()?);
    let directory = manifest.parent()?.canonicalize().ok()?;
    directory.strip_prefix(root).ok().map(Path::to_owned)
}

/// Directories of the path packages the runner depends on, transitively.
fn runner_packages(root: &Path, metadata: &Value) -> Result<BTreeSet<PathBuf>> {
    let packages = metadata["packages"]
        .as_array()
        .ok_or("workspace metadata lists no packages")?;
    let by_id = packages
        .iter()
        .filter_map(|package| Some((package["id"].as_str()?, package)))
        .collect::<BTreeMap<_, _>>();
    let runner = packages
        .iter()
        .find(|package| package["name"] == RUNNER && package["source"].is_null())
        .and_then(|package| package["id"].as_str())
        .ok_or("the workspace has no HIL runner package")?;
    let nodes = metadata["resolve"]["nodes"]
        .as_array()
        .ok_or("workspace metadata has no dependency graph")?
        .iter()
        .filter_map(|node| Some((node["id"].as_str()?, node)))
        .collect::<BTreeMap<_, _>>();
    let mut pending = vec![runner];
    let mut seen = BTreeSet::new();
    let mut directories = BTreeSet::new();
    while let Some(id) = pending.pop() {
        if !seen.insert(id) {
            continue;
        }
        let Some(package) = by_id.get(id) else {
            continue;
        };
        if !package["source"].is_null() {
            continue;
        }
        if let Some(directory) = package_directory(root, package) {
            directories.insert(directory);
        }
        for dependency in nodes
            .get(id)
            .and_then(|node| node["deps"].as_array())
            .into_iter()
            .flatten()
        {
            if let Some(id) = dependency["pkg"].as_str() {
                pending.push(id);
            }
        }
    }
    Ok(directories)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn only_build_and_run_inputs_are_in_the_closure() {
        let closure = Closure::from_directories(&["crates/hardware/esp32s31/hal", "hil/scenarios"]);
        assert!(closure.contains(Path::new("crates/hardware/esp32s31/hal/src/lib.rs")));
        assert!(closure.contains(Path::new("hil/scenarios/system/boot-smoke.toml")));
        assert!(closure.contains(Path::new("Cargo.lock")));
        assert!(!closure.contains(Path::new("docs/architecture.md")));
        assert!(!closure.contains(Path::new("crates/hardware/esp32s31/hal-extra/src/lib.rs")));
        assert!(!closure.contains(Path::new("tools/blobray/src/main.rs")));
    }

    #[test]
    fn stand_operation_leaves_the_evidence_current_and_observing_code_does_not() {
        let closure = Closure::from_directories(&[
            "hil/host/arbiter",
            "hil/host/stand",
            "hil/host/link",
            "hil/host/evidence",
            "hil/host/runner-ieee80211",
        ]);
        for neutral in [
            "hil/host/arbiter/src/queue.rs",
            "hil/host/board/src/esp_idf.rs",
            "hil/host/stand/src/post_mortem.rs",
            "hil/host/stand/src/recovery.rs",
            "hil/host/link/src/tests.rs",
            "hil/host/link/tests/session.rs",
            "hil/host/stand/README.md",
        ] {
            assert!(!closure.contains(Path::new(neutral)), "{neutral}");
        }
        for observing in [
            "hil/host/arbiter/src/spectrum.rs",
            "hil/host/link/src/lib.rs",
            "hil/host/evidence/src/verify.rs",
            "hil/host/runner-ieee80211/src/workload/traffic.rs",
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
        let mut closure = Closure::from_directories(&["hil/protocol"]);
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
    fn every_stand_operation_path_exists() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        for path in STAND_OPERATION.iter().chain(&OBSERVING) {
            assert!(root.join(path).exists(), "{path} no longer exists");
        }
    }

    #[test]
    fn the_runner_closure_follows_path_dependencies_only() {
        let root = std::env::temp_dir().join(format!("closure-{}", std::process::id()));
        for directory in ["hil/host/runner", "hil/host/core", "tools/other"] {
            fs::create_dir_all(root.join(directory)).unwrap();
        }
        let root = root.canonicalize().unwrap();
        let manifest = |directory: &str| root.join(directory).join("Cargo.toml");
        let metadata = json!({
            "packages": [
                {"id": "runner", "name": RUNNER, "source": null,
                 "manifest_path": manifest("hil/host/runner")},
                {"id": "core", "name": "core", "source": null,
                 "manifest_path": manifest("hil/host/core")},
                {"id": "other", "name": "other", "source": null,
                 "manifest_path": manifest("tools/other")},
                {"id": "serde", "name": "serde", "source": "registry+https://github.com/rust-lang/crates.io-index",
                 "manifest_path": "/registry/serde/Cargo.toml"},
            ],
            "resolve": {"nodes": [
                {"id": "runner", "deps": [{"pkg": "core"}, {"pkg": "serde"}]},
                {"id": "core", "deps": [{"pkg": "serde"}]},
                {"id": "other", "deps": []},
                {"id": "serde", "deps": []},
            ]},
        });
        assert_eq!(
            runner_packages(&root, &metadata).unwrap(),
            BTreeSet::from([
                PathBuf::from("hil/host/core"),
                PathBuf::from("hil/host/runner")
            ])
        );
    }
}
