//! The live RSSI read (`esp_ieee802154_get_recent_rssi`) against frames of
//! the reference peer at two transmit powers.
//!
//! The reference peer (`hil/peers/esp32c5-ieee802154`) sends data frames to
//! the device, first at a high and then at a low transmit power, which stands
//! in for two distances. One session on the device under test covers these
//! cells:
//!
//! 1. after each series the device reads the live RSSI of the most recent
//!    baseband reception, which is the series' last frame: it agrees with
//!    the RSSI the device reported for that frame;
//! 2. the live RSSI of the strong series exceeds the weak series' by at
//!    least a fixed margin: the read follows the received signal, not a
//!    constant.

use std::{fs, path::Path};

use oer_hil_execution::context::Context;
use oer_hil_link::SerialCapture;
use oer_hil_protocol::{
    ieee802154::Ieee802154SessionConfig, ieee802154::Ieee802154SessionMaintenancePolicy,
    ieee802154::Ieee802154SessionReceiveEvidence, ieee802154::ieee802154_frame_crc32c,
};
use serde::Serialize;

use super::peer_exchange::{
    COMMAND_TIMEOUT, DEVICE_EXTENDED, DEVICE_SHORT, IMAGE_KEYS_TIMEOUT, PAN_ID, PEER_EVENT_TIMEOUT,
    PEER_EXTENDED, PEER_SHORT, START_TIMEOUT, data_frame, expect_session,
};
use crate::{
    Result,
    peer::{PEER_TRANSCRIPT, Peer, PeerConfig, PeerEvent, PeerLink, PeerTranscript},
};

const REPORT_NAME: &str = "ieee802154-live-rssi.json";
/// The live read and the RSSI reported with the last frame differ by at most
/// this much.
const MAX_LIVE_FRAME_DB: i16 = 3;
/// The strong series' live RSSI exceeds the weak series' by at least this.
const MIN_POWER_STEP_DB: i16 = 10;
/// First sequence number of the frames of one series.
const SEQUENCE_BASE: u8 = 0x40;

pub struct Config {
    pub boots: u8,
    pub channel: u8,
    /// Frames per series.
    pub frames: u8,
    pub high_power_dbm: i8,
    pub low_power_dbm: i8,
}

/// What one series measured.
#[derive(Clone, Debug, Default, Serialize)]
pub struct Series {
    pub power_dbm: i8,
    /// RSSI the device reported for each frame of the series it received,
    /// in reception order.
    pub frame_rssi_dbm: Vec<i8>,
    /// The live RSSI read after the series.
    pub live_rssi_dbm: i8,
}

/// Everything one boot measured.
#[derive(Clone, Debug, Default, Serialize)]
pub struct Measurements {
    pub high: Series,
    pub low: Series,
}

/// The levels the cell decides on.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct Summary {
    pub high_live_dbm: i8,
    pub high_last_frame_dbm: i8,
    pub low_live_dbm: i8,
    pub low_last_frame_dbm: i8,
}

fn last_frame(step: &str, series: &Series) -> Result<i8> {
    series
        .frame_rssi_dbm
        .last()
        .copied()
        .ok_or_else(|| format!("the device received no {step} frame").into())
}

fn agrees(step: &str, live: i8, frame: i8) -> Result<()> {
    if (i16::from(live) - i16::from(frame)).abs() > MAX_LIVE_FRAME_DB {
        return Err(format!(
            "{step} live RSSI {live} dBm disagrees with its last frame's {frame} dBm"
        )
        .into());
    }
    Ok(())
}

/// Decide the cell from one boot's measurements.
pub fn evaluate(measurements: &Measurements) -> Result<Summary> {
    let summary = Summary {
        high_live_dbm: measurements.high.live_rssi_dbm,
        high_last_frame_dbm: last_frame("strong", &measurements.high)?,
        low_live_dbm: measurements.low.live_rssi_dbm,
        low_last_frame_dbm: last_frame("weak", &measurements.low)?,
    };
    agrees("strong", summary.high_live_dbm, summary.high_last_frame_dbm)?;
    agrees("weak", summary.low_live_dbm, summary.low_last_frame_dbm)?;
    if i16::from(summary.high_live_dbm) - i16::from(summary.low_live_dbm) < MIN_POWER_STEP_DB {
        return Err(format!(
            "live RSSI {} dBm at {} dBm is not {MIN_POWER_STEP_DB} dB above {} dBm at {} dBm",
            summary.high_live_dbm,
            measurements.high.power_dbm,
            summary.low_live_dbm,
            measurements.low.power_dbm
        )
        .into());
    }
    Ok(summary)
}

#[derive(Serialize)]
struct BootReport {
    boot: u8,
    summary: Summary,
    measurements: Measurements,
}

