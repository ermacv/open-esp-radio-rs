//! LE peripheral connections with the runner as the HCI Host.
//!
//! The image passes raw HCI through; this module is the whole Host. It resets
//! the Controller, declares one 27-octet Host ACL buffer with
//! Controller-to-Host flow control, and advertises connectable. The Linux
//! helper connects as central (`connect-reset`) on its own thread while this
//! thread pumps the Controller: every received ACL fragment's credit returns
//! explicitly (the first fragment of each L2CAP frame after a 300 ms hold),
//! each complete frame is echoed as one ACL packet, and the connection ends
//! as the scenario's termination asks: the peer resets or loses RF, or the
//! target Host disconnects or resets its Controller after both echoes.
//!
//! ACL timing observed here includes the HIL console link and supports no
//! throughput or latency claim.

use super::hci::{self, Packet};
use crate::{
    Result,
    fixture::bluetooth::{self as fixture, model::PeerAddress},
};
use hil_core::{context::Context, session::SerialCapture};
use oer_hil_protocol::{BLUETOOTH_PERIPHERAL_ACL_PAYLOAD_BYTES, BluetoothPeripheralTermination};
use serde::Serialize;
use std::{
    path::Path,
    time::{Duration, Instant},
};

/// Host ACL buffer the image declares: one 27-octet buffer, so the
/// Controller delivers each Link Layer fragment separately and waits for its
/// credit.
const HOST_ACL_BYTES: u16 = 27;
const HOST_ACL_BUFFERS: u16 = 1;
/// How long the Host keeps the first fragment's credit of each frame.
const CREDIT_HOLD: Duration = Duration::from_millis(300);
/// ACL fragments one echoed frame arrives in.
const FRAGMENTS_PER_FRAME: u32 =
    (BLUETOOTH_PERIPHERAL_ACL_PAYLOAD_BYTES as u32).div_ceil(HOST_ACL_BYTES as u32);
/// Echoes the helper requires before it ends the connection.
const ECHOES: u32 = 2;
/// Bound on one connection cycle, above the helper's own 45 s.
const CYCLE_DEADLINE: Duration = Duration::from_secs(50);
/// 100 ms advertising interval in 0.625 ms units.
const ADVERTISING_INTERVAL: u16 = 160;
/// The helper's requested connection interval, 120 ms, in 1.25 ms units.
const UPDATED_INTERVAL: u16 = 96;

const DISCONNECTION_COMPLETE: u8 = 0x05;
const NUMBER_OF_COMPLETED_PACKETS: u8 = 0x13;
const LE_META: u8 = 0x3e;
const LE_CONNECTION_COMPLETE: u8 = 0x01;
const LE_CONNECTION_UPDATE_COMPLETE: u8 = 0x03;
const REMOTE_USER_TERMINATED: u8 = 0x13;
const LOCAL_HOST_TERMINATED: u8 = 0x16;
const SUPERVISION_TIMEOUT: u8 = 0x08;
const PERIPHERAL_ROLE: u8 = 0x01;
/// First non-automatically-flushable packet, the LE Host's only start flag.
const HOST_START: u16 = 0b00;
const CONTROLLER_START: u16 = 0b10;
const CONTINUATION: u16 = 0b01;

#[derive(Clone, Copy, Debug)]
pub struct Config {
    pub connections: u8,
    pub hold_millis: u16,
    pub termination: BluetoothPeripheralTermination,
}

pub fn run(config: Config, output: &Path, context: &Context<'_>) -> Result<()> {
    let adapter = context
        .lab
        .bluetooth_adapter
        .ok_or("set [bluetooth] adapter in the local lab configuration")?;
    context.with_capture(output, |capture| {
        let mut report = Report {
            schema: 1,
            adapter: adapter.to_string(),
            connections: config.connections,
            hold_millis: config.hold_millis,
            termination: config.termination,
            address: None,
            cycles: Vec::new(),
            passed: false,
            error: None,
            cleanup_error: None,
        };
        let result = exercise(capture, adapter, config, output, &mut report);
        let cleanup = oer_process::cleanup(|| hci::command(capture, hci::RESET, &[]).map(|_| ()));
        report.passed = result.is_ok() && cleanup.is_ok();
        report.error = result.as_ref().err().map(ToString::to_string);
        report.cleanup_error = cleanup.as_ref().err().map(ToString::to_string);
        hil_core::durable::atomic_json(&output.join("bluetooth-peripheral.json"), &report)?;
        result.and(cleanup)
    })
}

