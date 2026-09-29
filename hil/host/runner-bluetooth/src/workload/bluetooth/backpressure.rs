//! Hold the only Host ACL credit until the link's supervision expires, then
//! reconnect on the same Controller epoch.
//!
//! The runner is the HCI Host of the `bluetooth-hci` image with one 27-octet
//! Host ACL buffer and Controller-to-Host flow control; the Linux kernel
//! connects as central over its fixed ATT channel. After an MTU exchange the
//! Host keeps the credit of the first fragment of an ATT Write Command. The
//! Controller then plans no connection event while the received data waits,
//! so the central loses the link and the Controller reports Disconnection
//! Complete with supervision timeout within the link's supervision timeout,
//! while the credit is still held. The Host drops what the ended connection
//! left queued, advertises again without Reset and requires a second
//! connection to deliver the same write in order with every credit returned.

use super::{hci, hci::Packet, peripheral::Host};
use crate::{
    Result,
    fixture::bluetooth::{att, model::PeerAddress},
};
use hil_core::{context::Context, session::SerialCapture};
use serde::Serialize;
use std::{
    path::Path,
    sync::mpsc,
    time::{Duration, Instant},
};

const DISCONNECTION_COMPLETE: u8 = 0x05;
const NUMBER_OF_COMPLETED_PACKETS: u8 = 0x13;
const LE_META: u8 = 0x3e;
const LE_CONNECTION_COMPLETE: u8 = 0x01;
const LE_CONNECTION_UPDATE_COMPLETE: u8 = 0x03;
const SUPERVISION_TIMEOUT: u8 = 0x08;
const PERIPHERAL_ROLE: u8 = 0x01;
const CONTROLLER_START: u16 = 0b10;
const CONTINUATION: u16 = 0b01;
const HOST_START: u16 = 0b00;
const ATT_CHANNEL: u16 = 0x0004;
const ATT_ERROR_RESPONSE: u8 = 0x01;
const ATT_EXCHANGE_MTU_REQUEST: u8 = 0x02;
const ATT_EXCHANGE_MTU_RESPONSE: u8 = 0x03;
const ATT_WRITE_COMMAND: u8 = 0x52;
const ATT_REQUEST_NOT_SUPPORTED: u8 = 0x06;
const MTU: u16 = 247;
/// Octets of the written value: the write crosses several 27-octet Host
/// buffers.
const VALUE_BYTES: usize = 100;
/// How early the Controller may report the timeout, for the console link's
/// observation delay.
const EARLY_MARGIN: Duration = Duration::from_millis(250);
/// How late it may report it while the credit is held.
const LATE_MARGIN: Duration = Duration::from_secs(1);
const CONNECTION_DEADLINE: Duration = Duration::from_secs(20);

pub fn run(output: &Path, context: &Context<'_>) -> Result<()> {
    let adapter = context
        .lab
        .bluetooth_adapter
        .ok_or("set [bluetooth] adapter in the local lab configuration")?;
    let mut owner = att::Owner::acquire(adapter, output)?;
    let result = context.with_capture(output, |capture| {
        let mut report = Report::default();
        let result = exercise(capture, &owner, &mut report);
        let cleanup = oer_process::cleanup(|| hci::command(capture, hci::RESET, &[]).map(|_| ()));
        report.passed = result.is_ok() && cleanup.is_ok();
        report.error = result.as_ref().err().map(ToString::to_string);
        report.cleanup_error = cleanup.as_ref().err().map(ToString::to_string);
        hil_core::durable::atomic_json(&output.join("acl-backpressure.json"), &report)?;
        result.and(cleanup)
    });
    let restore = oer_process::cleanup(|| owner.restore());
    result.and(restore)
}

/// The value written in both connections.
fn value() -> [u8; VALUE_BYTES] {
    core::array::from_fn(|index| (index as u8).wrapping_mul(29) ^ 0x5a)
}

fn write_command() -> Vec<u8> {
    let mut pdu = vec![ATT_WRITE_COMMAND, 0x01, 0x00];
    pdu.extend_from_slice(&value());
    pdu
}

/// What the central thread asks of the main thread and back.
enum Step {
    /// The MTU exchange completed; the main thread may arm the hold.
    Exchanged,
    /// Send the write.
    Write,
    /// Close the socket.
    Close,
}

fn exercise(capture: &SerialCapture, owner: &att::Owner, report: &mut Report) -> Result<()> {
    hci::require(capture)?;
    let host = Host::start(capture)?;
    let peer = host.address;
    report.held = Some(connection(capture, &host, owner, peer, true)?);
    report.recovered = Some(connection(capture, &host, owner, peer, false)?);
    Ok(())
}

