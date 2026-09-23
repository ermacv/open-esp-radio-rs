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
    /// Compile isolated Bluetooth profiles and reject Wi-Fi dependency leakage.
    Bluetooth,
    Safety,
    Network {
        #[arg(long)]
        dependencies_only: bool,
    },
    /// Check the pinned minimal Xarxa patch with the original Embassy and driver.
    NetworkBackpressure,
    Examples,
    /// Check links and static catalogs; select packages or --full for API builds.
    Docs {
        /// Check every public/private API profile, doctest and MCU consumer.
        #[arg(long, conflicts_with_all = ["package", "private"])]
        full: bool,
        /// Check public API and doctests for these packages' supported profiles.
        #[arg(short, long, value_name = "PACKAGE", action = clap::ArgAction::Append)]
        package: Vec<String>,
        /// Include private API documentation for the selected packages.
        #[arg(long, requires = "package")]
        private: bool,
        /// List the selected plan and inapplicable actions without running checks.
        #[arg(long)]
        list: bool,
        /// Copy complete, isolated HTML snapshots after required rustdoc checks.
        #[arg(long)]
        export_html: bool,
        /// At most two independent rustdoc Cargo cache groups in flight.
        #[arg(long, default_value_t = 2, value_parser = clap::value_parser!(u8).range(1..=2))]
        jobs: u8,
    },
    SourceOnly,
    BlobrayStandalone,
    BlobrayNextStandalone,
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
        Task::Build {
            build: Build::Hostapd,
        } => oer_xtask::hostapd::build(&ctx),
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
            Check::Bluetooth => checks::bluetooth::run(&ctx),
            Check::Safety => checks::safety::run(&ctx),
            Check::Network { dependencies_only } => checks::network::run(&ctx, dependencies_only),
            Check::NetworkBackpressure => oer_xtask::firmware::check_network_backpressure(&ctx),
            Check::Examples => checks::examples::run(&ctx),
            Check::Docs {
                full,
                package,
                private,
                list,
                export_html,
                jobs,
            } => {
                let scope = if full {
                    checks::docs::Scope::Full
                } else if package.is_empty() {
                    checks::docs::Scope::Static
                } else {
                    checks::docs::Scope::Packages {
                        names: package,
                        private,
                    }
                };
                checks::docs::run_selected(&ctx, scope, list, export_html, usize::from(jobs))
            }
            Check::SourceOnly => checks::source_only::run(&ctx),
            Check::BlobrayStandalone => checks::standalone::run(&ctx),
            Check::BlobrayNextStandalone => checks::standalone::run_next(&ctx),
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
    fn documentation_scopes_are_explicit_and_conflicting_inputs_are_rejected() {
        let default = Cli::try_parse_from(["xtask", "check", "docs"]).unwrap();
        assert!(matches!(default.command, Task::Check {
            check: Check::Docs { full: false, package, private: false, .. }
        } if package.is_empty()));
        assert!(Cli::try_parse_from(["xtask", "check", "docs", "--full", "--list"]).is_ok());
        assert!(
            Cli::try_parse_from([
                "xtask",
                "check",
                "docs",
                "--package",
                "oer-memory",
                "--private"
            ])
            .is_ok()
        );
        assert!(
            Cli::try_parse_from([
                "xtask",
                "check",
                "docs",
                "--full",
                "--package",
                "oer-memory"
            ])
            .is_err()
        );
        assert!(Cli::try_parse_from(["xtask", "check", "docs", "--private"]).is_err());
    }
}
