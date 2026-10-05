//! `oer-tidy [check] [--root DIR]` runs every integrity check and fails on
//! any problem; `oer-tidy workspaces [--json]` prints the manifest of every
//! Cargo workspace and `oer-tidy chips [--json]` every chip profile, one per
//! line or as a JSON array for CI matrices; `oer-tidy fetch` downloads what
//! every workspace's lock file names and the Cargo cache lacks.
//! `oer-tidy __command-tree` prints these commands for `cargo xtask check
//! docs`.

use std::{
    path::{Path, PathBuf},
    process::ExitCode,
    time::Instant,
};

use oer_repo::{Model, Repo};
use oer_tidy::Result;

const USAGE: &str = "usage: oer-tidy [check | workspaces | chips | fetch] [--json] [--root DIR]";

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(error) => {
            eprintln!("oer-tidy: {error}");
            ExitCode::FAILURE
        }
    }
}

/// The commands `oer-tidy` accepts, for `__command-tree`.
fn command_tree() -> Vec<oer_command_tree::CommandNode> {
    let node =
        |path: &[&str], subcommands: &[&str], flags: &[&str]| oer_command_tree::CommandNode {
            path: path.iter().map(|word| (*word).to_owned()).collect(),
            subcommands: subcommands.iter().map(|word| (*word).to_owned()).collect(),
            flags: flags.iter().map(|word| (*word).to_owned()).collect(),
            forwards: false,
        };
    vec![
        node(
            &["tidy"],
            &["check", "workspaces", "chips", "fetch"],
            &["--root"],
        ),
        node(&["tidy", "check"], &[], &[]),
        node(&["tidy", "workspaces"], &[], &["--json"]),
        node(&["tidy", "chips"], &[], &["--json"]),
        node(&["tidy", "fetch"], &[], &[]),
    ]
}

fn run() -> Result<ExitCode> {
    if oer_command_tree::requested() {
        println!("{}", oer_command_tree::json(&command_tree()));
        return Ok(ExitCode::SUCCESS);
    }
    let mut command = None;
    let mut root_argument = None;
    let mut json = false;
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--root" => {
                root_argument = Some(PathBuf::from(arguments.next().ok_or(USAGE)?));
            }
            "check" | "workspaces" | "chips" | "fetch" if command.is_none() => {
                command = Some(argument)
            }
            "--json" => json = true,
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(ExitCode::SUCCESS);
            }
            _ => return Err(USAGE.to_owned()),
        }
    }
    let started = Instant::now();
    let repo = Repo::from_git(&root_argument.unwrap_or_else(oer_process::built_root))?;
    match command.as_deref() {
        Some("workspaces") => {
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
        Some("chips") => {
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
        Some("fetch") if !json => {
            let cargo = oer_toolchain::cargo_program();
            oer_tidy::fetch::run(&repo, Path::new(&cargo))?;
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
