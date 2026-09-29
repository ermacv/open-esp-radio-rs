//! The leased board and the station network as the link reaches them.

use std::path::Path;

use oer_hil_link::{ApplicationReset, Dut, DutEvent, StationNetwork};
use oer_hil_protocol::wifi::{NetworkCredentials, NetworkIpv4Configuration};

use super::config::{LabConfig, StationConfig};

impl Dut for LabConfig {
    fn console(&self) -> &Path {
        &self.dut.serial
    }

    fn application_reset(&self) -> ApplicationReset {
        // Every supported chip restarts through its USB-Serial-JTAG console.
        oer_hil_board::reset::reset_usb_serial_jtag
    }

    fn startup_artifact(&self) -> Option<&Path> {
        self.dut.startup_artifact.as_deref()
    }

    fn journal(&self, event: DutEvent) {
        super::lock::record_board(
            &self.dut.serial,
            match event {
                DutEvent::StartupArtifactUploaded { path, sha256 } => {
                    oer_hil_arbiter::BoardEventKind::StartupArtifactUploaded { path, sha256 }
                }
                DutEvent::StartupArtifactWritten {
                    path,
                    sha256,
                    disposition,
                } => oer_hil_arbiter::BoardEventKind::StartupArtifactWritten {
                    path,
                    sha256,
                    disposition,
                },
            },
        );
    }
}

impl StationNetwork for StationConfig {
    fn ipv4(&self) -> NetworkIpv4Configuration {
        StationConfig::ipv4(self)
    }

    fn credentials(&self) -> oer_hil_link::Result<NetworkCredentials> {
        self.protocol_credentials()
    }
}
