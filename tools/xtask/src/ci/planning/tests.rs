use super::super::model::CommonEnvironment;
use super::*;

fn git(root: &Path, args: &[&str]) {
    oer_process::git::output(
        root,
        [
            "-c",
            "user.name=test",
            "-c",
            "user.email=test@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.hooksPath=/dev/null",
        ]
        .into_iter()
        .chain(args.iter().copied()),
    )
    .unwrap();
}

fn commit(root: &Path) {
    git(root, &["add", "."]);
    git(
        root,
        &["commit", "--quiet", "--allow-empty", "-m", "fixture"],
    );
}

fn fixture() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for (path, contents) in [
        ("crates/driver/src/lib.rs", "driver"),
        (".github/workflows/ci.yml", "workflow"),
        ("tools/xtask/src/lib.rs", "checker"),
        ("Cargo.lock", "lock"),
    ] {
        let path = dir.path().join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }
    git(dir.path(), &["init", "--quiet"]);
    commit(dir.path());
    dir
}

fn environment() -> Environment {
    Environment {
        common: CommonEnvironment {
            image: "ubuntu24".into(),
            os: "linux".into(),
            arch: "x86_64".into(),
            rustc: "rustc".into(),
            cargo: "cargo".into(),
            flags: BTreeMap::new(),
        },
        programs: BTreeMap::from([
            ("clang".into(), "clang".into()),
            ("lld".into(), "lld".into()),
        ]),
    }
}

#[test]
fn equal_trees_have_equal_keys_across_commits_and_branches() {
    let dir = fixture();
    let before = manifest(dir.path(), Workflow::Ci, environment()).unwrap();
    git(dir.path(), &["checkout", "-qb", "merge-result"]);
    commit(dir.path());
    let after = manifest(dir.path(), Workflow::Ci, environment()).unwrap();
    assert_ne!(before.commit, after.commit);
    assert_eq!(before.tree, after.tree);
    assert_eq!(before.jobs, after.jobs);
}

#[test]
fn every_tracked_source_and_policy_change_invalidates_every_job() {
    for file in [
        "crates/driver/src/lib.rs",
        ".github/workflows/ci.yml",
        "tools/xtask/src/lib.rs",
        "Cargo.lock",
        "vendor/dep/src/lib.rs",
        "vendor/dep/.cargo-checksum.json",
        ".cargo/config.toml",
    ] {
        let dir = fixture();
        let before = manifest(dir.path(), Workflow::Ci, environment()).unwrap();
        let path = dir.path().join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "changed").unwrap();
        commit(dir.path());
        let after = manifest(dir.path(), Workflow::Ci, environment()).unwrap();
        for (id, job) in before.jobs {
            assert_ne!(job.key, after.jobs[&id].key, "{file}: {id}");
        }
    }
}

#[test]
fn dirty_or_untracked_sources_cannot_be_hidden_behind_a_committed_tree() {
    for file in ["crates/driver/src/lib.rs", "new-source.rs"] {
        let dir = fixture();
        std::fs::write(dir.path().join(file), "changed").unwrap();
        assert!(
            manifest(dir.path(), Workflow::Ci, environment())
                .unwrap_err()
                .to_string()
                .contains("clean checkout")
        );
    }
}

#[test]
fn actual_common_versions_and_flags_change_all_keys_but_extra_tools_only_their_job() {
    let dir = fixture();
    let before = manifest(dir.path(), Workflow::Ci, environment()).unwrap();
    for field in ["rustc", "cargo", "flags"] {
        let mut changed = environment();
        match field {
            "rustc" => changed.common.rustc = "other rustc".into(),
            "cargo" => changed.common.cargo = "other cargo".into(),
            _ => {
                changed
                    .common
                    .flags
                    .insert("RUSTFLAGS".into(), "-C opt-level=2".into());
            }
        }
        let after = manifest(dir.path(), Workflow::Ci, changed).unwrap();
        for (id, job) in &before.jobs {
            assert_ne!(job.key, after.jobs[id].key, "{field}: {id}");
        }
    }
    for id in ["clang", "lld"] {
        let mut changed = environment();
        changed.programs.insert(id.into(), "new version".into());
        let after = manifest(dir.path(), Workflow::Ci, changed).unwrap();
        for (job, inputs) in &before.jobs {
            assert_eq!(inputs.key == after.jobs[job].key, job != "isa-conformance");
        }
    }
    assert!(
        !serde_json::to_string(&environment())
            .unwrap()
            .contains("image_version")
    );
}

#[test]
fn missing_checks_jobs_and_program_versions_reject_planning() {
    let dir = fixture();
    let before = manifest(dir.path(), Workflow::Ci, environment()).unwrap();
    for corrupt in ["job", "checks", "extra", "preparation"] {
        let mut plan = before.clone();
        match corrupt {
            "job" => {
                plan.jobs.remove("host");
            }
            "checks" => plan.jobs.get_mut("host").unwrap().checks.clear(),
            "extra" => {
                plan.jobs
                    .insert("unknown".into(), plan.jobs["host"].clone());
            }
            _ => plan.preparation = "unknown".into(),
        }
        assert!(
            validate_manifest(&plan, Workflow::Ci.spec()).is_err(),
            "{corrupt}"
        );
    }
    let mut missing = environment();
    missing.programs.remove("clang");
    assert!(manifest(dir.path(), Workflow::Ci, missing).is_err());
    // Docs declares no extra tools.
    assert!(
        manifest(
            dir.path(),
            Workflow::Docs,
            Environment {
                programs: BTreeMap::new(),
                ..environment()
            }
        )
        .is_ok()
    );
}

#[cfg(unix)]
#[test]
fn git_tree_modes_and_symlink_targets_are_inputs() {
    use std::os::unix::fs::PermissionsExt as _;
    let dir = fixture();
    let source = dir.path().join("crates/driver/src/lib.rs");
    let before = manifest(dir.path(), Workflow::Ci, environment()).unwrap();
    std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o755)).unwrap();
    commit(dir.path());
    let executable = manifest(dir.path(), Workflow::Ci, environment()).unwrap();
    assert_ne!(before.jobs["host"].key, executable.jobs["host"].key);
    std::os::unix::fs::symlink("Cargo.lock", dir.path().join("link")).unwrap();
    commit(dir.path());
    let linked = manifest(dir.path(), Workflow::Ci, environment()).unwrap();
    std::fs::remove_file(dir.path().join("link")).unwrap();
    std::os::unix::fs::symlink("crates/driver/src/lib.rs", dir.path().join("link")).unwrap();
    commit(dir.path());
    assert_ne!(
        linked.jobs["host"].key,
        manifest(dir.path(), Workflow::Ci, environment())
            .unwrap()
            .jobs["host"]
            .key
    );
}
