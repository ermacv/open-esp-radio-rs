//! A checkout's clean runs whose evidence it has not recorded yet.
//!
//! `cargo hil run` never writes tracked files: a clean run's passed
//! scenarios are noted in the checkout's [`PENDING`] list, and
//! `cargo qualification hil-evidence --pending` turns them into tracked
//! shards when the change they qualify is committed.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::Result;

/// The checkout's pending list, relative to its root.
pub const PENDING: &str = "target/hil/pending-evidence.json";

/// A clean run whose evidence is not recorded yet.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Pending {
    pub run: String,
    /// The scenarios that passed in it.
    pub scenarios: Vec<String>,
    /// The lease owner the run was made for; several sessions may share
    /// a checkout.
    pub owner: String,
}

/// The checkout's pending runs.
pub fn load(checkout: &Path) -> Result<Vec<Pending>> {
    match std::fs::read(checkout.join(PENDING)) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error.into()),
    }
}

/// Add runs with passed scenarios to the checkout's pending list.
pub fn remember(checkout: &Path, runs: &[Pending]) -> Result<()> {
    update(checkout, |pending| {
        for run in runs.iter().filter(|run| !run.scenarios.is_empty()) {
            if !pending.iter().any(|entry| entry.run == run.run) {
                pending.push(run.clone());
            }
        }
    })
}

/// Drop recorded or dismissed runs from the pending list.
pub fn forget(checkout: &Path, runs: &[String]) -> Result<()> {
    update(checkout, |pending| {
        pending.retain(|entry| !runs.contains(&entry.run));
    })
}

fn update(checkout: &Path, change: impl FnOnce(&mut Vec<Pending>)) -> Result<()> {
    let path = checkout.join(PENDING);
    std::fs::create_dir_all(path.parent().ok_or("pending list has no parent")?)?;
    let _lock = oer_process::lock::FileLock::acquire(
        &path.with_extension("lock"),
        oer_process::lock::Mode::Exclusive,
    )?;
    let mut pending = load(checkout)?;
    change(&mut pending);
    oer_durable::atomic_json(&path, &pending)
}
