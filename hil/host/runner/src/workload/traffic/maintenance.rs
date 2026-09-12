//! Shared station maintenance transaction and evidence validation.
use crate::{Result, session::SerialCapture};
use std::{fs, path::Path, time::Duration};

pub(crate) fn require(capture: &SerialCapture) -> Result<()> {
    if !capture
        .request_capabilities(Duration::from_secs(5))?
        .features
        .station_pause
    {
        return Err("image does not support station pause".into());
    }
    Ok(())
}

/// Hold the already-progressing traffic interval before issuing maintenance.
/// The duration comes from the validated scenario and is therefore archived
/// with the run rather than supplied by an unrecorded shell sleep.
pub(crate) fn wait_after_progress(delay: Duration) {
    if !delay.is_zero() {
        eprintln!(
            "station maintenance preconditioning: waiting {} ms after traffic progress",
            delay.as_millis()
        );
        std::thread::sleep(delay);
    }
}

pub(crate) fn run(
    capture: &SerialCapture,
    operation: open_esp_radio_hil_protocol::StationPauseOperation,
    output: &Path,
    progress: serde_json::Value,
) -> Result<()> {
    use open_esp_radio_hil_protocol::StationPauseOperation as Op;
    let features = capture
        .request_capabilities(Duration::from_secs(5))?
        .features;
    if matches!(operation, Op::Rfpll | Op::RfpllObserved) {
        // Completion and checked restoration of acquisition are
        // the prerequisite event, not a host delay or a fabricated
        // temperature. RFPLL rechecks freshness after its TX drain.
        let sample = capture.station_pause_round_trip(Op::Temperature, Duration::from_secs(2))?;
        fs::write(
            output.join("station-temperature.json"),
            serde_json::to_vec_pretty(&sample)?,
        )?;
        validate_pause(Op::Temperature, sample.evidence)?;
        validate_rfpll(Op::Temperature, sample.evidence.timings, sample.rfpll)?;
        validate_tx_waits(sample.evidence.timings, sample.tx_waits)?;
        validate_rx_gain(sample.evidence.timings, sample.rx_gain)?;
        if !sample.timer.is_some_and(|timer| timer.is_valid()) {
            return Err("missing or inconsistent temperature acquisition timer evidence".into());
        }
    }
    let timeout = if operation
        == open_esp_radio_hil_protocol::StationPauseOperation::TrackingService
    {
        Duration::from_micros(open_esp_radio_hil_protocol::STATION_TRACKING_SERVICE_WINDOW_MICROS)
            + Duration::from_secs(2)
    } else {
        Duration::from_secs(2)
    };
    let report = capture.station_pause_round_trip(operation, timeout)?;
    let evidence = report.evidence;
    let tx_waits = report.tx_waits;
    let timer = report.timer;
    let service = report.service;
    let rfpll = report.rfpll;
    fs::write(
        output.join("station-pause.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "operation": operation,
            "progress_before_request": progress,
            "evidence": evidence,
            "tx_waits": tx_waits,
            "rx_gain": report.rx_gain,
            "phy_rx_hot_sram": features.phy_rx_hot_sram,
            "timer": timer,
            "service": service,
            "rfpll": rfpll,
        }))?,
    )?;
    validate_pause(operation, evidence)?;
    validate_rfpll(operation, evidence.timings, rfpll)?;
    validate_service(operation, service)?;
    validate_tx_waits(evidence.timings, tx_waits)?;
    validate_rx_gain(evidence.timings, report.rx_gain)?;
    if !timer.is_some_and(|timer| timer.is_valid()) {
        return Err("missing or inconsistent platform timer evidence".into());
    }
    Ok(())
}

