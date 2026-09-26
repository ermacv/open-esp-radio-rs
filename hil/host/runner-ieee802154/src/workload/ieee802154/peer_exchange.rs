//! IEEE 802.15.4 exchange between the device under test and the reference
//! peer.
//!
//! The ESP32-C5 peer runs the vendor driver (`hil/peers/esp32c5-ieee802154`).
//! One session on the device under test covers four cells on one channel and
//! PAN:
//!
//! 1. the device transmits data frames requesting acknowledgement: each is
//!    acknowledged with its sequence number and arrives at the peer intact;
//! 2. the peer transmits data frames to the device: the device acknowledges
//!    each, and reports the same frames by digest;
//! 3. the peer transmits to another short address: the device neither
//!    acknowledges nor reports it;
//! 4. with the peer's short address in the device's pending table, the
//!    device's acknowledgement of the peer's data request carries frame
//!    pending.

use std::{fs, path::Path, time::Duration};

use hil_core::{context::Context, session::SerialCapture};
use oer_hil_protocol::{
    Ieee802154AirTxOutcome, Ieee802154SessionConfig, Ieee802154SessionFrame,
    Ieee802154SessionPendingMode, Ieee802154SessionPendingRequest,
    Ieee802154SessionReceiveEvidence, Ieee802154SessionResult, Ieee802154SessionTransmitEvidence,
    Ieee802154SessionTransmitRequest, ieee802154_frame_crc32c,
};
use serde::Serialize;

use crate::{
    Result,
    peer::{Peer, PeerConfig, PeerEvent, PeerLink},
};

const CAPABILITIES_TIMEOUT: Duration = Duration::from_secs(10);
/// Session start registers and calibrates the shared PHY.
const START_TIMEOUT: Duration = Duration::from_secs(30);
const COMMAND_TIMEOUT: Duration = Duration::from_secs(5);
/// Bound on one peer report after its trigger.
const PEER_EVENT_TIMEOUT: Duration = Duration::from_secs(1);
const REPORT_NAME: &str = "ieee802154-peer-exchange.json";

pub const PAN_ID: u16 = 0x4f45;
pub const DEVICE_SHORT: u16 = 0x0001;
pub const PEER_SHORT: u16 = 0x0002;
/// A short address nobody in the cell owns.
pub const FOREIGN_SHORT: u16 = 0x0099;
const DEVICE_EXTENDED: [u8; 8] = [0x01, 0x54, 0x31, 0x53, 0x33, 0x32, 0x65, 0x4f];
const PEER_EXTENDED: [u8; 8] = [0x02, 0x54, 0x31, 0x43, 0x35, 0x70, 0x65, 0x4f];

/// Sequence numbers of peer-originated frames start here, apart from the
/// device's.
const PEER_SEQUENCE_BASE: u8 = 0x80;

pub struct Config {
    pub boots: u8,
    pub channel: u8,
    pub frames: u8,
}

/// A data frame with PAN ID compression and short addresses.
pub fn data_frame(ack_request: bool, sequence: u8, destination: u16, source: u16) -> Vec<u8> {
    let control: u16 = 0x8841 | if ack_request { 0x0020 } else { 0 };
    let mut frame = control.to_le_bytes().to_vec();
    frame.push(sequence);
    frame.extend_from_slice(&PAN_ID.to_le_bytes());
    frame.extend_from_slice(&destination.to_le_bytes());
    frame.extend_from_slice(&source.to_le_bytes());
    frame.extend_from_slice(b"OER-154-");
    frame.push(sequence);
    frame
}

/// A data-request MAC command requesting acknowledgement.
pub fn data_request(sequence: u8, destination: u16, source: u16) -> Vec<u8> {
    let mut frame = 0x8863_u16.to_le_bytes().to_vec();
    frame.push(sequence);
    frame.extend_from_slice(&PAN_ID.to_le_bytes());
    frame.extend_from_slice(&destination.to_le_bytes());
    frame.extend_from_slice(&source.to_le_bytes());
    frame.push(0x04);
    frame
}

