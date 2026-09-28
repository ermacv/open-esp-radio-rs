//! Run one authenticated ESP32-S31 vendor-comparison scenario, or read the
//! pinned vendor code for a reviewer.
use clap::{Parser, Subcommand};
use oer_esp32s31_vendor_scenarios::run::{self, Scenario};
use oer_vendor_scenario_report::{Reviewer, inspect};
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Parser)]
#[command(about = "Typed ESP32-S31 vendor-comparison scenarios over the Blobray CLI")]
struct Cli {
    #[command(subcommand)]
    command: Command,
    /// Dep-info file of a library that decides the verdicts; `cargo xtask
    /// vendor-scenario` passes one for each, and shards are written only
    /// with them.
    #[arg(long = "verdict-dep-info", global = true, hide = true)]
    verdict_dep_info: Vec<PathBuf>,
}

#[derive(Subcommand)]
enum Command {
    #[command(flatten)]
    Scenario(Scenario),
    #[command(flatten)]
    Inspect(inspect::Command),
}

fn main() -> ExitCode {
    oer_esp32s31_vendor_scenarios::install();
    let cli = Cli::parse();
    match cli.command {
        Command::Scenario(scenario) => run::run(scenario, &Reviewer, cli.verdict_dep_info),
        Command::Inspect(command) => match inspect::run(command) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("error: {error}");
                ExitCode::FAILURE
            }
        },
    }
}
