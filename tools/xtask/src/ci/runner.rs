//! GitHub runner adapter: obtain evidence and results, call the independent
//! planner/gate, and write workflow outputs. No tool-specific branches.

use super::{coverage, environment, github::GitHub, model::*, planning};
use crate::{
    Result,
    registry::{self, Workflow},
};
use std::{collections::BTreeMap, io::Write as _, path::Path};

fn append(variable: &str, text: &str) -> Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(std::env::var(variable)?)?;
    file.write_all(text.as_bytes())?;
    Ok(())
}

fn load_plan(api: &GitHub, workflow: Workflow) -> Result<Plan> {
    let plan: Plan = api.document(api.run_id, "ci-plan", "ci-plan.json")?;
    if plan.manifest.schema != SCHEMA
        || plan.run_id != api.run_id
        || plan.manifest.commit != api.commit
        || plan.manifest.workflow != workflow
    {
        return Err("CI plan does not belong to this run, workflow and commit".into());
    }
    registry::validate_workflows()?;
    planning::validate_manifest(&plan.manifest, workflow.spec())?;
    coverage::validate_plan(&plan)?;
    Ok(plan)
}

fn load_proofs(api: &GitHub, workflow: Workflow) -> Result<Vec<Proof>> {
    let mut proofs = Vec::new();
    for run in api.runs(workflow)? {
        if !api
            .artifacts(run.id)?
            .iter()
            .any(|artifact| artifact.name == "ci-proof" && !artifact.expired)
        {
            continue;
        }
        match api.document::<Proof>(run.id, "ci-proof", "ci-proof.json") {
            Ok(proof)
                if coverage::belongs_to(
                    &proof,
                    workflow,
                    run.id,
                    run.run_attempt,
                    &run.head_sha,
                ) =>
            {
                proofs.push(proof)
            }
            _ => eprintln!("CI: ignoring invalid proof from run {}", run.id),
        }
    }
    Ok(proofs)
}

fn summary(plan: &Plan, repository: &str) -> String {
    let mut text = format!(
        "## CI coverage\n\nMode: `{:?}`; tree: `{}`.\n\n| Job | Action | Successful verification |\n| --- | --- | --- |\n",
        plan.mode, plan.manifest.tree
    );
    for (job, action) in &plan.actions {
        let decision = match (action.run, action.source.is_some(), plan.mode) {
            (true, true, Mode::Observe) => "run (would reuse)",
            (true, _, _) => "run",
            _ => "reuse",
        };
        let source = action.source.as_ref().map_or_else(
            || "none".to_owned(),
            |source| {
                format!(
                    "[run {}](https://github.com/{repository}/actions/runs/{})",
                    source.run_id, source.run_id
                )
            },
        );
        text.push_str(&format!("| {job} | {decision} | {source} |\n"));
    }
    text
}

fn outputs(plan: &Plan) -> String {
    let mut text = String::new();
    for (job, action) in &plan.actions {
        text.push_str(&format!("run_{}={}\n", job.replace('-', "_"), action.run));
    }
    text
}

pub fn prepare(root: &Path, workflow: Workflow, output: &Path) -> Result<()> {
    let api = GitHub::current(root)?;
    let manifest = planning::manifest(root, workflow, environment::read_workflow(root, workflow)?)?;
    if manifest.commit != api.commit {
        return Err("CI planning requires a clean checkout of GITHUB_SHA".into());
    }
    let mode = if std::env::var("GITHUB_EVENT_NAME")? == "push" {
        std::env::var("CI_REUSE_MODE")
            .unwrap_or_else(|_| "observe".to_owned())
            .parse()?
    } else {
        Mode::Full
    };
    let proofs = if mode == Mode::Full {
        Vec::new()
    } else {
        load_proofs(&api, workflow).unwrap_or_else(|_| {
            eprintln!("CI: evidence lookup unavailable; every job will execute");
            Vec::new()
        })
    };
    let plan = coverage::select(
        manifest,
        api.run_id,
        &proofs,
        mode,
        oer_durable::unix_seconds(),
    )?;
    oer_durable::atomic_json(output, &plan)?;
    append("GITHUB_OUTPUT", &outputs(&plan))?;
    append("GITHUB_STEP_SUMMARY", &summary(&plan, &api.repository))
}

pub fn verify(root: &Path, workflow: Workflow, output: &Path) -> Result<()> {
    let api = GitHub::current(root)?;
    let plan = load_plan(&api, workflow)?;
    let needs = needs(&std::env::var("CI_NEEDS")?)?;
    let proof = coverage::finish(
        &plan,
        &needs,
        api.run_id,
        api.attempt,
        oer_durable::unix_seconds(),
    )?;
    oer_durable::atomic_json(output, &proof)?;
    let mut text = summary(&plan, &api.repository);
    let unqualified: Vec<_> = plan
        .actions
        .iter()
        .filter(|(id, action)| action.run && !proof.jobs.contains_key(*id))
        .map(|(id, _)| id.as_str())
        .collect();
    if !unqualified.is_empty() {
        text.push_str(&format!(
            "\nExecuted without reusable coverage: {}.\n",
            unqualified.join(", ")
        ));
    }
    append("GITHUB_STEP_SUMMARY", &text)
}

fn needs(value: &str) -> Result<BTreeMap<String, Need>> {
    #[derive(serde::Deserialize)]
    struct Status {
        result: JobResult,
        outputs: Option<BTreeMap<String, String>>,
    }
    Ok(serde_json::from_str::<BTreeMap<String, Status>>(value)?
        .into_iter()
        .map(|(id, status)| {
            (
                id,
                Need {
                    result: status.result,
                    reusable: status
                        .outputs
                        .as_ref()
                        .and_then(|outputs| outputs.get("reusable"))
                        .is_some_and(|value| value == "true"),
                },
            )
        })
        .collect())
}

fn qualification(root: &Path, expected: &Environment, programs: &[registry::Program]) -> bool {
    let reusable = environment::check(root, expected, programs).unwrap_or_else(|error| {
        eprintln!("CI: could not qualify the execution environment: {error}");
        false
    });
    if !reusable {
        eprintln!("CI: execution continues without reusable coverage for the planned environment");
    }
    reusable
}

fn qualification_output(reusable: bool) -> Result<()> {
    append("GITHUB_OUTPUT", &format!("reusable={reusable}\n"))
}

pub fn check_environment(root: &Path, workflow: Workflow, job: &str) -> Result<()> {
    let api = GitHub::current(root)?;
    let spec = workflow.spec().job(job)?;
    let reusable = match load_plan(&api, workflow) {
        Ok(plan) => qualification(root, &plan.manifest.environment, spec.programs),
        Err(error) => {
            eprintln!("CI: cannot qualify execution without a valid input plan: {error}");
            false
        }
    };
    qualification_output(reusable)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_explicit_job_outputs_qualify_new_coverage() {
        for outputs in [
            serde_json::json!({}),
            serde_json::Value::Null,
            serde_json::json!({"reusable": "false"}),
            serde_json::json!({"reusable": "yes"}),
        ] {
            let value = serde_json::json!({"check": {"result": "success", "outputs": outputs}});
            let results = needs(&value.to_string()).unwrap();
            assert_eq!(results["check"].result, JobResult::Success);
            assert!(!results["check"].reusable);
        }
        assert!(!needs(r#"{"check":{"result":"success"}}"#).unwrap()["check"].reusable);
        assert!(needs(r#"{"check":{"result":"success","outputs":{"reusable":"true"}}}"#)
            .unwrap()["check"].reusable);
    }
}
