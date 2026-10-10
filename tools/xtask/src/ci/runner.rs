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

/// The branch `main`, which every run checks whole.
const MAIN: &str = "refs/heads/main";

/// The root workspace's manifest and lock: its lint policy
/// (`[workspace.lints]`), `[patch]` pins and shared dependencies reach chip
/// code that only path triggers select.
const ROOT_WORKSPACE: &[&str] = &["Cargo.toml", "Cargo.lock"];

/// A changed file no check's trigger can answer for: the workflows, whose
/// job setup (caches, tools, versions) every check runs under, the toolchain
/// and lint configuration every build reads, and the root workspace's
/// manifest and lock. A change to one runs every check.
fn unscopable(files: &[String]) -> Option<&String> {
    files.iter().find(|file| {
        file.starts_with(".github/workflows/")
            || crate::gate::GLOBAL.contains(&file.as_str())
            || ROOT_WORKSPACE.contains(&file.as_str())
    })
}

/// The checks of each job of `workflow` that the change from the merge base
/// of the run's commit with `main` to the checkout reaches. The checkout is
/// shallow, so GitHub names the merge base and only that commit is fetched.
fn change_scope(root: &Path, api: &GitHub, workflow: Workflow) -> Result<Scope> {
    #[derive(serde::Deserialize)]
    struct Commit {
        sha: String,
    }
    #[derive(serde::Deserialize)]
    struct Compare {
        merge_base_commit: Commit,
    }
    let base = api
        .query::<Compare>(&format!("compare/main...{}", api.commit))?
        .merge_base_commit
        .sha;
    oer_process::git::output(
        root,
        [
            "fetch",
            "--quiet",
            "--no-tags",
            "--depth=1",
            "origin",
            &base,
        ],
    )?;
    let ctx = oer_process::Checkout::new(root)?;
    let files = crate::gate::committed(&ctx, &base)?;
    if let Some(file) = unscopable(&files) {
        return Err(format!("{file} changes how CI itself runs").into());
    }
    let change = crate::gate::Change::of(&ctx, files, &base, Some("HEAD"), registry::Tier::Full)?;
    if let Some(package) = registry::check_tooling(&change) {
        return Err(format!(
            "the change reaches {}, which the checks run with",
            package.name
        )
        .into());
    }
    let checks = workflow
        .spec()
        .jobs
        .iter()
        .map(|job| {
            let ids = registry::of_job_for(&change, job.id)
                .iter()
                .map(|check| check.id.to_owned())
                .collect();
            (job.id.to_owned(), ids)
        })
        .collect();
    Ok(Scope { base, checks })
}

/// What a job does, as the run summary says it: which checks it runs when
/// a change selects some, and whether it could have reused coverage.
fn decision(action: &Action, mode: Mode) -> String {
    if action.unaffected() {
        return "skip (change reaches no check)".to_owned();
    }
    if !action.run {
        return "reuse".to_owned();
    }
    let run = match &action.checks {
        Some(checks) => format!("run {}", checks.join(", ")),
        None => "run".to_owned(),
    };
    if action.source.is_some() && mode == Mode::Observe {
        format!("{run} (would reuse)")
    } else {
        run
    }
}

fn summary(plan: &Plan, repository: &str) -> String {
    let mut text = format!(
        "## CI coverage\n\nMode: `{:?}`; tree: `{}`; {}.\n\n| Job | Action | Successful verification |\n| --- | --- | --- |\n",
        plan.mode,
        plan.manifest.tree,
        plan.change_base.as_ref().map_or_else(
            || "every check".to_owned(),
            |base| format!("checks the change from `{base}` reaches")
        )
    );
    for (job, action) in &plan.actions {
        let decision = decision(action, plan.mode);
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

/// `run_<job>` and `checks_<job>` (`all` or the comma-separated checks to
/// pass to `check tier --checks`) for each job.
fn outputs(plan: &Plan) -> String {
    let mut text = String::new();
    for (job, action) in &plan.actions {
        let name = job.replace('-', "_");
        let checks = action
            .checks
            .as_ref()
            .map_or_else(|| "all".to_owned(), |checks| checks.join(","));
        text.push_str(&format!(
            "run_{name}={}\nchecks_{name}={checks}\n",
            action.run
        ));
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
    // A branch push skips the audited checks its change does not reach;
    // `main`, a manual run and any failure to tell the change run everything.
    let scope = (std::env::var("GITHUB_EVENT_NAME")? == "push"
        && std::env::var("GITHUB_REF")? != MAIN)
        .then(|| {
            change_scope(root, &api, workflow)
                .inspect_err(|error| {
                    eprintln!("CI: the change is unknown ({error}); every check will run");
                })
                .ok()
        })
        .flatten();
    let plan = coverage::select(
        manifest,
        api.run_id,
        &proofs,
        mode,
        oer_durable::unix_seconds(),
        scope.as_ref(),
    )?;
    oer_durable::atomic_json(output, &plan)?;
    append("GITHUB_OUTPUT", &outputs(&plan))?;
    append("GITHUB_STEP_SUMMARY", &summary(&plan, &api.repository))
}

pub fn verify(root: &Path, workflow: Workflow, output: &Path) -> Result<()> {
    let api = GitHub::current(root)?;
    let plan = load_plan(&api, workflow)?;
    if plan.change_base.is_some() && std::env::var("GITHUB_REF")? == MAIN {
        return Err("a run on main must check everything, not a change".into());
    }
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
    fn a_change_to_ci_or_the_toolchain_runs_every_check() {
        let files = |paths: &[&str]| {
            paths
                .iter()
                .map(|path| (*path).to_owned())
                .collect::<Vec<_>>()
        };
        // Another workspace's manifest selects its packages, so it scopes.
        let scoped = files(&[
            "tools/xtask/src/ci.rs",
            "docs/index.md",
            "tools/blobray/Cargo.toml",
        ]);
        assert!(unscopable(&scoped).is_none());
        for path in [
            ".github/workflows/ci.yml",
            "rust-toolchain.toml",
            "clippy.toml",
            "Cargo.toml",
            "Cargo.lock",
        ] {
            assert_eq!(
                unscopable(&files(&["hil/a.rs", path])).map(String::as_str),
                Some(path)
            );
        }
    }

    #[test]
    fn the_summary_names_the_checks_a_change_runs_in_every_mode() {
        let record = Record {
            inputs: Job {
                key: "k".into(),
                checks: vec!["a".into(), "b".into()],
                inputs: 1,
            },
            verified_at: 1,
            run_id: 1,
        };
        let action = |source: Option<Record>, checks: Option<Vec<String>>| Action {
            run: true,
            source,
            checks,
        };
        let subset = Some(vec!["b".to_owned()]);
        assert_eq!(
            decision(&action(None, subset.clone()), Mode::Reuse),
            "run b"
        );
        assert_eq!(
            decision(&action(Some(record.clone()), subset), Mode::Observe),
            "run b (would reuse)"
        );
        assert_eq!(
            decision(&action(Some(record.clone()), None), Mode::Observe),
            "run (would reuse)"
        );
        let skipped = Action {
            run: false,
            source: None,
            checks: Some(Vec::new()),
        };
        assert_eq!(
            decision(&skipped, Mode::Observe),
            "skip (change reaches no check)"
        );
        let reused = Action {
            run: false,
            source: Some(record),
            checks: None,
        };
        assert_eq!(decision(&reused, Mode::Reuse), "reuse");
    }

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
