//! `oer-tidy [check] [--root DIR]` runs every integrity check and fails on
//! any problem; `oer-tidy workspaces [--json]` prints the manifest of every
//! Cargo workspace and `oer-tidy chips [--json]` every chip profile, one per
//! line or as a JSON array for CI matrices.

use std::{
    path::{Path, PathBuf},
    process::ExitCode,
    time::Instant,
};

use oer_tidy::{Result, chips::Chips, manifest::Manifests, repo::Repo, workspaces};

const USAGE: &str = "usage: oer-tidy [check | workspaces | chips] [--json] [--root DIR]";

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(error) => {
            eprintln!("oer-tidy: {error}");
            ExitCode::FAILURE
        }
    }
}

/// The repository root: `--root`, or the checkout holding this package.
fn root(argument: Option<PathBuf>) -> PathBuf {
    argument.unwrap_or_else(|| {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap_or_else(|_| PathBuf::from("."))
    })
}

fn run() -> Result<ExitCode> {
    let mut command = None;
    let mut root_argument = None;
    let mut json = false;
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--root" => {
                root_argument = Some(PathBuf::from(arguments.next().ok_or(USAGE)?));
            }
            "check" | "workspaces" | "chips" if command.is_none() => command = Some(argument),
            "--json" => json = true,
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(ExitCode::SUCCESS);
            }
            _ => return Err(USAGE.to_owned()),
        }
    }
    let started = Instant::now();
    let repo = Repo::from_git(&root(root_argument))?;
    match command.as_deref() {
        Some("workspaces") => {
            let manifests = workspaces::discover(&Manifests::load(&repo)?);
            if json {
                println!("{}", serde_json::Value::from(manifests));
            } else {
                for manifest in manifests {
                    println!("{manifest}");
                }
            }
            return Ok(ExitCode::SUCCESS);
        }
        Some("chips") => {
            let chips = Chips::load(&repo)?;
            if json {
                let chips: Vec<serde_json::Value> = chips
                    .profiles()
                    .iter()
                    .map(|chip| {
                        serde_json::json!({
                            "id": chip.id,
                            "family": chip.family,
                            "rust-target": chip.rust_target,
                        })
                    })
                    .collect();
                println!("{}", serde_json::Value::from(chips));
            } else {
                for id in chips.ids() {
                    println!("{id}");
                }
            }
            return Ok(ExitCode::SUCCESS);
        }
        _ if json => return Err(USAGE.to_owned()),
        _ => {}
    }
    let outcomes = oer_tidy::run(&repo)?;
    let mut failed = false;
    for outcome in &outcomes {
        if outcome.problems.is_empty() {
            println!("tidy {}: ok", outcome.check);
            continue;
        }
        failed = true;
        println!(
            "tidy {}: {} problem(s)",
            outcome.check,
            outcome.problems.len()
        );
        for problem in &outcome.problems {
            println!("  {problem}");
        }
    }
    println!(
        "tidy: {} in {:.2} s (exceptions: tools/tidy/allowlist.toml)",
        if failed { "FAILED" } else { "passed" },
        started.elapsed().as_secs_f64()
    );
    Ok(if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}