fn exercise(
    capture: &SerialCapture,
    adapter: fixture::model::Adapter,
    config: Config,
    output: &Path,
    report: &mut Report,
) -> Result<()> {
    hci::require(capture)?;
    let host = Host::start(capture)?;
    report.address = Some(host.address.to_string());
    for cycle in 1..=config.connections {
        let directory = output.join(format!("connection-{cycle:03}"));
        host.advertise()?;
        let mut observed = Cycle::default();
        let result = std::thread::scope(|scope| {
            let helper = scope.spawn(|| {
                fixture::connect_profile_in(
                    &directory,
                    adapter,
                    host.address,
                    config.hold_millis,
                    config.termination,
                    false,
                    false,
                )
            });
            let served = host.serve(config.termination, &mut observed, || helper.is_finished());
            let helper = helper
                .join()
                .map_err(|_| "Bluetooth helper thread panicked")?;
            served.and(helper.map(|_| ()))
        });
        observed.error = result.as_ref().err().map(ToString::to_string);
        report.cycles.push(observed);
        result.map_err(|error| format!("connection {cycle}: {error}"))?;
        if config.termination == BluetoothPeripheralTermination::TargetReset {
            host.initialize()?;
        }
    }
    Ok(())
}

/// The Host end of one image's Controller.
struct Host<'a> {
    capture: &'a SerialCapture,
    address: PeerAddress,
}

impl<'a> Host<'a> {
    fn start(capture: &'a SerialCapture) -> Result<Self> {
        let host = Self {
            capture,
            address: PeerAddress([0; 6]),
        };
        host.initialize()?;
        let address: [u8; 6] = hci::command(capture, hci::READ_BD_ADDR, &[])?
            .try_into()
            .map_err(|_| "Read BD_ADDR returned no address")?;
        Ok(Self {
            address: PeerAddress(address),
            ..host
        })
    }

    /// Reset the Controller and configure flow control; the Controller's
    /// own ACL buffers must carry one whole frame.
    fn initialize(&self) -> Result<()> {
        let capture = self.capture;
        hci::command(capture, hci::RESET, &[])?;
        hci::command(capture, hci::SET_EVENT_MASK, &hci::EVENT_MASK_WITH_LE_META)?;
        let buffers = hci::command(capture, hci::LE_READ_BUFFER_SIZE, &[])?;
        let length = buffers
            .get(0..2)
            .map(|bytes| u16::from_le_bytes([bytes[0], bytes[1]]))
            .ok_or("LE Read Buffer Size returned no length")?;
        if usize::from(length) < BLUETOOTH_PERIPHERAL_ACL_PAYLOAD_BYTES
            || buffers.get(2) == Some(&0)
        {
            return Err(
                format!("Controller ACL buffers {buffers:02x?} cannot carry one frame").into(),
            );
        }
        let mut host_buffers = [0; 7];
        host_buffers[0..2].copy_from_slice(&HOST_ACL_BYTES.to_le_bytes());
        host_buffers[3..5].copy_from_slice(&HOST_ACL_BUFFERS.to_le_bytes());
        hci::command(capture, hci::HOST_BUFFER_SIZE, &host_buffers)?;
        hci::command(capture, hci::SET_CONTROLLER_TO_HOST_FLOW_CONTROL, &[0x01])?;
        Ok(())
    }

    /// Connectable undirected advertising from the public address.
    fn advertise(&self) -> Result<()> {
        let mut parameters = [0; 15];
        parameters[0..2].copy_from_slice(&ADVERTISING_INTERVAL.to_le_bytes());
        parameters[2..4].copy_from_slice(&ADVERTISING_INTERVAL.to_le_bytes());
        parameters[13] = 0x07;
        hci::command(
            self.capture,
            hci::LE_SET_ADVERTISING_PARAMETERS,
            &parameters,
        )?;
        hci::command(
            self.capture,
            hci::LE_SET_ADVERTISING_DATA,
            &hci::data(&[0x02, 0x01, 0x06])?,
        )?;
        hci::command(self.capture, hci::LE_SET_ADVERTISING_ENABLE, &[1])?;
        Ok(())
    }

