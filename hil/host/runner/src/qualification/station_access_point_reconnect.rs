//! Controlled upstream loss and explicit same-channel STA+AP restart.

use std::{fs, path::Path, time::Duration};

use open_esp_radio_hil_protocol::{
    StationDisconnectReason, StationLifecycleEvent, WifiNetworkInterface, WifiRole,
    WifiStationAccessPointRequest,
};

use crate::{
    Result,
    evidence::traffic_capture::{SerialCapture, probe_udp_rx_ready_via},
    qualification::scenario::PhyExpectation,
    qualification::station_access_point::wait_for_endpoints,
    transport::controlled_ap::ControlledAp,
    transport::controlled_client::ControlledClient,
    transport::lab_config::LabConfig,
    transport::wifi_control::{report_stack, require_transition, stop_station},
};

const TARGET_RX_PORT: u16 = 4_323;
const BEACON_LOSS_TIMEOUT: Duration = Duration::from_secs(15);

pub(crate) fn run(
    timeout: Duration,
    output: &Path,
    lab: &LabConfig,
    phy: PhyExpectation,
) -> Result<()> {
    fs::create_dir_all(output)?;
    let mut upstream = ControlledAp::start(&lab.station, &lab.station_fixture, phy)?;
    let capture = SerialCapture::start_with_reset(&lab.device.serial);
    let result = qualify(&capture, timeout, lab, &mut upstream);
    let capture_result = capture.finish_to(output);
    result?;
    capture_result?;
    eprintln!("wifi_sta_ap_reconnect=PASS");
    eprintln!("uart_log={}", output.join("uart.log").display());
    Ok(())
}

fn qualify(
    capture: &SerialCapture,
    timeout: Duration,
    lab: &LabConfig,
    upstream: &mut ControlledAp,
) -> Result<()> {
    let mut lifecycle_cursor = capture.station_lifecycle_cursor();
    let capabilities = capture.prepare_station(lab, timeout)?;
    if !capabilities.features.simultaneous_station_access_point
        || !capabilities.features.station_lifecycle_events
    {
        return Err("firmware lacks paired-role or lifecycle evidence".into());
    }
    let initial_generation = expect_connected(capture, &mut lifecycle_cursor, timeout)?;
    let _ = stop_station(capture, timeout)?;
    expect_event(
        capture,
        &mut lifecycle_cursor,
        timeout,
        StationLifecycleEvent::Disconnected {
            generation: initial_generation,
            reason: StationDisconnectReason::LinkPolicy,
        },
        "initial station stop",
    )?;

    let first = start_pair(capture, timeout, lab)?;
    let first_generation = expect_connected(capture, &mut lifecycle_cursor, timeout)?;
    let first_client = ControlledClient::connect(&lab.access_point)?;
    require_both_endpoints(capture, timeout)?;

    upstream.stop()?;
    let expected_loss = StationLifecycleEvent::Disconnected {
        generation: first_generation,
        reason: StationDisconnectReason::BeaconLoss,
    };
    let observed_loss = capture.wait_station_lifecycle_event_optional(
        &mut lifecycle_cursor,
        timeout.min(BEACON_LOSS_TIMEOUT),
    )?;
    if observed_loss != Some(expected_loss) {
        // Preserve the paired control state which caused the timeout. An
        // isolation reset would erase whether the beacon monitor was unarmed,
        // expired but unserviced, or refreshed by a wrongly routed frame.
        let stopped = capture.wait_station_access_point_stop(
            capture.request_station_access_point_stop()?,
            timeout,
        )?;
        first_client.restore()?;
        return Err(format!(
            "paired upstream beacon loss reported {observed_loss:?}, expected {expected_loss:?}; explicit diagnostic teardown: {:?}",
            stopped.transition
        )
        .into());
    }

    // The public paired handle remains the explicit capability that joins the
    // already-triggered rollback. Consuming it must return the complete idle
    // graph; no AP-only survivor or hidden station retry is permitted.
    let first_stopped = capture
        .wait_station_access_point_stop(capture.request_station_access_point_stop()?, timeout)?;
    require_transition(
        first_stopped.transition,
        WifiRole::StationAccessPoint,
        WifiRole::Idle,
    )?;
    if first_stopped.transition.generation != first.generation {
        return Err("beacon-loss teardown returned a different paired generation".into());
    }
    first_client.restore()?;

    upstream.restart()?;
    let second = start_pair(capture, timeout, lab)?;
    if second.generation == first.generation {
        return Err("explicit paired restart reused the previous generation".into());
    }
    let second_generation = expect_connected(capture, &mut lifecycle_cursor, timeout)?;
    if second_generation != first_generation.wrapping_add(1) {
        return Err(format!(
            "paired lifecycle generation did not advance after beacon loss: first={first_generation} second={second_generation}"
        )
        .into());
    }
    let second_client = ControlledClient::connect(&lab.access_point)?;
    require_both_endpoints(capture, timeout)?;

    let second_stopped = capture
        .wait_station_access_point_stop(capture.request_station_access_point_stop()?, timeout)?;
    require_transition(
        second_stopped.transition,
        WifiRole::StationAccessPoint,
        WifiRole::Idle,
    )?;
    if second_stopped.transition.generation != second.generation {
        return Err("explicit restart teardown returned a different generation".into());
    }
    second_client.restore()?;
    report_stack(capture, timeout, "sta-ap-reconnect-stopped")?;
    Ok(())
}

fn start_pair(
    capture: &SerialCapture,
    timeout: Duration,
    lab: &LabConfig,
) -> Result<open_esp_radio_hil_protocol::WifiRoleTransitionEvidence> {
    let transition = capture.wait_wifi_role_transition(
        capture.request_station_access_point_start(WifiStationAccessPointRequest {
            station_credentials: lab.station.protocol_credentials()?,
            access_point: lab.access_point.protocol_request()?,
        })?,
        timeout,
    )?;
    require_transition(transition, WifiRole::Idle, WifiRole::StationAccessPoint)?;
    Ok(transition)
}

fn require_both_endpoints(capture: &SerialCapture, timeout: Duration) -> Result<()> {
    let (station, access_point) = wait_for_endpoints(capture, timeout)?;
    probe_udp_rx_ready_via(
        capture,
        WifiNetworkInterface::Station,
        station,
        None,
        TARGET_RX_PORT,
        timeout,
    )?;
    probe_udp_rx_ready_via(
        capture,
        WifiNetworkInterface::AccessPoint,
        access_point,
        None,
        TARGET_RX_PORT,
        timeout,
    )?;
    Ok(())
}

fn expect_connected(capture: &SerialCapture, cursor: &mut usize, timeout: Duration) -> Result<u32> {
    let event = capture.wait_station_lifecycle_event(cursor, timeout)?;
    match event {
        StationLifecycleEvent::Connected { generation } => Ok(generation),
        other => Err(format!("paired start reported {other:?} instead of Connected").into()),
    }
}

fn expect_event(
    capture: &SerialCapture,
    cursor: &mut usize,
    timeout: Duration,
    expected: StationLifecycleEvent,
    edge: &str,
) -> Result<()> {
    let actual = capture.wait_station_lifecycle_event(cursor, timeout)?;
    if actual != expected {
        return Err(format!("{edge} reported {actual:?}, expected {expected:?}").into());
    }
    Ok(())
}
