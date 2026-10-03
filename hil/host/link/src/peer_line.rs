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

/// Whether the peer on `link` answers `SYNC` with an `@READY` line of any
/// image and protocol: a live console. Every stand peer speaks `SYNC`; the
/// wait is bounded by `SYNC_ATTEMPTS` waits of `timeout`.
pub fn answers_sync<L: PeerLink>(link: &mut L, timeout: Duration) -> Result<bool> {
    let answer = |line: &str| {
        let line = line.trim_start();
        if line.starts_with("@READY") {
            SyncAnswer::Ready(0)
        } else if line.starts_with("@ERR") {
            SyncAnswer::Rejected
        } else {
            SyncAnswer::Other
        }
    };
    Ok(synchronize_link(link, timeout, answer)?.is_some())
}

/// Line transport to the peer.
pub trait PeerLink {
    fn send(&mut self, line: &str) -> Result<()>;
    /// The next complete line before `deadline`, or `None` on timeout.
    fn receive(&mut self, deadline: Instant) -> Result<Option<String>>;
}

/// The peer's serial console.
pub struct SerialLink {
    path: String,
    port: Box<dyn serialport::SerialPort>,
    buffered: Vec<u8>,
}

/// A transport failure of a peer's console, naming the console and what was
/// being done, so a run report says the peer — not the device under test —
/// failed. The I/O error stays the source: the runner classifies it as an
/// infrastructure fault.
#[derive(Debug)]
pub struct PeerConsoleError {
    path: String,
    operation: String,
    source: Box<dyn std::error::Error + Send + Sync + 'static>,
}

impl PeerConsoleError {
    fn new(
        path: &str,
        operation: impl Into<String>,
        source: impl std::error::Error + Send + Sync + 'static,
    ) -> Self {
        Self {
            path: path.to_owned(),
            operation: operation.into(),
            source: Box::new(source),
        }
    }
}

impl std::fmt::Display for PeerConsoleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "peer console {}: {} failed: {}",
            self.path, self.operation, self.source
        )
    }
}

impl std::error::Error for PeerConsoleError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&*self.source)
    }
}

impl SerialLink {
    /// Open the console without resetting the peer. RTS is released before
    /// DTR, so the lines never pass through the reset state (RTS asserted,
    /// DTR released); output the running image printed before is dropped.
    /// A USB Serial/JTAG reset of an ESP32-C5 whose IEEE 802.15.4 radio runs
    /// can leave it in ROM download, so the peer is never reset from here.
    pub fn open(path: &Path) -> Result<Self> {
        let path = path.to_string_lossy().into_owned();
        let failed = |operation: &str, error: serialport::Error| {
            PeerConsoleError::new(&path, operation, error)
        };
        let mut port = serialport::new(&path, 115_200)
            .timeout(Duration::from_millis(50))
            .open()
            .map_err(|error| failed("open", error))?;
        port.write_request_to_send(false)
            .map_err(|error| failed("release RTS", error))?;
        port.write_data_terminal_ready(false)
            .map_err(|error| failed("release DTR", error))?;
        thread::sleep(Duration::from_millis(50));
        port.clear(serialport::ClearBuffer::Input)
            .map_err(|error| failed("clear input", error))?;
        Ok(Self {
            path,
            port,
            buffered: Vec::new(),
        })
    }
}

impl PeerLink for SerialLink {
    fn send(&mut self, line: &str) -> Result<()> {
        let command = line.split(' ').next().unwrap_or(line);
        self.port
            .write_all(line.as_bytes())
            .and_then(|()| self.port.write_all(b"\n"))
            .and_then(|()| self.port.flush())
            .map_err(|error| {
                PeerConsoleError::new(&self.path, format!("write {command}"), error)
            })?;
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
                Err(error) => {
                    return Err(PeerConsoleError::new(&self.path, "read", error).into());
                }
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

#[cfg(test)]
mod console_error_tests {
    use super::*;

    #[test]
    fn a_console_failure_names_the_peer_console_and_keeps_the_io_cause() {
        let error = PeerConsoleError::new(
            "/dev/ttyACM0",
            "write CFG",
            std::io::Error::from(ErrorKind::BrokenPipe),
        );
        assert!(
            error
                .to_string()
                .starts_with("peer console /dev/ttyACM0: write CFG failed: "),
            "{error}"
        );
        let source = std::error::Error::source(&error).expect("an I/O cause");
        assert!(source.is::<std::io::Error>());
    }
}

#[cfg(test)]
mod sync_tests {
    use super::*;
    use std::collections::VecDeque;

    struct Scripted(VecDeque<String>, Vec<String>);

    impl PeerLink for Scripted {
        fn send(&mut self, line: &str) -> Result<()> {
            self.1.push(line.to_owned());
            Ok(())
        }
        fn receive(&mut self, _deadline: Instant) -> Result<Option<String>> {
            Ok(self.0.pop_front())
        }
    }

    fn scripted(lines: &[&str]) -> Scripted {
        Scripted(
            lines.iter().map(|line| format!("{line}\n")).collect(),
            vec![],
        )
    }

    #[test]
    fn any_ready_line_is_a_live_console() {
        let mut link = scripted(&[
            "boot noise",
            "@READY protocol=1 target=esp32c5 stack=openthread",
        ]);
        assert!(answers_sync(&mut link, Duration::from_millis(1)).unwrap());
        assert!(link.1.contains(&"SYNC".to_owned()));
    }

    #[test]
    fn a_silent_console_is_not_live_after_every_attempt() {
        let mut link = scripted(&[]);
        assert!(!answers_sync(&mut link, Duration::from_millis(1)).unwrap());
        assert_eq!(
            link.1.iter().filter(|line| *line == "SYNC").count(),
            SYNC_ATTEMPTS
        );
    }
}
