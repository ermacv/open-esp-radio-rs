//! Recording HIL evidence as an explicit step.
//!
//! `cargo hil run` never writes tracked files: a clean run's passed scenarios
//! are noted in this checkout's pending list, `target/hil/pending-evidence.json`,
//! and `cargo hil evidence record` turns runs into tracked shards when the
//! change they qualify is committed. `cargo xtask check changed` reads the
//! pending list and reminds of runs of qualification scenarios that are not
//! recorded yet.

use std::{
    collections::BTreeSet,
    ffi::OsString,
    path::{Path, PathBuf},
};

use oer_hil_schema::run::{Outcome, RunState};
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
    /// checkout. `None` for entries noted before owners were.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
}

const HELP: &str = "\
usage: cargo hil evidence record [--run ID ...] [--since REV]
       cargo hil evidence pending

record   write the qualifying observations of runs as tracked shards in
         hil/evidence/<chip>/: the --run runs, the clean completed runs of
         commits in REV..HEAD with --since, or else this checkout's pending runs
pending  list this checkout's clean runs whose evidence is not recorded";

/// `cargo hil evidence ...`.
pub fn command(ctx: &Context, args: &[OsString]) -> Result<std::process::ExitCode> {
    let args = args
        .iter()
        .map(|arg| arg.to_str().ok_or("arguments must be UTF-8"))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    match args.split_first() {
        Some((&"record", rest)) => record(ctx, rest)?,
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
                    entry.owner.as_deref().unwrap_or("unknown owner")
                );
            }
        }
        _ => {
            println!("{HELP}");
        }
    }
    Ok(std::process::ExitCode::SUCCESS)
}

fn record(ctx: &Context, args: &[&str]) -> Result<()> {
    let mut runs = Vec::new();
    let mut since = None;
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        match *arg {
            "--run" => runs.push(rest.next().ok_or("--run needs a run ID")?.to_string()),
            "--since" => since = Some(*rest.next().ok_or("--since needs a revision")?),
            other => return Err(format!("unknown argument {other}\n{HELP}").into()),
        }
    }
    if let Some(since) = since {
        runs.extend(runs_since(ctx, since)?);
    } else if runs.is_empty() {
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

/// Clean, completed runs in the store built from a commit in `since..HEAD`.
fn runs_since(ctx: &Context, since: &str) -> Result<Vec<String>> {
    let output = ctx
        .command("git")
        .args(["rev-list", &format!("{since}..HEAD")])
        .current_dir(&ctx.root)
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "git rev-list {since}..HEAD failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into());
    }
    let commits = String::from_utf8(output.stdout)?
        .lines()
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
    let store = crate::hil_store::shared_runs(crate::hil::HIL_TARGET)?;
    let mut runs = Vec::new();
    for entry in std::fs::read_dir(&store)? {
        let path = entry?.path();
        let Ok(bytes) = std::fs::read(path.join("manifest.json")) else {
            continue;
        };
        // A manifest outside the known vocabulary is not selected.
        let Ok(manifest) = serde_json::from_slice::<Manifest>(&bytes) else {
            continue;
        };
        if selected_since(&manifest, &commits) {
            runs.push(manifest.run_id);
        }
    }
    Ok(runs)
}

/// The part of a run manifest that selects it.
#[derive(Deserialize)]
struct Manifest {
    run_id: String,
    state: RunState,
    repository: Repository,
}

#[derive(Deserialize)]
struct Repository {
    commit: Option<String>,
    dirty: bool,
}

