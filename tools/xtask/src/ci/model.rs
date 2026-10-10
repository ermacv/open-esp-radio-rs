//! Serializable CI contracts, independent of runner state and GitHub APIs.

use crate::registry::Workflow;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, str::FromStr};

pub const SCHEMA: u32 = 4;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CommonEnvironment {
    pub image: String,
    pub os: String,
    pub arch: String,
    pub rustc: String,
    pub cargo: String,
    pub flags: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Environment {
    pub common: CommonEnvironment,
    pub programs: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Job {
    pub key: String,
    pub checks: Vec<String>,
    pub inputs: usize,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Manifest {
    pub schema: u32,
    pub workflow: Workflow,
    pub preparation: String,
    pub commit: String,
    pub tree: String,
    pub environment: Environment,
    pub jobs: BTreeMap<String, Job>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Full,
    Observe,
    Reuse,
}

impl FromStr for Mode {
    type Err = String;
    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        match value {
            "full" => Ok(Self::Full),
            "observe" => Ok(Self::Observe),
            "reuse" => Ok(Self::Reuse),
            _ => Err("CI_REUSE_MODE must be full, observe or reuse".to_owned()),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Record {
    #[serde(flatten)]
    pub inputs: Job,
    pub verified_at: u64,
    pub run_id: u64,
}

/// What a job does in a run. It runs all its checks (`checks: None`), only
/// those of them a change reaches (`Some`, non-empty), is skipped because a
/// successful run with the same inputs covers it (`source`), or is skipped
/// because the change reaches none of its checks (`Some`, empty).
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Action {
    pub run: bool,
    pub source: Option<Record>,
    #[serde(default)]
    pub checks: Option<Vec<String>>,
}

impl Action {
    /// Skipped because the change reaches none of the job's checks.
    pub fn unaffected(&self) -> bool {
        !self.run && self.source.is_none() && self.checks.as_ref().is_some_and(Vec::is_empty)
    }
}

/// The change a branch run is scoped to: its merge base with `main` and the
/// checks of each job it reaches. `main` always runs whole.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Scope {
    pub base: String,
    pub checks: BTreeMap<String, Vec<String>>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Plan {
    #[serde(flatten)]
    pub manifest: Manifest,
    pub run_id: u64,
    pub mode: Mode,
    pub actions: BTreeMap<String, Action>,
    /// The merge base a change-scoped plan selected checks against.
    #[serde(default)]
    pub change_base: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Proof {
    pub schema: u32,
    pub workflow: Workflow,
    pub run_id: u64,
    pub attempt: u64,
    pub commit: String,
    pub tree: String,
    pub jobs: BTreeMap<String, Record>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum JobResult {
    Success,
    Failure,
    Cancelled,
    Skipped,
}

#[derive(Deserialize)]
pub struct Need {
    pub result: JobResult,
    /// The executing job's environment qualified its planned input identity.
    pub reusable: bool,
}
