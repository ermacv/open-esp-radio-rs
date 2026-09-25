//! Read-only retention candidates. This is not garbage collection or integrity
//! verification: declarations are inspected without hashing multi-GB bundles.
use crate::Result;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

#[derive(Serialize)]
pub(super) struct Report {
    schema: u16,
    mode: &'static str,
    integrity_verified: bool,
    /// Logical inventory sizes include shared hard links, not reclaimable space.
    candidate_logical_bytes: u64,
    /// CAS is deliberately untouched; a digest reference is not a hash check.
    cas_policy: &'static str,
    runs: Vec<Run>,
}

#[derive(Serialize)]
struct Run {
    id: String,
    keep_reasons: BTreeSet<String>,
    logical_bytes: Option<u64>,
    declared_cas_references: usize,
}

#[derive(Deserialize)]
struct Manifest {
    schema: u16,
    run_id: String,
    state: String,
    firmware: Vec<Firmware>,
}
#[derive(Deserialize)]
struct Firmware {
    replayed_from: Option<Replay>,
}
#[derive(Deserialize)]
struct Replay {
    source_run_id: String,
}
#[derive(Deserialize)]
struct Suite {
    schema: u16,
    run_id: String,
    outcome: String,
    started_unix_millis: u64,
    scenarios: Vec<Scenario>,
}
#[derive(Deserialize)]
struct Scenario {
    scenario: String,
}
#[derive(Deserialize)]
struct Inventory {
    schema: u16,
    run_id: String,
    files: Vec<File>,
}
#[derive(Deserialize)]
struct File {
    size_bytes: u64,
    sha256: String,
}

fn read<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    if !fs::symlink_metadata(path)?.is_file() {
        return Err(format!("not a regular metadata file: {}", path.display()).into());
    }
    Ok(serde_json::from_slice(&fs::read(path)?)?)
}

