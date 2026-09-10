//! Narrow privileged entry point: no shell, arbitrary opcodes or output paths.

#[cfg(target_os = "linux")]
mod connection_reset;
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
    /// Connect to one public LE peer, hold 0..5000 ms, then Reset the adapter.
    ConnectReset {
        #[arg(long)]
        adapter: model::Adapter,
        #[arg(long)]
        peer: model::PeerAddress,
        #[arg(long, default_value = "0", value_parser = clap::value_parser!(u16).range(0..=5000))]
        hold_ms: u16,
    },
}

fn main() {
    use clap::Parser as _;
    let command = Cli::parse().command;
    if let Command::ConnectReset {
        adapter,
        peer,
        hold_ms,
    } = command
    {
        let mut report = model::ConnectionReset::new(adapter, peer, hold_ms);
        let result = (|| -> Result<()> {
            let _signals = oer_process::install_signal_handlers()?;
            #[cfg(target_os = "linux")]
            {
                owner::connect_reset(adapter, peer, hold_ms, &mut report)
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
        if !report.passed(adapter, peer, hold_ms) {
            std::process::exit(1);
        }
        return;
    }
    let Command::Check { adapter } = command else {
        unreachable!()
    };
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

#[cfg(test)]
mod cli_tests {
    use super::*;
    use clap::Parser as _;

    #[test]
    fn finite_reset_command_rejects_unbounded_or_arbitrary_arguments() {
        let valid = [
            "helper",
            "connect-reset",
            "--adapter",
            "hci0",
            "--peer",
            "30:ED:A0:F3:F6:D1",
        ];
        assert!(Cli::try_parse_from(valid).is_ok());
        for tail in [
            vec!["--hold-ms", "5001"],
            vec!["--hold-ms", "-1"],
            vec!["--opcode", "0x0c03"],
            vec!["--output", "/etc/test"],
        ] {
            assert!(Cli::try_parse_from(valid.into_iter().chain(tail)).is_err());
        }
        for peer in [
            "../hci0",
            "30:ED:A0:F3:F6",
            "30:ED:A0:F3:F6:D1:00",
            "30:ED:A0:F3:F6:+1",
            "30:ED:A0:F3:F6:GG",
        ] {
            assert!(
                Cli::try_parse_from([
                    "helper",
                    "connect-reset",
                    "--adapter",
                    "hci0",
                    "--peer",
                    peer
                ])
                .is_err()
            );
        }
        assert!(Cli::try_parse_from(valid.into_iter().chain(["--hold-ms", "5000"])).is_ok());
    }
}