fn validate_pause(
    operation: open_esp_radio_hil_protocol::StationPauseOperation,
    evidence: open_esp_radio_hil_protocol::StationPauseEvidence,
) -> Result<()> {
    if evidence.result != open_esp_radio_hil_protocol::StationPauseResult::Resumed {
        return Err(format!("station pause failed: {:?}", evidence.result).into());
    }
    if evidence
        .timings
        .is_some_and(|timings| !timings.is_complete())
    {
        return Err("PHY timing evidence is invalid, failed or incomplete".into());
    }
    if !matches!(
        operation,
        open_esp_radio_hil_protocol::StationPauseOperation::Access
            | open_esp_radio_hil_protocol::StationPauseOperation::Synthetic { .. }
            | open_esp_radio_hil_protocol::StationPauseOperation::TrackingService
    ) && !evidence
        .tracking
        .is_some_and(|tracking| !tracking.inhibited)
    {
        return Err("requested PHY tracking did not execute uninhibited due work".into());
    }
    if operation == open_esp_radio_hil_protocol::StationPauseOperation::Calibration
        && !evidence
            .tracking
            .is_some_and(|tracking| tracking.common_calibrated && tracking.wifi_calibrated)
    {
        return Err("requested common/Wi-Fi calibration did not complete".into());
    }
    use open_esp_radio_hil_protocol::StationPauseOperation as Op;
    if let Op::Synthetic {
        duration_micros, ..
    } = operation
    {
        if duration_micros == 0
            || duration_micros > 200_000
            || evidence.elapsed_micros < u64::from(duration_micros)
        {
            return Err("synthetic pause did not cover the requested bounded hold".into());
        }
        if evidence.tracking.is_some()
            || evidence.timings.is_some_and(|t| {
                t.calibration.started != 0 || t.rfpll.started != 0 || t.temperature.started != 0
            })
        {
            return Err("synthetic pause unexpectedly performed PHY work".into());
        }
    }
    if matches!(operation, Op::Tracking | Op::Calibration) {
        validate_tracking_parent(evidence)?;
    }
    if operation == Op::Temperature {
        let timings = evidence
            .timings
            .ok_or("missing temperature acquisition timing")?;
        if timings.temperature.started != 1
            || timings.temperature.completed != 1
            || [
                timings.rfpll,
                timings.wifi_power,
                timings.bluetooth_ieee802154_power,
                timings.wifi_i2c,
                timings.calibration,
            ]
            .iter()
            .any(|timing| timing.started != 0)
        {
            return Err("temperature pause did not complete exactly one acquisition".into());
        }
    }
    if matches!(operation, Op::Rfpll | Op::RfpllCheck | Op::RfpllObserved) {
        let timings = evidence.timings.ok_or("missing RFPLL completion timing")?;
        if timings.rfpll.started != 1
            || timings.rfpll.completed != 1
            || (operation == Op::Rfpll && timings.rfpll.elapsed_micros == 0)
            || [
                timings.wifi_power,
                timings.bluetooth_ieee802154_power,
                timings.wifi_i2c,
                timings.calibration,
                timings.temperature,
            ]
            .iter()
            .any(|timing| timing.started != 0)
            || evidence.tracking.is_some_and(|tracking| {
                tracking.common_calibrated
                    || tracking.wifi_calibrated
                    || tracking.bluetooth_ieee802154_calibrated
            })
        {
            return Err("RFPLL pause did not complete exactly the requested operation".into());
        }
    }
    if matches!(operation, Op::CommonCalibration | Op::TxCalibration) {
        let tracking = evidence
            .tracking
            .ok_or("missing selected calibration evidence")?;
        if (operation == Op::CommonCalibration
            && (!tracking.common_calibrated || tracking.wifi_calibrated))
            || (operation == Op::TxCalibration
                && (!tracking.wifi_calibrated || tracking.common_calibrated))
        {
            return Err(
                "selected calibration did not complete exactly its requested branch".into(),
            );
        }
    }
    Ok(())
}

