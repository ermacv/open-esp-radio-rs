//! IEEE 802.15.4 RF sleep with the reference peer.
//!
//! The session closes RF whenever the device's radio sleeps, as ESP-IDF does
//! with tickless idle and modem retention:
//!
//! 1. the device transmits data frames from sleep: each opens RF, is
//!    acknowledged by the peer and arrives intact, and RF closes again;
//! 2. the peer transmits to the sleeping device: nothing acknowledges or
//!    reports the frame;
//! 3. receive opens RF: the peer's next frame is acknowledged and reported.
//!
//! The stop evidence must count exactly one close after start and after
//! each transmission, and one open per transmission and for the receive.

use std::{fs, path::Path};

use hil_core::{context::Context, session::SerialCapture};
use oer_hil_protocol::{
    Ieee802154SessionConfig, Ieee802154SessionMaintenancePolicy, Ieee802154SessionRfCounts,
    Ieee802154SessionRfPolicy, Ieee802154SessionTransmitRequest, Ieee802154SessionTxMode,
};
use serde::Serialize;

use super::peer_exchange::{
    CAPABILITIES_TIMEOUT, COMMAND_TIMEOUT, DEVICE_EXTENDED, DEVICE_SHORT, PAN_ID,
    PEER_EVENT_TIMEOUT, PEER_EXTENDED, PEER_SHORT, START_TIMEOUT, check_device_received,
    check_device_transmit, check_peer_acknowledged, check_peer_received, check_peer_unacknowledged,
    data_frame, expect_session, session_frame,
};
use crate::{
    Result,
    peer::{PEER_TRANSCRIPT, Peer, PeerConfig, PeerLink, PeerTranscript},
};

const REPORT_NAME: &str = "ieee802154-rf-sleep.json";
/// Sequence numbers of the peer's frames, apart from the device's.
const PEER_SEQUENCE_BASE: u8 = 0x90;

pub struct Config {
    pub boots: u8,
    pub channel: u8,
    /// Device transmissions from sleep.
    pub frames: u8,
}

/// The RF counts `frames` transmissions from sleep and one receive produce:
/// a close after start and after each transmission, an open for each
/// transmission and for the receive.
pub fn validate(rf: &Ieee802154SessionRfCounts, frames: u8) -> Result<()> {
    let expected = u16::from(frames) + 1;
    if rf.failed {
        return Err("an RF close or open failed".into());
    }
    if rf.closes != expected || rf.opens != expected {
        return Err(format!(
            "RF closed {} and opened {} times, expected {expected} each",
            rf.closes, rf.opens
        )
        .into());
    }
    Ok(())
}

#[derive(Serialize)]
struct BootReport {
    boot: u8,
    transmissions: u8,
    closes: u16,
    opens: u16,
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
        let transcript = PeerTranscript::default();
        let result = context.with_capture(&boot_output, |capture| {
            let mut peer = Peer::open_recorded(&peer_config.serial, &transcript)?;
            session(capture, &mut peer, &config, boot)
        });
        // What the peer saw tells a missing transmission from a missing
        // acknowledgement.
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
        "ieee802154_rf_sleep=PASS boots={} frames={}",
        config.boots, config.frames
    );
    Ok(())
}

fn session<L: PeerLink>(
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
                maintenance_policy: Ieee802154SessionMaintenancePolicy::Vendor,
                background_maintenance: false,
                enhanced_ack: false,
                rf_policy: Ieee802154SessionRfPolicy::CloseWhenAsleep,
                wifi_coexistence: false,
            },
            START_TIMEOUT,
        )?,
    )?;

    for sequence in 0..config.frames {
        let frame = data_frame(true, sequence, PEER_SHORT, DEVICE_SHORT);
        let evidence = capture.transmit_ieee802154_session(
            Ieee802154SessionTransmitRequest {
                frame: session_frame(&frame)?,
                mode: Ieee802154SessionTxMode::Direct,
                max_frame_retries: 0,
            },
            COMMAND_TIMEOUT,
        )?;
        check_device_transmit(&evidence, sequence)?;
        check_peer_received(peer.next_event(PEER_EVENT_TIMEOUT)?, &frame)?;
    }

    // The sleeping device neither acknowledges nor reports.
    let asleep = data_frame(true, PEER_SEQUENCE_BASE, DEVICE_SHORT, PEER_SHORT);
    peer.transmit(false, &asleep)?;
    check_peer_unacknowledged(peer.next_event(PEER_EVENT_TIMEOUT)?)?;
    check_device_received(&capture.collect_ieee802154_session()?, &[])?;

    capture.receive_ieee802154_session()?;
    let awake = data_frame(true, PEER_SEQUENCE_BASE + 1, DEVICE_SHORT, PEER_SHORT);
    peer.transmit(false, &awake)?;
    check_peer_acknowledged(
        peer.next_event(PEER_EVENT_TIMEOUT)?,
        PEER_SEQUENCE_BASE + 1,
        false,
    )?;
    check_device_received(&capture.collect_ieee802154_session()?, &[awake])?;

    let stopped = capture.stop_ieee802154_session(COMMAND_TIMEOUT)?;
    expect_session("stop", stopped.result)?;
    validate(&stopped.rf, config.frames)?;
    peer.sleep()?;
    Ok(BootReport {
        boot,
        transmissions: config.frames,
        closes: stopped.rf.closes,
        opens: stopped.rf.opens,
    })
}

fn write_report(output: &Path, reports: &[BootReport], failure: Option<&str>) -> Result<()> {
    let document = match failure {
        None => serde_json::json!({
            "schema": 1,
            "status": "passed",
            "result": "rf-closed-while-asleep-and-opened-for-each-operation",
            "not_proven": ["modem-retention", "light-sleep", "current-consumption"],
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rf_counts_must_match_the_operations_exactly() {
        let counts = |closes, opens, failed| Ieee802154SessionRfCounts {
            closes,
            opens,
            failed,
        };
        validate(&counts(4, 4, false), 3).unwrap();
        assert!(validate(&counts(4, 4, true), 3).is_err());
        assert!(validate(&counts(3, 4, false), 3).is_err());
        assert!(validate(&counts(4, 5, false), 3).is_err());
        assert!(validate(&counts(0, 0, false), 3).is_err());
    }
}
