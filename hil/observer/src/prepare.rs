//! Build the HIL observer (the runner) with a receipt from Cargo's actual
//! artifacts.
use crate::{artifacts, compile::compile, receipt};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// Where a checkout keeps the private copies of the runners it built and
/// their receipts, relative to its root: one directory per executable
/// digest. (The observer *builds* runs refer to live in the run store's
/// [`crate::store::DIRECTORY`].)
pub const RUNNERS: &str = "target/hil/runners";

/// A prepared runner: its private copy and this invocation's receipt.
#[derive(Clone, Debug)]
pub struct Prepared {
    pub runner: PathBuf,
    pub receipt: PathBuf,
}

/// Build the runner of the checkout at `root`, keep a content-addressed
/// private copy below [`RUNNERS`]`/<sha256>/runner`, write this invocation's
/// receipt beside it and publish the checkout's [`receipt::CURRENT`].
pub fn prepare(root: &Path) -> Result<Prepared> {
    let compilation = compile(root)?;
    let executable = &compilation.executable;
    let artifacts = &compilation.artifacts;
    let bytes = fs::read(executable)?;
    let hash = oer_durable::sha256_bytes(&bytes);
    let directory = root.join(RUNNERS).join(&hash);
    fs::create_dir_all(&directory)?;
    let runner = directory.join("runner");
    // A private copy prevents concurrent Cargo rebuilds from replacing this run's inode.
    if !runner.exists() {
        fs::write(&runner, &bytes)?;
        fs::set_permissions(&runner, fs::metadata(executable)?.permissions())?;
    }
    if fs::read(&runner)? != bytes {
        return Err("observer executable identity conflict".into());
    }
    let output = oer_process::output(Command::new(&runner).arg("--observer-build"), None)?;
    if !output.status.success() {
        return Err("cannot read executable's embedded build".into());
    }
    let mut build: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    artifacts::apply(&mut build["resolved"], artifacts)?;
    build["resolved"]["selected_profile"] = serde_json::json!(compilation.profile);
    let document = serde_json::json!({"executable_sha256":hash,"build":build,"artifacts":artifacts,"profile":compilation.profile});
    // Each invocation owns its receipt, including concurrent launches of the same binary.
    let bytes = serde_json::to_vec(&document)?;
    let receipt_path = directory.join(format!(
        "receipt-{}.json",
        oer_durable::sha256_bytes(&bytes)
    ));
    if !receipt_path.exists() {
        oer_durable::atomic_write(&receipt_path, &bytes)?;
    }
    if fs::read(&receipt_path)? != bytes {
        return Err("observer receipt identity conflict".into());
    }
    oer_durable::atomic_write(&root.join(receipt::CURRENT), &bytes)?;
    drop(compilation);
    Ok(Prepared {
        runner,
        receipt: receipt_path,
    })
}
