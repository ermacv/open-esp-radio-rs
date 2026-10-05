//! Run one authenticated vendor-comparison scenario of a chip, or read the
//! pinned vendor code for a reviewer: `vendor-scenarios <chip> <command>`.
//!
//! The scenario engine serves one chip per process, and a chip's command
//! line reads its pins for argument defaults, so the chip is the first
//! argument: it installs its scenario library before its command line is
//! built, and the other chips' are never built. The reviewer commands are
//! shared.
use clap::{Parser, Subcommand};
use oer_vendor_scenario_report::{Reviewer, inspect};
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;

/// The chips with typed vendor scenarios.
const CHIPS: [&str; 2] = ["esp32s31", "esp32c5"];

#[derive(Parser)]
#[command(about = "Typed vendor-comparison scenarios over the Blobray CLI")]
struct ChipCli<S: Subcommand> {
    #[command(subcommand)]
    command: Command<S>,
    /// Dep-info file of a library that decides the verdicts; `cargo xtask
    /// vendor-scenario` passes one for each, and shards are written only
    /// with them.
    #[arg(long = "verdict-dep-info", global = true, hide = true)]
    verdict_dep_info: Vec<PathBuf>,
}

#[derive(Subcommand)]
enum Command<S: Subcommand> {
    #[command(flatten)]
    Scenario(Box<S>),
    #[command(flatten)]
    Inspect(inspect::Command),
}

/// Parses `arguments` as the command line of `chip`, whose library is
/// installed, and runs the command; `run` executes a scenario.
fn dispatch<S: Subcommand>(
    chip: &str,
    arguments: Vec<OsString>,
    run: impl FnOnce(S, Vec<PathBuf>) -> ExitCode,
) -> ExitCode {
    let name = OsString::from(format!("vendor-scenarios {chip}"));
    let cli = ChipCli::<S>::parse_from(std::iter::once(name).chain(arguments));
    match cli.command {
        Command::Scenario(scenario) => run(*scenario, cli.verdict_dep_info),
        Command::Inspect(command) => match inspect::run(command) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("error: {error}");
                ExitCode::FAILURE
            }
        },
    }
}

fn main() -> ExitCode {
    let mut arguments = std::env::args_os().skip(1);
    let chip = arguments.next();
    let rest: Vec<OsString> = arguments.collect();
    match chip.as_ref().and_then(|chip| chip.to_str()) {
        Some(chip @ "esp32s31") => {
            oer_esp32s31_vendor_scenarios::install();
            dispatch(chip, rest, |scenario, verdict| {
                oer_esp32s31_vendor_scenarios::run::run(scenario, &Reviewer, verdict)
            })
        }
        Some(chip @ "esp32c5") => {
            oer_esp32c5_vendor_scenarios::install();
            dispatch(chip, rest, |scenario, verdict| {
                oer_esp32c5_vendor_scenarios::run::run(scenario, &Reviewer, verdict)
            })
        }
        Some("-h" | "--help") => {
            println!("{}", usage());
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!("{}", usage());
            ExitCode::FAILURE
        }
    }
}

fn usage() -> String {
    format!(
        "usage: vendor-scenarios <{}> <scenario or reviewer command> [ARGS]...\n\
         `vendor-scenarios <chip> --help` lists the chip's commands",
        CHIPS.join("|")
    )
}
