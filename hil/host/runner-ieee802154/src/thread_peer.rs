//! Driver of the Thread reference peer.
//!
//! The peer is the ESP32-C5 running `hil/peers/esp32c5-openthread`: ESP-IDF's
//! OpenThread stack behind a line protocol on its console (see that README).
//! It shares the board and the transport with the IEEE 802.15.4 peer.
//! Commands are answered by `@OK`/`@ERR`; received datagrams arrive at any
//! time and are queued while a command waits for its answer.

use std::{
    collections::VecDeque,
    net::Ipv6Addr,
    path::Path,
    time::{Duration, Instant},
};

use crate::{
    Result,
    peer::{PeerLink, PeerTranscript, RecordingLink, SerialLink},
};

/// Protocol version the driver speaks.
pub const THREAD_PEER_PROTOCOL: u32 = 1;
/// Board-journal image name of `hil/peers/esp32c5-openthread`.
pub const THREAD_PEER_IMAGE: &str = "openthread-peer";
/// How to restore the peer firmware when another consumer replaced it.
pub const THREAD_PEER_REFLASH: &str =
    "restore it with `cargo hil firmware flash openthread-peer --board <peer board>`";
/// OpenThread starts its task before the peer reports ready.
const READY_TIMEOUT: Duration = Duration::from_secs(10);
const COMMAND_TIMEOUT: Duration = Duration::from_secs(2);

/// The peer's view of its Thread interface.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ThreadPeerState {
    /// OpenThread's device role name: `disabled`, `detached`, `child`,
    /// `router` or `leader`.
    pub role: String,
    pub rloc16: u16,
    /// The mesh-local EID.
    pub eid: Ipv6Addr,
}

/// One datagram the peer's socket received.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ThreadDatagram {
    pub source: Ipv6Addr,
    pub port: u16,
    pub payload: Vec<u8>,
}

/// One parsed protocol line.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Line {
    Ready { protocol: u32 },
    Ok(String),
    Err { command: String, reason: String },
    Dataset(Vec<u8>),
    State(ThreadPeerState),
    Received(ThreadDatagram),
}

fn hex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(text.get(index..index + 2)?, 16).ok())
        .collect()
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn field<'a>(fields: &[&'a str], key: &str) -> Option<&'a str> {
    fields
        .iter()
        .find_map(|field| field.strip_prefix(key)?.strip_prefix('='))
}

/// Parse one line; lines without the `@` prefix are not protocol lines.
pub(crate) fn parse_line(line: &str) -> Option<Line> {
    let line = line.trim_end_matches(['\r', '\n']).strip_prefix('@')?;
    let fields: Vec<&str> = line.split(' ').collect();
    match *fields.first()? {
        "READY" if field(&fields, "stack") == Some("openthread") => Some(Line::Ready {
            protocol: field(&fields, "protocol")?.parse().ok()?,
        }),
        "OK" => Some(Line::Ok((*fields.get(1)?).to_owned())),
        "ERR" => Some(Line::Err {
            command: (*fields.get(1)?).to_owned(),
            reason: fields.get(2..)?.join(" "),
        }),
        "DATASET" => Some(Line::Dataset(hex(fields.get(1)?)?)),
        "STATE" => Some(Line::State(ThreadPeerState {
            role: field(&fields, "role")?.to_owned(),
            rloc16: u16::from_str_radix(field(&fields, "rloc16")?, 16).ok()?,
            eid: field(&fields, "eid")?.parse().ok()?,
        })),
        "UDPRX" => Some(Line::Received(ThreadDatagram {
            source: fields.get(1)?.parse().ok()?,
            port: fields.get(2)?.parse().ok()?,
            payload: hex(fields.get(3)?)?,
        })),
        _ => None,
    }
}

/// The Thread reference peer.
pub struct ThreadPeer<L> {
    link: L,
    received: VecDeque<ThreadDatagram>,
}

