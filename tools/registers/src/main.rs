use clap::{Parser, Subcommand};
use std::path::PathBuf;
#[derive(Parser)]
#[command(about = "Validate reviewed registers and publish SVD/PAC source")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    /// Initialize empty peripheral geometry from an explicit source request.
    InitModel {
        #[arg(long)]
        request: PathBuf,
        #[arg(long)]
        directory: PathBuf,
    },
    /// Capture SVD into a native editable model without accepting its claims.
    ImportSvd {
        #[arg(long)]
        source: PathBuf,
        #[arg(long)]
        directory: PathBuf,
        #[arg(long)]
        chip: String,
        #[arg(long)]
        address_space: String,
    },
    Validate {
        #[arg(long)]
        manifest: PathBuf,
    },
    Generate {
        #[arg(long)]
        manifest: PathBuf,
        #[arg(long)]
        check: bool,
    },
}
fn run() -> oer_register_tool::Result<()> {
    let _signals = oer_process::install_signal_handlers()?;
    match Cli::parse().command {
        Command::InitModel { request, directory } => {
            let count = oer_register_tool::initialize_model(&request, &directory)?;
            println!("initialized {count} unreviewed peripherals");
        }
        Command::ImportSvd {
            source,
            directory,
            chip,
            address_space,
        } => {
            let count = oer_register_tool::import_svd(&source, &directory, &chip, &address_space)?;
            println!("imported {count} unreviewed peripherals; original XML retained");
        }
        Command::Validate { manifest } => {
            oer_register_tool::Publication::load(&manifest)?;
            println!("register publication inputs valid");
        }
        Command::Generate { manifest, check } => {
            oer_register_tool::Publication::load(&manifest)?.generate(check)?;
            println!(
                "register outputs {}",
                if check { "verified" } else { "written" }
            );
        }
    }
    Ok(())
}
fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("register publication failed: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}
