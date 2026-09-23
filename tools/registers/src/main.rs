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
