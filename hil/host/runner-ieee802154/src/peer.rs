//! Driver of the IEEE 802.15.4 reference peer.
//!
//! The peer is an ESP32-C5 running `hil/peers/esp32c5-ieee802154`: the vendor
//! driver behind a line protocol on its console (see that README). Commands
//! are answered by `@OK`/`@ERR`; driver reports arrive as events at any time
//! and are queued while a command waits for its answer.

use std::{
    collections::VecDeque,
    path::Path,
    time::{Duration, Instant},
};

use crate::Result;
pub use hil_core::fixture::peer_line::{
    PeerLink, PeerTranscript, RecordingLink, SerialLink, SyncAnswer, synchronize_link,
};

/// Protocol version the driver speaks.
pub const PEER_PROTOCOL: u32 = 1;
/// Board-journal image name of `hil/peers/esp32c5-ieee802154`.
pub const PEER_IMAGE: &str = "ieee802154-peer";
/// How to restore the peer firmware when another consumer replaced it.
pub const PEER_REFLASH: &str =
    "restore it with `cargo hil firmware flash ieee802154-peer --board <peer board>`";
/// File name of a boot's peer transcript in its artifact directory.
pub const PEER_TRANSCRIPT: &str = "peer-transcript.log";
const READY_TIMEOUT: Duration = Duration::from_secs(5);
const COMMAND_TIMEOUT: Duration = Duration::from_secs(2);

/// Radio settings of the peer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PeerConfig {
    pub channel: u8,
    pub pan_id: u16,
    pub short_address: u16,
    /// Extended address in over-the-air (little-endian) byte order.
    pub extended_address: [u8; 8],
    pub promiscuous: bool,
    pub power_dbm: i8,
}

/// One frame the peer received: MAC bytes without PHR and FCS.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PeerFrame {
    pub bytes: Vec<u8>,
    pub rssi_dbm: i8,
    pub lqi: u8,
    /// The peer's automatic acknowledgement carried frame pending.
    pub pending: bool,
    pub channel: u8,
}

/// The acknowledgement of one peer transmission.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PeerAck {
    pub bytes: Vec<u8>,
    pub pending: bool,
    pub rssi_dbm: i8,
    pub lqi: u8,
}

/// One driver report.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PeerEvent {
    Received(PeerFrame),
    Transmitted {
        acknowledgement: Option<PeerAck>,
    },
    /// The vendor `esp_ieee802154_tx_error_t` code.
    TransmitFailed(i32),
    EnergyDetected(i8),
}

/// One parsed protocol line.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Line {
    Ready { protocol: u32 },
    Ok(String),
    Err { command: String, reason: String },
    Event(PeerEvent),
    Burst(PeerBurst),
}

/// Transmissions of one burst, reported when it stops.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PeerBurst {
    pub sent: u32,
    pub failed: u32,
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

fn field<'a>(fields: &[&'a str], key: &str) -> Option<&'a str> {
    fields
        .iter()
        .find_map(|field| field.strip_prefix(key)?.strip_prefix('='))
}

fn flag(text: &str) -> Option<bool> {
    match text {
        "0" => Some(false),
        "1" => Some(true),
        _ => None,
    }
}

