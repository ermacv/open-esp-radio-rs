//! Pure reuse and gate rules. All evidence, results and time are explicit
//! arguments; no environment, filesystem, clock or provider calls occur.

use super::model::*;
use crate::{Result, registry::Workflow};
use std::collections::{BTreeMap, BTreeSet};

const MAX_AGE: u64 = 7 * 24 * 60 * 60;

fn covers(record: &Record, inputs: &Job, at: u64) -> bool {
    record.inputs == *inputs
        && record.run_id > 0
        && at >= record.verified_at
        && at - record.verified_at <= MAX_AGE
}

pub fn belongs_to(
    proof: &Proof,
    workflow: Workflow,
    run_id: u64,
    attempt: u64,
    commit: &str,
) -> bool {
    proof.schema == SCHEMA
        && proof.workflow == workflow
        && proof.run_id == run_id
        && proof.attempt == attempt
        && proof.commit == commit
}

/// Reject ambiguous or incomplete topology before using it to skip work.
fn validate(manifest: &Manifest) -> Result<()> {
    let mut ids = BTreeSet::from([manifest.preparation.as_str()]);
    if manifest.schema != SCHEMA || manifest.jobs.is_empty() {
        return Err("invalid CI manifest schema or jobs".into());
    }
    for id in manifest.jobs.keys() {
        if !ids.insert(id) {
            return Err("duplicate CI preparation or check job".into());
        }
    }
    Ok(())
}

/// The plan: each job is reused when a proof covers its inputs (in `Reuse`
/// mode); otherwise, for a change-`scope`d branch run, it runs the checks the
/// change reaches, or is skipped when it reaches none; otherwise it runs all.
pub fn select(
    manifest: Manifest,
    run_id: u64,
    proofs: &[Proof],
    mode: Mode,
    at: u64,
    scope: Option<&Scope>,
) -> Result<Plan> {
    validate(&manifest)?;
    let actions = manifest
        .jobs
        .iter()
        .map(|(job, inputs)| {
            let source = (mode != Mode::Full)
                .then(|| {
                    proofs
                        .iter()
                        .filter(|proof| {
                            proof.schema == SCHEMA && proof.workflow == manifest.workflow
                        })
                        .filter_map(|proof| proof.jobs.get(job))
                        .find(|record| covers(record, inputs, at))
                        .cloned()
                })
                .flatten();
            let reused = mode == Mode::Reuse && source.is_some();
            let action = match scope.and_then(|scope| scope.checks.get(job)) {
                Some(checks) if !reused && checks.is_empty() => Action {
                    run: false,
                    source: None,
                    checks: Some(Vec::new()),
                },
                Some(checks) if !reused && *checks != inputs.checks => Action {
                    run: true,
                    source,
                    checks: Some(checks.clone()),
                },
                _ => Action {
                    run: !reused,
                    source,
                    checks: None,
                },
            };
            (job.clone(), action)
        })
        .collect();
    Ok(Plan {
        manifest,
        run_id,
        mode,
        actions,
        change_base: scope.map(|scope| scope.base.clone()),
    })
}

/// A decoded plan must still schedule each declared job exactly once, and
/// select checks or skip a job by change only when it is change-scoped.
pub(super) fn validate_plan(plan: &Plan) -> Result<()> {
    validate(&plan.manifest)?;
    if plan.actions.keys().ne(plan.manifest.jobs.keys()) {
        return Err("CI actions do not cover the manifest".into());
    }
    for (job, action) in &plan.actions {
        let Some(checks) = &action.checks else {
            continue;
        };
        let declared = &plan.manifest.jobs[job].checks;
        if plan.change_base.is_none()
            || action.run == checks.is_empty()
            || (!action.run && action.source.is_some())
            || checks.iter().any(|check| !declared.contains(check))
        {
            return Err(format!("{job} has an inconsistent change selection").into());
        }
    }
    Ok(())
}

pub fn finish(
    plan: &Plan,
    needs: &BTreeMap<String, Need>,
    run_id: u64,
    attempt: u64,
    at: u64,
) -> Result<Proof> {
    validate_plan(plan)?;
    let mut expected: BTreeSet<&str> = plan.manifest.jobs.keys().map(String::as_str).collect();
    expected.insert(&plan.manifest.preparation);
    if needs.keys().map(String::as_str).collect::<BTreeSet<_>>() != expected
        || needs[&plan.manifest.preparation].result != JobResult::Success
        || plan.run_id != run_id
    {
        return Err("CI jobs or successful preparation do not match the plan".into());
    }
    let mut jobs = BTreeMap::new();
    for (job, inputs) in &plan.manifest.jobs {
        let action = &plan.actions[job];
        let record = if action.run {
            if needs[job].result != JobResult::Success {
                return Err(
                    format!("{job} must run successfully; got {:?}", needs[job].result).into(),
                );
            }
            // Success satisfies the gate even during a runner-image rollout.
            // Only a qualified execution of every check establishes
            // reusable coverage for the input identities computed by
            // preparation.
            if !needs[job].reusable || action.checks.is_some() {
                continue;
            }
            Record {
                inputs: inputs.clone(),
                verified_at: at,
                run_id,
            }
        } else if action.unaffected() {
            // The change reaches none of the job's checks: nothing to prove,
            // and nothing to reuse later. `main` is never change-scoped.
            if plan.change_base.is_none() || needs[job].result != JobResult::Skipped {
                return Err(format!("{job} has an unexpected skip or result").into());
            }
            continue;
        } else {
            if plan.mode != Mode::Reuse || needs[job].result != JobResult::Skipped {
                return Err(format!("{job} has an unexpected skip or result").into());
            }
            action
                .source
                .as_ref()
                .filter(|record| covers(record, inputs, at))
                .ok_or_else(|| format!("{job} has no valid coverage for its skipped result"))?
                .clone()
        };
        // Reuse preserves the original verification date and cannot renew TTL.
        jobs.insert(job.clone(), record);
    }
    Ok(Proof {
        schema: SCHEMA,
        workflow: plan.manifest.workflow,
        run_id,
        attempt,
        commit: plan.manifest.commit.clone(),
        tree: plan.manifest.tree.clone(),
        jobs,
    })
}

#[cfg(test)]
mod tests;
