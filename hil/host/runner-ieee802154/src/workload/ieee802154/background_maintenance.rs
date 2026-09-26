//! Background shared PHY maintenance of the running IEEE 802.15.4 client.
//!
//! One session runs with background maintenance for several tracking
//! periods under the chosen admission policy: under the vendor policy the
//! device runs the PHY's periodic tracking loop with the radio running,
//! under the quiesced policy it pauses its MAC for each tracking. The device
//! must track the shared PHY on its own and still transmit afterwards. No
//! peer is needed.

use std::{fs, path::Path, time::Duration};

use hil_core::{context::Context, session::SerialCapture};
use oer_hil_protocol::{
    Ieee802154AirTxOutcome, Ieee802154SessionConfig, Ieee802154SessionMaintenancePolicy,
    Ieee802154SessionStopEvidence, Ieee802154SessionTransmitRequest,
};
use serde::Serialize;

use super::peer_exchange::{DEVICE_SHORT, PEER_SHORT, data_frame, expect_session, session_frame};
use crate::Result;

const CAPABILITIES_TIMEOUT: Duration = Duration::from_secs(10);
const START_TIMEOUT: Duration = Duration::from_secs(30);
const COMMAND_TIMEOUT: Duration = Duration::from_secs(5);
const REPORT_NAME: &str = "ieee802154-background-maintenance.json";

pub struct Config {
    pub boots: u8,
    pub channel: u8,
    pub policy: Ieee802154SessionMaintenancePolicy,
    /// Whole tracking periods the session runs.
    pub periods: u8,
}

/// The session lasts this long; the first attempt comes one period after
/// start, so `periods` attempts fit.
fn run_time(periods: u8) -> Duration {
    Duration::from_millis(1_000 * u64::from(periods) + 500)
}

/// Tracking must run on at least every other attempt: the domain's schedule
/// is due one period after the last tracking, and an attempt landing just
/// before that instant finds it not yet due.
pub(crate) fn validate(evidence: &Ieee802154SessionStopEvidence, periods: u8) -> Result<()> {
    expect_session("stop", evidence.result)?;
    let counts = evidence.maintenance;
    if counts.failed {
        return Err("background PHY maintenance failed".into());
    }
    if counts.awaiting_other_clients != 0 {
        return Err("another radio client was active during the session".into());
    }
    let minimum = u16::from(periods).div_ceil(2);
    if counts.tracked < minimum {
        return Err(format!(
            "background maintenance tracked {} times in {periods} periods, expected at least {minimum}",
            counts.tracked
        )
        .into());
    }
    Ok(())
}

#[derive(Serialize)]
struct BootReport {
    boot: u8,
    tracked: u16,
    not_due: u16,
    busy: u16,
}

pub fn run(config: Config, output: &Path, context: &Context<'_>) -> Result<()> {
    fs::create_dir_all(output)?;
    let mut reports = Vec::new();
    for boot in 1..=config.boots {
        let boot_output = output.join(format!("boot-{boot:03}"));
        let result = context.with_capture(&boot_output, |capture| session(capture, &config));
        match result.and_then(|evidence| {
            validate(&evidence, config.periods)?;
            Ok(evidence)
        }) {
            Ok(evidence) => reports.push(BootReport {
                boot,
                tracked: evidence.maintenance.tracked,
                not_due: evidence.maintenance.not_due,
                busy: evidence.maintenance.busy,
            }),
            Err(error) => {
                fs::write(
                    output.join(REPORT_NAME),
                    serde_json::to_vec_pretty(&serde_json::json!({
                        "schema": 1,
                        "status": "failed",
                        "failure": error.to_string(),
                        "boots": reports,
                    }))?,
                )?;
                return Err(error);
            }
        }
    }
    fs::write(
        output.join(REPORT_NAME),
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema": 1,
            "status": "passed",
            "result": "periodic-tracking-inside-the-client-quiescence-window",
            "not_proven": ["tracking-during-traffic", "joint-maintenance-with-other-clients"],
            "boots": reports,
        }))?,
    )?;
    println!(
        "ieee802154_background_maintenance=PASS boots={} periods={}",
        config.boots, config.periods
    );
    Ok(())
}

fn session(capture: &SerialCapture, config: &Config) -> Result<Ieee802154SessionStopEvidence> {
    let capabilities = capture.request_capabilities(CAPABILITIES_TIMEOUT)?;
    if !capabilities.features.ieee802154_session {
        return Err("firmware does not advertise IEEE 802.15.4 sessions".into());
    }
    expect_session(
        "start",
        capture.start_ieee802154_session(
            Ieee802154SessionConfig {
                channel: config.channel,
                pan_id: super::peer_exchange::PAN_ID,
                short_address: DEVICE_SHORT,
                extended_address: [0; 8],
                promiscuous: false,
                maintenance_policy: config.policy,
                background_maintenance: true,
            },
            START_TIMEOUT,
        )?,
    )?;
    capture.receive_ieee802154_session()?;
    std::thread::sleep(run_time(config.periods));
    let transmitted = capture.transmit_ieee802154_session(
        Ieee802154SessionTransmitRequest {
            frame: session_frame(&data_frame(false, 1, PEER_SHORT, DEVICE_SHORT))?,
            cca: false,
        },
        COMMAND_TIMEOUT,
    )?;
    expect_session("transmit", transmitted.result)?;
    if transmitted.outcome != Ieee802154AirTxOutcome::Success {
        return Err(format!("transmit after maintenance ended {:?}", transmitted.outcome).into());
    }
    capture.stop_ieee802154_session(COMMAND_TIMEOUT)
}

#[cfg(test)]
mod tests {
    use oer_hil_protocol::{Ieee802154SessionMaintenanceCounts, Ieee802154SessionResult};

    use super::*;

    fn evidence(tracked: u16) -> Ieee802154SessionStopEvidence {
        Ieee802154SessionStopEvidence {
            result: Ieee802154SessionResult::Done,
            maintenance: Ieee802154SessionMaintenanceCounts {
                not_due: 1,
                tracked,
                ..Default::default()
            },
        }
    }

    #[test]
    fn tracking_must_run_on_every_other_attempt() {
        validate(&evidence(2), 4).unwrap();
        validate(&evidence(2), 3).unwrap();
        assert!(validate(&evidence(1), 3).is_err());
        assert!(run_time(3) > Duration::from_secs(3));
    }

    #[test]
    fn a_failed_or_shared_maintenance_fails_the_run() {
        let mut failed = evidence(4);
        failed.maintenance.failed = true;
        assert!(validate(&failed, 4).is_err());
        let mut shared = evidence(4);
        shared.maintenance.awaiting_other_clients = 1;
        assert!(validate(&shared, 4).is_err());
        let mut stopped = evidence(4);
        stopped.result = Ieee802154SessionResult::StopFailed;
        assert!(validate(&stopped, 4).is_err());
    }
}