    /// Pump one connection until it ended as `termination` asks and the
    /// helper has finished.
    fn serve(
        &self,
        termination: BluetoothPeripheralTermination,
        cycle: &mut Cycle,
        helper_finished: impl Fn() -> bool,
    ) -> Result<()> {
        let started = Instant::now();
        let mut link = Link::default();
        loop {
            if started.elapsed() > CYCLE_DEADLINE {
                return Err("the connection cycle exceeded its deadline".into());
            }
            if let Some(handle) = link.handle
                && let Some(until) = link.held_until
                && Instant::now() >= until
            {
                hci::return_credits(self.capture, handle, 1)?;
                link.held_until = None;
                cycle.credits_returned += 1;
            }
            if link.ended && helper_finished() {
                return link.finish(termination, cycle);
            }
            // A local termination waits until the Controller has sent the
            // second echo, so that the peer receives it first.
            if link.echoes == ECHOES && !link.controller_busy && !link.terminated_locally {
                match termination {
                    BluetoothPeripheralTermination::TargetDisconnect => {
                        let handle = link.handle.ok_or("echoes without a connection")?;
                        let [low, high] = handle.to_le_bytes();
                        let status = hci::command_status(
                            self.capture,
                            hci::DISCONNECT,
                            &[low, high, REMOTE_USER_TERMINATED],
                        )?;
                        if status != 0 {
                            return Err(format!("Disconnect returned status {status:#04x}").into());
                        }
                    }
                    BluetoothPeripheralTermination::TargetReset => {
                        hci::command(self.capture, hci::RESET, &[])?;
                        link.ended = true;
                    }
                    BluetoothPeripheralTermination::PeerReset
                    | BluetoothPeripheralTermination::PeerRfkill => {}
                }
                link.terminated_locally = true;
            }
            let wait = if link.held_until.is_some() {
                Duration::from_millis(20)
            } else {
                Duration::from_millis(200)
            };
            let Some(packet) = hci::next_packet(self.capture, wait)? else {
                if link.ended && !helper_finished() {
                    std::thread::sleep(Duration::from_millis(50));
                }
                continue;
            };
            match packet {
                Packet::Event(event) => link.event(&event, cycle)?,
                Packet::Acl(packet) => {
                    let (handle, frame) = link.acl(&packet, cycle)?;
                    if link.held_until.is_none() {
                        hci::return_credits(self.capture, handle, 1)?;
                        cycle.credits_returned += 1;
                    }
                    if let Some(frame) = frame {
                        self.echo(&mut link, handle, &frame, cycle)?;
                    }
                }
            }
        }
    }

    /// Send one frame back as one ACL packet once the Controller has a
    /// buffer for it.
    fn echo(&self, link: &mut Link, handle: u16, frame: &[u8], cycle: &mut Cycle) -> Result<()> {
        let deadline = Instant::now() + Duration::from_secs(5);
        while link.controller_busy {
            if Instant::now() > deadline {
                return Err("the Controller returned no ACL buffer for the echo".into());
            }
            match hci::next_packet(self.capture, Duration::from_millis(200))? {
                Some(Packet::Event(event)) => link.event(&event, cycle)?,
                Some(Packet::Acl(_)) => {
                    return Err("ACL data arrived before the previous echo completed".into());
                }
                None => {}
            }
        }
        let mut packet = Vec::with_capacity(4 + frame.len());
        packet.extend_from_slice(&(handle | (HOST_START << 12)).to_le_bytes());
        packet.extend_from_slice(&(frame.len() as u16).to_le_bytes());
        packet.extend_from_slice(frame);
        hci::acl(self.capture, &packet)?;
        link.controller_busy = true;
        link.echoes += 1;
        cycle.echoes += 1;
        Ok(())
    }
}