/// An immediate acknowledgement of `sequence`, and its frame-pending bit.
pub fn acknowledgement(frame: &[u8], sequence: u8) -> Option<bool> {
    match frame {
        [control, 0x00, ack_sequence] if control & 0x07 == 0x02 && *ack_sequence == sequence => {
            Some(control & 0x10 != 0)
        }
        _ => None,
    }
}

#[derive(Serialize)]
struct BootReport {
    boot: u8,
    device_to_peer: Vec<String>,
    peer_to_device: Vec<String>,
    filtered: String,
    pending: String,
}

pub fn run(config: Config, output: &Path, context: &Context<'_>) -> Result<()> {
    fs::create_dir_all(output)?;
    let peer_config = context
        .lab
        .ieee802154_peer
        .clone()
        .ok_or("the lab has no [ieee802154_peer]")?;
    let mut reports = Vec::new();
    for boot in 1..=config.boots {
        let boot_output = output.join(format!("boot-{boot:03}"));
        let result = context.with_capture(&boot_output, |capture| {
            let mut peer = Peer::open(&peer_config.serial)?;
            exchange(capture, &mut peer, &config, boot)
        });
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
        "ieee802154_peer_exchange=PASS boots={} frames={}",
        config.boots, config.frames
    );
    Ok(())
}

