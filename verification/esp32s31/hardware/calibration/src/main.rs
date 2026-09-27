//! Hardware cross-check of the vendor and production ESP32-S31 PHY
//! calibration on one board.
//!
//! `capture` runs under the HIL stand lease: it flashes the vendor
//! calibration firmware and records the vendor objects of each cold boot,
//! then flashes the production image and records the retained calibration
//! of each cold boot. `compare` projects both sides onto the reviewed
//! `phy_param` relation the tracking scenario compares, and writes a dated
//! summary with a MATCH, DIFF or INCOMPLETE verdict.
use clap::{Parser, Subcommand};
use std::path::PathBuf;

mod capture;
mod compare;
mod production;
mod registers;
mod vendor;

// The shared relation also names offsets only the tracking scenario seeds.
#[path = "../../../probes/radio/library/src/calibration_projection.rs"]
mod calibration_projection;
#[path = "../../../scenarios/src/phy/committed.rs"]
#[allow(dead_code)]
mod committed;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Parser)]
#[command(about)]
struct Arguments {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Capture vendor and production cold calibrations on the leased board.
    Capture(capture::Arguments),
    /// Compare captured calibrations and write the dated summary.
    Compare(compare::Arguments),
}

fn main() -> std::process::ExitCode {
    let arguments = Arguments::parse();
    let result = match arguments.command {
        Command::Capture(arguments) => capture::run(&arguments),
        Command::Compare(arguments) => compare::run(&arguments),
    };
    match result {
        Ok(code) => code,
        Err(error) => {
            eprintln!("error: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

/// The repository root, from this package's manifest directory.
fn repository_root() -> PathBuf {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../..");
    root.canonicalize().unwrap_or(root)
}
