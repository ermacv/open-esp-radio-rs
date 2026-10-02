//! Recording HIL evidence as an explicit step.
//!
//! `cargo hil run` never writes tracked files: a clean run's passed scenarios
//! are noted in this checkout's pending list, `target/hil/pending-evidence.json`,
//! and `cargo hil evidence record` turns runs into tracked shards when the
//! change they qualify is committed. `cargo xtask check changed` reads the
//! pending list and reminds of runs of qualification scenarios that are not
//! recorded yet.

use std::{ffi::OsString, path::Path};

use oer_hil_schema::run::Outcome;
use serde::{Deserialize, Serialize};

use crate::{Context, Result};

/// This checkout's pending list, relative to its root.
const PENDING: &str = "target/hil/pending-evidence.json";

/// A clean run whose evidence is not recorded yet.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Pending {
    pub run: String,
    /// The scenarios that passed in it.
    pub scenarios: Vec<String>,
    /// The lease owner the run was made for; several sessions may share a
    /// checkout.
    pub owner: String,
}

const HELP: &str = "\
usage: cargo hil evidence record [--run ID ...]
       cargo hil evidence pending

record   write the qualifying observations of runs as tracked shards in
         hil/evidence/<chip>/: the --run runs, or else this checkout's pending
         runs
pending  list this checkout's clean runs whose evidence is not recorded
dismiss  --run ID ...: drop runs whose evidence will not be recorded, such as
         runs whose inputs changed since, from the pending list";

/// `cargo hil evidence ...`.
pub fn command(ctx: &Context, args: &[OsString]) -> Result<std::process::ExitCode> {
    let args = args
        .iter()
        .map(|arg| arg.to_str().ok_or("arguments must be UTF-8"))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    match args.split_first() {
        Some((&"record", rest)) => record(ctx, rest)?,
        Some((&"dismiss", rest)) => {
            let runs = dismiss(&ctx.root, rest)?;
            println!("hil: dismissed {runs} pending run(s)");
        }
        Some((&"pending", [])) => {
            let pending = live(&load(&ctx.root)?)?;
            if pending.is_empty() {
                println!("no pending HIL evidence in this checkout");
            }
            for entry in pending {
                println!(
                    "{}  {}  ({})",
                    entry.run,
                    entry.scenarios.join(", "),
                    entry.owner
                );
            }
        }
        _ => {
            println!("{HELP}");
        }
    }
    Ok(std::process::ExitCode::SUCCESS)
}

/// The `--run ID` arguments of an evidence command.
fn run_arguments(args: &[&str]) -> Result<Vec<String>> {
    let mut runs = Vec::new();
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        match *arg {
            "--run" => runs.push(rest.next().ok_or("--run needs a run ID")?.to_string()),
            other => return Err(format!("unknown argument {other}\n{HELP}").into()),
        }
    }
    Ok(runs)
}

fn record(ctx: &Context, args: &[&str]) -> Result<()> {
    let mut runs = run_arguments(args)?;
    if runs.is_empty() {
        runs = live(&load(&ctx.root)?)?
            .into_iter()
            .map(|entry| entry.run)
            .collect();
    }
    runs.sort();
    runs.dedup();
    if runs.is_empty() {
        println!("hil: no runs to record");
        return Ok(());
    }
    let store = crate::hil_store::shared_runs(crate::hil::HIL_TARGET)?;
    if let Some(missing) = runs.iter().find(|run| !store.join(run).is_dir()) {
        return Err(format!("run {missing} is not in the run store").into());
    }
    let (_, receipt) = crate::hil::prepare(ctx)?;
    crate::hil::record_evidence(ctx, &receipt, &runs)?;
    forget(&ctx.root, &runs)?;
    // The evaluator's HIL-EVIDENCE line counts the shards it wrote; a run
    // whose observations do not qualify on this checkout writes none.
    println!(
        "hil: the evaluator read {} run(s); commit any hil/evidence changes with the change \
         they qualify",
        runs.len()
    );
    Ok(())
}

/// The part of a suite summary naming passed scenarios.
#[derive(Deserialize)]
struct Suite {
    scenarios: Vec<SuiteScenario>,
}

#[derive(Deserialize)]
struct SuiteScenario {
    scenario: String,
    outcome: Outcome,
}

