//! Frame loss of a receiving IEEE 802.15.4 client under background shared
//! PHY maintenance.
//!
//! One session per boot receives with background maintenance under the
//! chosen admission policy, while the reference peer sends a numbered stream
//! at a fixed interval. The device counts the stream's frames by their
//! counters; the report compares them with what the peer sent, so the vendor
//! and the quiesced admission compare on one firmware.

use std::{
    fs,
    path::Path,
    time::{Duration, Instant},
};

use hil_core::{context::Context, session::SerialCapture};
use oer_hil_protocol::{
    IEEE802154_STREAM_MAGIC, Ieee802154SessionConfig, Ieee802154SessionMaintenancePolicy,
    Ieee802154SessionReceiveEvidence, Ieee802154SessionRfPolicy, Ieee802154SessionStopEvidence,
    Ieee802154SessionStreamReceipt,
};
use serde::Serialize;

use super::peer_exchange::{
    DEVICE_EXTENDED, DEVICE_SHORT, PAN_ID, PEER_EXTENDED, PEER_SHORT, expect_session,
};
use crate::{
    Result,
    peer::{Peer, PeerConfig, PeerEvent, PeerLink},
};

const CAPABILITIES_TIMEOUT: Duration = Duration::from_secs(10);
const START_TIMEOUT: Duration = Duration::from_secs(30);
const COMMAND_TIMEOUT: Duration = Duration::from_secs(5);
/// The peer's last frames and its report after the nominal stream time.
const STREAM_SLACK: Duration = Duration::from_secs(5);
const REPORT_NAME: &str = "ieee802154-background-maintenance-stream.json";

pub struct Config {
    pub boots: u8,
    pub channel: u8,
    pub policy: Ieee802154SessionMaintenancePolicy,
    /// Whole tracking periods the stream lasts.
    pub periods: u8,
    /// Milliseconds between two stream frames.
    pub interval_ms: u16,
    /// Whether background maintenance runs beside the stream.
    pub maintenance: bool,
}

impl Config {
    /// Frames of one stream: the tracking periods at the interval.
    pub fn frames(&self) -> u16 {
        (1_000 * u16::from(self.periods)) / self.interval_ms
    }
}

/// A data frame from the peer to the device, without ACK request, whose
/// payload is the stream magic and a counter the peer fills in.
pub fn stream_template() -> Vec<u8> {
    let mut frame = 0x8841_u16.to_le_bytes().to_vec();
    frame.push(0);
    frame.extend_from_slice(&PAN_ID.to_le_bytes());
    frame.extend_from_slice(&DEVICE_SHORT.to_le_bytes());
    frame.extend_from_slice(&PEER_SHORT.to_le_bytes());
    frame.extend_from_slice(&IEEE802154_STREAM_MAGIC);
    frame.extend_from_slice(&[0, 0]);
    frame
}

#[derive(Clone, Debug, Serialize)]
struct BootReport {
    boot: u8,
    sent: u32,
    peer_done: u32,
    peer_failed: u32,
    received: u16,
    lost: u32,
    duplicates: u16,
    missing_runs: u16,
    longest_missing_run: u16,
    tracked: u16,
    not_due: u16,
    busy: u16,
    /// Every frame the device received, stream or not.
    device_frames: u16,
    /// MAC lengths of the first frames the device recorded.
    recorded_lengths: Vec<u8>,
}

/// The peer's stream totals.
#[derive(Clone, Copy, Debug)]
struct Sent {
    sent: u32,
    done: u32,
    failed: u32,
}

fn report(
    boot: u8,
    sent: Sent,
    collected: &Ieee802154SessionReceiveEvidence,
    evidence: &Ieee802154SessionStopEvidence,
) -> BootReport {
    let Ieee802154SessionStreamReceipt {
        received,
        duplicates,
        missing_runs,
        longest_missing_run,
        ..
    } = evidence.stream;
    BootReport {
        boot,
        sent: sent.sent,
        peer_done: sent.done,
        peer_failed: sent.failed,
        received,
        lost: sent.done.saturating_sub(u32::from(received)),
        duplicates,
        missing_runs,
        longest_missing_run,
        tracked: evidence.maintenance.tracked,
        not_due: evidence.maintenance.not_due,
        busy: evidence.maintenance.busy,
        device_frames: collected.total,
        recorded_lengths: collected.frames.iter().map(|frame| frame.length).collect(),
    }
}

/// The stream must reach the device at all, and maintenance must behave as
/// in the background-maintenance scenario, or not run in the baseline.
pub(crate) fn validate(
    evidence: &Ieee802154SessionStopEvidence,
    periods: u8,
    maintenance: bool,
) -> Result<()> {
    if maintenance {
        super::background_maintenance::validate(evidence, periods)?;
    } else if evidence.maintenance.tracked != 0 {
        return Err("the baseline tracked the PHY".into());
    }
    if evidence.stream.received == 0 {
        return Err("the device received no frame of the peer stream".into());
    }
    if evidence.stream.out_of_range != 0 {
        return Err("the device received stream counters beyond the stream".into());
    }
    Ok(())
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
            let (sent, received, evidence) = session(capture, &mut peer, &config)?;
            // The boot's report stays with the run even when it fails.
            reports.push(report(boot, sent, &received, &evidence));
            validate(&evidence, config.periods, config.maintenance)
        });
        match result {
            Ok(()) => {}
            Err(error) => {
                write_report(output, &config, &reports, Some(&error.to_string()))?;
                return Err(error);
            }
        }
    }
    write_report(output, &config, &reports, None)?;
    println!(
        "ieee802154_background_maintenance_stream=PASS boots={} frames={}",
        config.boots,
        config.frames()
    );
    Ok(())
}

