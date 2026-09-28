//! Run one authenticated ESP32-C5 vendor-comparison scenario.
use clap::{Parser, Subcommand};
use oer_esp32c5_vendor_scenarios::{
    LIBRARY, PROBES_MANIFEST, PROBES_PACKAGE, ROM, decisions, phy_i2c,
};
use oer_vendor_scenario_engine::harness::{Budget, Result};
use oer_vendor_scenario_engine::leaf::{self, LeafOptions};
use oer_vendor_scenario_engine::shard::{self, ProbeImages};
use oer_vendor_scenario_engine::{artifacts, inspect, observation, session, state};
use std::path::PathBuf;
use std::process::ExitCode;

/// Target triple of the ESP32-C5 probe images.
const PROBES_TARGET: &str = "riscv32imac-unknown-none-elf";
/// The probe images of the ESP32-C5 scenarios.
const PROBES: ProbeImages = ProbeImages {
    manifest: PROBES_MANIFEST,
    packages: &[PROBES_PACKAGE],
    target: PROBES_TARGET,
};

#[derive(Parser)]
#[command(about = "Typed ESP32-C5 vendor-comparison scenarios over the Blobray CLI")]
struct Cli {
    #[command(subcommand)]
    scenario: Scenario,
}

#[derive(Subcommand)]
enum Scenario {
    /// Every read and write of the pinned vendor code to the addresses from
    /// `start` up to `end`, one word when `end` is omitted, with the bits each
    /// store clears, sets or takes from a computed value.
    Xref {
        #[arg(value_parser = oer_vendor_scenario_engine::inspect::parse_address)]
        start: u32,
        #[arg(value_parser = oer_vendor_scenario_engine::inspect::parse_address)]
        end: Option<u32>,
    },
    /// One pinned vendor function, annotated, from every artifact defining it.
    Show { function: String },
    /// Every print of the pinned vendor code that passes bits of the
    /// addresses from `start` up to `end` (one word when omitted) to a
    /// format conversion, with the format text up to that conversion.
    Prints {
        #[arg(value_parser = oer_vendor_scenario_engine::inspect::parse_address)]
        start: u32,
        #[arg(value_parser = oer_vendor_scenario_engine::inspect::parse_address)]
        end: Option<u32>,
    },
    /// Every read and write of the pinned vendor code to a structure field
    /// reached through pointers whose last offsets are `offsets`, from any
    /// argument or symbol: `0x34 0` is the word at offset 0 of the pointer
    /// stored at offset 0x34.
    Fields {
        #[arg(
            required = true,
            allow_hyphen_values = true,
            value_parser = oer_vendor_scenario_engine::inspect::parse_offset
        )]
        offsets: Vec<i32>,
    },
    /// The analog-register I2C transport of `libphy.a[phy_i2c.o]`.
    PhyI2c {
        #[command(flatten)]
        common: Common,
    },
    /// Every scenario, with the reviewed decisions checked for staleness.
    All {
        #[command(flatten)]
        common: Common,
    },
}

#[derive(clap::Args)]
struct Common {
    /// `blobray` executable.
    #[arg(long)]
    binary: PathBuf,
    /// Pinned `libphy.a`.
    #[arg(long, default_value_os_t = artifacts::default_path(LIBRARY))]
    library: PathBuf,
    /// Pinned ROM ELF of the stand's chip revision.
    #[arg(long, default_value_os_t = artifacts::default_path(ROM))]
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
    /// Write each scenario's shard of the evidence index into this directory.
    #[arg(long)]
    index: Option<PathBuf>,
}

/// The transport suite: every leaf must MATCH and meet its expectation.
fn phy_i2c(common: &Common) -> Result<session::Claims> {
    let options = LeafOptions {
        binary: common.binary.clone(),
        suite: &phy_i2c::PHY_I2C,
        archives: vec![common.library.clone()],
        rom: common.rom.clone(),
        firmware: None,
        production: common.production.clone(),
        linker: common.linker.clone(),
        output: common.output.clone(),
        budget: common.budget,
        patches: vec![],
    };
    let mut run = leaf::LeafRun::new(&options)?;
    leaf::exercise(&mut run)?;
    let claims = run.session.claims(
        phy_i2c::PHY_I2C.id,
        &run.roots,
        &leaf::claims(&run),
        oer_vendor_scenario_engine::coverage::decisions()?,
    )?;
    println!(
        "authenticated PHY I2C transport leaves passed {}",
        run.run.display()
    );
    Ok(claims)
}

/// Write `scenario`'s shard when an index directory was given.
fn record(common: &Common, scenario: &str, claims: &session::Claims) -> Result<()> {
    if let Some(directory) = &common.index {
        let index = shard::shard(
            scenario,
            &common.production,
            claims,
            &PROBES,
            env!("CARGO_PKG_NAME"),
        )?;
        shard::write(directory, &index)?;
    }
    Ok(())
}

/// Every scenario; each reviewed decision must still review something.
fn all(common: &Common) -> Result<()> {
    let claims = phy_i2c(common)?;
    let root = observation::root()?;
    let unobserved = claims.lines.unobserved();
    let mut sources = observation::Sources::default();
    sources.check(&root, decisions::observation::DECISIONS, &unobserved)?;
    let (reviewed, untriaged) =
        sources.classify(&root, decisions::observation::DECISIONS, &unobserved)?;
    println!(
        "{} of {} executed production hardware lines observed; {} unobserved reviewed, {} untriaged",
        claims.lines.observed.len(),
        claims.lines.executed.len(),
        reviewed.len(),
        untriaged.len(),
    );
    for line in &untriaged {
        println!("  untriaged line {}:{}", line.0.display(), line.1);
    }
    oer_vendor_scenario_engine::coverage::Observed::of(&claims.closures)
        .check("all", oer_vendor_scenario_engine::coverage::decisions()?)?;
    state::check(decisions::state::DECISIONS, &claims.unprojected)?;
    record(common, "phy-i2c", &claims)
}

fn main() -> ExitCode {
    oer_esp32c5_vendor_scenarios::install();
    let result = match Cli::parse().scenario {
        Scenario::PhyI2c { common } => {
            phy_i2c(&common).and_then(|claims| record(&common, "phy-i2c", &claims))
        }
        Scenario::All { common } => all(&common),
        Scenario::Xref { start, end } => inspect::load().map(|corpus| {
            let end = end.unwrap_or(start.saturating_add(inspect::WORD));
            print!("{}", inspect::xref(&corpus, start, end));
        }),
        Scenario::Prints { start, end } => inspect::load().map(|corpus| {
            let end = end.unwrap_or(start.saturating_add(inspect::WORD));
            print!("{}", inspect::prints(&corpus, start, end));
            ExitCode::SUCCESS
        }),
        Scenario::Fields { offsets } => inspect::load().map(|corpus| {
            print!("{}", inspect::fields(&corpus, &offsets));
            ExitCode::SUCCESS
        }),
        Scenario::Show { function } => inspect::load().and_then(|corpus| {
            let text = inspect::show(&corpus, &function)
                .ok_or_else(|| format!("no pinned artifact defines {function}"))?;
            print!("{text}");
            Ok(())
        }),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}