/// One standalone Wi-Fi parent, with registered RFPLL disabled. Counts prove
/// completed coverage, not execution order or independent hardware readback.
fn validate_tracking_parent(
    evidence: open_esp_radio_hil_protocol::StationPauseEvidence,
) -> Result<()> {
    let timing = evidence.timings.ok_or("missing whole PHY parent timing")?;
    let tracking = evidence
        .tracking
        .ok_or("missing whole PHY parent outcome")?;
    if [
        timing.wifi_i2c,
        timing.wifi_power,
        timing.calibration,
        timing.temperature,
    ]
    .iter()
    .any(|operation| operation.started != 1 || operation.completed != 1)
        || timing.rfpll.started != 0
        || timing.bluetooth_ieee802154_power.started != 0
        || tracking.bluetooth_ieee802154_calibrated
    {
        return Err("whole PHY parent did not complete exactly the standalone Wi-Fi graph".into());
    }
    for (operations, committed) in [
        (
            &[timing.dcode, timing.rx_gain, timing.channel_restore][..],
            tracking.common_calibrated,
        ),
        (
            &[timing.tx_dc_pwdet, timing.tx_gain_publication][..],
            tracking.wifi_calibrated,
        ),
    ] {
        let expected = u16::from(committed);
        if operations
            .iter()
            .any(|operation| operation.started != expected || operation.completed != expected)
        {
            return Err(
                "whole PHY parent calibration flags disagree with completed children".into(),
            );
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "maintenance/tests.rs"]
mod pause_tests;

fn validate_tx_waits(
    timings: Option<open_esp_radio_hil_protocol::PhyTimingEvidence>,
    waits: Option<open_esp_radio_hil_protocol::PhyTxWaitEvidence>,
) -> Result<()> {
    match (timings, waits) {
        (Some(timing), Some(waits)) if waits.fits(timing.tx_dc_pwdet) => Ok(()),
        (None, None) => Ok(()),
        _ => Err("missing or inconsistent TX calibration wait evidence".into()),
    }
}

fn validate_service(
    operation: open_esp_radio_hil_protocol::StationPauseOperation,
    service: Option<open_esp_radio_hil_protocol::StationTrackingServiceEvidence>,
) -> Result<()> {
    use open_esp_radio_hil_protocol::{
        STATION_TRACKING_SERVICE_WINDOW_MICROS, StationPauseOperation,
    };
    match (operation, service) {
        (StationPauseOperation::TrackingService, Some(report))
            if report.is_valid()
                && report.operations[0] >= 3
                && report.elapsed_micros >= STATION_TRACKING_SERVICE_WINDOW_MICROS =>
        {
            Ok(())
        }
        (StationPauseOperation::TrackingService, _) => Err(
            "automatic PHY service did not provide a valid continuous observation window".into(),
        ),
        (_, None) => Ok(()),
        (_, Some(_)) => Err("unexpected automatic PHY service detail".into()),
    }
}

fn validate_rfpll(
    operation: open_esp_radio_hil_protocol::StationPauseOperation,
    timings: Option<open_esp_radio_hil_protocol::PhyTimingEvidence>,
    detail: Option<open_esp_radio_hil_protocol::RfpllEvidence>,
) -> Result<()> {
    use open_esp_radio_hil_protocol::StationPauseOperation as Op;
    let count = timings.map_or(0, |timing| timing.rfpll.completed);
    match (count, detail) {
        (0, None) if !matches!(operation, Op::Rfpll | Op::RfpllCheck | Op::RfpllObserved) => Ok(()),
        (1, Some(detail))
            if detail.is_valid()
                && (operation != Op::Rfpll
                    || (detail.threshold == 0 && detail.correction.is_some()))
                && (!matches!(operation, Op::RfpllCheck | Op::RfpllObserved)
                    || detail.threshold == 15)
                && (!matches!(operation, Op::Rfpll | Op::RfpllObserved)
                    || detail.sample_age_micros.is_some_and(|age| {
                        age <= open_esp_radio_hil_protocol::STATION_RFPLL_SAMPLE_MAX_AGE_MICROS
                    })) =>
        {
            Ok(())
        }
        _ => Err("missing, duplicate or inconsistent RFPLL terminal detail".into()),
    }
}

fn validate_rx_gain(
    timings: Option<open_esp_radio_hil_protocol::PhyTimingEvidence>,
    detail: Option<open_esp_radio_hil_protocol::PhyRxGainEvidence>,
) -> Result<()> {
    match (timings, detail) {
        (Some(timing), Some(detail)) if detail.fits(timing.rx_gain) => Ok(()),
        (None, None) => Ok(()),
        _ => Err("missing or inconsistent RX gain detail evidence".into()),
    }
}
