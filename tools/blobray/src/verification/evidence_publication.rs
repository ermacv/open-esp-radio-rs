//! Incremental suite evidence: immutable history and an atomic current index.
use super::VendorEvidenceIndex;
use crate::{Result, application::generated_file};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
};

pub(crate) struct Publication {
    path: PathBuf,
    project: String,
    lock: fs::File,
}
impl Drop for Publication {
    fn drop(&mut self) {
        let _ = self.lock.unlock();
    }
}
impl Publication {
    pub(crate) fn acquire(path: &Path, project: &str) -> Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let lock = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path.with_extension("lock"))?;
        lock.lock()?;
        Ok(Self {
            path: path.into(),
            project: project.into(),
            lock,
        })
    }
    pub(crate) fn begin(&self, suite: &str) -> Result<()> {
        self.update(suite, None)
    }
    pub(crate) fn complete(&self, suite: &str, index: &VendorEvidenceIndex) -> Result<()> {
        self.update(suite, Some(serde_json::to_value(index)?))
    }
    pub(crate) fn check(path: &Path, expected: &VendorEvidenceIndex) -> Result<()> {
        let actual: Value = serde_json::from_slice(&fs::read(path)?)?;
        if actual["project"] != expected.project
            || actual["command"] != "project verify vendor evidence index"
        {
            return Err(crate::Error::invalid(
                "vendor evidence index identity changed",
            ));
        }
        let selected = &expected.suite_states;
        if actual["schema_version"] != 2
            || selected
                .keys()
                .any(|s| actual["suite_states"][s] != "complete")
        {
            return Err(crate::Error::invalid(
                "selected vendor suite has no completed current publication",
            ));
        }
        let entries = actual["entries"]
            .as_array()
            .ok_or_else(|| crate::Error::invalid("invalid vendor entries"))?
            .iter()
            .filter(|e| {
                e["suite"]
                    .as_str()
                    .is_some_and(|s| selected.contains_key(s))
            })
            .cloned()
            .collect::<Vec<_>>();
        if Value::Array(entries) != serde_json::to_value(&expected.entries)? {
            return Err(crate::Error::invalid(
                "selected vendor evidence differs; rerun without --check",
            ));
        }
        Ok(())
    }
    fn update(&self, suite: &str, completed: Option<Value>) -> Result<()> {
        let mut index: Value = if self.path.exists() {
            serde_json::from_slice(&fs::read(&self.path)?)?
        } else {
            json!({"schema_version":2,"command":"project verify vendor evidence index","project":self.project,"complete_project_run":false,"entries":[],"suite_states":{}})
        };
        if !matches!(index["schema_version"].as_u64(), Some(1 | 2))
            || index["project"] != self.project
            || index["command"] != "project verify vendor evidence index"
        {
            return Err(crate::Error::invalid(
                "cannot merge an incompatible vendor evidence index",
            ));
        }
        if index["schema_version"] == 1 {
            if index["complete_project_run"] != true {
                return Err(crate::Error::invalid(
                    "incomplete legacy evidence cannot seed an incremental index",
                ));
            }
            let states = index["entries"]
                .as_array()
                .ok_or_else(|| crate::Error::invalid("invalid vendor entries"))?
                .iter()
                .filter_map(|e| e["suite"].as_str())
                .map(|s| (s.to_owned(), json!("complete")))
                .collect::<serde_json::Map<_, _>>();
            index["suite_states"] = Value::Object(states);
        }
        if !index["suite_states"].is_object() || !index["entries"].is_array() {
            return Err(crate::Error::invalid("invalid incremental suite state"));
        }
        self.archive(&index)?;
        index["schema_version"] = json!(2);
        index["complete_project_run"] = json!(false);
        index["entries"]
            .as_array_mut()
            .ok_or_else(|| crate::Error::invalid("invalid vendor entries"))?
            .retain(|e| e["suite"] != suite);
        index["suite_states"][suite] = json!(if completed.is_some() {
            "complete"
        } else {
            "incomplete"
        });
        if let Some(completed) = completed {
            let entries = completed["entries"]
                .as_array()
                .ok_or_else(|| crate::Error::invalid("invalid completed suite index"))?;
            if completed["project"] != self.project || entries.iter().any(|e| e["suite"] != suite) {
                return Err(crate::Error::invalid(
                    "completed evidence does not belong to the selected suite",
                ));
            }
            index["entries"]
                .as_array_mut()
                .unwrap()
                .extend(entries.iter().cloned());
        }
        index["entries"].as_array_mut().unwrap().sort_by_key(|e| {
            (
                e["suite"].to_string(),
                e["source"].to_string(),
                e["symbol"].to_string(),
            )
        });
        self.archive(&index)?;
        generated_file::write_or_check_json(
            &self.path,
            &index,
            false,
            "incremental vendor evidence",
            true,
        )
    }
    fn archive(&self, index: &Value) -> Result<()> {
        let bytes = serde_json::to_vec(index)?;
        let hash = format!("{:x}", Sha256::digest(&bytes));
        let history = self
            .path
            .with_extension("history")
            .join(format!("{hash}.json"));
        generated_file::write_or_check_bytes(
            &history,
            &bytes,
            history.exists(),
            "vendor evidence history",
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn independent_suite_completion_preserves_history_and_replaces_old_pass_on_interruption() {
        let root = std::env::temp_dir().join(format!(
            "blobray-incremental-evidence-{}",
            std::process::id()
        ));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("index.json");
        let publication = Publication::acquire(&path, "test").unwrap();
        let index = |suite: &str, status: &str| json!({"project":"test","entries":[{"suite":suite,"source":"vendor","symbol":"entry","status":status}]});
        publication
            .update("wifi", Some(index("wifi", "match")))
            .unwrap();
        publication
            .update("ble", Some(index("ble", "diff")))
            .unwrap();
        let before: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(before["entries"].as_array().unwrap().len(), 2);
        publication.begin("ble").unwrap();
        let during: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(during["entries"].as_array().unwrap().len(), 1);
        assert_eq!(during["entries"][0]["suite"], "wifi");
        assert_eq!(during["suite_states"]["ble"], "incomplete");
        let historical = fs::read_dir(path.with_extension("history"))
            .unwrap()
            .map(|e| fs::read_to_string(e.unwrap().path()).unwrap())
            .collect::<Vec<_>>();
        assert!(historical.iter().any(|s| s.contains("diff")));
        publication.begin("wifi").unwrap();
        let after: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert!(after["entries"].as_array().unwrap().is_empty());
        drop(publication);
        fs::remove_dir_all(root).unwrap();
    }
}
