use super::*;

/// Every `cargo xtask ... check tier TIER --job JOB` of the workflows, with
/// the file that runs it.
fn workflow_calls() -> Vec<(String, Tier, String)> {
    let root = oer_process::built_root();
    let mut calls = Vec::new();
    for entry in std::fs::read_dir(root.join(".github/workflows")).unwrap() {
        let path = entry.unwrap().path();
        let text = std::fs::read_to_string(&path).unwrap();
        for line in text
            .lines()
            .filter(|line| !line.trim_start().starts_with('#'))
        {
            let words: Vec<&str> = line.split_whitespace().collect();
            let Some(at) = words.windows(2).position(|pair| pair == ["check", "tier"]) else {
                continue;
            };
            assert!(line.contains("cargo xtask"), "{line}");
            let tier: Tier = words[at + 2].parse().unwrap();
            let job = words
                .windows(2)
                .find(|pair| pair[0] == "--job")
                .map(|pair| pair[1].to_owned())
                .unwrap_or_else(|| panic!("{}: `{line}` names no --job", path.display()));
            calls.push((path.display().to_string(), tier, job));
        }
    }
    calls
}

#[test]
fn ids_are_unique_and_every_job_runs_a_check() {
    let ids: BTreeSet<&str> = CHECKS.iter().map(|check| check.id).collect();
    assert_eq!(ids.len(), CHECKS.len());
    for (job, tiers) in jobs() {
        assert!(!tiers.is_empty(), "{job}");
    }
}

#[test]
fn the_workflows_run_every_check_through_the_registry() {
    let calls = workflow_calls();
    for (file, tier, job) in &calls {
        assert!(
            !of_job(*tier, job).is_empty(),
            "{file} runs `check tier {tier} --job {job}`, which runs nothing"
        );
    }
    for check in CHECKS {
        assert!(
            calls
                .iter()
                .any(|(_, tier, job)| job == check.job && *tier >= check.tier),
            "no workflow runs check `{}` (job {}, tier {})",
            check.id,
            check.job,
            check.tier
        );
    }
}

#[test]
fn the_required_check_waits_for_every_ci_job() {
    let text = std::fs::read_to_string(oer_process::built_root().join(".github/workflows/ci.yml"))
        .unwrap();
    let jobs: BTreeSet<&str> = text
        .split_once("\njobs:\n")
        .unwrap()
        .1
        .lines()
        .filter_map(|line| line.strip_prefix("  ")?.strip_suffix(':'))
        .filter(|name| !name.starts_with(' ') && !name.starts_with('#'))
        .collect();
    let required = text.split_once("\n  ci-ok:\n").unwrap().1;
    let needs: BTreeSet<&str> = required
        .lines()
        .find_map(|line| line.trim().strip_prefix("needs: ["))
        .unwrap()
        .trim_end_matches(']')
        .split(',')
        .map(str::trim)
        .collect();
    let mut expected = jobs.clone();
    expected.remove("ci-ok");
    assert_eq!(needs, expected);
}

#[test]
fn each_workflow_job_reports_its_execution_qualification_to_the_gate() {
    for workflow in Workflow::ALL {
        let text = std::fs::read_to_string(
            oer_process::built_root().join(format!(".github/workflows/{workflow}.yml")),
        )
        .unwrap();
        for job in workflow.spec().jobs {
            let body = text.split_once(&format!("\n  {}:\n", job.id)).unwrap().1;
            let body = body
                .lines()
                .take_while(|line| !line.starts_with("  ") || line.starts_with("    "))
                .collect::<Vec<_>>()
                .join("\n");
            assert!(
                body.contains("reusable: ${{ steps.environment.outputs.reusable }}"),
                "{workflow}: {}",
                job.id
            );
            assert!(body.contains("- id: environment"), "{workflow}: {}", job.id);
            assert!(body.contains(&format!(
                "cargo xtask ci check-environment --workflow {workflow} --job {}",
                job.id
            )));
        }
    }
}

/// A change in a fixture tree of one host package and one chip package.
fn change(files: &[&str], tier: Tier) -> Change {
    let directory = tempfile::tempdir().unwrap();
    for (path, text) in [
        (
            "Cargo.toml",
            "[workspace]\nmembers = [\"tools/tool\", \"crates/driver\"]\n",
        ),
        (
            "tools/tool/Cargo.toml",
            "[package]\nname = \"tool\"\n[package.metadata.open-radio]\nlayer = \"tool\"\nplatform = \"host\"\nhost-layer = \"build\"\n",
        ),
        (
            "crates/driver/Cargo.toml",
            "[package]\nname = \"driver\"\n[package.metadata.open-radio]\nlayer = \"hardware\"\nplatform = \"chip\"\nchip = \"chip-a\"\n",
        ),
    ] {
        let path = directory.path().join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    let tree = gate::Tree::of(
        oer_repo::Model::load(&oer_repo::Repo::from_dir(directory.path()).unwrap()).unwrap(),
    );
    let files: Vec<String> = files.iter().map(|file| (*file).to_owned()).collect();
    let selection = gate::select(&tree, &files);
    let affected = gate::affected(&tree, &selection);
    Change {
        files,
        tree,
        selection,
        affected,
        tier,
    }
}

fn ids(checks: Vec<&Check>) -> Vec<&'static str> {
    checks.into_iter().map(|check| check.id).collect()
}

#[test]
fn a_host_source_change_runs_the_fast_host_checks() {
    assert_eq!(
        ids(of_change(&change(&["tools/tool/src/lib.rs"], Tier::Fast))),
        [
            "tidy",
            "fmt",
            "capabilities",
            "clippy",
            "test",
            "feature-sets"
        ]
    );
}

#[test]
fn chip_code_type_checks_images_and_full_adds_the_chip_audits() {
    let fast = ids(of_change(&change(
        &["crates/driver/src/lib.rs"],
        Tier::Fast,
    )));
    assert!(fast.contains(&"images-type-check"), "{fast:?}");
    assert!(!fast.contains(&"architecture"), "{fast:?}");
    let full = ids(of_change(&change(
        &["crates/driver/src/lib.rs"],
        Tier::Full,
    )));
    for id in [
        "images-type-check",
        "architecture",
        "examples-type-check",
        "provenance",
    ] {
        assert!(full.contains(&id), "{id} missing from {full:?}");
    }
}

#[test]
fn a_change_never_runs_a_whole_tier_check_or_one_above_its_tier() {
    let everything = change(
        &[
            "crates/driver/Cargo.toml",
            "platform/chip-a/chip.toml",
            "README.md",
        ],
        Tier::Full,
    );
    for check in of_change(&everything) {
        assert!(
            check.trigger.is_some() && check.tier <= Tier::Full,
            "{}",
            check.id
        );
    }
    let selected = ids(of_change(&everything));
    assert!(selected.contains(&"docs") && selected.contains(&"example-link"));
    assert!(!selected.contains(&"final-images") && !selected.contains(&"image-classes"));
}

#[test]
fn a_job_runs_its_checks_up_to_its_tier() {
    let full: Vec<&str> = ids(of_job(Tier::Full, "verification"));
    assert_eq!(full, ["provenance"]);
    let nightly: Vec<&str> = ids(of_job(Tier::Nightly, "verification"));
    assert_eq!(
        nightly,
        ["provenance", "vendor-probes", "host-stands", "verification"]
    );
    assert!(of_job(Tier::Full, "no-such-job").is_empty());
    assert_eq!("nightly".parse::<Tier>(), Ok(Tier::Nightly));
    assert!("weekly".parse::<Tier>().is_err());
}