pub(super) fn inspect(root: &Path, keep: &[String]) -> Result<Report> {
    let target = root.join("target/hil").join(super::TARGET);
    let directory = target.join("runs");
    // Refuse alternate directory trees rather than following links outside the store.
    if !fs::symlink_metadata(&directory)?.is_dir() {
        return Err("run inventory requires a real runs directory".into());
    }
    let mut runs = BTreeMap::new();
    let mut referenced = BTreeSet::new();
    let mut latest = BTreeMap::<String, (u64, String)>::new();
    for entry in fs::read_dir(&directory)? {
        oer_process::check_cancelled()?;
        let entry = entry?;
        let id = entry
            .file_name()
            .into_string()
            .map_err(|_| "non-UTF8 run ID")?;
        super::validate_id(&id)?;
        let mut run = Run {
            id: id.clone(),
            keep_reasons: BTreeSet::new(),
            logical_bytes: None,
            declared_cas_references: 0,
        };
        let mut inspect_run = || -> Result<()> {
            if !entry.file_type()?.is_dir() {
                return Err("not a real run directory".into());
            }
            let manifest: Manifest = read(&entry.path().join("manifest.json"))?;
            if manifest.schema != 2 || manifest.run_id != id {
                return Err("unknown manifest identity/schema".into());
            }
            for firmware in manifest.firmware {
                if let Some(replay) = firmware.replayed_from {
                    super::validate_id(&replay.source_run_id)?;
                    referenced.insert(replay.source_run_id);
                }
            }
            if manifest.state != "completed" {
                run.keep_reasons.insert("active-or-incomplete".into());
                return Ok(());
            }
            let suite: Suite = read(&entry.path().join("suite.json"))?;
            let inventory: Inventory = read(&entry.path().join("integrity.json"))?;
            if suite.schema != 2
                || inventory.schema != 2
                || suite.run_id != id
                || inventory.run_id != id
                || suite.scenarios.is_empty()
            {
                return Err("unknown sealed metadata identity/schema or empty suite".into());
            }
            if suite.outcome != "passed" {
                run.keep_reasons.insert("non-passing-experiment".into());
            }
            let mut bytes = 0u64;
            for file in inventory.files {
                bytes = bytes
                    .checked_add(file.size_bytes)
                    .ok_or("inventory size overflow")?;
                if file.sha256.len() != 64 || !file.sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
                    return Err("invalid declared digest".into());
                }
                let object = target
                    .join("objects/sha256")
                    .join(&file.sha256[..2])
                    .join(&file.sha256);
                if object.try_exists()? {
                    run.declared_cas_references += 1;
                }
            }
            run.logical_bytes = Some(bytes);
            if suite.outcome == "passed" {
                for scenario in suite.scenarios {
                    let candidate = (suite.started_unix_millis, id.clone());
                    let selected = latest
                        .entry(scenario.scenario)
                        .or_insert_with(|| candidate.clone());
                    if candidate > *selected {
                        *selected = candidate;
                    }
                }
            }
            Ok(())
        };
        if let Err(error) = inspect_run() {
            run.keep_reasons
                .insert(format!("unclassified-metadata: {error}"));
        }
        runs.insert(id, run);
    }
    // Imported archive manifests are retained independently, and their local
    // runs are protected. Never unpack exports or query a remote repository.
    let archives = target.join("archives");
    if archives.try_exists()? {
        if !fs::symlink_metadata(&archives)?.is_dir() {
            return Err("archives directory is a link".into());
        }
        for entry in fs::read_dir(archives)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                return Err("unclassified archive entry".into());
            }
            let archive: super::Manifest = read(&entry.path().join("archive.json"))?;
            if archive.schema != 1 || archive.target != super::TARGET {
                return Err("unclassified archive schema".into());
            }
            for id in archive.runs {
                if let Some(run) = runs.get_mut(&id) {
                    run.keep_reasons.insert("imported-archive".into());
                }
            }
        }
    }
    for id in keep {
        super::validate_id(id)?;
        runs.get_mut(id)
            .ok_or_else(|| format!("explicit keep run does not exist: {id}"))?
            .keep_reasons
            .insert("explicit-baseline-or-experiment".into());
    }
    for id in referenced {
        if let Some(run) = runs.get_mut(&id) {
            run.keep_reasons.insert("firmware-replay-source".into());
        }
    }
    for (_, id) in latest.into_values() {
        runs.get_mut(&id)
            .unwrap()
            .keep_reasons
            .insert("latest-passing-scenario".into());
    }
    let candidate_logical_bytes = runs
        .values()
        .filter(|r| r.keep_reasons.is_empty())
        .try_fold(0u64, |sum, r| sum.checked_add(r.logical_bytes.unwrap_or(0)))
        .ok_or("candidate size overflow")?;
    Ok(Report {
        schema: 1,
        mode: "dry-run-only; concurrent writers may change inventory",
        integrity_verified: false,
        candidate_logical_bytes,
        cas_policy: "retain-all; logical bytes are not reclaimable bytes; no deletion authorization",
        runs: runs.into_values().collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn make(root: &Path, id: &str, outcome: &str, time: u64, replay: Option<&str>) {
        let dir = root.join("target/hil/esp32s31/runs").join(id);
        fs::create_dir_all(&dir).unwrap();
        let firmware = replay
            .map(|id| serde_json::json!([{"replayed_from":{"source_run_id":id}}]))
            .unwrap_or(serde_json::json!([]));
        for (name, value) in [
            (
                "manifest.json",
                serde_json::json!({"schema":2,"run_id":id,"state":"completed","firmware":firmware}),
            ),
            (
                "suite.json",
                serde_json::json!({"schema":2,"run_id":id,"outcome":outcome,"started_unix_millis":time,"scenarios":[{"scenario":"test"}]}),
            ),
            (
                "integrity.json",
                serde_json::json!({"schema":2,"run_id":id,"files":[{"size_bytes":10,"sha256":"a".repeat(64)}]}),
            ),
        ] {
            fs::write(dir.join(name), serde_json::to_vec(&value).unwrap()).unwrap();
        }
    }
    #[test]
    fn protects_failures_latest_replays_explicit_and_incomplete_without_deleting() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        for (id, outcome, time, replay) in [
            ("baseline", "passed", 1, None),
            ("old", "passed", 2, None),
            ("failed", "failed", 3, None),
            ("source", "passed", 4, None),
            ("latest", "passed", 5, Some("source")),
        ] {
            make(root, id, outcome, time, replay);
        }
        let active = root.join("target/hil/esp32s31/runs/active");
        fs::create_dir(&active).unwrap();
        let report = inspect(root, &["baseline".into()]).unwrap();
        let candidates: Vec<_> = report
            .runs
            .iter()
            .filter(|r| r.keep_reasons.is_empty())
            .map(|r| r.id.as_str())
            .collect();
        assert_eq!(candidates, ["old"]);
        assert_eq!(report.candidate_logical_bytes, 10);
        assert!(
            root.join("target/hil/esp32s31/runs/old/integrity.json")
                .is_file()
        );
        assert!(inspect(root, &["missing".into()]).is_err());
        assert!(inspect(root, &["../old".into()]).is_err());
    }
}
