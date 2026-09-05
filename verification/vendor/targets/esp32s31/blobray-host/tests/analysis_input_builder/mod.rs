//! Exercise build arguments with a process-local Cargo stub, without building ELFs.

use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
};

use super::repository_root;

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

struct BuilderFixture {
    directory: PathBuf,
    capture: PathBuf,
}

impl BuilderFixture {
    fn new() -> Self {
        let directory = std::env::temp_dir().join(format!(
            "oer-builder-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&directory).unwrap();
        let cargo = directory.join("cargo");
        fs::write(
            &cargo,
            "#!/usr/bin/env bash\nset -euo pipefail\nprintf '%s\\0' \"$@\" >> \"$BUILD_CAPTURE\"\nprintf '\\n' >> \"$BUILD_CAPTURE\"\n",
        )
        .unwrap();
        fs::set_permissions(&cargo, fs::Permissions::from_mode(0o755)).unwrap();
        Self {
            capture: directory.join("arguments"),
            directory,
        }
    }

    fn run(&self, jobs: Option<&str>) -> Output {
        let mut paths = vec![self.directory.clone()];
        paths.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap()));
        let mut command = Command::new(
            repository_root().join("verification/vendor/targets/esp32s31/build-analysis-inputs"),
        );
        command
            .env("PATH", std::env::join_paths(paths).unwrap())
            .env("BUILD_CAPTURE", &self.capture)
            .env_remove("OPEN_RADIO_ANALYSIS_BUILD_JOBS");
        if let Some(jobs) = jobs {
            command.env("OPEN_RADIO_ANALYSIS_BUILD_JOBS", jobs);
        }
        command.output().unwrap()
    }

    fn calls(&self) -> Vec<Vec<String>> {
        fs::read_to_string(&self.capture)
            .unwrap()
            .lines()
            .map(|line| {
                line.trim_end_matches('\0')
                    .split('\0')
                    .map(str::to_owned)
                    .collect()
            })
            .collect()
    }
}

impl Drop for BuilderFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}

#[test]
fn locked_probe_builds_leave_parallelism_to_cargo_by_default() {
    let fixture = BuilderFixture::new();
    let output = fixture.run(None);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let calls = fixture.calls();
    let declared = Command::new(
        repository_root().join("verification/vendor/targets/esp32s31/build-analysis-inputs"),
    )
    .arg("--list-roles")
    .output()
    .unwrap();
    assert!(declared.status.success());
    assert_eq!(
        calls.len(),
        String::from_utf8(declared.stdout).unwrap().lines().count()
    );
    assert!(!calls.is_empty());
    for arguments in calls {
        assert_eq!(arguments[0], "build");
        assert!(arguments.iter().any(|argument| argument == "--locked"));
        assert!(
            !arguments
                .iter()
                .any(|argument| argument == "--jobs" || argument.starts_with("-j"))
        );
    }
}

#[test]
fn explicit_positive_job_limit_reaches_each_locked_probe_build() {
    let fixture = BuilderFixture::new();
    let output = fixture.run(Some("4"));
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let calls = fixture.calls();
    assert!(!calls.is_empty());
    for arguments in calls {
        assert!(arguments.iter().any(|argument| argument == "--locked"));
        assert!(arguments.windows(2).any(|pair| pair == ["--jobs", "4"]));
    }
}

#[test]
fn invalid_job_limits_fail_before_invoking_cargo() {
    for jobs in ["", "0", "-1", "1.5", "many", "2 3"] {
        let fixture = BuilderFixture::new();
        let output = fixture.run(Some(jobs));
        assert_eq!(output.status.code(), Some(2), "jobs={jobs:?}");
        assert!(String::from_utf8_lossy(&output.stderr).contains("must be a positive integer"));
        assert!(
            !fixture.capture.exists(),
            "invalid jobs reached Cargo: {jobs:?}"
        );
    }
}
