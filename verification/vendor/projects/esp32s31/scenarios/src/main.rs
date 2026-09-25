//! Run one authenticated ESP32-S31 vendor-comparison scenario.
use clap::{Parser, Subcommand};
use oer_esp32s31_vendor_scenarios::{
    calibration_leaves, calibration_prefix,
    gain::{Gain, Options},
    gain_state::{self, Unmet},
    harness::{Budget, Result},
    harness_edges, i2c, i2c_transport, rfpll,
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
    /// PHY I2C command memory and transport, harness call edges, calibration
    /// leaves and PBus/DCODE prefix (`--sdk`) and RFPLL (`--phy-sdk`). Each
    /// missing optional input is an unmet obligation.
    I2c {
        #[command(flatten)]
        common: Common,
        /// Authenticated linked SDK firmware (crystal-clock symbol companion).
        #[arg(long)]
        sdk: Option<PathBuf>,
        /// Authenticated SDK firmware with the RFPLL diagnostics symbol; requires `--sdk`.
        #[arg(long, requires = "sdk")]
        phy_sdk: Option<PathBuf>,
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
    #[command(flatten)]
    budget: Budget,
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
        budget: common.budget,
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
    g.preserve(0)?;
    Ok(finish(
        &unmet,
        "authenticated gain arithmetic/publication, gain state and source-free replay passed",
        &g.run,
    ))
}

fn i2c(common: Common, sdk: Option<PathBuf>, phy_sdk: Option<PathBuf>) -> Result<ExitCode> {
    let options = i2c::Options {
        binary: common.binary,
        library: common.library,
        rom: common.rom,
        production: common.production,
        linker: common.linker,
        output: common.output,
        budget: common.budget,
        sdk,
        phy_sdk,
    };
    let mut ctx = i2c::I2c::new(&options)?;
    let unmet = i2c::unmet(options.sdk.is_some(), options.phy_sdk.is_some());
    ctx.runner.doc("unmet-obligations", &unmet)?;
    if options.phy_sdk.is_some() {
        rfpll::exercise(&mut ctx)?;
    }
    if options.sdk.is_some() {
        calibration_prefix::exercise(&mut ctx)?;
    }
    let positive = ctx.command_memory()?;
    i2c_transport::exercise(&mut ctx)?;
    if options.sdk.is_some() {
        calibration_leaves::exercise(&mut ctx)?;
    }
    harness_edges::exercise(&mut ctx)?;
    // All original source copies were deleted before linking/execution. Preserve
    // the full project closure, including probe/ROM bytes and negative evidence.
    ctx.preserve(positive)?;
    Ok(finish(
        &unmet,
        "authenticated PHY comparisons and source-free replay passed",
        &ctx.run,
    ))
}

fn main() -> ExitCode {
    let result = match Cli::parse().scenario {
        Scenario::Gain { common, rftest } => gain(common, rftest),
        Scenario::I2c {
            common,
            sdk,
            phy_sdk,
        } => i2c(common, sdk, phy_sdk),
    };
    result.unwrap_or_else(|error| {
        eprintln!("error: {error}");
        ExitCode::FAILURE
    })
}
