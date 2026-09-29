//! A Thread exchange between the device's OpenThread radio and ESP-IDF's
//! OpenThread stack.
//!
//! The Thread reference peer (`hil/peers/esp32c5-openthread`) forms a new
//! network as its leader. One Thread session on the device under test covers
//! these cells:
//!
//! 1. with the peer's active dataset, OpenThread over the device's radio
//!    attaches to the network as the leader's child and takes a mesh-local
//!    EID;
//! 2. the device sends UDP datagrams to the peer's mesh-local EID: each
//!    arrives intact from the device's EID;
//! 3. the peer sends UDP datagrams to the device's mesh-local EID: the device
//!    receives each intact from the peer's EID;
//! 4. the session stops and the client leaves the shared radio.

use std::{
    fs,
    net::Ipv6Addr,
    path::Path,
    time::{Duration, Instant},
};

use hil_core::{context::Context, session::SerialCapture};
use oer_hil_protocol::{
    ieee802154::Ieee802154SessionResult, ieee802154::Ieee802154ThreadDataset,
    ieee802154::Ieee802154ThreadPayload, ieee802154::Ieee802154ThreadReceiveEvidence,
    ieee802154::Ieee802154ThreadRole, ieee802154::Ieee802154ThreadSendRequest,
    ieee802154::Ieee802154ThreadStartRequest,
};
use serde::Serialize;

use super::peer_exchange::{CAPABILITIES_TIMEOUT, COMMAND_TIMEOUT, START_TIMEOUT};
use crate::{
    Result,
    peer::{PEER_TRANSCRIPT, PeerLink, PeerTranscript},
    thread_peer::{ThreadDatagram, ThreadPeer},
};

const REPORT_NAME: &str = "ieee802154-thread-exchange.json";
/// The port both sockets bind.
pub const UDP_PORT: u16 = 1212;
/// A new network elects its leader after about one leader-election period.
const LEADER_TIMEOUT: Duration = Duration::from_secs(20);
/// An end device attaches within a few MLE parent-request rounds.
const ATTACH_TIMEOUT: Duration = Duration::from_secs(60);
const POLL_INTERVAL: Duration = Duration::from_millis(500);
/// Bound on one datagram's delivery over one hop.
const DELIVERY_TIMEOUT: Duration = Duration::from_secs(5);

pub struct Config {
    pub boots: u8,
    pub channel: u8,
    pub pan_id: u16,
    pub datagrams: u8,
}

/// The payload of datagram `index` in one direction.
pub fn payload(direction: &str, index: u8) -> Vec<u8> {
    format!("OER-THREAD-{direction}-{index}").into_bytes()
}

#[derive(Serialize)]
struct BootReport {
    boot: u8,
    peer_eid: String,
    device_eid: String,
    device_rloc16: String,
    attach_millis: u128,
    device_to_peer: Vec<String>,
    peer_to_device: Vec<String>,
}

pub fn run(config: Config, output: &Path, context: &Context<'_>) -> Result<()> {
    fs::create_dir_all(output)?;
    let peer_config = context.lab.peer.clone().ok_or("the lab has no [peer]")?;
    let mut reports = Vec::new();
    for boot in 1..=config.boots {
        let boot_output = output.join(format!("boot-{boot:03}"));
        let transcript = PeerTranscript::default();
        let result = context.with_capture(&boot_output, |capture| {
            let mut peer = ThreadPeer::open_recorded(&peer_config.serial()?, &transcript)?;
            exchange(capture, &mut peer, &config, boot)
        });
        transcript.save(&boot_output.join(PEER_TRANSCRIPT))?;
        match result {
            Ok(report) => reports.push(report),
            Err(error) => {
                write_report(output, &reports, Some(&error.to_string()))?;
                return Err(error);
            }
        }
    }
    write_report(output, &reports, None)?;
    println!(
        "ieee802154_thread_exchange=PASS boots={} datagrams={}",
        config.boots, config.datagrams
    );
    Ok(())
}

fn write_report(output: &Path, reports: &[BootReport], failure: Option<&str>) -> Result<()> {
    let document = match failure {
        None => serde_json::json!({
            "schema": 1,
            "status": "passed",
            "result": "attached-child-and-udp-both-ways-with-vendor-openthread",
            "not_proven": [
                "router-role",
                "sleepy-end-device",
                "csl",
                "commissioning",
                "multi-hop",
            ],
            "boots": reports,
        }),
        Some(failure) => serde_json::json!({
            "schema": 1,
            "status": "failed",
            "failure": failure,
            "boots": reports,
        }),
    };
    fs::write(
        output.join(REPORT_NAME),
        serde_json::to_vec_pretty(&document)?,
    )?;
    Ok(())
}

fn expect(step: &str, result: Ieee802154SessionResult) -> Result<()> {
    if result == Ieee802154SessionResult::Done {
        Ok(())
    } else {
        Err(format!("Thread session {step} ended {result:?}").into())
    }
}

/// The datagram arrived intact, from `source`.
pub fn check_datagram(
    datagram: Option<ThreadDatagram>,
    source: Ipv6Addr,
    expected: &[u8],
) -> Result<()> {
    let datagram = datagram.ok_or("the peer received no datagram")?;
    if datagram.source != source || datagram.port != UDP_PORT || datagram.payload != expected {
        return Err(format!(
            "the peer received {:?} from [{}]:{}, expected {:?} from [{source}]:{UDP_PORT}",
            String::from_utf8_lossy(&datagram.payload),
            datagram.source,
            datagram.port,
            String::from_utf8_lossy(expected),
        )
        .into());
    }
    Ok(())
}

