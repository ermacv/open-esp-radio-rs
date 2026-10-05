//! Which runs the store keeps: the prune rule and the size budget.
//!
//! Pins and the rule keep what agents cite, replay and compare.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

use oer_hil_run_bundle::{
    RunStore,
    run::RunState,
    store::{Notes, Sidecar},
};

use crate::{Result, run::Run};

/// The prune rule.
#[derive(Debug)]
pub struct Retention {
    /// Every run younger than this is kept.
    pub keep_days: u64,
    /// The newest failed runs kept per scenario and image class.
    pub keep_failed: usize,
}

/// Why each run is kept; runs without a reason may be deleted.
pub fn retained(
    runs: &[Run],
    rule: &Retention,
    now_millis: u64,
    pinned: &BTreeSet<String>,
    cited: &BTreeSet<String>,
) -> BTreeMap<String, String> {
    let mut keep = BTreeMap::new();
    let recent = now_millis.saturating_sub(rule.keep_days * 24 * 3600 * 1000);
    for run in runs {
        let id = run.id().to_owned();
        if pinned.contains(&id) {
            keep.insert(id, String::from("pinned"));
        } else if cited.contains(&id) {
            keep.insert(id, String::from("cited by a committed shard"));
        } else if run.started_millis() >= recent {
            keep.insert(id, format!("younger than {} days", rule.keep_days));
        } else if run.state() == RunState::Running && !run.abandoned {
            keep.insert(id, String::from("in progress"));
        } else if run.outcome().is_none() && !run.abandoned {
            keep.insert(id, String::from("incomplete"));
        }
        for source in run.bundle.replayed_runs().unwrap_or_default() {
            keep.entry(source)
                .or_insert_with(|| format!("firmware replayed by {}", run.id()));
        }
    }
    let mut passed = BTreeMap::new();
    let mut failed: BTreeMap<(String, String), Vec<&Run>> = BTreeMap::new();
    for run in runs.iter().rev() {
        for scenario in run.scenarios() {
            let key = (scenario.scenario.clone(), scenario.image.id().to_owned());
            if scenario.outcome.is_passed() {
                passed.entry(key).or_insert(run.id().to_owned());
            } else {
                failed.entry(key).or_default().push(run);
            }
        }
    }
    for ((scenario, image), run) in passed {
        keep.entry(run)
            .or_insert_with(|| format!("latest pass of {scenario} [{image}]"));
    }
    for ((scenario, image), runs) in failed {
        for run in runs.into_iter().take(rule.keep_failed) {
            keep.entry(run.id().to_owned())
                .or_insert_with(|| format!("recent failure of {scenario} [{image}]"));
        }
    }
    keep
}

/// The runs to delete, oldest first, so that a store of these `runs`
/// (oldest first) with these exclusive `sizes` fits `budget` bytes. Only a
/// run that no rule but its age keeps is deleted: `kept` holds the runs the
/// other rules keep.
pub fn over_budget<'a>(
    runs: &'a [Run],
    kept: &BTreeMap<String, String>,
    sizes: &BTreeMap<String, u64>,
    budget: u64,
) -> Vec<&'a Run> {
    let size = |run: &Run| sizes.get(run.id()).copied().unwrap_or(0);
    let mut total: u64 = runs.iter().map(size).sum();
    let mut deleted = Vec::new();
    for run in runs {
        if total <= budget {
            break;
        }
        if kept.contains_key(run.id()) {
            continue;
        }
        total = total.saturating_sub(size(run));
        deleted.push(run);
    }
    deleted
}

/// Bytes only this directory holds: files without other hard links.
pub fn exclusive_bytes(directory: &Path) -> u64 {
    use std::os::unix::fs::MetadataExt as _;
    let mut total = 0;
    let mut pending = vec![directory.to_owned()];
    while let Some(path) = pending.pop() {
        let Ok(entries) = fs::read_dir(&path) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            if metadata.is_dir() {
                pending.push(entry.path());
            } else if metadata.nlink() == 1 {
                total += metadata.len();
            }
        }
    }
    total
}

