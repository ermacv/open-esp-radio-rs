//! Run one authenticated ESP32-S31 vendor-comparison scenario.
use clap::{Parser, Subcommand};
use oer_esp32s31_vendor_scenarios::{
    calibration_leaves, calibration_prefix, channel,
    gain::{Gain, Options},
    gain_state::{self, Unmet},
    harness::{Budget, Result},
    harness_edges, i2c, i2c_transport,
    phy::PhyOptions,
    research, rfpll, rx_gain, tracking, tx_dc,
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
    /// Channel restoration over installed ROM callbacks: full-root channel,
    /// temperature prefix over every sensor range and stuck-readiness containment.
    Channel {
        #[command(flatten)]
        common: Common,
    },
    /// Complete RX-gain root: publication guards, DC calibration and
    /// failed-channel, minimum-search and shared-budget containment.
    RxGain {
        #[command(flatten)]
        common: Common,
        /// Authenticated SDK firmware supplying the never-executed diagnostics symbol.
        #[arg(long)]
        phy_sdk: PathBuf,
    },
    /// Complete TX-DC/PWDET root: Wi-Fi/BT DC rows over constant and
    /// alternating SAR samples, and PBus/SAR fault containment.
    TxDc {
        #[command(flatten)]
        common: Common,
        /// Authenticated SDK firmware supplying the never-executed diagnostics symbol.
        #[arg(long)]
        phy_sdk: PathBuf,
    },
    /// Combined calibration and parameter tracking parents with their real
    /// children, RFPLL corrections and failed-TX containment.
    Tracking {
        #[command(flatten)]
        common: Common,
        /// Authenticated SDK firmware supplying the never-executed diagnostics symbol.
        #[arg(long)]
        phy_sdk: PathBuf,
    },
    /// Captured PHY research, navigation, register/data review and
    /// source-free preservation of every retained result.
    Research {
        /// `blobray` executable.
        #[arg(long)]
        binary: PathBuf,
        /// Pinned `libphy.a`.
        #[arg(long)]
        library: PathBuf,
        /// Pinned ROM ELF.
        #[arg(long)]
        rom: PathBuf,
        /// External linker selected for image preparation.
        #[arg(long)]
        linker: PathBuf,
        /// Ignored output root; each run creates a new `run-*` directory.
        #[arg(long)]
        output: PathBuf,
        #[command(flatten)]
        budget: Budget,
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

impl Common {
    fn phy(self) -> PhyOptions {
        PhyOptions {
            binary: self.binary,
            library: self.library,
            rom: self.rom,
            production: self.production,
            linker: self.linker,
            output: self.output,
            budget: self.budget,
        }
    }
}

fn channel(common: Common) -> Result<ExitCode> {
    let options = common.phy();
    let mut ctx = channel::Channel::new(&options)?;
    channel::exercise(&mut ctx)?;
    ctx.preserve(0)?;
    Ok(finish(
        &[],
        "authenticated channel restoration, temperature prefix, containment and source-free replay passed",
        &ctx.run,
    ))
}

fn rx_gain(common: Common, phy_sdk: PathBuf) -> Result<ExitCode> {
    let mut ctx = rx_gain::RxGain::new(&common.phy(), &phy_sdk)?;
    rx_gain::exercise(&mut ctx)?;
    ctx.preserve(0)?;
    Ok(finish(
        &[],
        "authenticated RX gain publication, calibration, containment and source-free replay passed",
        &ctx.run,
    ))
}

fn tx_dc(common: Common, phy_sdk: PathBuf) -> Result<ExitCode> {
    let mut ctx = tx_dc::TxDc::new(&common.phy(), &phy_sdk)?;
    tx_dc::exercise(&mut ctx)?;
    ctx.preserve(0)?;
    Ok(finish(
        &[],
        "authenticated TX-DC/PWDET calibration, fault containment and source-free replay passed",
        &ctx.run,
    ))
}

fn tracking(common: Common, phy_sdk: PathBuf) -> Result<ExitCode> {
    let mut ctx = tracking::Tracking::new(&common.phy(), &phy_sdk)?;
    tracking::exercise(&mut ctx)?;
    ctx.preserve(0)?;
    Ok(finish(
        &[],
        "authenticated tracking parents, failed-TX containment and source-free replay passed",
        &ctx.run,
    ))
}

fn main() -> ExitCode {
    let result = match Cli::parse().scenario {
        Scenario::Gain { common, rftest } => gain(common, rftest),
        Scenario::Channel { common } => channel(common),
        Scenario::RxGain { common, phy_sdk } => rx_gain(common, phy_sdk),
        Scenario::TxDc { common, phy_sdk } => tx_dc(common, phy_sdk),
        Scenario::Tracking { common, phy_sdk } => tracking(common, phy_sdk),
        Scenario::Research {
            binary,
            library,
            rom,
            linker,
            output,
            budget,
        } => research::exercise(&research::Options {
            binary,
            library,
            rom,
            linker,
            output,
            budget,
        })
        .map(|run| {
            println!(
                "authenticated PHY research, review and source-free preservation passed {}",
                run.display()
            );
            ExitCode::SUCCESS
        }),
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
