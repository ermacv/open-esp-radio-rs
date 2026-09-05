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
    Examples,
    SourceOnly,
    BlobrayStandalone,
}

#[derive(Subcommand)]
enum Build {
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
            Check::Examples => checks::examples::run(&ctx),
            Check::SourceOnly => checks::source_only::run(&ctx),
            Check::BlobrayStandalone => checks::standalone::run(&ctx),
        },
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
