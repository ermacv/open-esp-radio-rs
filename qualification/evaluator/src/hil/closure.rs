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

/// Workspaces whose path packages a firmware image builds.
const FIRMWARE_WORKSPACES: [&str; 2] = [
    "hil/targets/esp32s31/Cargo.toml",
    "platform/esp32s31/bootstrap/Cargo.toml",
];

/// The host package that executes scenarios and records the observation.
const RUNNER: &str = "oer-hil-runner";

/// Directories read whole: the firmware workspace, the platform's linker and
/// stack inputs, the scenario catalog and Cargo's configuration.
const DIRECTORIES: [&str; 4] = [
    "hil/targets/esp32s31",
    "platform/esp32s31",
    "hil/scenarios",
    ".cargo",
];

/// Workspace files that configure every build.
const FILES: [&str; 4] = [
    "Cargo.toml",
    "Cargo.lock",
    "rust-toolchain.toml",
    "rust-toolchain",
];

/// The firmware workspaces' manifests and lock files, which every image
/// build reads.
const FIRMWARE_CONFIGURATION: [&str; 4] = [
    "hil/targets/esp32s31/Cargo.toml",
    "hil/targets/esp32s31/Cargo.lock",
    "platform/esp32s31/bootstrap/Cargo.toml",
    "platform/esp32s31/bootstrap/Cargo.lock",
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
    "hil/host/runner-core/src/device",
    "hil/host/runner-core/src/post_mortem.rs",
    "hil/host/runner-core/src/recovery.rs",
    "hil/host/runner-core/src/usb_events.rs",
    "hil/host/runner-core/src/profile.rs",
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
}

impl Closure {
    /// The closure of the checkout at `root`, from Cargo's locked metadata.
    pub(super) fn of(root: &Path) -> Result<Self> {
        let root = root.canonicalize()?;
        let mut directories = DIRECTORIES
            .map(PathBuf::from)
            .into_iter()
            .collect::<BTreeSet<_>>();
        for workspace in FIRMWARE_WORKSPACES {
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
        })
    }

    /// The closure of one run, narrower than the checkout's: the files its
    /// firmware images were compiled from, which each build recorded as
    /// `source-inputs.json`, the files of the scenarios it executed, the
    /// firmware workspaces' configuration, Cargo's configuration and the
    /// runner's packages (`runner`, from [`runner_directories`]). A change
    /// to another image class, crate or scenario leaves it current. `None`
    /// when an image of the run has no recorded inputs.
    pub(super) fn of_run(
        root: &Path,
        run: &Path,
        runner: &BTreeSet<PathBuf>,
    ) -> Result<Option<Self>> {
        let mut files = FIRMWARE_CONFIGURATION
            .map(PathBuf::from)
            .into_iter()
            .collect::<BTreeSet<_>>();
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
            if inputs.schema != 1 {
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
        let mut directories = runner.clone();
        directories.insert(PathBuf::from(".cargo"));
        Ok(Some(Self { directories, files }))
    }

    pub(super) fn contains(&self, path: &Path) -> bool {
        if is_stand_operation(path) {
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
        }
    }
}

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
            "hil/host/runner-core",
            "hil/host/runner-ieee80211",
        ]);
        for neutral in [
            "hil/host/arbiter/src/queue.rs",
            "hil/host/runner-core/src/device/mod.rs",
            "hil/host/runner-core/src/post_mortem.rs",
            "hil/host/runner-core/src/image/tests.rs",
            "hil/host/runner-core/tests/session.rs",
            "hil/host/runner-core/README.md",
        ] {
            assert!(!closure.contains(Path::new(neutral)), "{neutral}");
        }
        for observing in [
            "hil/host/arbiter/src/spectrum.rs",
            "hil/host/runner-core/src/session.rs",
            "hil/host/runner-core/src/evidence/verify.rs",
            "hil/host/runner-ieee80211/src/workload/traffic.rs",
        ] {
            assert!(closure.contains(Path::new(observing)), "{observing}");
        }
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