/// One kernel connection: MTU exchange, then the write, held or delivered.
fn connection(
    capture: &SerialCapture,
    host: &Host<'_>,
    owner: &att::Owner,
    peer: PeerAddress,
    hold: bool,
) -> Result<Connection> {
    host.advertise()?;
    let (to_central, central_steps) = mpsc::channel::<Step>();
    let (to_host, host_steps) = mpsc::channel::<Step>();
    std::thread::scope(|scope| {
        let central = scope.spawn(move || -> Result<()> {
            let socket = owner.connect(peer)?;
            socket.send(&[ATT_EXCHANGE_MTU_REQUEST, MTU as u8, (MTU >> 8) as u8])?;
            if socket.receive()? != [ATT_EXCHANGE_MTU_RESPONSE, MTU as u8, (MTU >> 8) as u8] {
                return Err("the ATT MTU exchange failed".into());
            }
            to_host.send(Step::Exchanged)?;
            loop {
                match central_steps.recv()? {
                    Step::Write => socket.send(&write_command())?,
                    Step::Close | Step::Exchanged => return Ok(()),
                }
            }
        });
        let mut link = Link::new(hold);
        let pumped = pump(capture, &mut link, &host_steps, &to_central);
        let _ = to_central.send(Step::Close);
        let central = central.join().map_err(|_| "ATT central thread panicked")?;
        let observed = link.observed;
        pumped.and(central)?;
        Ok(observed)
    })
}

fn pump(
    capture: &SerialCapture,
    link: &mut Link,
    host_steps: &mpsc::Receiver<Step>,
    to_central: &mpsc::Sender<Step>,
) -> Result<()> {
    let started = Instant::now();
    loop {
        if started.elapsed() > CONNECTION_DEADLINE {
            return Err(format!("the connection did not finish: {:?}", link.observed).into());
        }
        if !link.armed && host_steps.try_recv().is_ok() {
            link.armed = true;
            to_central.send(Step::Write)?;
        }
        let Some(packet) = hci::next_packet(capture, Duration::from_millis(50))? else {
            continue;
        };
        match packet {
            Packet::Event(event) => {
                if link.event(&event)? {
                    return link.finish();
                }
            }
            Packet::Acl(packet) => {
                let (handle, frame, keep) = link.acl(&packet)?;
                if !keep {
                    hci::return_credits(capture, handle, 1)?;
                    link.observed.credits_returned += 1;
                }
                if let Some(frame) = frame {
                    if let Some(response) = link.frame(&frame)? {
                        link.send(capture, handle, &response)?;
                    }
                    if link.observed.delivered && !link.hold {
                        // The recovered write arrived; end the connection.
                        to_central.send(Step::Close)?;
                    }
                }
            }
        }
    }
}

/// Host state of one connection.
struct Link {
    hold: bool,
    armed: bool,
    handle: Option<u16>,
    frame: Vec<u8>,
    held_at: Option<Instant>,
    supervision: Option<Duration>,
    controller_busy: u32,
    observed: Connection,
}

impl Link {
    fn new(hold: bool) -> Self {
        Self {
            hold,
            armed: false,
            handle: None,
            frame: Vec::new(),
            held_at: None,
            supervision: None,
            controller_busy: 0,
            observed: Connection::default(),
        }
    }

    /// Handle one event; `true` once the connection ended.
    fn event(&mut self, event: &[u8]) -> Result<bool> {
        let parameters = event.get(2..).unwrap_or_default();
        match (event.first().copied(), parameters.first().copied()) {
            (Some(LE_META), Some(LE_CONNECTION_COMPLETE)) => {
                // Subevent, status, handle, role, peer kind, peer, interval,
                // latency, supervision timeout.
                if parameters.len() < 18 || parameters[1] != 0 || parameters[4] != PERIPHERAL_ROLE {
                    return Err(format!("the connection failed: {event:02x?}").into());
                }
                if self.handle.is_some() {
                    return Err("a second connection completed".into());
                }
                self.handle = Some(u16::from_le_bytes([parameters[2], parameters[3]]));
                self.supervision = Some(units(parameters[16], parameters[17]));
            }
            (Some(LE_META), Some(LE_CONNECTION_UPDATE_COMPLETE)) => {
                if parameters.len() >= 10 && parameters[1] == 0 {
                    self.supervision = Some(units(parameters[8], parameters[9]));
                }
            }
            (Some(NUMBER_OF_COMPLETED_PACKETS), Some(count)) => {
                for index in 0..usize::from(count) {
                    let entry = parameters
                        .get(1 + index * 4..5 + index * 4)
                        .ok_or("short Number Of Completed Packets")?;
                    if Some(u16::from_le_bytes([entry[0], entry[1]])) == self.handle {
                        let done = u32::from(u16::from_le_bytes([entry[2], entry[3]]));
                        self.controller_busy = self.controller_busy.saturating_sub(done);
                    }
                }
            }
            (Some(DISCONNECTION_COMPLETE), _) => {
                if parameters.len() < 4
                    || parameters[0] != 0
                    || Some(u16::from_le_bytes([parameters[1], parameters[2]])) != self.handle
                {
                    return Err(format!("unexpected Disconnection Complete {event:02x?}").into());
                }
                self.observed.reason = Some(parameters[3]);
                self.observed.supervision_millis = self.supervision.map(|s| s.as_millis() as u64);
                self.observed.held_for_millis =
                    self.held_at.map(|held| held.elapsed().as_millis() as u64);
                return Ok(true);
            }
            _ => {}
        }
        Ok(false)
    }