impl ThreadPeer<RecordingLink<SerialLink>> {
    /// Take over the peer on `path` and wait until it reports ready,
    /// recording every line of the session into `transcript`.
    pub fn open_recorded(path: &Path, transcript: &PeerTranscript) -> Result<Self> {
        let link = RecordingLink::new(SerialLink::open(path)?, transcript.clone());
        Self::synchronize(link)
    }
}

impl<L: PeerLink> ThreadPeer<L> {
    /// Leave any network and close the socket with `SYNC`, and wait for the
    /// peer's `@READY` line on `link`.
    pub fn synchronize(mut link: L) -> Result<Self> {
        link.send("SYNC")?;
        let deadline = Instant::now() + READY_TIMEOUT;
        while let Some(line) = link.receive(deadline)? {
            if let Some(Line::Ready { protocol }) = parse_line(&line) {
                if protocol != THREAD_PEER_PROTOCOL {
                    return Err(format!("Thread peer speaks protocol {protocol}").into());
                }
                return Ok(Self {
                    link,
                    received: VecDeque::new(),
                });
            }
        }
        Err(format!(
            "Thread peer did not answer SYNC; the board may carry other firmware: \
             {THREAD_PEER_REFLASH}"
        )
        .into())
    }

    /// Send `line` and wait for the answer to `name`; returns the report
    /// lines printed before it.
    fn command(&mut self, name: &str, line: &str) -> Result<Vec<Line>> {
        self.link.send(line)?;
        let deadline = Instant::now() + COMMAND_TIMEOUT;
        let mut reports = Vec::new();
        while let Some(line) = self.link.receive(deadline)? {
            match parse_line(&line) {
                Some(Line::Ok(command)) if command == name => return Ok(reports),
                Some(Line::Err { command, reason }) if command == name => {
                    return Err(format!("Thread peer rejected {name}: {reason}").into());
                }
                Some(Line::Received(datagram)) => self.received.push_back(datagram),
                Some(report @ (Line::Dataset(_) | Line::State(_))) => reports.push(report),
                _ => {}
            }
        }
        Err(format!("Thread peer did not answer {name}").into())
    }

    /// Form a new network on `channel` and `pan_id` as its leader.
    pub fn form(&mut self, channel: u8, pan_id: u16) -> Result<()> {
        self.command("FORM", &format!("FORM {channel} {pan_id:#06x}"))
            .map(|_| ())
    }

    /// The active operational dataset TLVs.
    pub fn dataset(&mut self) -> Result<Vec<u8>> {
        self.command("DATASET", "DATASET")?
            .into_iter()
            .find_map(|report| match report {
                Line::Dataset(tlvs) => Some(tlvs),
                _ => None,
            })
            .ok_or_else(|| "Thread peer answered DATASET without the dataset".into())
    }

    pub fn state(&mut self) -> Result<ThreadPeerState> {
        self.command("STATE", "STATE")?
            .into_iter()
            .find_map(|report| match report {
                Line::State(state) => Some(state),
                _ => None,
            })
            .ok_or_else(|| "Thread peer answered STATE without its state".into())
    }

    pub fn open_udp(&mut self, port: u16) -> Result<()> {
        self.command("UDP", &format!("UDP OPEN {port}")).map(|_| ())
    }

    pub fn send_udp(&mut self, destination: Ipv6Addr, port: u16, payload: &[u8]) -> Result<()> {
        self.command(
            "UDP",
            &format!("UDP SEND {destination} {port} {}", to_hex(payload)),
        )
        .map(|_| ())
    }

    /// The next datagram the peer received before `timeout`, or `None`.
    pub fn next_datagram(&mut self, timeout: Duration) -> Result<Option<ThreadDatagram>> {
        if let Some(datagram) = self.received.pop_front() {
            return Ok(Some(datagram));
        }
        let deadline = Instant::now() + timeout;
        while let Some(line) = self.link.receive(deadline)? {
            if let Some(Line::Received(datagram)) = parse_line(&line) {
                return Ok(Some(datagram));
            }
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests;
