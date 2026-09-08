//! Narrow privileged entry point: no shell, arbitrary opcodes or output paths.

#[cfg(target_os = "linux")]
mod hci;
mod model;
#[cfg(target_os = "linux")]
mod owner;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(clap::Parser)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(clap::Subcommand)]
enum Command {
    /// Check DTM v2 on LE 1M, channel 0, PRBS9, with 100 ms RX/TX windows.
    Check {
        #[arg(long)]
        adapter: model::Adapter,
    },
}

fn main() {
    use clap::Parser as _;
    let Command::Check { adapter } = Cli::parse().command;
    let mut report = model::Check::new(adapter);
    let result = (|| -> Result<()> {
        let _signals = oer_process::install_signal_handlers()?;
        #[cfg(target_os = "linux")]
        {
            owner::check(adapter, &mut report)
        }
        #[cfg(not(target_os = "linux"))]
        {
            Err("Bluetooth fixture requires Linux".into())
        }
    })();
    if let Err(error) = result {
        report.errors.push(error.to_string());
    }
    if let Err(error) = serde_json::to_writer(std::io::stdout().lock(), &report) {
        eprintln!("cannot write Bluetooth result: {error}");
        std::process::exit(1);
    }
    if !report.passed(adapter) {
        std::process::exit(1);
    }
}
