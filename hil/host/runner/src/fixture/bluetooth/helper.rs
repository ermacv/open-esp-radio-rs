//! Narrow privileged entry point: no shell, arbitrary opcodes or output paths.

#[cfg(target_os = "linux")]
mod connection_reset;
#[cfg(target_os = "linux")]
mod hci;
mod model;
#[cfg(target_os = "linux")]
mod owner;
#[cfg(target_os = "linux")]
mod security_failure;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Clone, Copy, clap::ValueEnum)]
enum TerminationArg {
    PeerReset,
    PeerRfkill,
    TargetDisconnect,
    TargetReset,
}

impl From<TerminationArg> for open_esp_radio_hil_protocol::BluetoothPeripheralTermination {
    fn from(value: TerminationArg) -> Self {
        match value {
            TerminationArg::PeerReset => Self::PeerReset,
            TerminationArg::PeerRfkill => Self::PeerRfkill,
            TerminationArg::TargetDisconnect => Self::TargetDisconnect,
            TerminationArg::TargetReset => Self::TargetReset,
        }
    }
}

#[derive(clap::Parser)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(clap::Subcommand)]
enum Command {
    /// Print the exact finite interface understood by this helper.
    Capabilities,
    /// Exercise one fixed initial-key refusal or mismatch, without sending application data.
    SecurityFailure {
        #[arg(long)]
        adapter: model::Adapter,
        #[arg(long)]
        peer: model::PeerAddress,
        #[arg(long)]
        failure: open_esp_radio_hil_protocol::BluetoothSecurityFailure,
        /// After missing-key rejection, require remote-version completion before Disconnect.
        #[arg(long)]
        read_version_before_disconnect: bool,
    },
    /// Check DTM v2 on LE 1M, channel 0, PRBS9, with 100 ms RX/TX windows.
    Check {
        #[arg(long)]
        adapter: model::Adapter,
    },
    /// Connect to one public LE peer and execute one finite termination mode.
    ConnectReset {
        #[arg(long)]
        adapter: model::Adapter,
        #[arg(long)]
        peer: model::PeerAddress,
        #[arg(long, default_value = "0", value_parser = clap::value_parser!(u16).range(0..=5000))]
        hold_ms: u16,
        #[arg(long, value_enum, default_value = "peer-reset")]
        termination: TerminationArg,
        /// Fixed public test LTK, encryption before either ACL echo.
        #[arg(long)]
        encrypted: bool,
        /// Replace the public LTK between the two ACL echoes, on the same handle.
        #[arg(long, requires = "encrypted")]
        key_refresh: bool,
    },
}

fn main() {
    use clap::Parser as _;
    let command = Cli::parse().command;
    if matches!(&command, Command::Capabilities) {
        println!("{}", model::HELPER_CAPABILITIES);
        return;
    }
    #[cfg(target_os = "linux")]
    if std::env::var("OPEN_RADIO_GENERATION_BOUND").as_deref() != Ok("linux-bluetooth") {
        use std::os::unix::process::CommandExt as _;
        let error = std::process::Command::new("/usr/local/libexec/open-radio-bluetooth-launcher")
            .args(std::env::args_os().skip(1))
            .exec();
        eprintln!("Bluetooth fixture launcher: {error}");
        std::process::exit(1);
    }
    #[cfg(target_os = "linux")]
    let _software = match open_esp_radio_hil_runner::fixture_install::launcher::adopt_lease(
        "linux-bluetooth",
    ) {
        Ok(lease) => lease,
        Err(error) => {
            eprintln!("Bluetooth fixture software lease: {error}");
            std::process::exit(1);
        }
    };
    if let Command::SecurityFailure {
        adapter,
        peer,
        failure,
        read_version_before_disconnect,
    } = command
    {
        let mut report = model::security_failure::Report::new(adapter, peer, failure);
        report.read_version_before_disconnect = read_version_before_disconnect;
        let result = (|| -> Result<()> {
            if read_version_before_disconnect
                && failure != open_esp_radio_hil_protocol::BluetoothSecurityFailure::MissingKey
            {
                return Err("remote-version diagnostic requires missing-key rejection".into());
            }
            let _signals = oer_process::install_signal_handlers()?;
            #[cfg(target_os = "linux")]
            {
                owner::security_failure(adapter, peer, &mut report)
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
            eprintln!("cannot write security failure result: {error}");
            std::process::exit(1);
        }
        if !report.passed(adapter, peer, failure) {
            std::process::exit(1);
        }
        return;
    }
    if let Command::ConnectReset {
        adapter,
        peer,
        hold_ms,
        termination,
        encrypted,
        key_refresh,
    } = command
    {
        let termination = termination.into();
        let mut report = model::ConnectionReset::new(adapter, peer, hold_ms, termination);
        report.encrypted = encrypted;
        report.key_refresh = key_refresh;
        let result = (|| -> Result<()> {
            let _signals = oer_process::install_signal_handlers()?;
            #[cfg(target_os = "linux")]
            {
                owner::connect_reset(adapter, peer, hold_ms, termination, &mut report)
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
        if !report.passed_profile(adapter, peer, hold_ms, termination, encrypted, key_refresh) {
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
        for termination in [
            "peer-reset",
            "peer-rfkill",
            "target-disconnect",
            "target-reset",
        ] {
            assert!(
                Cli::try_parse_from(valid.into_iter().chain(["--termination", termination]))
                    .is_ok()
            );
        }
        assert!(
            Cli::try_parse_from(valid.into_iter().chain(["--termination", "disconnect-now"]))
                .is_err()
        );
    }

    #[test]
    fn capabilities_is_a_parameterless_finite_command() {
        assert!(Cli::try_parse_from(["helper", "capabilities"]).is_ok());
        assert!(Cli::try_parse_from(["helper", "capabilities", "--adapter", "hci0"]).is_err());
    }
}