/// The scenarios that passed in `run`, from its suite summary.
pub fn passed_scenarios(run: &Path) -> Vec<String> {
    std::fs::read(run.join("suite.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Suite>(&bytes).ok())
        .map(|suite| {
            suite
                .scenarios
                .into_iter()
                .filter(|scenario| scenario.outcome == Outcome::Passed)
                .map(|scenario| scenario.scenario)
                .collect()
        })
        .unwrap_or_default()
}

/// Add clean runs with passed scenarios to this checkout's pending list.
pub fn remember(root: &Path, runs: &[Pending]) -> Result<()> {
    update(root, |pending| {
        for run in runs.iter().filter(|run| !run.scenarios.is_empty()) {
            if !pending.iter().any(|entry| entry.run == run.run) {
                pending.push(run.clone());
            }
        }
    })
}

/// Drop the `--run` runs whose evidence will not be recorded from the pending
/// list; returns how many were named.
fn dismiss(root: &Path, args: &[&str]) -> Result<usize> {
    let runs = run_arguments(args)?;
    if runs.is_empty() {
        return Err(format!("dismiss needs --run ID\n{HELP}").into());
    }
    forget(root, &runs)?;
    Ok(runs.len())
}

/// Drop recorded or dismissed runs from the pending list.
fn forget(root: &Path, runs: &[String]) -> Result<()> {
    update(root, |pending| {
        pending.retain(|entry| !runs.contains(&entry.run));
    })
}

fn update(root: &Path, change: impl FnOnce(&mut Vec<Pending>)) -> Result<()> {
    let path = root.join(PENDING);
    std::fs::create_dir_all(path.parent().ok_or("pending list has no parent")?)?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path.with_extension("lock"))?;
    fs2::FileExt::lock_exclusive(&lock)?;
    let mut pending = load(root)?;
    change(&mut pending);
    let mut file = tempfile::NamedTempFile::new_in(path.parent().ok_or("no parent")?)?;
    serde_json::to_writer_pretty(&mut file, &pending)?;
    file.persist(&path)?;
    Ok(())
}

fn load(root: &Path) -> Result<Vec<Pending>> {
    match std::fs::read(root.join(PENDING)) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error.into()),
    }
}

/// The pending runs still in the run store; pruned runs cannot be recorded.
fn live(pending: &[Pending]) -> Result<Vec<Pending>> {
    let store = crate::hil_store::shared_runs(crate::hil::HIL_TARGET)?;
    Ok(pending
        .iter()
        .filter(|entry| store.join(&entry.run).is_dir())
        .cloned()
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pending(run: &str, scenarios: &[&str]) -> Pending {
        Pending {
            run: run.into(),
            scenarios: scenarios.iter().map(|s| s.to_string()).collect(),
            owner: String::from("wifi"),
        }
    }

    #[test]
    fn the_pending_list_keeps_runs_with_passed_scenarios_until_recorded() {
        let root = tempfile::tempdir().unwrap();
        remember(
            root.path(),
            &[
                pending("r1", &["a"]),
                pending("r2", &[]),
                pending("r3", &["b"]),
            ],
        )
        .unwrap();
        remember(root.path(), &[pending("r1", &["a"])]).unwrap();
        assert_eq!(
            load(root.path()).unwrap(),
            [pending("r1", &["a"]), pending("r3", &["b"])]
        );
        forget(root.path(), &[String::from("r1")]).unwrap();
        assert_eq!(load(root.path()).unwrap(), [pending("r3", &["b"])]);
    }

    #[test]
    fn a_pending_entry_names_its_owner() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join(PENDING);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, br#"[{"run":"r1","scenarios":["a"]}]"#).unwrap();
        assert!(load(root.path()).is_err());
    }

    #[test]
    fn dismissing_drops_only_the_named_runs_and_needs_one() {
        let root = tempfile::tempdir().unwrap();
        remember(
            root.path(),
            &[
                pending("r1", &["a"]),
                pending("r2", &["a"]),
                pending("r3", &["b"]),
            ],
        )
        .unwrap();
        assert!(dismiss(root.path(), &[]).is_err());
        assert!(dismiss(root.path(), &["r1"]).is_err());
        assert_eq!(
            dismiss(root.path(), &["--run", "r1", "--run", "r3"]).unwrap(),
            2
        );
        assert_eq!(load(root.path()).unwrap(), [pending("r2", &["a"])]);
    }

    #[test]
    fn passed_scenarios_come_from_the_suite_summary() {
        let run = tempfile::tempdir().unwrap();
        std::fs::write(
            run.path().join("suite.json"),
            r#"{"scenarios":[{"scenario":"a","outcome":"passed"},{"scenario":"b","outcome":"failed"}]}"#,
        )
        .unwrap();
        assert_eq!(passed_scenarios(run.path()), ["a"]);
    }
}