    /// Consume one fragment: its handle, the frame it completed and whether
    /// the Host keeps its credit.
    fn acl(&mut self, packet: &[u8]) -> Result<(u16, Option<Vec<u8>>, bool)> {
        let header = packet.get(0..4).ok_or("short ACL packet")?;
        let raw = u16::from_le_bytes([header[0], header[1]]);
        let handle = raw & 0x0fff;
        let data = &packet[4..];
        if Some(handle) != self.handle
            || data.len() != usize::from(u16::from_le_bytes([header[2], header[3]]))
        {
            return Err(format!("ACL fragment {header:02x?} does not match the connection").into());
        }
        if self.held_at.is_some() {
            return Err("the Controller delivered data beyond the held credit".into());
        }
        self.observed.fragments += 1;
        match raw >> 12 & 0b11 {
            CONTROLLER_START if self.frame.is_empty() => {}
            CONTINUATION if !self.frame.is_empty() => {}
            flag => return Err(format!("ACL fragment boundary {flag:#04b} out of order").into()),
        }
        self.frame.extend_from_slice(data);
        let keep = self.hold && self.armed;
        if keep {
            self.held_at = Some(Instant::now());
        }
        let expected = self
            .frame
            .get(0..2)
            .map(|length| 4 + usize::from(u16::from_le_bytes([length[0], length[1]])));
        match expected {
            Some(expected) if self.frame.len() > expected => {
                Err("ACL fragments exceed their L2CAP length".into())
            }
            Some(expected) if self.frame.len() == expected => {
                Ok((handle, Some(std::mem::take(&mut self.frame)), keep))
            }
            _ => Ok((handle, None, keep)),
        }
    }

    /// Serve one L2CAP frame; returns the ATT response to send, if any.
    fn frame(&mut self, frame: &[u8]) -> Result<Option<Vec<u8>>> {
        let channel = u16::from_le_bytes([frame[2], frame[3]]);
        let pdu = &frame[4..];
        if channel != ATT_CHANNEL {
            self.observed.other_frames += 1;
            return Ok(None);
        }
        Ok(match pdu.first().copied() {
            Some(ATT_EXCHANGE_MTU_REQUEST) => {
                self.observed.mtu_exchanged = true;
                Some(vec![ATT_EXCHANGE_MTU_RESPONSE, MTU as u8, (MTU >> 8) as u8])
            }
            Some(ATT_WRITE_COMMAND) => {
                if pdu != write_command().as_slice() {
                    return Err("the ATT write arrived corrupted".into());
                }
                self.observed.delivered = true;
                None
            }
            // Every other request is refused; commands and responses need
            // no answer.
            Some(opcode) if opcode & 0x01 == 0 && opcode & 0x40 == 0 => {
                self.observed.refused_requests += 1;
                Some(vec![
                    ATT_ERROR_RESPONSE,
                    opcode,
                    0,
                    0,
                    ATT_REQUEST_NOT_SUPPORTED,
                ])
            }
            _ => None,
        })
    }

    fn send(&mut self, capture: &SerialCapture, handle: u16, pdu: &[u8]) -> Result<()> {
        if self.controller_busy >= 4 {
            return Err("the Controller's ACL buffers are all in use".into());
        }
        let mut packet = (handle | (HOST_START << 12)).to_le_bytes().to_vec();
        packet.extend_from_slice(&((pdu.len() + 4) as u16).to_le_bytes());
        packet.extend_from_slice(&(pdu.len() as u16).to_le_bytes());
        packet.extend_from_slice(&ATT_CHANNEL.to_le_bytes());
        packet.extend_from_slice(pdu);
        hci::acl(capture, &packet)?;
        self.controller_busy += 1;
        Ok(())
    }

