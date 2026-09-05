//! Controlled prolonged AP absence and bounded retry-exhaustion qualification.

use crate::execution::context::Context;
use std::{
    fs,
    path::Path,
    time::{Duration, Instant},
};

use open_esp_radio_hil_protocol::{
    StationAttemptFailureReason, StationDisconnectReason, StationFailureStage,
    StationLifecycleEvent,
};

use crate::{
    Result, fixture::controlled_ap::ControlledAp, scenario::PhyExpectation, session::SerialCapture,
};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);
const QUALIFIED_ATTEMPTS: u16 = 3;

pub(crate) struct Config {
    pub(crate) timeout: Duration,
}

pub(crate) fn run(
    options: Config,
    output: &Path,
    context: &Context<'_>,
    phy: PhyExpectation,
) -> Result<()> {
    let options = options.validate()?;
    fs::create_dir_all(output)?;

    let mut ap = ControlledAp::start(&context.lab.station, &context.lab.station_fixture, phy)?;
    let result = context.with_capture(output, |capture| {
        let mut cursor = capture.station_lifecycle_cursor();
        qualify(capture, context, &mut cursor, &mut ap, options.timeout)
    });
    drop(ap);
    result?;
    eprintln!("station_ap_absence=PASS");
    eprintln!("uart_log={}", output.join("uart.log").display());
    Ok(())
}

fn qualify(
    capture: &SerialCapture,
    context: &Context<'_>,
    cursor: &mut usize,
    ap: &mut ControlledAp,
    timeout: Duration,
) -> Result<()> {
    let capabilities = capture.prepare_station(context, timeout)?;
    if !capabilities.features.station_lifecycle_events {
        return Err("firmware does not advertise reliable station lifecycle events".into());
    }

    expect_event(
        capture,
        cursor,
        timeout,
        StationLifecycleEvent::Connected { generation: 0 },
        "initial connection",
    )?;
    let absence_started = Instant::now();
    ap.stop()?;
    expect_event(
        capture,
        cursor,
        timeout,
        StationLifecycleEvent::Disconnected {
            generation: 0,
            reason: StationDisconnectReason::BeaconLoss,
        },
        "beacon-loss disconnect",
    )?;

    for attempt in 1..QUALIFIED_ATTEMPTS {
        expect_event(
            capture,
            cursor,
            timeout,
            StationLifecycleEvent::AttemptFailed {
                generation: 1,
                attempt,
                stage: StationFailureStage::CandidateSelection,
                reason: StationAttemptFailureReason::NoCandidate,
            },
            "no-candidate attempt",
        )?;
    }
    expect_event(
        capture,
        cursor,
        timeout,
        StationLifecycleEvent::RetryExhausted {
            generation: 1,
            attempts: QUALIFIED_ATTEMPTS,
            stage: StationFailureStage::CandidateSelection,
            reason: StationAttemptFailureReason::NoCandidate,
        },
        "retry exhaustion",
    )?;
    eprintln!(
        "station_ap_absence_exhausted_ms={}",
        absence_started.elapsed().as_millis()
    );
    Ok(())
}

fn expect_event(
    capture: &SerialCapture,
    cursor: &mut usize,
    timeout: Duration,
    expected: StationLifecycleEvent,
    transition: &str,
) -> Result<()> {
    let actual = capture
        .wait_station_lifecycle_event(cursor, timeout)
        .map_err(|error| format!("station {transition}: {error}"))?;
    validate_event(actual, expected, transition)
}

fn validate_event(
    actual: StationLifecycleEvent,
    expected: StationLifecycleEvent,
    transition: &str,
) -> Result<()> {
    if actual != expected {
        return Err(
            format!("station {transition} reported {actual:?}, expected {expected:?}").into(),
        );
    }
    Ok(())
}

impl Default for Config {
    fn default() -> Self {
        Self {
            timeout: DEFAULT_TIMEOUT,
        }
    }
}

#[cfg(test)]
mod tests;

impl Config {
    fn validate(self) -> Result<Self> {
        if !(Duration::from_secs(30)..=Duration::from_secs(300)).contains(&self.timeout) {
            return Err("AP timeout must be in 30..=300 seconds".into());
        }
        Ok(self)
    }
}
