//! The leased board and the station network as the link reaches them, and
//! the consoles the link reads, opened through the board I/O's one port
//! opener ([`oer_devices::port::Port`]).

use std::{path::Path, time::Duration};

use oer_devices::port::{Lines, Port, Settings};
use oer_hil_link::{ConsoleOpener, Dut, DutEvent, SerialLine, StationNetwork, peer::SerialLink};
use oer_hil_protocol::wifi::{NetworkCredentials, NetworkIpv4Configuration};

use super::config::{LabConfig, StationConfig};

impl Dut for LabConfig {
    fn console(&self) -> &Path {
        &self.dut.serial
    }

    fn console_with_reset(&self) -> ConsoleOpener {
        let port = self.dut.serial.clone();
        Box::new(move || {
            let mut serial = Port::open(&port, DUT_CONSOLE)?;
            // Every supported chip restarts through its USB-Serial-JTAG console.
            oer_devices::reset::reset_usb_serial_jtag(&mut serial)?;
            Ok(Box::new(serial) as Box<dyn SerialLine>)
        })
    }

    fn startup_artifact(&self) -> Option<&Path> {
        self.dut.startup_artifact.as_deref()
    }

    fn journal(&self, event: DutEvent) {
        let kind = match event {
            DutEvent::StartupArtifactUploaded { path, sha256 } => {
                oer_stand_journal::BoardEventKind::StartupArtifactUploaded { path, sha256 }
            }
            DutEvent::StartupArtifactWritten {
                path,
                sha256,
                disposition,
            } => oer_stand_journal::BoardEventKind::StartupArtifactWritten {
                path,
                sha256,
                disposition,
            },
        };
        // A journal failure is reported and never fails the run.
        if let Err(error) = oer_stand_arbiter::Arbiter::open()
            .and_then(|arbiter| arbiter.journal().record(Some(self.dut.mac.clone()), kind))
        {
            eprintln!("hil-arbiter: cannot record board change: {error}");
        }
    }
}

/// A DUT console for a capture: its lines as they are, retried while a
/// previous session's reader still holds the port.
const DUT_CONSOLE: Settings = Settings::CONSOLE
    .lines(Lines::Kept)
    .timeout(Duration::ZERO)
    .busy(Duration::from_secs(2));

/// Opens the console at `port` of a running target without touching its
/// lines, for [`oer_hil_link::SerialCapture::attach`].
pub fn attach_console(port: &Path) -> ConsoleOpener {
    let port = port.to_owned();
    Box::new(move || Ok(Box::new(Port::open(&port, DUT_CONSOLE)?) as Box<dyn SerialLine>))
}

/// The console of the peer at `path`, opened without resetting it: RTS is
/// released before DTR, so the lines never pass through the reset state;
/// output the running image printed before is dropped.
pub fn peer_console(path: &Path) -> crate::Result<SerialLink> {
    let label = path.to_string_lossy();
    let failed = |operation: &str, error: std::io::Error| {
        oer_hil_link::peer::PeerConsoleError::new(&label, operation, error)
    };
    let mut port = Port::open(path, Settings::CONSOLE.timeout(Duration::from_millis(50)))
        .map_err(|error| failed("open", error))?;
    std::thread::sleep(Duration::from_millis(50));
    port.clear_input()
        .map_err(|error| failed("clear input", error))?;
    Ok(SerialLink::new(path, Box::new(port)))
}

impl StationNetwork for StationConfig {
    fn ipv4(&self) -> NetworkIpv4Configuration {
        StationConfig::ipv4(self)
    }

    fn credentials(&self) -> oer_hil_link::Result<NetworkCredentials> {
        self.protocol_credentials()
    }
}
