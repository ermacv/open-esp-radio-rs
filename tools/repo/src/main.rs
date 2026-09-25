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
    /// Build an observer with its Cargo receipt, then forward HIL arguments.
    #[command(disable_help_flag = true)]
    Hil {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<std::ffi::OsString>,
    },
    /// Prepare the current observer configuration without running HIL.
    HilObserver,
    /// Build Blobray and the typed vendor scenarios, then run one scenario with
    /// the forwarded arguments (for example `gain --library ...`).
    #[command(disable_help_flag = true)]
    VendorScenario {
        #[arg(long, default_value = "esp32s31")]
        chip: String,
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<std::ffi::OsString>,
    },
    /// Build API documentation from each package's `[package.metadata.docs.rs]`
    /// with `RUSTDOCFLAGS=-D warnings`, then run host doctests.
    Doc,
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
    /// Check local Markdown links and the static qualification catalogs.
    Docs,
    /// Build the PHY library for the chip target and audit its artifact and graph.
    Phy,
    /// Build both final HIL application images and run their target audits.
    Images,
    BlobrayStandalone,
}

#[derive(Subcommand)]
enum Build {
    /// Build the pinned hostapd with explicit HIL coexistence policy support.
    Hostapd,
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

fn run() -> Result<std::process::ExitCode> {
    let cli = Cli::parse();
    let ctx = match cli.root {
        Some(root) => Context::new(root)?,
        None => Context::discover()?,
    };
    let _signals = process::install_signal_handlers()?;
    match cli.command {
        Task::HilObserver => {
            oer_xtask::hil::prepare(&ctx)?;
            println!(
                "{}",
                ctx.root.join("target/hil/current-observer.json").display()
            );
            Ok(())
        }
        Task::Hil { args } => return oer_xtask::hil::run(&ctx, &args),
        Task::VendorScenario { chip, args } => {
            return oer_xtask::vendor_scenario::run(&ctx, &chip, &args);
        }
        Task::Build {
            build: Build::Hostapd,
        } => oer_xtask::hostapd::build(&ctx),
        Task::Doc => oer_xtask::doc::run(&ctx),
        Task::Check { check } => match check {
            Check::Metadata => checks::metadata::run(&ctx).map(|_| ()),
            Check::Architecture => checks::architecture::run(&ctx),
            Check::Safety => checks::safety::run(&ctx),
            Check::Network { dependencies_only } => checks::network::run(&ctx, dependencies_only),
            Check::NetworkBackpressure => oer_xtask::firmware::check_network_backpressure(&ctx),
            Check::Docs => checks::docs::run(&ctx),
            Check::Phy => checks::phy::run(&ctx),
            Check::Images => checks::images::run(&ctx),
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
    }?;
    Ok(std::process::ExitCode::SUCCESS)
}

fn main() -> std::process::ExitCode {
    match run() {
        Ok(status) => status,
        Err(error) => {
            eprintln!("xtask: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn documentation_check_has_no_api_matrix_modes() {
        assert!(matches!(
            Cli::try_parse_from(["xtask", "check", "docs"])
                .unwrap()
                .command,
            Task::Check { check: Check::Docs }
        ));
        assert!(Cli::try_parse_from(["xtask", "check", "docs", "--full"]).is_err());
    }
}
