//! What a session needs from its cell, and the per-scenario initialization
//! the target protocol applies.

use std::path::Path;

use oer_hil_protocol::wifi::{NetworkCredentials, NetworkIpv4Configuration};
use oer_hil_scenario::Settings;

use crate::Result;

/// Restart the application of a board whose console `serial` holds open.
pub type ApplicationReset = fn(&mut dyn serialport::SerialPort) -> serialport::Result<()>;

/// The board under test as the link reaches it. The stand implements it for
/// the board it leased.
pub trait Dut {
    /// The board's console port.
    fn console(&self) -> &Path;

    /// How the board restarts into its application through its console, so a
    /// capture sees the boot from its first byte.
    fn application_reset(&self) -> ApplicationReset;

    /// Where the board keeps its startup artifact between runs, if it has one.
    fn startup_artifact(&self) -> Option<&Path>;

    /// Journal a change the link made to the board.
    fn journal(&self, event: DutEvent);
}

/// A change the link makes to a board, which the stand journals.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DutEvent {
    /// The host handed the board its stored startup artifact.
    StartupArtifactUploaded { path: String, sha256: String },
    /// The board returned a startup artifact the host stored.
    StartupArtifactWritten {
        path: String,
        sha256: String,
        disposition: String,
    },
}

/// The network a station-mode target joins. The stand implements it from its
/// laboratory configuration.
pub trait StationNetwork {
    fn ipv4(&self) -> NetworkIpv4Configuration;

    fn credentials(&self) -> Result<NetworkCredentials>;
}

/// What a session needs from its repetition: the board, the station network
/// and the selected scenario's target initialization settings.
#[derive(Clone, Copy)]
pub struct Target<'a> {
    pub dut: &'a dyn Dut,
    pub station: &'a dyn StationNetwork,
    pub settings: Settings,
}
