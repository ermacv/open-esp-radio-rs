//! Driver of the IEEE 802.15.4 reference peer.
//!
//! The peer is an ESP32-C5 running `hil/peers/esp32c5-ieee802154`: the vendor
//! driver behind a line protocol on its console (see that README). Commands
//! are answered by `@OK`/`@ERR`; driver reports arrive as events at any time
//! and are queued while a command waits for its answer.

use std::{
    collections::VecDeque,
    io::{ErrorKind, Read, Write},
    path::Path,
    thread,
    time::{Duration, Instant},
};

use crate::Result;

/// Protocol version the driver speaks.
pub const PEER_PROTOCOL: u32 = 1;
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
    /// Open the console and hard-reset the peer (RTS pulse with DTR
    /// released, as esptool does), dropping output of the previous boot.
    pub fn open_with_reset(path: &Path) -> Result<Self> {
        let mut port = serialport::new(path.to_string_lossy(), 115_200)
            .timeout(Duration::from_millis(50))
            .open()?;
        port.write_data_terminal_ready(false)?;
        port.write_request_to_send(true)?;
        thread::sleep(Duration::from_millis(100));
        port.clear(serialport::ClearBuffer::Input)?;
        port.write_request_to_send(false)?;
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

/// The reference peer.
pub struct Peer<L> {
    link: L,
    events: VecDeque<PeerEvent>,
}

impl Peer<SerialLink> {
    /// Reset the peer on `path` and wait until it reports ready.
    pub fn open(path: &Path) -> Result<Self> {
        Self::start(SerialLink::open_with_reset(path)?)
    }
}

impl<L: PeerLink> Peer<L> {
    /// Wait for the peer's `@READY` line on `link`.
    pub fn start(mut link: L) -> Result<Self> {
        let deadline = Instant::now() + READY_TIMEOUT;
        while let Some(line) = link.receive(deadline)? {
            if let Some(Line::Ready { protocol }) = parse_line(&line) {
                if protocol != PEER_PROTOCOL {
                    return Err(format!("IEEE 802.15.4 peer speaks protocol {protocol}").into());
                }
                return Ok(Self {
                    link,
                    events: VecDeque::new(),
                });
            }
        }
        Err("IEEE 802.15.4 peer did not report ready".into())
    }

    fn command(&mut self, name: &str, line: &str) -> Result<()> {
        self.link.send(line)?;
        let deadline = Instant::now() + COMMAND_TIMEOUT;
        while let Some(line) = self.link.receive(deadline)? {
            match parse_line(&line) {
                Some(Line::Ok(command)) if command == name => return Ok(()),
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
