//! Job identities from a complete Git tree and actual tool versions.
//! The planner does not inspect Cargo graphs or choose narrower boundaries.

use super::model::{Environment, Job, Manifest, SCHEMA};
use crate::{
    Result,
    registry::{self, Program, Workflow, WorkflowSpec},
};
use std::{collections::BTreeMap, path::Path};

fn versions<'a>(
    environment: &'a Environment,
    programs: &[Program],
) -> Result<BTreeMap<&'static str, &'a str>> {
    programs
        .iter()
        .map(|program| {
            environment
                .programs
                .get(program.id)
                .map(|version| (program.id, version.as_str()))
                .ok_or_else(|| {
                    format!("CI environment lacks declared program {}", program.id).into()
                })
        })
        .collect()
}

/// A decoded plan must describe exactly the checks owned by the registry.
pub(super) fn validate_manifest(manifest: &Manifest, spec: &WorkflowSpec) -> Result<()> {
    spec.validate()?;
    let expected: BTreeMap<_, _> = spec.jobs.iter().map(|job| (job.id, job)).collect();
    if manifest.schema != SCHEMA
        || manifest.workflow != spec.workflow
        || manifest.preparation != spec.preparation
        || manifest
            .jobs
            .keys()
            .map(String::as_str)
            .ne(expected.keys().copied())
        || expected
            .iter()
            .any(|(id, job)| manifest.jobs[*id].checks != job.checks())
    {
        return Err("CI plan does not cover the current workflow declarations and checks".into());
    }
    Ok(())
}

pub fn manifest(root: &Path, workflow: Workflow, environment: Environment) -> Result<Manifest> {
    registry::validate_workflows()?;
    let snapshot = oer_repo::index::IndexSnapshot::read(root)?;
    if snapshot.dirty {
        return Err("CI input planning requires a clean checkout".into());
    }
    let spec = workflow.spec();
    let tree = oer_process::git::text(root, ["rev-parse", "HEAD^{tree}"])?;
    let jobs = spec
        .jobs
        .iter()
        .map(|job| {
            let checks = job.checks();
            let key = oer_durable::sha256_bytes(&serde_json::to_vec(&serde_json::json!({
                "schema": SCHEMA, "workflow": workflow, "tree": tree,
                "job": job.id, "checks": checks, "common": environment.common,
                "programs": versions(&environment, job.programs)?,
            }))?);
            Ok((
                job.id.to_owned(),
                Job {
                    key,
                    checks,
                    inputs: snapshot.tracked.len(),
                },
            ))
        })
        .collect::<Result<_>>()?;
    Ok(Manifest {
        schema: SCHEMA,
        workflow,
        preparation: spec.preparation.to_owned(),
        commit: snapshot.commit,
        tree,
        environment,
        jobs,
    })
}

pub fn plan(root: &Path, workflow: Workflow, environment: &Path, output: &Path) -> Result<()> {
    oer_durable::atomic_json(
        output,
        &manifest(
            root,
            workflow,
            serde_json::from_slice(&std::fs::read(environment)?)?,
        )?,
    )
}

#[cfg(test)]
mod tests;
