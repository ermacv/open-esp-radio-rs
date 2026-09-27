//! Regenerate the vendor evidence shards whose recorded sources changed.
//!
//! Each shard records the digests of the sources its verdicts depend on, so a
//! shard is stale exactly when one of them changed. A shard that no longer
//! parses, such as one git left from a conflicting merge, is stale too: the
//! repository's attributes merge shards as binary files, and this task
//! rewrites them from the scenarios instead of by hand.
#[path = "../../../verification/schema/scenario-evidence.rs"]
#[allow(
    dead_code,
    reason = "this task reads only shard identities and source digests"
)]
mod scenario_evidence;

use crate::{Context, Result};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// Shard directory of `chip`, relative to the repository root.
fn directory(chip: &str) -> Result<&'static str> {
    match chip {
        "esp32s31" => Ok("verification/esp32s31/evidence/scenarios"),
        other => Err(format!("no evidence index for chip {other}").into()),
    }
}

/// The comparison probe packages a scenario run needs.
const RADIO_PROBES: &str = "oer-esp32s31-probe-radio-elf";
const BLUETOOTH_PROBES: &str = "oer-esp32s31-probe-bluetooth-elf";

/// Scenario names of the shards in `directory` that are stale or unreadable.
pub fn stale(root: &Path, directory: &Path) -> Result<Vec<String>> {
    let mut names = vec![];
    for entry in std::fs::read_dir(root.join(directory))? {
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()) != Some(scenario_evidence::SHARD_EXTENSION) {
            continue;
        }
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or("shard without a name")?
            .to_owned();
        let current = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str::<scenario_evidence::Index>(&text).ok())
            .is_some_and(|shard| shard.scenario == name && shard.is_current(root));
        if !current {
            names.push(name);
        }
    }
    names.sort();
    Ok(names)
}

/// Rewrite the named shards, or every stale shard when none is named.
pub fn run(
    ctx: &Context,
    chip: &str,
    scenarios: Vec<String>,
    linker: PathBuf,
    limit_mode: String,
    output: PathBuf,
) -> Result<ExitCode> {
    let directory = Path::new(directory(chip)?);
    let selected = if scenarios.is_empty() {
        stale(&ctx.root, directory)?
    } else {
        scenarios
    };
    if selected.is_empty() {
        println!("every evidence shard is current");
        return Ok(ExitCode::SUCCESS);
    }
    let linker = resolve(&linker)?;
    println!("regenerating evidence shards: {}", selected.join(", "));
    crate::checks::vendor::run(ctx, chip, false)?;
    let radio = crate::checks::vendor::elf(ctx, RADIO_PROBES)?;
    let bluetooth = crate::checks::vendor::elf(ctx, BLUETOOTH_PROBES)?;
    // Several scenarios run concurrently under one budget in `all`.
    let runs: Vec<String> = if selected.len() > 1 {
        vec!["all".into()]
    } else {
        selected
    };
    for scenario in runs {
        let mut args: Vec<OsString> = vec![
            scenario.clone().into(),
            "--production".into(),
            radio.clone().into(),
            "--linker".into(),
            linker.clone().into(),
            "--output".into(),
            output.join(&scenario).into(),
            "--limit-mode".into(),
            limit_mode.clone().into(),
            "--index".into(),
            ctx.root.join(directory).into(),
        ];
        if scenario == "all" || scenario == "bluetooth" {
            args.extend(["--bluetooth-production".into(), bluetooth.clone().into()]);
        }
        let code = crate::vendor_scenario::run(ctx, chip, &args)?;
        if code != ExitCode::SUCCESS {
            return Ok(code);
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// `program` itself when it names a path, otherwise its first match on
/// `PATH`.
fn resolve(program: &Path) -> Result<PathBuf> {
    if program.components().count() > 1 {
        return Ok(program.to_path_buf());
    }
    std::env::var_os("PATH")
        .into_iter()
        .flat_map(|paths| std::env::split_paths(&paths).collect::<Vec<_>>())
        .map(|directory| directory.join(program))
        .find(|candidate| candidate.is_file())
        .ok_or_else(|| format!("{} is not on PATH", program.display()).into())
}

#[cfg(test)]
mod tests;