/// Parse one line; lines without the `@` prefix are not protocol lines.
pub(crate) fn parse_line(line: &str) -> Option<Line> {
    let line = line.trim_end_matches(['\r', '\n']).strip_prefix('@')?;
    let fields: Vec<&str> = line.split(' ').collect();
    match *fields.first()? {
        "READY" => Some(Line::Ready {
            protocol: field(&fields, "protocol")?.parse().ok()?,
        }),
        "OK" => Some(Line::Ok((*fields.get(1)?).to_owned())),
        "ERR" => Some(Line::Err {
            command: (*fields.get(1)?).to_owned(),
            reason: fields.get(2..)?.join(" "),
        }),
        "RX" => Some(Line::Event(PeerEvent::Received(PeerFrame {
            bytes: hex(fields.get(1)?)?,
            rssi_dbm: field(&fields, "rssi")?.parse().ok()?,
            lqi: field(&fields, "lqi")?.parse().ok()?,
            pending: flag(field(&fields, "pending")?)?,
            channel: field(&fields, "ch")?.parse().ok()?,
        }))),
        "TXDONE" => {
            let acknowledgement = match field(&fields, "ack")? {
                "-" => None,
                bytes => Some(PeerAck {
                    bytes: hex(bytes)?,
                    pending: flag(field(&fields, "pending")?)?,
                    rssi_dbm: field(&fields, "rssi")?.parse().ok()?,
                    lqi: field(&fields, "lqi")?.parse().ok()?,
                }),
            };
            Some(Line::Event(PeerEvent::Transmitted { acknowledgement }))
        }
        "TXFAIL" => Some(Line::Event(PeerEvent::TransmitFailed(
            fields.get(1)?.parse().ok()?,
        ))),
        "ED" => Some(Line::Event(PeerEvent::EnergyDetected(
            fields.get(1)?.parse().ok()?,
        ))),
        "BURST" => Some(Line::Burst(PeerBurst {
            sent: field(&fields, "sent")?.parse().ok()?,
            failed: field(&fields, "failed")?.parse().ok()?,
        })),
        _ => None,
    }
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Render the `CFG` command.
pub(crate) fn cfg_line(config: &PeerConfig) -> String {
    format!(
        "CFG {} {:04x} {:04x} {} {} {}",
        config.channel,
        config.pan_id,
        config.short_address,
        to_hex(&config.extended_address),
        u8::from(config.promiscuous),
        config.power_dbm,
    )
}

/// The reference peer.
pub struct Peer<L> {
    link: L,
    events: VecDeque<PeerEvent>,
}

impl Peer<SerialLink> {
    /// Take over the peer on `path` and wait until it reports ready.
    pub fn open(path: &Path) -> Result<Self> {
        Self::synchronize(SerialLink::open(path)?)
    }
}

impl Peer<RecordingLink<SerialLink>> {
    /// Take over the peer on `path` and wait until it reports ready,
    /// recording every line of the session into `transcript`.
    pub fn open_recorded(path: &Path, transcript: &PeerTranscript) -> Result<Self> {
        let link = RecordingLink::new(SerialLink::open(path)?, transcript.clone());
        Self::synchronize(link)
    }
}

impl<L: PeerLink> Peer<L> {
    /// Return the running peer on `link` to its defaults with `SYNC` and
    /// wait for its `@READY` line.
    pub fn synchronize(mut link: L) -> Result<Self> {
        let answer = |line: &str| match parse_line(line) {
            Some(Line::Ready { protocol }) => SyncAnswer::Ready(protocol),
            Some(Line::Err { .. }) => SyncAnswer::Rejected,
            _ => SyncAnswer::Other,
        };
        match synchronize_link(&mut link, READY_TIMEOUT, answer)? {
            Some(PEER_PROTOCOL) => Ok(Self {
                link,
                events: VecDeque::new(),
            }),
            Some(protocol) => Err(format!("IEEE 802.15.4 peer speaks protocol {protocol}").into()),
            None => Err(format!(
                "IEEE 802.15.4 peer did not answer SYNC; the board may carry other \
                 firmware: {PEER_REFLASH}"
            )
            .into()),
        }
    }

    fn command(&mut self, name: &str, line: &str) -> Result<()> {
        self.command_reporting(name, line).map(|_| ())
    }

    /// Send `line` and wait for the reply to `name`; returns the burst
    /// report printed before it, if any.
    fn command_reporting(&mut self, name: &str, line: &str) -> Result<Option<PeerBurst>> {
        self.link.send(line)?;
        let deadline = Instant::now() + COMMAND_TIMEOUT;
        let mut burst = None;
        while let Some(line) = self.link.receive(deadline)? {
            match parse_line(&line) {
                Some(Line::Burst(report)) => burst = Some(report),
                Some(Line::Ok(command)) if command == name => return Ok(burst),
                Some(Line::Err { command, reason }) if command == name => {
                    return Err(format!("IEEE 802.15.4 peer rejected {name}: {reason}").into());
                }
                Some(Line::Event(event)) => self.events.push_back(event),
                _ => {}
            }
        }
        Err(format!("IEEE 802.15.4 peer did not answer {name}").into())
    }

    pub fn configure(&mut self, config: &PeerConfig) -> Result<()> {
        self.command("CFG", &cfg_line(config))
    }

    pub fn receive(&mut self) -> Result<()> {
        self.command("RX", "RX")
    }

    pub fn sleep(&mut self) -> Result<()> {
        self.command("SLEEP", "SLEEP")
    }

    /// Start one transmission of MAC bytes (without FCS). Its outcome
    /// arrives as a [`PeerEvent`].
    pub fn transmit(&mut self, cca: bool, frame: &[u8]) -> Result<()> {
        self.command("TX", &format!("TX {} {}", u8::from(cca), to_hex(frame)))
    }

    /// Start transmitting MAC bytes (without FCS) back to back without CCA
    /// until [`Self::stop_burst`]; the peer reports no event for them.
    pub fn burst(&mut self, frame: &[u8]) -> Result<()> {
        self.command("BURST", &format!("BURST {}", to_hex(frame)))
    }

    /// Stop the burst after its last transmission and return its counts.
    pub fn stop_burst(&mut self) -> Result<PeerBurst> {
        self.command_reporting("BURST", "BURST STOP")?
            .ok_or_else(|| "IEEE 802.15.4 peer stopped a burst without its report".into())
    }

    pub fn set_pending_mode(&mut self, mode: u8) -> Result<()> {
        self.command("PENDING", &format!("PENDING {mode}"))
    }

    pub fn add_pending_short(&mut self, short_address: u16) -> Result<()> {
        self.command("PENDING", &format!("PENDING ADD {short_address:04x}"))
    }

    pub fn clear_pending(&mut self) -> Result<()> {
        self.command("PENDING", "PENDING CLEAR")
    }

    /// The next driver report before `timeout`, or `None`.
    pub fn next_event(&mut self, timeout: Duration) -> Result<Option<PeerEvent>> {
        if let Some(event) = self.events.pop_front() {
            return Ok(Some(event));
        }
        let deadline = Instant::now() + timeout;
        while let Some(line) = self.link.receive(deadline)? {
            if let Some(Line::Event(event)) = parse_line(&line) {
                return Ok(Some(event));
            }
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests;