/// Host state of one connection.
#[derive(Default)]
struct Link {
    handle: Option<u16>,
    frame: Vec<u8>,
    fragments: u32,
    held_until: Option<Instant>,
    /// An echo occupies the Controller's buffer until its completion.
    controller_busy: bool,
    echoes: u32,
    terminated_locally: bool,
    ended: bool,
    reason: Option<u8>,
}

impl Link {
    fn event(&mut self, event: &[u8], cycle: &mut Cycle) -> Result<()> {
        let parameters = event.get(2..).unwrap_or_default();
        match (event.first().copied(), parameters.first().copied()) {
            (Some(LE_META), Some(LE_CONNECTION_COMPLETE)) => {
                // Subevent, status, handle, role, peer kind, peer, interval.
                if parameters.len() < 18 {
                    return Err(format!("short LE Connection Complete {event:02x?}").into());
                }
                if self.handle.is_some() {
                    return Err("a second connection completed on the same cycle".into());
                }
                let status = parameters[1];
                if status != 0 || parameters[4] != PERIPHERAL_ROLE {
                    return Err(format!("the connection failed: {event:02x?}").into());
                }
                let handle = u16::from_le_bytes([parameters[2], parameters[3]]);
                self.handle = Some(handle);
                cycle.handle = Some(handle);
                cycle.central =
                    Some(PeerAddress(parameters[6..12].try_into().expect("six")).to_string());
                cycle.interval = Some(u16::from_le_bytes([parameters[12], parameters[13]]));
            }
            (Some(LE_META), Some(LE_CONNECTION_UPDATE_COMPLETE)) => {
                if parameters.len() < 10 {
                    return Err(format!("short LE Connection Update Complete {event:02x?}").into());
                }
                let interval = u16::from_le_bytes([parameters[4], parameters[5]]);
                if parameters[1] != 0
                    || Some(u16::from_le_bytes([parameters[2], parameters[3]])) != self.handle
                {
                    return Err(format!("the connection update failed: {event:02x?}").into());
                }
                cycle.updated_interval = Some(interval);
            }
            (Some(NUMBER_OF_COMPLETED_PACKETS), Some(count)) => {
                for index in 0..usize::from(count) {
                    let entry = parameters
                        .get(1 + index * 4..5 + index * 4)
                        .ok_or("short Number Of Completed Packets")?;
                    if Some(u16::from_le_bytes([entry[0], entry[1]])) == self.handle
                        && entry[2..4] != [0, 0]
                    {
                        self.controller_busy = false;
                    }
                }
            }
            (Some(DISCONNECTION_COMPLETE), _) => {
                // Status, handle, reason.
                if parameters.len() < 4
                    || parameters[0] != 0
                    || Some(u16::from_le_bytes([parameters[1], parameters[2]])) != self.handle
                {
                    return Err(format!("unexpected Disconnection Complete {event:02x?}").into());
                }
                self.reason = Some(parameters[3]);
                cycle.reason = Some(parameters[3]);
                self.ended = true;
            }
            _ => cycle.other_events += 1,
        }
        Ok(())
    }

    /// Consume one ACL fragment; returns its handle and the frame it
    /// completed.
    fn acl(&mut self, packet: &[u8], cycle: &mut Cycle) -> Result<(u16, Option<Vec<u8>>)> {
        let header = packet.get(0..4).ok_or("short ACL packet")?;
        let raw = u16::from_le_bytes([header[0], header[1]]);
        let length = usize::from(u16::from_le_bytes([header[2], header[3]]));
        let data = &packet[4..];
        let handle = raw & 0x0fff;
        if data.len() != length || Some(handle) != self.handle {
            return Err(format!("ACL fragment {header:02x?} does not match the connection").into());
        }
        if data.len() > usize::from(HOST_ACL_BYTES) {
            return Err("the Controller exceeded the Host ACL buffer".into());
        }
        cycle.fragments += 1;
        match raw >> 12 & 0b11 {
            CONTROLLER_START if self.frame.is_empty() => {
                self.fragments = 0;
                self.held_until = Some(Instant::now() + CREDIT_HOLD);
                cycle.credits_held += 1;
            }
            CONTINUATION if !self.frame.is_empty() => {}
            flag => return Err(format!("ACL fragment boundary {flag:#04b} out of order").into()),
        }
        self.frame.extend_from_slice(data);
        self.fragments += 1;
        let expected = self
            .frame
            .get(0..2)
            .map(|length| 4 + usize::from(u16::from_le_bytes([length[0], length[1]])));
        match expected {
            Some(expected) if self.frame.len() > expected => {
                Err("ACL fragments exceed their L2CAP length".into())
            }
            Some(expected) if self.frame.len() == expected => {
                if self.fragments != FRAGMENTS_PER_FRAME {
                    return Err(format!(
                        "a frame arrived in {} fragments, not {FRAGMENTS_PER_FRAME}",
                        self.fragments
                    )
                    .into());
                }
                Ok((handle, Some(std::mem::take(&mut self.frame))))
            }
            _ => Ok((handle, None)),
        }
    }

