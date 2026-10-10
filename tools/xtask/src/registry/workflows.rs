//! Declarative CI jobs and tool versions, owned by the check registry.
//! This catalog has no dependency on CI planning or transport.

use super::{Tier, of_job};
use crate::Result;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    str::FromStr,
};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Workflow {
    Ci,
    Docs,
}

impl Workflow {
    pub const ALL: [Self; 2] = [Self::Ci, Self::Docs];

    pub const fn name(self) -> &'static str {
        match self {
            Self::Ci => "ci",
            Self::Docs => "docs",
        }
    }

    pub fn spec(self) -> &'static WorkflowSpec {
        match self {
            Self::Ci => &CI,
            Self::Docs => &DOCS,
        }
    }
}

impl fmt::Display for Workflow {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name())
    }
}

impl FromStr for Workflow {
    type Err = String;
    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|workflow| workflow.name() == value)
            .ok_or_else(|| "workflow must be ci or docs".to_owned())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Program {
    pub id: &'static str,
    pub program: &'static str,
    pub arguments: &'static [&'static str],
}

#[derive(Clone, Copy, Debug)]
pub struct JobSpec {
    pub id: &'static str,
    pub programs: &'static [Program],
}

impl JobSpec {
    pub fn checks(self) -> Vec<String> {
        of_job(Tier::Full, self.id)
            .iter()
            .map(|check| check.id.to_owned())
            .collect()
    }
}

#[derive(Clone, Copy, Debug)]
pub struct WorkflowSpec {
    pub workflow: Workflow,
    pub preparation: &'static str,
    pub jobs: &'static [JobSpec],
}

impl WorkflowSpec {
    pub fn programs(self) -> Result<Vec<Program>> {
        let mut programs = BTreeMap::new();
        for program in self.jobs.iter().flat_map(|job| job.programs) {
            if programs
                .insert(program.id, *program)
                .is_some_and(|previous| previous != *program)
            {
                return Err(
                    format!("conflicting CI program declarations for {}", program.id).into(),
                );
            }
        }
        Ok(programs.into_values().collect())
    }

    pub fn job(self, id: &str) -> Result<&'static JobSpec> {
        self.jobs
            .iter()
            .find(|job| job.id == id)
            .ok_or_else(|| format!("unknown {} job {id}", self.workflow).into())
    }

    pub fn validate(self) -> Result<()> {
        let mut ids = BTreeSet::from([self.preparation]);
        for job in self.jobs {
            if !ids.insert(job.id) {
                return Err("duplicate CI job ID".into());
            }
        }
        let mut outputs = BTreeSet::new();
        if ids.iter().any(|id| !outputs.insert(id.replace('-', "_"))) {
            return Err("CI IDs produce conflicting workflow output names".into());
        }
        self.programs()?;
        Ok(())
    }
}

const CONFORMANCE_PROGRAMS: &[Program] = &[
    Program {
        id: "clang",
        program: "clang",
        arguments: &["--version"],
    },
    Program {
        id: "lld",
        program: "ld.lld",
        arguments: &["--version"],
    },
];

const fn tree_job(id: &'static str) -> JobSpec {
    JobSpec { id, programs: &[] }
}

const CI: WorkflowSpec = WorkflowSpec {
    workflow: Workflow::Ci,
    preparation: "prepare",
    // Jobs run in parallel and a pull request waits for the longest, so the
    // longest checks get jobs of their own.
    jobs: &[
        tree_job("host"),
        tree_job("host-lint"),
        tree_job("architecture"),
        tree_job("architecture-clippy"),
        tree_job("firmware"),
        tree_job("models"),
        JobSpec {
            programs: CONFORMANCE_PROGRAMS,
            ..tree_job("isa-conformance")
        },
        tree_job("images"),
        tree_job("images-correctness"),
        tree_job("example-link"),
        tree_job("verification"),
    ],
};

const DOCS: WorkflowSpec = WorkflowSpec {
    workflow: Workflow::Docs,
    preparation: "prepare",
    jobs: &[tree_job("docs")],
};

/// A new full-tier check job must get a workflow declaration; it cannot
/// silently disappear behind a green gate.
pub fn validate_workflows() -> Result<()> {
    let mut declared = BTreeSet::new();
    for workflow in Workflow::ALL {
        let spec = workflow.spec();
        spec.validate()?;
        for job in spec.jobs {
            if job.checks().is_empty() || !declared.insert(job.id) {
                return Err("CI workflow jobs must own checks exactly once".into());
            }
        }
    }
    let expected: BTreeSet<_> = super::jobs()
        .into_iter()
        .filter(|(_, tiers)| tiers.iter().any(|tier| *tier <= Tier::Full))
        .map(|(job, _)| job)
        .collect();
    if declared != expected {
        return Err("CI workflow declarations do not cover the check registry".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declarations_cover_all_full_tier_checks() {
        validate_workflows().unwrap();
        assert_eq!(CI.programs().unwrap(), CONFORMANCE_PROGRAMS);
        assert!(DOCS.programs().unwrap().is_empty());
    }

    #[test]
    fn ambiguous_names_and_programs_are_rejected() {
        const COLLISION: WorkflowSpec = WorkflowSpec {
            jobs: &[tree_job("a-b"), tree_job("a_b")],
            ..DOCS
        };
        assert!(COLLISION.validate().is_err());
        const DUPLICATE: WorkflowSpec = WorkflowSpec {
            jobs: &[tree_job("prepare")],
            ..DOCS
        };
        assert!(DUPLICATE.validate().is_err());
        const PROGRAMS: WorkflowSpec = WorkflowSpec {
            jobs: &[
                JobSpec {
                    programs: CONFORMANCE_PROGRAMS,
                    ..tree_job("a")
                },
                JobSpec {
                    programs: &[Program {
                        id: "clang",
                        program: "other-clang",
                        arguments: &["--version"],
                    }],
                    ..tree_job("b")
                },
            ],
            ..DOCS
        };
        assert!(PROGRAMS.validate().is_err());
    }
}