    fn finish(&self) -> Result<()> {
        let observed = &self.observed;
        if !observed.mtu_exchanged {
            return Err("the connection ended before the MTU exchange".into());
        }
        if !self.hold {
            return if observed.delivered {
                Ok(())
            } else {
                Err("the recovered connection did not deliver the write".into())
            };
        }
        supervised(
            observed.reason,
            self.supervision,
            self.held_at.map(|held| held.elapsed()),
        )
    }
}

/// A 10 ms unit count.
fn units(low: u8, high: u8) -> Duration {
    Duration::from_millis(u64::from(u16::from_le_bytes([low, high])) * 10)
}

/// The held connection ended by supervision timeout within its timeout of
/// the hold.
fn supervised(
    reason: Option<u8>,
    supervision: Option<Duration>,
    held_for: Option<Duration>,
) -> Result<()> {
    let (Some(supervision), Some(held_for)) = (supervision, held_for) else {
        return Err("the credit was never held on a known connection".into());
    };
    if reason != Some(SUPERVISION_TIMEOUT)
        || held_for + EARLY_MARGIN < supervision
        || held_for > supervision + LATE_MARGIN
    {
        return Err(format!(
            "the held connection ended with {reason:?} after {held_for:?}, supervision {supervision:?}"
        )
        .into());
    }
    Ok(())
}

#[derive(Debug, Default, Serialize)]
struct Report {
    held: Option<Connection>,
    recovered: Option<Connection>,
    passed: bool,
    error: Option<String>,
    cleanup_error: Option<String>,
}

/// What the Host observed on one connection.
#[derive(Clone, Debug, Default, Serialize)]
struct Connection {
    mtu_exchanged: bool,
    delivered: bool,
    fragments: u32,
    credits_returned: u32,
    refused_requests: u32,
    other_frames: u32,
    reason: Option<u8>,
    supervision_millis: Option<u64>,
    held_for_millis: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supervision_must_end_the_held_link_within_its_timeout() {
        let second = Duration::from_secs(2);
        assert!(supervised(Some(SUPERVISION_TIMEOUT), Some(second), Some(second)).is_ok());
        for (reason, held) in [
            (Some(SUPERVISION_TIMEOUT), Duration::from_millis(1500)),
            (Some(SUPERVISION_TIMEOUT), Duration::from_millis(3100)),
            (Some(0x13), second),
            (None, second),
        ] {
            assert!(
                supervised(reason, Some(second), Some(held)).is_err(),
                "{reason:?} {held:?}"
            );
        }
        assert!(supervised(Some(SUPERVISION_TIMEOUT), None, Some(second)).is_err());
    }

    #[test]
    fn the_armed_first_fragment_is_held_and_nothing_may_follow_it() {
        let mut link = Link::new(true);
        link.handle = Some(1);
        link.armed = true;
        let mut fragment = vec![0x01, 0x20, 5, 0];
        fragment.extend_from_slice(&[103, 0, 4, 0, ATT_WRITE_COMMAND]);
        let (_, frame, keep) = link.acl(&fragment).unwrap();
        assert!(frame.is_none() && keep && link.held_at.is_some());
        let continuation = [0x01, 0x10, 1, 0, 0];
        assert!(link.acl(&continuation).is_err());
    }

    #[test]
    fn the_att_responder_answers_mtu_refuses_requests_and_checks_the_write() {
        let mut link = Link::new(false);
        let frame = |pdu: &[u8]| {
            let mut frame = (pdu.len() as u16).to_le_bytes().to_vec();
            frame.extend_from_slice(&ATT_CHANNEL.to_le_bytes());
            frame.extend_from_slice(pdu);
            frame
        };
        assert_eq!(
            link.frame(&frame(&[ATT_EXCHANGE_MTU_REQUEST, 247, 0]))
                .unwrap(),
            Some(vec![ATT_EXCHANGE_MTU_RESPONSE, 247, 0])
        );
        assert_eq!(
            link.frame(&frame(&[0x10, 1, 0, 0xff, 0xff, 0x00, 0x28]))
                .unwrap(),
            Some(vec![
                ATT_ERROR_RESPONSE,
                0x10,
                0,
                0,
                ATT_REQUEST_NOT_SUPPORTED
            ])
        );
        assert_eq!(link.frame(&frame(&write_command())).unwrap(), None);
        assert!(link.observed.delivered);
        let mut corrupted = write_command();
        corrupted[10] ^= 1;
        assert!(link.frame(&frame(&corrupted)).is_err());
    }
}
