//! Build each chip's caller-owned vendor comparison artifacts.

use crate::{Context, Result};
use oer_process as process;
use std::{ffi::OsStr, num::NonZeroUsize, process::Command};

const JOBS: &str = "OPEN_RADIO_ANALYSIS_BUILD_JOBS";

struct Probe {
    /// The chip whose `verification/<chip>/probes` workspace owns the image.
    chip: &'static str,
    role: &'static str,
    package: &'static str,
    target_directory: &'static str,
}

const PROBES: [Probe; 4] = [
    Probe {
        chip: "esp32s31",
        role: "rust-artifact",
        package: "oer-esp32s31-probe-radio-elf",
        target_directory: "target/verification/esp32s31-probes",
    },
    Probe {
        chip: "esp32s31",
        role: "rust-artifact:wifi-registers",
        package: "oer-esp32s31-probe-register-elf",
        target_directory: "target/verification/esp32s31-register-probes",
    },
    Probe {
        chip: "esp32s31",
        role: "rust-artifact:bluetooth",
        package: "oer-esp32s31-probe-bluetooth-elf",
        target_directory: "target/verification/esp32s31-bluetooth-probes",
    },
    Probe {
        chip: "esp32c5",
        role: "rust-artifact",
        package: "oer-esp32c5-probe-radio-elf",
        target_directory: "target/verification/esp32c5-probes",
    },
];

/// The probe images of `chip`, in build order.
fn probes(chip: &str) -> impl Iterator<Item = &'static Probe> {
    PROBES.iter().filter(move |probe| probe.chip == chip)
}

/// The built comparison ELF of the probe `package`.
pub fn elf(context: &Context, package: &str) -> Result<std::path::PathBuf> {
    let probe = PROBES
        .iter()
        .find(|p| p.package == package)
        .ok_or_else(|| format!("no vendor probe package {package}"))?;
    Ok(context
        .root
        .join(probe.target_directory)
        .join(target(context, probe.chip)?)
        .join("release")
        .join(probe.package))
}

/// The Rust target of `chip`'s firmware, from its `chip.toml`.
fn target(context: &Context, chip: &str) -> Result<String> {
    Ok(oer_chip_profile::Profile::load(&context.root, chip)?.rust_target)
}

pub fn run(context: &Context, chip: &str, list_roles: bool) -> Result<()> {
    if probes(chip).next().is_none() {
        return Err(format!("unsupported vendor-probe chip: {chip}").into());
    }
    if list_roles {
        // Declaration only: no build is executed or inferred by this listing.
        for probe in probes(chip) {
            println!("{}", probe.role);
        }
        return Ok(());
    }
    build(
        context,
        chip,
        std::env::var_os(JOBS).as_deref(),
        |command| {
            process::run(command)?;
            let arguments: Vec<_> = command.get_args().collect();
            let package = arguments
                .windows(2)
                .find(|pair| pair[0] == "--package")
                .and_then(|pair| pair[1].to_str())
                .ok_or("missing probe package")?;
            let directory = command
                .get_envs()
                .find(|(key, _)| *key == "CARGO_TARGET_DIR")
                .and_then(|(_, value)| value)
                .ok_or("missing probe target directory")?;
            let probe = PROBES
                .iter()
                .find(|probe| probe.package == package)
                .ok_or("unknown probe package")?;
            let elf = std::path::Path::new(directory)
                .join(target(context, probe.chip)?)
                .join("release")
                .join(package);
            let catalog = oer_probe_codegen::validate_elf(&std::fs::read(&elf)?, package)?;
            eprintln!(
                "Validated {} executable probe entries in {}",
                catalog.entries.len(),
                elf.display()
            );
            Ok(())
        },
    )?;
    eprintln!(
        "Rust analysis inputs are ready; the typed vendor scenarios compare them (cargo xtask vendor-scenario)."
    );
    Ok(())
}

fn build(
    context: &Context,
    chip: &str,
    jobs: Option<&OsStr>,
    mut execute: impl FnMut(&mut Command) -> Result<()>,
) -> Result<()> {
    // Validate before any invocation; stop at the first failed artifact.
    let jobs = parse_jobs(jobs)?;
    let target = target(context, chip)?;
    for probe in probes(chip) {
        eprintln!("Building {chip} {} comparison probe", probe.role);
        crate::phase::timed(&format!("build {chip} {} probe", probe.role), || {
            execute(&mut command(context, probe, &target, jobs))
        })?;
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

fn command(context: &Context, probe: &Probe, target: &str, jobs: Option<NonZeroUsize>) -> Command {
    let mut command = context.cargo();
    command
        .args(["build", "--manifest-path"])
        .arg(
            context
                .root
                .join("verification")
                .join(probe.chip)
                .join("probes/Cargo.toml"),
        )
        .args([
            "--package",
            probe.package,
            "--target",
            target,
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
