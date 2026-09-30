//! A line-protocol console of a reference peer board.
//!
//! The stand's ESP-IDF peers (`hil/peers/*`) speak a text protocol on their
//! USB Serial/JTAG console: `@READY protocol=N ...` at boot and after `SYNC`,
//! `@OK <command>` or `@ERR <command> <reason>` for every command, and `@`
//! events at any time. This module owns the transport shared by every peer
//! driver: opening the console without resetting the board, synchronizing
//! with `SYNC`, and recording a transcript for the run's artifacts.

use std::{
    fs,
    io::{ErrorKind, Read, Write},
    path::Path,
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

use crate::Result;

/// A catalog image of the reference peer board that a scenario needs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PeerImage {
    /// The board-journal and catalog image name.
    pub name: &'static str,
    /// How to restore it when another consumer replaced it.
    pub reflash: &'static str,
}

/// How a line answers a `SYNC`.
pub enum SyncAnswer {
    /// The peer reported ready with this protocol.
    Ready(u32),
    /// The peer rejected a line: bytes of the `SYNC` were lost.
    Rejected,
    Other,
}

/// Attempts of one synchronization: bytes sent right after the port opens
/// can be lost.
const SYNC_ATTEMPTS: usize = 3;

/// Send `SYNC` until the peer reports ready; returns its protocol. An empty
/// line first ends whatever partial line the peer holds, and a rejected
/// `SYNC` is sent again.
pub fn synchronize_link<L: PeerLink>(
    link: &mut L,
    timeout: Duration,
    answer: impl Fn(&str) -> SyncAnswer,
) -> Result<Option<u32>> {
    for _ in 0..SYNC_ATTEMPTS {
        link.send("")?;
        link.send("SYNC")?;
        let deadline = Instant::now() + timeout;
        while let Some(line) = link.receive(deadline)? {
            match answer(&line) {
                SyncAnswer::Ready(protocol) => return Ok(Some(protocol)),
                SyncAnswer::Rejected => break,
                SyncAnswer::Other => {}
            }
        }
    }
    Ok(None)
}

/// Line transport to the peer.
pub trait PeerLink {
    fn send(&mut self, line: &str) -> Result<()>;
    /// The next complete line before `deadline`, or `None` on timeout.
    fn receive(&mut self, deadline: Instant) -> Result<Option<String>>;
}

/// The peer's serial console.
pub struct SerialLink {
    port: Box<dyn serialport::SerialPort>,
    buffered: Vec<u8>,
}

impl SerialLink {
    /// Open the console without resetting the peer. RTS is released before
    /// DTR, so the lines never pass through the reset state (RTS asserted,
    /// DTR released); output the running image printed before is dropped.
    /// A USB Serial/JTAG reset of an ESP32-C5 whose IEEE 802.15.4 radio runs
    /// can leave it in ROM download, so the peer is never reset from here.
    pub fn open(path: &Path) -> Result<Self> {
        let mut port = serialport::new(path.to_string_lossy(), 115_200)
            .timeout(Duration::from_millis(50))
            .open()?;
        port.write_request_to_send(false)?;
        port.write_data_terminal_ready(false)?;
        thread::sleep(Duration::from_millis(50));
        port.clear(serialport::ClearBuffer::Input)?;
        Ok(Self {
            port,
            buffered: Vec::new(),
        })
    }
}

impl PeerLink for SerialLink {
    fn send(&mut self, line: &str) -> Result<()> {
        self.port.write_all(line.as_bytes())?;
        self.port.write_all(b"\n")?;
        self.port.flush()?;
        Ok(())
    }

    fn receive(&mut self, deadline: Instant) -> Result<Option<String>> {
        loop {
            if let Some(end) = self.buffered.iter().position(|&byte| byte == b'\n') {
                let line: Vec<u8> = self.buffered.drain(..=end).collect();
                return Ok(Some(String::from_utf8_lossy(&line).into_owned()));
            }
            if Instant::now() >= deadline {
                return Ok(None);
            }
            let mut chunk = [0; 256];
            match self.port.read(&mut chunk) {
                Ok(read) => self.buffered.extend_from_slice(&chunk[..read]),
                Err(error) if error.kind() == ErrorKind::TimedOut => {}
                Err(error) => return Err(error.into()),
            }
        }
    }
}

/// Every line exchanged with the peer, with its time since the link opened,
/// shared by the link that records it and the workload that saves it.
#[derive(Clone, Debug)]
pub struct PeerTranscript {
    started: Instant,
    lines: Arc<Mutex<Vec<String>>>,
}

impl Default for PeerTranscript {
    fn default() -> Self {
        Self {
            started: Instant::now(),
            lines: Arc::default(),
        }
    }
}

impl PeerTranscript {
    fn record(&self, direction: &str, line: &str) {
        let millis = self.started.elapsed().as_millis();
        let line = line.trim_end_matches(['\r', '\n']);
        if let Ok(mut lines) = self.lines.lock() {
            lines.push(format!("{millis:>8} {direction} {line}"));
        }
    }

    /// The recorded lines: `>` sent to the peer, `<` received from it.
    pub fn lines(&self) -> Vec<String> {
        self.lines
            .lock()
            .map(|lines| lines.clone())
            .unwrap_or_default()
    }

    /// Write the recorded lines to `path`.
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut text = self.lines().join("\n");
        text.push('\n');
        fs::write(path, text)?;
        Ok(())
    }
}

/// A link that records every line it sends and receives.
pub struct RecordingLink<L> {
    link: L,
    transcript: PeerTranscript,
}

impl<L> RecordingLink<L> {
    pub fn new(link: L, transcript: PeerTranscript) -> Self {
        Self { link, transcript }
    }
}

impl<L: PeerLink> PeerLink for RecordingLink<L> {
    fn send(&mut self, line: &str) -> Result<()> {
        self.transcript.record(">", line);
        self.link.send(line)
    }

    fn receive(&mut self, deadline: Instant) -> Result<Option<String>> {
        let line = self.link.receive(deadline)?;
        if let Some(line) = &line {
            self.transcript.record("<", line);
        }
        Ok(line)
    }
}