fn write_report(
    output: &Path,
    config: &Config,
    boots: &[BootReport],
    failure: Option<&str>,
) -> Result<()> {
    fs::write(
        output.join(REPORT_NAME),
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema": 1,
            "status": if failure.is_some() { "failed" } else { "passed" },
            "failure": failure,
            "policy": format!("{:?}", config.policy),
            "maintenance": config.maintenance,
            "frames": config.frames(),
            "interval_ms": config.interval_ms,
            "boots": boots,
        }))?,
    )?;
    Ok(())
}

fn session<L: PeerLink>(
    capture: &SerialCapture,
    peer: &mut Peer<L>,
    config: &Config,
) -> Result<(
    Sent,
    Ieee802154SessionReceiveEvidence,
    Ieee802154SessionStopEvidence,
)> {
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
    expect_session(
        "start",
        capture.start_ieee802154_session(
            Ieee802154SessionConfig {
                channel: config.channel,
                pan_id: PAN_ID,
                short_address: DEVICE_SHORT,
                extended_address: DEVICE_EXTENDED,
                promiscuous: false,
                maintenance_policy: config.policy,
                background_maintenance: config.maintenance,
                enhanced_ack: false,
                rf_policy: Ieee802154SessionRfPolicy::AlwaysOn,
                wifi_coexistence: false,
            },
            START_TIMEOUT,
        )?,
    )?;
    capture.receive_ieee802154_session()?;
    let frames = config.frames();
    peer.stream(frames, config.interval_ms, &stream_template())?;
    let nominal = Duration::from_millis(u64::from(frames) * u64::from(config.interval_ms));
    let sent = await_stream(peer, nominal + STREAM_SLACK)?;
    let received = capture.collect_ieee802154_session()?;
    let evidence = capture.stop_ieee802154_session(COMMAND_TIMEOUT)?;
    Ok((sent, received, evidence))
}

fn await_stream<L: PeerLink>(peer: &mut Peer<L>, bound: Duration) -> Result<Sent> {
    let deadline = Instant::now() + bound;
    while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
        if let Some(PeerEvent::StreamDone { sent, done, failed }) = peer.next_event(remaining)? {
            return Ok(Sent { sent, done, failed });
        }
    }
    Err("the peer did not report the end of its stream".into())
}

#[cfg(test)]
mod tests {
    use oer_hil_protocol::{
        Ieee802154SessionMaintenanceCounts, Ieee802154SessionResult, ieee802154_stream_counter,
    };

    use super::*;

    #[test]
    fn the_template_is_a_stream_frame_to_the_device() {
        let template = stream_template();
        assert_eq!(ieee802154_stream_counter(&template), Some(0));
        assert_eq!(&template[5..7], &DEVICE_SHORT.to_le_bytes());
    }

    #[test]
    fn loss_is_what_the_peer_sent_and_the_device_did_not_receive() {
        let evidence = Ieee802154SessionStopEvidence {
            result: Ieee802154SessionResult::Done,
            maintenance: Ieee802154SessionMaintenanceCounts {
                tracked: 5,
                ..Default::default()
            },
            stream: Ieee802154SessionStreamReceipt {
                received: 1_990,
                span: 2_000,
                missing_runs: 3,
                longest_missing_run: 6,
                ..Default::default()
            },
            ..Default::default()
        };
        let sent = Sent {
            sent: 2_000,
            done: 2_000,
            failed: 0,
        };
        let received = Ieee802154SessionReceiveEvidence {
            total: 1_990,
            ..Default::default()
        };
        let report = report(1, sent, &received, &evidence);
        assert_eq!(report.lost, 10);
        assert_eq!(report.longest_missing_run, 6);
        validate(&evidence, 10, true).unwrap();
    }

    #[test]
    fn a_stream_the_device_never_saw_fails() {
        let evidence = Ieee802154SessionStopEvidence {
            result: Ieee802154SessionResult::Done,
            maintenance: Ieee802154SessionMaintenanceCounts {
                tracked: 5,
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(validate(&evidence, 10, true).is_err());
    }

    #[test]
    fn ten_periods_at_five_milliseconds_are_two_thousand_frames() {
        let config = Config {
            boots: 1,
            channel: 15,
            policy: Ieee802154SessionMaintenancePolicy::Vendor,
            periods: 10,
            interval_ms: 5,
            maintenance: true,
        };
        assert_eq!(config.frames(), 2_000);
    }
}