/// The device received exactly `expected`, in order, each from `source`.
pub fn check_device_received(
    evidence: &Ieee802154ThreadReceiveEvidence,
    source: Ipv6Addr,
    expected: &[Vec<u8>],
) -> Result<()> {
    expect("collection", evidence.result)?;
    let received: Vec<(Ipv6Addr, u16, &[u8])> = evidence
        .datagrams
        .iter()
        .map(|datagram| {
            (
                Ipv6Addr::from(datagram.source),
                datagram.port,
                datagram.payload.as_slice(),
            )
        })
        .collect();
    let wanted: Vec<(Ipv6Addr, u16, &[u8])> = expected
        .iter()
        .map(|payload| (source, UDP_PORT, payload.as_slice()))
        .collect();
    if usize::from(evidence.total) != expected.len() || received != wanted {
        return Err(format!(
            "the device received {} datagrams {received:?}, expected {wanted:?}",
            evidence.total
        )
        .into());
    }
    Ok(())
}

fn wait_for<T>(
    timeout: Duration,
    what: &str,
    mut poll: impl FnMut() -> Result<Option<T>>,
) -> Result<T> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(value) = poll()? {
            return Ok(value);
        }
        if Instant::now() >= deadline {
            return Err(format!("{what} within {timeout:?}").into());
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

fn exchange<L: PeerLink>(
    capture: &SerialCapture,
    peer: &mut ThreadPeer<L>,
    config: &Config,
    boot: u8,
) -> Result<BootReport> {
    let capabilities = capture.request_capabilities(CAPABILITIES_TIMEOUT)?;
    if !capabilities.has::<oer_hil_protocol::ieee802154::Thread>() {
        return Err("firmware does not advertise Thread sessions".into());
    }
    peer.form(config.channel, config.pan_id)?;
    let leader = wait_for(LEADER_TIMEOUT, "the peer did not lead its network", || {
        let state = peer.state()?;
        Ok((state.role == "leader").then_some(state))
    })?;
    let dataset = Ieee802154ThreadDataset::from_slice(&peer.dataset()?)
        .map_err(|_| "the peer's dataset exceeds the session capacity")?;
    peer.open_udp(UDP_PORT)?;

    let started = Instant::now();
    expect(
        "start",
        capture.start_ieee802154_thread(
            Ieee802154ThreadStartRequest {
                dataset,
                rx_on_when_idle: true,
                udp_port: UDP_PORT,
            },
            START_TIMEOUT,
        )?,
    )?;
    let attached = wait_for(ATTACH_TIMEOUT, "the device did not attach", || {
        let state = capture.query_ieee802154_thread(COMMAND_TIMEOUT)?;
        expect("query", state.result)?;
        Ok(match (state.role, state.mesh_local_eid) {
            (Ieee802154ThreadRole::Child, Some(eid)) => Some((state.rloc16, eid)),
            _ => None,
        })
    })?;
    let attach_millis = started.elapsed().as_millis();
    let (device_rloc16, device_eid) = (attached.0, Ipv6Addr::from(attached.1));

    let mut device_to_peer = Vec::new();
    for index in 0..config.datagrams {
        let sent = payload("D2P", index);
        expect(
            "send",
            capture.send_ieee802154_thread(
                Ieee802154ThreadSendRequest {
                    destination: leader.eid.octets(),
                    port: UDP_PORT,
                    payload: Ieee802154ThreadPayload::from_slice(&sent)
                        .map_err(|_| "payload exceeds the session capacity")?,
                },
                COMMAND_TIMEOUT,
            )?,
        )?;
        check_datagram(peer.next_datagram(DELIVERY_TIMEOUT)?, device_eid, &sent)?;
        device_to_peer.push(format!("#{index} delivered"));
    }

    let mut expected = Vec::new();
    for index in 0..config.datagrams {
        let sent = payload("P2D", index);
        peer.send_udp(device_eid, UDP_PORT, &sent)?;
        expected.push(sent);
    }
    let mut received = Ieee802154ThreadReceiveEvidence::default();
    let deadline = Instant::now() + DELIVERY_TIMEOUT;
    while usize::from(received.total) < expected.len() && Instant::now() < deadline {
        std::thread::sleep(POLL_INTERVAL);
        let collected = capture.collect_ieee802154_thread(COMMAND_TIMEOUT)?;
        received.result = collected.result;
        received.total = received.total.saturating_add(collected.total);
        for datagram in collected.datagrams {
            let _ = received.datagrams.push(datagram);
        }
    }
    check_device_received(&received, leader.eid, &expected)?;
    let peer_to_device = (0..config.datagrams)
        .map(|index| format!("#{index} delivered"))
        .collect();

    expect("stop", capture.stop_ieee802154_thread(START_TIMEOUT)?)?;
    Ok(BootReport {
        boot,
        peer_eid: leader.eid.to_string(),
        device_eid: device_eid.to_string(),
        device_rloc16: format!("{device_rloc16:#06x}"),
        attach_millis,
        device_to_peer,
        peer_to_device,
    })
}

#[cfg(test)]
mod tests;
