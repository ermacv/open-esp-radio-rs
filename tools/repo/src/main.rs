use std::path::PathBuf;

use clap::{Parser, Subcommand};
use oer_xtask::{Context, Result, checks, process};

#[derive(Parser)]
#[command(about = "Repository checks and build orchestration")]
struct Cli {
    #[arg(long, global = true)]
    root: Option<PathBuf>,
    #[command(subcommand)]
    command: Task,
}

#[derive(Subcommand)]
enum Task {
    /// Report the basic source workflow tools in the current environment.
    Doctor,
    Check {
        #[command(subcommand)]
        check: Check,
    },
    Build {
        #[command(subcommand)]
        build: Build,
    },
}

#[derive(Subcommand)]
enum Check {
    Metadata,
    Architecture,
    Safety,
    Network {
        #[arg(long)]
        dependencies_only: bool,
    },
    /// Check the pinned minimal Xarxa patch with the original Embassy and driver.
    NetworkBackpressure,
    Examples,
    SourceOnly,
    BlobrayStandalone,
}

#[derive(Subcommand)]
enum Build {
    /// Build and audit a bootable example with the shared ESP32-S31 bootstrap.
    Firmware {
        #[arg(value_parser = ["station", "access-point", "monitor", "bluetooth-controller"])]
        example: String,
        #[arg(long)]
        flash: bool,
        #[arg(long, requires = "flash")]
        port: Option<PathBuf>,
        #[arg(long, requires_all = ["flash", "port"])]
        monitor: bool,
        #[arg(long, value_delimiter = ',')]
        features: Vec<String>,
        #[arg(long)]
        no_default_features: bool,
        /// Network implementation: upstream-xarxa (default), patched-xarxa, upstream-smoltcp or owned-xarxa.
        #[arg(long)]
        network: Option<oer_firmware::network::Integration>,
    },
    VendorProbes {
        #[arg(long, default_value = "esp32s31")]
        chip: String,
        #[arg(long)]
        list_roles: bool,
    },
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    let ctx = match cli.root {
        Some(root) => Context::new(root)?,
        None => Context::discover()?,
    };
    let _signals = process::install_signal_handlers()?;
    match cli.command {
        Task::Doctor => {
            process::run(ctx.cargo().arg("--version"))?;
            for tool in ["rustc", "git"] {
                process::run(ctx.command(tool).arg("--version"))?;
            }
            println!("repository: {}", ctx.root.display());
            Ok(())
        }
        Task::Check { check } => match check {
            Check::Metadata => checks::metadata::run(&ctx).map(|_| ()),
            Check::Architecture => checks::architecture::run(&ctx),
            Check::Safety => checks::safety::run(&ctx),
            Check::Network { dependencies_only } => checks::network::run(&ctx, dependencies_only),
            Check::NetworkBackpressure => oer_xtask::firmware::check_network_backpressure(&ctx),
            Check::Examples => checks::examples::run(&ctx),
            Check::SourceOnly => checks::source_only::run(&ctx),
            Check::BlobrayStandalone => checks::standalone::run(&ctx),
        },
        Task::Build {
            build:
                Build::Firmware {
                    example,
                    flash,
                    port,
                    monitor,
                    features,
                    no_default_features,
                    network,
                },
        } => {
            let output = oer_xtask::firmware::build(
                &ctx,
                &example,
                &features,
                no_default_features,
                network,
            )?;
            if flash {
                oer_xtask::firmware::flash(&ctx, &output, port.as_deref(), monitor)?;
            }
            Ok(())
        }
        Task::Build {
            build: Build::VendorProbes { chip, list_roles },
        } => checks::vendor::run(&ctx, &chip, list_roles),
    }
}

fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("xtask: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}
