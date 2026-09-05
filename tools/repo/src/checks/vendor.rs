//! Build the three caller-owned ESP32-S31 vendor comparison artifacts.

use crate::{Context, Result, process};
use std::{ffi::OsStr, num::NonZeroUsize, process::Command};

const PROJECT: &str = "verification/vendor/projects/esp32s31";
const TARGET: &str = "riscv32imafc-unknown-none-elf";
const JOBS: &str = "OPEN_RADIO_ANALYSIS_BUILD_JOBS";

struct Probe {
    role: &'static str,
    package: &'static str,
    target_directory: &'static str,
}

const PROBES: [Probe; 3] = [
    Probe {
        role: "rust-artifact",
        package: "open-esp-radio-verification-esp32s31-probes-elf",
        target_directory: "target/verification/esp32s31-probes",
    },
    Probe {
        role: "rust-artifact:wifi-registers",
        package: "open-esp-radio-verification-esp32s31-register-probes-elf",
        target_directory: "target/verification/esp32s31-register-probes",
    },
    Probe {
        role: "rust-artifact:bluetooth",
        package: "open-esp-radio-verification-esp32s31-bluetooth-probes-elf",
        target_directory: "target/verification/esp32s31-bluetooth-probes",
    },
];

pub fn run(context: &Context, chip: &str, list_roles: bool) -> Result<()> {
    if chip != "esp32s31" {
        return Err(format!("unsupported vendor-probe chip: {chip}").into());
    }
    if list_roles {
        // Declaration only: no build is executed or inferred by this listing.
        for probe in PROBES {
            println!("{}", probe.role);
        }
        return Ok(());
    }
    build(context, std::env::var_os(JOBS).as_deref(), process::run)?;
    eprintln!("Rust analysis inputs are ready. Bind caller-owned source artifacts, then run:");
    eprintln!(
        "  cargo blobray --project {PROJECT}/vendor-project.toml --run-spec {PROJECT}/local.toml project status"
    );
    Ok(())
}

fn build(
    context: &Context,
    jobs: Option<&OsStr>,
    mut execute: impl FnMut(&mut Command) -> Result<()>,
) -> Result<()> {
    // Validate before any invocation; stop at the first failed artifact.
    let jobs = parse_jobs(jobs)?;
    for probe in &PROBES {
        eprintln!("Building ESP32-S31 {} comparison probe", probe.role);
        execute(&mut command(context, probe, jobs))?;
    }
    Ok(())
}

fn parse_jobs(value: Option<&OsStr>) -> Result<Option<NonZeroUsize>> {
    let Some(value) = value else { return Ok(None) };
    let value = value
        .to_str()
        .ok_or_else(|| format!("{JOBS} must be a positive integer"))?;
    if value.is_empty()
        || value.starts_with('0')
        || !value.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(format!("{JOBS} must be a positive integer").into());
    }
    Ok(Some(value.parse::<NonZeroUsize>().map_err(|_| {
        format!("{JOBS} must be a positive integer within the host size range")
    })?))
}

fn command(context: &Context, probe: &Probe, jobs: Option<NonZeroUsize>) -> Command {
    let mut command = context.cargo();
    command
        .args(["build", "--manifest-path"])
        .arg(context.root.join(PROJECT).join("probes/Cargo.toml"))
        .args([
            "--package",
            probe.package,
            "--target",
            TARGET,
            "--release",
            "--locked",
        ])
        .env(
            "CARGO_TARGET_DIR",
            context.root.join(probe.target_directory),
        );
    if let Some(jobs) = jobs {
        command.arg("--jobs").arg(jobs.to_string());
    }
    command
}

#[cfg(test)]
mod tests;