    fn finish(&self, termination: BluetoothPeripheralTermination, cycle: &Cycle) -> Result<()> {
        if self.echoes != ECHOES {
            return Err(format!(
                "{} of {ECHOES} echoes before the connection ended",
                self.echoes
            )
            .into());
        }
        if cycle.updated_interval != Some(UPDATED_INTERVAL) {
            return Err(format!(
                "the connection update to {UPDATED_INTERVAL} was not observed: {:?}",
                cycle.updated_interval
            )
            .into());
        }
        let reasons: &[u8] = match termination {
            BluetoothPeripheralTermination::PeerReset => &[SUPERVISION_TIMEOUT],
            BluetoothPeripheralTermination::PeerRfkill => {
                &[SUPERVISION_TIMEOUT, REMOTE_USER_TERMINATED]
            }
            BluetoothPeripheralTermination::TargetDisconnect => &[LOCAL_HOST_TERMINATED],
            BluetoothPeripheralTermination::TargetReset => &[],
        };
        match self.reason {
            Some(reason) if reasons.contains(&reason) => Ok(()),
            None if reasons.is_empty() => Ok(()),
            reason => Err(format!("the connection ended with reason {reason:?}").into()),
        }
    }
}

#[derive(Serialize)]
struct Report {
    schema: u8,
    adapter: String,
    connections: u8,
    hold_millis: u16,
    termination: BluetoothPeripheralTermination,
    address: Option<String>,
    cycles: Vec<Cycle>,
    passed: bool,
    error: Option<String>,
    cleanup_error: Option<String>,
}

/// What the Host observed on one connection.
#[derive(Debug, Default, Serialize)]
struct Cycle {
    handle: Option<u16>,
    central: Option<String>,
    interval: Option<u16>,
    updated_interval: Option<u16>,
    fragments: u32,
    credits_held: u32,
    credits_returned: u32,
    echoes: u32,
    reason: Option<u8>,
    other_events: u32,
    error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use oer_hil_protocol::bluetooth_peripheral_acl_payload;

    const HANDLE: u16 = 0x0001;

    fn connected() -> (Link, Cycle) {
        let mut link = Link::default();
        let mut cycle = Cycle::default();
        let mut complete = vec![
            LE_META,
            19,
            LE_CONNECTION_COMPLETE,
            0,
            1,
            0,
            PERIPHERAL_ROLE,
            0,
        ];
        complete.extend_from_slice(&[0x11; 6]);
        complete.extend_from_slice(&[80, 0, 0, 0, 200, 0, 0]);
        link.event(&complete, &mut cycle).unwrap();
        (link, cycle)
    }

    fn fragments(payload: &[u8]) -> Vec<Vec<u8>> {
        payload
            .chunks(usize::from(HOST_ACL_BYTES))
            .enumerate()
            .map(|(index, chunk)| {
                let flag = if index == 0 {
                    CONTROLLER_START
                } else {
                    CONTINUATION
                };
                let mut packet = (HANDLE | flag << 12).to_le_bytes().to_vec();
                packet.extend_from_slice(&(chunk.len() as u16).to_le_bytes());
                packet.extend_from_slice(chunk);
                packet
            })
            .collect()
    }