/// Whether a run manifest is a clean completed run of one of `commits`.
fn selected_since(manifest: &Manifest, commits: &BTreeSet<String>) -> bool {
    manifest.state == RunState::Completed
        && !manifest.repository.dirty
        && manifest
            .repository
            .commit
            .as_ref()
            .is_some_and(|commit| commits.contains(commit))
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

/// Drop recorded runs from the pending list.
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

/// Every scenario a qualification catalog requires evidence from.
fn qualification_scenarios(root: &Path) -> Result<BTreeSet<String>> {
    fn collect(value: &toml::Value, into: &mut BTreeSet<String>) {
        match value {
            toml::Value::Table(table) => {
                for (key, value) in table {
                    if key == "scenario"
                        && let Some(name) = value.as_str()
                    {
                        into.insert(name.to_owned());
                    }
                    collect(value, into);
                }
            }
            toml::Value::Array(values) => values.iter().for_each(|value| collect(value, into)),
            _ => {}
        }
    }
    let mut scenarios = BTreeSet::new();
    let mut directories = vec![root.join("qualification/catalog")];
    while let Some(directory) = directories.pop() {
        for entry in std::fs::read_dir(&directory)? {
            let path: PathBuf = entry?.path();
            if path.is_dir() {
                directories.push(path);
            } else if path
                .extension()
                .is_some_and(|extension| extension == "toml")
            {
                collect(
                    &toml::from_str(&std::fs::read_to_string(&path)?)?,
                    &mut scenarios,
                );
            }
        }
    }
    Ok(scenarios)
}

/// The reminder `check changed` prints, when this checkout has pending runs
/// of qualification scenarios.
/// With `OER_HIL_OWNER` set, only that owner's runs are named.
pub fn reminder(root: &Path) -> Result<Option<String>> {
    let pending = load(root)?;
    if pending.is_empty() {
        return Ok(None);
    }
    let owner = std::env::var(oer_hil_arbiter::OWNER_ENV)
        .ok()
        .filter(|owner| !owner.is_empty());
    Ok(reminder_for(
        &of_owner(live(&pending)?, owner.as_deref()),
        &qualification_scenarios(root)?,
    ))
}

/// The pending runs of `owner`, or all of them without one.
fn of_owner(pending: Vec<Pending>, owner: Option<&str>) -> Vec<Pending> {
    pending
        .into_iter()
        .filter(|entry| owner.is_none_or(|owner| entry.owner.as_deref() == Some(owner)))
        .collect()
}

fn reminder_for(pending: &[Pending], qualifying: &BTreeSet<String>) -> Option<String> {
    let mut runs = Vec::new();
    let mut scenarios = BTreeSet::new();
    for entry in pending {
        let qualified = entry
            .scenarios
            .iter()
            .filter(|scenario| qualifying.contains(*scenario))
            .collect::<Vec<_>>();
        if !qualified.is_empty() {
            runs.push(entry);
            scenarios.extend(qualified);
        }
    }
    (!runs.is_empty()).then(|| {
        let owners = runs
            .iter()
            .map(|entry| entry.owner.as_deref().unwrap_or("unknown owner"))
            .collect::<BTreeSet<_>>();
        format!(
            "{} clean run(s) of qualification scenarios by {} have no recorded evidence ({}); \
             record it with the change it qualifies: cargo hil evidence record{}",
            runs.len(),
            owners.into_iter().collect::<Vec<_>>().join(", "),
            scenarios
                .into_iter()
                .cloned()
                .collect::<Vec<_>>()
                .join(", "),
            runs.iter()
                .map(|entry| format!(" --run {}", entry.run))
                .collect::<String>()
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pending(run: &str, scenarios: &[&str]) -> Pending {
        Pending {
            run: run.into(),
            scenarios: scenarios.iter().map(|s| s.to_string()).collect(),
            owner: Some(String::from("wifi")),
        }
    }

    #[test]
    fn an_owner_sees_only_its_own_pending_runs() {
        let mine = pending("r1", &["a"]);
        let theirs = Pending {
            owner: Some(String::from("802154")),
            ..pending("r2", &["a"])
        };
        let both = vec![mine.clone(), theirs];
        assert_eq!(of_owner(both.clone(), Some("wifi")), [mine]);
        assert_eq!(of_owner(both, None).len(), 2);
    }

    #[test]
    fn the_reminder_names_only_runs_of_qualification_scenarios() {
        let qualifying = BTreeSet::from([String::from("station-reconnect")]);
        assert_eq!(
            reminder_for(&[pending("r1", &["diagnostic-x"])], &qualifying),
            None
        );
        let reminder = reminder_for(
            &[
                pending("r1", &["diagnostic-x"]),
                pending("r2", &["station-reconnect", "diagnostic-x"]),
            ],
            &qualifying,
        )
        .unwrap();
        assert!(reminder.starts_with("1 clean run(s)"), "{reminder}");
        assert!(reminder.contains("(station-reconnect)"), "{reminder}");
        assert!(
            reminder.ends_with("cargo hil evidence record --run r2"),
            "{reminder}"
        );
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
    fn since_selects_clean_completed_runs_of_the_commits() {
        let commits = BTreeSet::from([String::from("c1")]);
        let manifest = |state: RunState, dirty: bool, commit: &str| Manifest {
            run_id: String::from("r"),
            state,
            repository: Repository {
                commit: Some(commit.into()),
                dirty,
            },
        };
        assert!(selected_since(
            &manifest(RunState::Completed, false, "c1"),
            &commits
        ));
        assert!(!selected_since(
            &manifest(RunState::Completed, true, "c1"),
            &commits
        ));
        assert!(!selected_since(
            &manifest(RunState::Interrupted, false, "c1"),
            &commits
        ));
        assert!(!selected_since(
            &manifest(RunState::Completed, false, "c2"),
            &commits
        ));
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

    #[test]
    fn the_catalogs_name_their_scenarios() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        assert!(
            qualification_scenarios(&root)
                .unwrap()
                .contains("station-reconnect")
        );
    }
}
