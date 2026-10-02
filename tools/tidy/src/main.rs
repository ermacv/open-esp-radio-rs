//! `oer-tidy [check] [--root DIR]` runs every integrity check and fails on
//! any problem; `oer-tidy workspaces [--root DIR]` prints the manifest of
//! every Cargo workspace, one per line.

use std::{
    path::{Path, PathBuf},
    process::ExitCode,
    time::Instant,
};

use oer_tidy::{Result, manifest::Manifests, repo::Repo, workspaces};

const USAGE: &str = "usage: oer-tidy [check | workspaces] [--root DIR]";

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
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--root" => {
                root_argument = Some(PathBuf::from(arguments.next().ok_or(USAGE)?));
            }
            "check" | "workspaces" if command.is_none() => command = Some(argument),
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(ExitCode::SUCCESS);
            }
            _ => return Err(USAGE.to_owned()),
        }
    }
    let started = Instant::now();
    let repo = Repo::from_git(&root(root_argument))?;
    if command.as_deref() == Some("workspaces") {
        for manifest in workspaces::discover(&Manifests::load(&repo)?) {
            println!("{manifest}");
        }
        return Ok(ExitCode::SUCCESS);
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
