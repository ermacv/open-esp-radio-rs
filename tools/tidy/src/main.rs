//! `oer-tidy [check] [--root DIR]` runs every integrity check and fails on
//! any problem; `oer-tidy workspaces [--json]` prints the manifest of every
//! Cargo workspace and `oer-tidy chips [--json]` every chip profile, one per
//! line or as a JSON array for CI matrices; `oer-tidy fetch` downloads what
//! every workspace's lock file names and the Cargo cache lacks.
//! `oer-tidy __command-tree` prints these commands, walked from the clap
//! parser, for `cargo xtask check docs`.

use std::{
    path::{Path, PathBuf},
    process::ExitCode,
    time::Instant,
};

use oer_repo::{Model, Repo};
use oer_tidy::Result;

/// `oer-tidy`, which `cargo tidy` runs.
#[derive(clap::Parser)]
#[command(
    name = "oer-tidy",
    about = "Fast text policy over the repository model"
)]
struct Cli {
    /// The checkout to read; default: the checkout this binary was built from.
    #[arg(long, global = true, value_name = "DIR")]
    root: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(clap::Subcommand)]
enum Command {
    /// Run every integrity check and fail on any problem (the default).
    Check,
    /// Print the manifest of every Cargo workspace.
    Workspaces {
        /// One JSON array, for CI matrices.
        #[arg(long)]
        json: bool,
    },
    /// Print every chip profile.
    Chips {
        /// One JSON array, for CI matrices.
        #[arg(long)]
        json: bool,
    },
    /// Download what every workspace's lock file names and the Cargo cache
    /// lacks.
    Fetch,
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(error) => {
            eprintln!("oer-tidy: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<ExitCode> {
    use clap::{CommandFactory as _, Parser as _};
    if oer_command_tree::requested() {
        let tree = oer_command_tree::command_tree(&Cli::command(), &[String::from("tidy")]);
        println!("{}", oer_command_tree::json(&tree));
        return Ok(ExitCode::SUCCESS);
    }
    let cli = Cli::parse();
    let started = Instant::now();
    let repo = Repo::from_git(&cli.root.unwrap_or_else(oer_process::built_root))?;
    match cli.command {
        Some(Command::Workspaces { json }) => {
            let manifests = Model::load(&repo)?.workspaces().to_vec();
            if json {
                println!("{}", serde_json::Value::from(manifests));
            } else {
                for manifest in manifests {
                    println!("{manifest}");
                }
            }
            return Ok(ExitCode::SUCCESS);
        }
        Some(Command::Chips { json }) => {
            let chips = oer_repo::Chips::load(&repo)?;
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
        Some(Command::Fetch) => {
            let cargo = oer_toolchain::cargo_program();
            oer_tidy::fetch::run(&repo, Path::new(&cargo))?;
            return Ok(ExitCode::SUCCESS);
        }
        Some(Command::Check) | None => {}
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
