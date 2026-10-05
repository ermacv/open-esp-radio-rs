//! What a session needs from its cell, and the per-scenario initialization
//! the target protocol applies.

use std::path::Path;

use oer_hil_protocol::wifi::{NetworkCredentials, NetworkIpv4Configuration};

use crate::Result;

/// A serial line the stand opened for the link. The stand's board I/O
/// (`oer_hil_board::port::Port`) is the one opener of every serial line;
/// the link only reads and writes what it is handed, and takes a capture's
/// descriptor over for its nonblocking reader.
pub trait SerialLine: std::io::Read + std::io::Write + std::os::fd::AsRawFd + Send {
    /// Hand the open descriptor over.
    fn into_raw_descriptor(self: Box<Self>) -> std::os::fd::RawFd;
}

impl<T> SerialLine for T
where
    T: std::io::Read + std::io::Write + std::os::fd::AsRawFd + std::os::fd::IntoRawFd + Send,
{
    fn into_raw_descriptor(self: Box<Self>) -> std::os::fd::RawFd {
        (*self).into_raw_fd()
    }
}

/// Opens a board's console for a capture, on the capture's worker once the
/// capture owns its output.
pub type ConsoleOpener = Box<dyn FnOnce() -> std::io::Result<Box<dyn SerialLine>> + Send>;

/// The board under test as the link reaches it. The stand implements it for
/// the board it leased.
pub trait Dut {
    /// The board's console port.
    fn console(&self) -> &Path;

    /// Open the board's console and restart its application through it, so
    /// a capture sees the boot from its first byte.
    fn console_with_reset(&self) -> ConsoleOpener;

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
    pub settings: oer_hil_protocol::wifi::TargetSettings,
}