    #[test]
    fn a_frame_reassembles_from_its_exact_fragments_and_holds_the_first_credit() {
        let (mut link, mut cycle) = connected();
        let payload = bluetooth_peripheral_acl_payload();
        let packets = fragments(&payload);
        assert_eq!(packets.len() as u32, FRAGMENTS_PER_FRAME);
        let mut completed = None;
        for (index, packet) in packets.iter().enumerate() {
            let (handle, frame) = link.acl(packet, &mut cycle).unwrap();
            assert_eq!(handle, HANDLE);
            if index == 0 {
                assert!(link.held_until.is_some());
            }
            completed = frame;
        }
        assert_eq!(completed.as_deref(), Some(&payload[..]));
        assert_eq!(cycle.credits_held, 1);
        assert_eq!(cycle.fragments, FRAGMENTS_PER_FRAME);
    }

    #[test]
    fn out_of_order_boundaries_and_foreign_handles_fail() {
        let (mut link, mut cycle) = connected();
        let packets = fragments(&bluetooth_peripheral_acl_payload());
        assert!(link.acl(&packets[1], &mut cycle).is_err());
        let (mut link, mut cycle) = connected();
        let mut foreign = packets[0].clone();
        foreign[0] = 2;
        assert!(link.acl(&foreign, &mut cycle).is_err());
    }

    #[test]
    fn completed_packets_free_the_controller_buffer_only_for_the_connection() {
        let (mut link, mut cycle) = connected();
        link.controller_busy = true;
        link.event(&[NUMBER_OF_COMPLETED_PACKETS, 5, 1, 2, 0, 1, 0], &mut cycle)
            .unwrap();
        assert!(link.controller_busy);
        link.event(&[NUMBER_OF_COMPLETED_PACKETS, 5, 1, 1, 0, 1, 0], &mut cycle)
            .unwrap();
        assert!(!link.controller_busy);
    }

    #[test]
    fn each_termination_requires_its_own_disconnection_reason() {
        let (mut link, mut cycle) = connected();
        link.echoes = ECHOES;
        cycle.updated_interval = Some(UPDATED_INTERVAL);
        link.event(
            &[DISCONNECTION_COMPLETE, 4, 0, 1, 0, SUPERVISION_TIMEOUT],
            &mut cycle,
        )
        .unwrap();
        assert!(
            link.finish(BluetoothPeripheralTermination::PeerReset, &cycle)
                .is_ok()
        );
        assert!(
            link.finish(BluetoothPeripheralTermination::PeerRfkill, &cycle)
                .is_ok()
        );
        assert!(
            link.finish(BluetoothPeripheralTermination::TargetDisconnect, &cycle)
                .is_err()
        );
        assert!(
            link.finish(BluetoothPeripheralTermination::TargetReset, &cycle)
                .is_err()
        );
        let (mut link, cycle) = connected();
        link.echoes = ECHOES;
        let cycle = Cycle {
            updated_interval: Some(UPDATED_INTERVAL),
            ..cycle
        };
        assert!(
            link.finish(BluetoothPeripheralTermination::TargetReset, &cycle)
                .is_ok()
        );
        link.echoes = 1;
        assert!(
            link.finish(BluetoothPeripheralTermination::TargetReset, &cycle)
                .is_err()
        );
    }

    #[test]
    fn a_failed_connection_or_update_is_an_error() {
        let mut link = Link::default();
        let mut cycle = Cycle::default();
        let mut failed = vec![
            LE_META,
            19,
            LE_CONNECTION_COMPLETE,
            0x3e,
            1,
            0,
            PERIPHERAL_ROLE,
            0,
        ];
        failed.extend_from_slice(&[0; 13]);
        assert!(link.event(&failed, &mut cycle).is_err());
        let (mut link, mut cycle) = connected();
        let update = [
            LE_META,
            10,
            LE_CONNECTION_UPDATE_COMPLETE,
            0x3b,
            1,
            0,
            96,
            0,
            0,
            0,
            200,
            0,
        ];
        assert!(link.event(&update, &mut cycle).is_err());
    }
}