/// What a prune deleted, or would delete.
#[derive(Debug, Default)]
pub struct Pruned {
    /// The deleted runs, oldest first, with the bytes only they held.
    pub removed: Vec<(Run, u64)>,
    /// How many runs a rule keeps.
    pub kept: usize,
    /// How many runs the store held.
    pub total: usize,
    /// How many observer builds no kept run names were deleted.
    pub observers: usize,
}

impl Pruned {
    pub fn freed(&self) -> u64 {
        self.removed.iter().map(|(_, bytes)| bytes).sum()
    }
}

/// Delete (with `apply`; else only list) the runs of `store` that no rule
/// keeps: not pinned, not cited by a shard of the checkout at `checkout`,
/// not replayed, not younger than the rule, not the latest pass or a recent
/// failure of a scenario. With a `budget` in bytes, a store over it also
/// loses its oldest runs that only their age kept.
pub fn prune(
    store: &RunStore,
    checkout: &Path,
    rule: &Retention,
    budget: Option<u64>,
    apply: bool,
) -> Result<Pruned> {
    let all = Run::all(store)?;
    let now = oer_durable::unix_millis();
    let pinned = store
        .read::<Notes>(Sidecar::Pins)?
        .into_keys()
        .collect::<BTreeSet<_>>();
    let cited = cited_by_shards(checkout);
    let keep = retained(&all, rule, now, &pinned, &cited);
    let mut pruned = Pruned {
        total: all.len(),
        ..Pruned::default()
    };
    let (remaining, deleted): (Vec<Run>, Vec<Run>) =
        all.into_iter().partition(|run| keep.contains_key(run.id()));
    for run in deleted {
        let bytes = exclusive_bytes(run.directory());
        pruned.removed.push((run, bytes));
    }
    if let Some(budget) = budget {
        let sizes = remaining
            .iter()
            .map(|run| (run.id().to_owned(), exclusive_bytes(run.directory())))
            .collect::<BTreeMap<_, _>>();
        let kept = retained(
            &remaining,
            &Retention {
                keep_days: 0,
                keep_failed: rule.keep_failed,
            },
            now,
            &pinned,
            &cited,
        );
        let over = over_budget(&remaining, &kept, &sizes, budget)
            .into_iter()
            .map(|run| run.id().to_owned())
            .collect::<BTreeSet<_>>();
        for run in remaining {
            if over.contains(run.id()) {
                let bytes = sizes[run.id()];
                pruned.removed.push((run, bytes));
            } else {
                pruned.kept += 1;
            }
        }
    } else {
        pruned.kept = remaining.len();
    }
    if apply {
        for (run, _) in &pruned.removed {
            fs::remove_dir_all(run.directory())?;
        }
        pruned.observers = crate::runs::collect_observers(store)?;
    }
    Ok(pruned)
}

/// Run IDs the committed HIL evidence shards of the checkout at `root` cite.
pub fn cited_by_shards(root: &Path) -> BTreeSet<String> {
    fn collect(value: &serde_json::Value, found: &mut BTreeSet<String>) {
        match value {
            serde_json::Value::Object(map) => {
                for (name, value) in map {
                    if (name == "run-id" || name == "run_id")
                        && let Some(text) = value.as_str()
                    {
                        found.insert(text.to_owned());
                    }
                    collect(value, found);
                }
            }
            serde_json::Value::Array(items) => items.iter().for_each(|item| collect(item, found)),
            _ => {}
        }
    }
    let mut cited = BTreeSet::new();
    let Ok(targets) = fs::read_dir(root.join("hil/evidence")) else {
        return cited;
    };
    for target in targets.flatten() {
        let Ok(shards) = fs::read_dir(target.path()) else {
            continue;
        };
        for shard in shards.flatten() {
            if let Some(value) = fs::read(shard.path())
                .ok()
                .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            {
                collect(&value, &mut cited);
            }
        }
    }
    cited
}

#[cfg(test)]
mod tests;
