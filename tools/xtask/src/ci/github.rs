//! Read-only GitHub transport through the runner's authenticated `gh`.

use crate::Result;
use crate::registry::Workflow;
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Deserialize)]
pub(super) struct Repository {
    pub full_name: String,
}

#[derive(Deserialize)]
pub(super) struct Run {
    pub id: u64,
    pub head_sha: String,
    pub run_attempt: u64,
    pub status: String,
    pub conclusion: Option<String>,
    pub event: String,
    pub head_repository: Option<Repository>,
}

impl Run {
    pub fn eligible(&self, repository: &str) -> bool {
        self.status == "completed"
            && self.conclusion.as_deref() == Some("success")
            && self.event == "push"
            && self
                .head_repository
                .as_ref()
                .is_some_and(|head| head.full_name == repository)
    }
}

#[derive(Deserialize)]
pub(super) struct Artifact {
    pub name: String,
    pub expired: bool,
}

pub(super) struct GitHub {
    pub repository: String,
    pub run_id: u64,
    pub attempt: u64,
    pub commit: String,
    root: PathBuf,
}

impl GitHub {
    pub fn current(root: &Path) -> Result<Self> {
        let repository = std::env::var("GITHUB_REPOSITORY")?;
        let parts: Vec<_> = repository.split('/').collect();
        if parts.len() != 2
            || parts.iter().any(|part| {
                part.is_empty()
                    || !part
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || b"_.-".contains(&byte))
            })
        {
            return Err("invalid GITHUB_REPOSITORY".into());
        }
        Ok(Self {
            repository,
            run_id: std::env::var("GITHUB_RUN_ID")?.parse()?,
            attempt: std::env::var("GITHUB_RUN_ATTEMPT")?.parse()?,
            commit: std::env::var("GITHUB_SHA")?,
            root: root.to_owned(),
        })
    }

    pub fn query<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T> {
        let output = oer_process::capture(
            std::process::Command::new("gh")
                .current_dir(&self.root)
                .args(["api", &format!("repos/{}/{path}", self.repository)]),
        )?;
        Ok(serde_json::from_slice(&output.stdout)?)
    }

    pub fn artifacts(&self, run: u64) -> Result<Vec<Artifact>> {
        #[derive(Deserialize)]
        struct Response {
            artifacts: Vec<Artifact>,
        }
        Ok(self
            .query::<Response>(&format!("actions/runs/{run}/artifacts?per_page=100"))?
            .artifacts)
    }

    pub fn download(&self, run: u64, name: &str, directory: &Path) -> Result<()> {
        // `gh` handles the artifact redirect without exposing the token or
        // passing it to the signed storage URL. No shell or archive commands.
        oer_process::capture(
            std::process::Command::new("gh")
                .current_dir(&self.root)
                .args([
                    "run",
                    "download",
                    &run.to_string(),
                    "--repo",
                    &self.repository,
                    "--name",
                    name,
                    "--dir",
                ])
                .arg(directory),
        )?;
        Ok(())
    }

    pub fn document<T: serde::de::DeserializeOwned>(
        &self,
        run: u64,
        name: &str,
        file: &str,
    ) -> Result<T> {
        let directory = tempfile::tempdir()?;
        self.download(run, name, directory.path())?;
        let path = directory.path().join(file);
        if std::fs::metadata(&path)?.len() > 8 * 1024 * 1024 {
            return Err("CI document exceeds size limit".into());
        }
        Ok(serde_json::from_slice(&std::fs::read(path)?)?)
    }

    pub fn runs(&self, workflow: Workflow) -> Result<Vec<Run>> {
        #[derive(Deserialize)]
        struct Response {
            workflow_runs: Vec<Run>,
        }
        Ok(self
            .query::<Response>(&format!(
                "actions/workflows/{workflow}.yml/runs?status=success&event=push&per_page=30"
            ))?
            .workflow_runs
            .into_iter()
            .filter(|run| run.eligible(&self.repository))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evidence_requires_a_completed_successful_push_in_this_repository() {
        let run = serde_json::json!({
            "id": 10, "head_sha": "commit", "run_attempt": 1,
            "status": "completed", "conclusion": "success", "event": "push",
            "head_repository": {"full_name": "owner/repo"},
        });
        let parse = |value| serde_json::from_value::<Run>(value).unwrap();
        assert!(parse(run.clone()).eligible("owner/repo"));
        assert!(!parse(run.clone()).eligible("fork/repo"));
        for (field, value) in [
            ("status", serde_json::json!("in_progress")),
            ("conclusion", serde_json::json!("failure")),
            ("conclusion", serde_json::json!("cancelled")),
            ("conclusion", serde_json::Value::Null),
            ("event", serde_json::json!("pull_request")),
            ("event", serde_json::json!("workflow_dispatch")),
            ("head_repository", serde_json::Value::Null),
        ] {
            let mut changed = run.clone();
            changed[field] = value;
            assert!(!parse(changed).eligible("owner/repo"), "{field}");
        }
    }
}