pub fn run(config: Config, output: &Path, context: &Context<'_>) -> Result<()> {
    fs::create_dir_all(output)?;
    let peer_config = context.lab.peer.clone().ok_or("the lab has no [peer]")?;
    let mut reports = Vec::new();
    for boot in 1..=config.boots {
        let boot_output = output.join(format!("boot-{boot:03}"));
        let transcript = PeerTranscript::default();
        let result = context.with_capture(&boot_output, |capture| {
            let mut peer = Peer::open_recorded(&peer_config.serial()?, &transcript)?;
            measure(capture, &mut peer, &config)
        });
        transcript.save(&boot_output.join(PEER_TRANSCRIPT))?;
        let outcome = result.and_then(|measurements| {
            let summary = evaluate(&measurements)?;
            Ok(BootReport {
                boot,
                summary,
                measurements,
            })
        });
        match outcome {
            Ok(report) => reports.push(report),
            Err(error) => {
                write_report(output, &reports, Some(&error.to_string()))?;
                return Err(error);
            }
        }
    }
    write_report(output, &reports, None)?;
    println!(
        "ieee802154_live_rssi=PASS boots={} frames={}",
        config.boots, config.frames
    );
    Ok(())
}

fn write_report(output: &Path, reports: &[BootReport], failure: Option<&str>) -> Result<()> {
    let document = match failure {
        None => serde_json::json!({
            "schema": 1,
            "status": "passed",
            "result": "live-rssi-follows-the-last-frame-at-two-peer-powers",
            "not_proven": [
                "absolute-rssi-accuracy",
                "live-rssi-of-bluetooth-receptions",
                "read-side-effects",
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

/// RSSI of the received frames that are `sent`, in reception order.
fn frame_rssi(evidence: &Ieee802154SessionReceiveEvidence, sent: &[Vec<u8>]) -> Result<Vec<i8>> {
    expect_session("collect", evidence.result)?;
    let digests: Vec<(u32, usize)> = sent
        .iter()
        .map(|frame| (ieee802154_frame_crc32c(frame), frame.len()))
        .collect();
    Ok(evidence
        .frames
        .iter()
        .filter(|received| digests.contains(&(received.crc32c, usize::from(received.length))))
        .map(|received| received.rssi_dbm)
        .collect())
}

/// One series at `power_dbm`: the peer's frames, then the live read.
fn series<L: PeerLink>(
    capture: &SerialCapture,
    peer: &mut Peer<L>,
    config: &Config,
    power_dbm: i8,
) -> Result<Series> {
    peer.configure(&PeerConfig {
        channel: config.channel,
        pan_id: PAN_ID,
        short_address: PEER_SHORT,
        extended_address: PEER_EXTENDED,
        promiscuous: false,
        power_dbm,
    })?;
    // Frames of the previous series are not this one's.
    let _ = capture.collect_ieee802154_session()?;
    let mut sent = Vec::new();
    for index in 0..config.frames {
        // Without an ACK request, so the device's last baseband activity is
        // the reception itself.
        let frame = data_frame(false, SEQUENCE_BASE + index, DEVICE_SHORT, PEER_SHORT);
        peer.transmit(false, &frame)?;
        match peer.next_event(PEER_EVENT_TIMEOUT)? {
            Some(PeerEvent::Transmitted { .. }) => {}
            other => return Err(format!("the peer did not transmit: {other:?}").into()),
        }
        sent.push(frame);
    }
    let frame_rssi_dbm = frame_rssi(&capture.collect_ieee802154_session()?, &sent)?;
    let live = capture.read_ieee802154_session_recent_rssi(COMMAND_TIMEOUT)?;
    expect_session("RSSI read", live.result)?;
    Ok(Series {
        power_dbm,
        frame_rssi_dbm,
        live_rssi_dbm: live.rssi_dbm,
    })
}

fn measure<L: PeerLink>(
    capture: &SerialCapture,
    peer: &mut Peer<L>,
    config: &Config,
) -> Result<Measurements> {
    let capabilities = capture.request_image_keys(IMAGE_KEYS_TIMEOUT)?;
    if !capabilities.has::<oer_hil_protocol::ieee802154::Session>() {
        return Err("firmware does not advertise IEEE 802.15.4 sessions".into());
    }
    expect_session(
        "start",
        capture.start_ieee802154_session(
            Ieee802154SessionConfig {
                channel: config.channel,
                pan_id: PAN_ID,
                short_address: DEVICE_SHORT,
                extended_address: DEVICE_EXTENDED,
                promiscuous: false,
                maintenance_policy: Ieee802154SessionMaintenancePolicy::Vendor,
                background_maintenance: false,
                enhanced_ack: false,
                wifi_coexistence: false,
            },
            START_TIMEOUT,
        )?,
    )?;
    capture.receive_ieee802154_session()?;
    let high = series(capture, peer, config, config.high_power_dbm)?;
    let low = series(capture, peer, config, config.low_power_dbm)?;
    expect_session(
        "stop",
        capture.stop_ieee802154_session(START_TIMEOUT)?.result,
    )?;
    Ok(Measurements { high, low })
}

#[cfg(test)]
mod tests;