fn write_report(output: &Path, reports: &[BootReport], failure: Option<&str>) -> Result<()> {
    let document = match failure {
        None => serde_json::json!({
            "schema": 1,
            "status": "passed",
            "result": "acknowledged-exchange-filtering-and-pending-with-vendor-peer",
            "not_proven": [
                "enhanced-acknowledgement",
                "csma-ca",
                "security",
                "calibrated-output-power",
                "timed-exchange",
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

fn session_frame(bytes: &[u8]) -> Result<Ieee802154SessionFrame> {
    Ieee802154SessionFrame::from_slice(bytes)
        .map_err(|_| "frame exceeds the session capacity".into())
}

fn exchange<L: PeerLink>(
    capture: &SerialCapture,
    peer: &mut Peer<L>,
    config: &Config,
    boot: u8,
) -> Result<BootReport> {
    let capabilities = capture.request_capabilities(CAPABILITIES_TIMEOUT)?;
    if !capabilities.features.ieee802154_session {
        return Err("firmware does not advertise IEEE 802.15.4 sessions".into());
    }
    peer.configure(&PeerConfig {
        channel: config.channel,
        pan_id: PAN_ID,
        short_address: PEER_SHORT,
        extended_address: PEER_EXTENDED,
        promiscuous: false,
        power_dbm: 0,
    })?;
    peer.receive()?;
    expect_session(
        "start",
        capture.start_ieee802154_session(
            Ieee802154SessionConfig {
                channel: config.channel,
                pan_id: PAN_ID,
                short_address: DEVICE_SHORT,
                extended_address: DEVICE_EXTENDED,
                promiscuous: false,
            },
            START_TIMEOUT,
        )?,
    )?;

    let mut device_to_peer = Vec::new();
    for sequence in 0..config.frames {
        let frame = data_frame(true, sequence, PEER_SHORT, DEVICE_SHORT);
        let evidence = capture.transmit_ieee802154_session(
            Ieee802154SessionTransmitRequest {
                frame: session_frame(&frame)?,
                cca: false,
            },
            COMMAND_TIMEOUT,
        )?;
        check_device_transmit(&evidence, sequence)?;
        check_peer_received(peer.next_event(PEER_EVENT_TIMEOUT)?, &frame)?;
        device_to_peer.push(format!("seq={sequence} acknowledged delivered"));
    }

    capture.receive_ieee802154_session()?;
    let mut sent = Vec::new();
    let mut peer_to_device = Vec::new();
    for index in 0..config.frames {
        let sequence = PEER_SEQUENCE_BASE + index;
        let frame = data_frame(true, sequence, DEVICE_SHORT, PEER_SHORT);
        peer.transmit(false, &frame)?;
        check_peer_acknowledged(peer.next_event(PEER_EVENT_TIMEOUT)?, sequence, false)?;
        sent.push(frame);
        peer_to_device.push(format!("seq={sequence} acknowledged"));
    }
    check_device_received(&capture.collect_ieee802154_session()?, &sent)?;

    let foreign = data_frame(true, 0x70, FOREIGN_SHORT, PEER_SHORT);
    peer.transmit(false, &foreign)?;
    check_peer_unacknowledged(peer.next_event(PEER_EVENT_TIMEOUT)?)?;
    check_device_received(&capture.collect_ieee802154_session()?, &[])?;

    capture.set_ieee802154_session_pending(Ieee802154SessionPendingRequest {
        mode: Ieee802154SessionPendingMode::Enabled,
        short_address: Some(PEER_SHORT),
    })?;
    let request = data_request(0x71, DEVICE_SHORT, PEER_SHORT);
    peer.transmit(false, &request)?;
    check_peer_acknowledged(peer.next_event(PEER_EVENT_TIMEOUT)?, 0x71, true)?;
    let _ = capture.collect_ieee802154_session()?;

    expect_session("stop", capture.stop_ieee802154_session(COMMAND_TIMEOUT)?)?;
    peer.sleep()?;
    Ok(BootReport {
        boot,
        device_to_peer,
        peer_to_device,
        filtered: String::from("foreign destination unacknowledged and unreported"),
        pending: String::from("data request acknowledged with frame pending"),
    })
}

fn expect_session(step: &str, result: Ieee802154SessionResult) -> Result<()> {
    if result == Ieee802154SessionResult::Done {
        Ok(())
    } else {
        Err(format!("IEEE 802.15.4 session {step} ended {result:?}").into())
    }
}

pub(crate) fn check_device_transmit(
    evidence: &Ieee802154SessionTransmitEvidence,
    sequence: u8,
) -> Result<()> {
    expect_session("transmit", evidence.result)?;
    if evidence.outcome != Ieee802154AirTxOutcome::Success {
        return Err(format!(
            "device transmit seq={sequence} ended {:?}",
            evidence.outcome
        )
        .into());
    }
    match evidence
        .acknowledgement
        .as_ref()
        .and_then(|ack| acknowledgement(&ack.frame, sequence))
    {
        Some(false) => Ok(()),
        Some(true) => Err(format!("peer acknowledged seq={sequence} with frame pending").into()),
        None => {
            Err(format!("device transmit seq={sequence} has no matching acknowledgement").into())
        }
    }
}

pub(crate) fn check_peer_received(event: Option<PeerEvent>, frame: &[u8]) -> Result<()> {
    match event {
        Some(PeerEvent::Received(received)) if received.bytes == frame => Ok(()),
        other => Err(format!("peer did not receive the device frame: {other:?}").into()),
    }
}

pub(crate) fn check_peer_acknowledged(
    event: Option<PeerEvent>,
    sequence: u8,
    pending: bool,
) -> Result<()> {
    match event {
        Some(PeerEvent::Transmitted {
            acknowledgement: Some(ack),
        }) if acknowledgement(&ack.bytes, sequence) == Some(pending) => Ok(()),
        other => Err(format!(
            "device acknowledgement of seq={sequence} (pending={pending}) missing: {other:?}"
        )
        .into()),
    }
}

pub(crate) fn check_peer_unacknowledged(event: Option<PeerEvent>) -> Result<()> {
    match event {
        Some(PeerEvent::TransmitFailed(_)) => Ok(()),
        other => Err(format!("a foreign-address frame was acknowledged: {other:?}").into()),
    }
}

pub(crate) fn check_device_received(
    evidence: &Ieee802154SessionReceiveEvidence,
    sent: &[Vec<u8>],
) -> Result<()> {
    expect_session("collect", evidence.result)?;
    if usize::from(evidence.total) != sent.len() || evidence.frames.len() != sent.len() {
        return Err(format!(
            "device received {} frames, expected {}",
            evidence.total,
            sent.len()
        )
        .into());
    }
    for (index, (received, frame)) in evidence.frames.iter().zip(sent).enumerate() {
        if usize::from(received.length) != frame.len()
            || received.crc32c != ieee802154_frame_crc32c(frame)
        {
            return Err(format!("device frame {index} differs from the peer's").into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
