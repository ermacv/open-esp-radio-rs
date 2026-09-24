//! Run one authenticated ESP32-S31 vendor-comparison scenario.
use clap::{Parser, Subcommand};
use oer_esp32s31_vendor_scenarios::{
    gain::{Gain, Options},
    gain_state::{self, Unmet},
    harness::{LimitMode, Result},
};
use std::{path::PathBuf, process::ExitCode};

#[derive(Parser)]
#[command(about = "Typed ESP32-S31 vendor-comparison scenarios over the Blobray CLI")]
struct Cli {
    #[command(subcommand)]
    scenario: Scenario,
}

#[derive(Subcommand)]
enum Scenario {
    /// Wi-Fi/BT gain arithmetic and publication, calibration storage and the
    /// RF-test power producer. Missing `--rftest` is an unmet obligation.
    Gain {
        #[command(flatten)]
        common: Common,
        /// Authenticated `librftest.a`; without it the producer obligation stays unmet.
        #[arg(long)]
        rftest: Option<PathBuf>,
    },
}

#[derive(clap::Args)]
struct Common {
    /// `blobray` executable.
    #[arg(long)]
    binary: PathBuf,
    /// Pinned `libphy.a`.
    #[arg(long)]
    library: PathBuf,
    /// Pinned ROM ELF.
    #[arg(long)]
    rom: PathBuf,
    /// Freshly built production probe ELF.
    #[arg(long)]
    production: PathBuf,
    /// External linker selected for image preparation.
    #[arg(long)]
    linker: PathBuf,
    /// Ignored output root; each run creates a new `run-*` directory.
    #[arg(long)]
    output: PathBuf,
    #[arg(long, value_enum)]
    limit_mode: LimitMode,
}

fn finish(unmet: &[Unmet], message: &str, run: &std::path::Path) -> ExitCode {
    if unmet.is_empty() {
        println!("{message} {}", run.display());
        ExitCode::SUCCESS
    } else {
        let ids: Vec<_> = unmet.iter().map(|o| o.id).collect();
        println!(
            "INCOMPLETE: unmet obligations: {} {}",
            ids.join(", "),
            run.display()
        );
        ExitCode::from(2)
    }
}

fn gain(common: Common, rftest: Option<PathBuf>) -> Result<ExitCode> {
    let options = Options {
        binary: common.binary,
        library: common.library,
        rom: common.rom,
        production: common.production,
        linker: common.linker,
        output: common.output,
        limit_mode: common.limit_mode,
        rftest,
    };
    let mut g = Gain::new(&options)?;
    let unmet = gain_state::exercise(&mut g, options.rftest.is_some())?;
    g.runner.doc("unmet-obligations", &unmet)?;
    g.coefficient_boundaries()?;
    g.characterize()?;
    g.wifi()?;
    g.publish_wifi()?;
    g.bluetooth()?;
    g.additive()?;
    g.negative()?;
    g.preserve()?;
    Ok(finish(
        &unmet,
        "authenticated gain arithmetic/publication, gain state and source-free replay passed",
        &g.run,
    ))
}

fn main() -> ExitCode {
    let result = match Cli::parse().scenario {
        Scenario::Gain { common, rftest } => gain(common, rftest),
    };
    result.unwrap_or_else(|error| {
        eprintln!("error: {error}");
        ExitCode::FAILURE
    })
}
