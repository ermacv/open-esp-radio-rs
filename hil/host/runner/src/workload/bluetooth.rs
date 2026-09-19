//! Independent RF observations with silence controls in both directions.

pub(crate) mod backpressure;
pub(crate) mod calibration;
pub(crate) mod deadline;
mod encrypted_maintenance;
pub(crate) mod gatt;
pub(crate) mod phy_watchdog;
pub(crate) mod secure_gatt;
pub(crate) mod security_failure;

use crate::{Result, execution::context::Context, fixture::bluetooth, session::SerialCapture};
use open_esp_radio_hil_protocol::{
    BLUETOOTH_PERIPHERAL_ACL_LL_FRAGMENTS, BluetoothDtmOperation as Operation, BluetoothDtmResult,
    BluetoothPeripheralEvidence, BluetoothPeripheralOperation, BluetoothPeripheralTermination,
};
use std::{
    path::Path,
    time::{Duration, Instant},
};

#[derive(Default, serde::Serialize)]
struct Counts {
    pc_silence: Option<u16>,
    esp_silence: Option<u16>,
    esp_to_pc: Option<u16>,
    pc_to_esp: Option<u16>,
    quiet_end_counts: Vec<u16>,
    quiet_reset_restarts: u16,
}

pub(crate) fn run(
    boots: u8,
    minimum_packets: u16,
    quiet_cycles: Option<u16>,
    output: &Path,
    context: &Context<'_>,
) -> Result<()> {
    let adapter = context
        .lab
        .bluetooth_adapter
        .ok_or("set [bluetooth] adapter in the local lab configuration")?;
    for boot in 1..=boots {
        let directory = output.join(format!("boot-{boot:03}"));
        context.with_capture(&directory, |capture| {
            let mut counts = Counts::default();
            let result = probe(capture, adapter, &directory, &mut counts, quiet_cycles)
                .and_then(|()| validate(&counts, minimum_packets, quiet_cycles));
            let cleanup = oer_process::cleanup(|| capture.bluetooth_dtm(Operation::Reset));
            crate::evidence::run::atomic_json(&directory.join("bluetooth-dtm.json"), &serde_json::json!({
                "schema": 2, "quiet_cycles": quiet_cycles, "adapter": adapter.to_string(), "phy": "LE-1M", "channel": 0,
                "payload": "PRBS9", "payload_bytes": 37, "minimum_packets": minimum_packets,
                "counts": counts, "passed": result.is_ok() && cleanup.is_ok(),
                "error": result.as_ref().err().map(ToString::to_string),
                "esp_restored": cleanup.is_ok(), "cleanup_error": cleanup.as_ref().err().map(ToString::to_string),
            }))?;
            match (result, cleanup) {
                (Ok(()), Ok(_)) => Ok(()),
                (Err(error), Ok(_)) => Err(error),
                (Ok(()), Err(error)) => Err(error),
                (Err(error), Err(cleanup)) => Err(format!("{error}; ESP cleanup: {cleanup}").into()),
            }
        })?;
    }
    Ok(())
}

fn probe(
    capture: &SerialCapture,
    adapter: bluetooth::model::Adapter,
    output: &Path,
    counts: &mut Counts,
    quiet_cycles: Option<u16>,
) -> Result<()> {
    let caps = capture.request_capabilities(Duration::from_secs(10))?;
    if !caps.features.bluetooth_dtm {
        return Err("firmware lacks Bluetooth DTM control".into());
    }
    capture.bluetooth_dtm(Operation::Reset)?;
    capture.bluetooth_dtm(Operation::Transmit)?;
    let forward = bluetooth::run_in(&output.join("esp-to-pc"), adapter)?;
    counts.esp_to_pc = forward.rx_packets;
    if end(capture)? != 0 {
        return Err("ESP transmitter returned a receiver count".into());
    }

    // Every cycle proves both terminal paths and a post-Reset restart without
    // a board reset. Retain partial counts so a failed cycle remains visible.
    for _ in 0..quiet_cycles.unwrap_or(1) {
        // An omitted count preserves the original three End controls and
        // one Reset restart, including archived scenario snapshots.
        for _ in 0..if quiet_cycles.is_some() { 1 } else { 3 } {
            capture.bluetooth_dtm(Operation::Receive)?;
            oer_process::sleep(Duration::from_millis(300))?;
            counts.quiet_end_counts.push(end(capture)?);
            counts.esp_silence = counts.quiet_end_counts.first().copied();
        }
        capture.bluetooth_dtm(Operation::Receive)?;
        oer_process::sleep(Duration::from_millis(300))?;
        capture.bluetooth_dtm(Operation::Reset)?;
        capture.bluetooth_dtm(Operation::Receive)?;
        oer_process::sleep(Duration::from_millis(300))?;
        counts.quiet_end_counts.push(end(capture)?);
        counts.quiet_reset_restarts += 1;
    }

    // The peer helper first receives (ESP is also RX: the PC silence control),
    // then transmits for 100 ms into the already armed ESP receiver.
    capture.bluetooth_dtm(Operation::Receive)?;
    let reverse = bluetooth::run_in(&output.join("pc-to-esp"), adapter)?;
    counts.pc_silence = reverse.rx_packets;
    counts.pc_to_esp = Some(end(capture)?);
    if !same_peer(&forward, &reverse) {
        return Err("Bluetooth adapter identity changed between directions".into());
    }
    Ok(())
}

fn same_peer(a: &bluetooth::model::Check, b: &bluetooth::model::Check) -> bool {
    a.address.is_some() && a.address == b.address && a.version.is_some() && a.version == b.version
}

fn end(capture: &SerialCapture) -> Result<u16> {
    match capture.bluetooth_dtm(Operation::End)?.result {
        BluetoothDtmResult::Complete {
            received_packets: Some(count),
        } => Ok(count),
        _ => Err("DTM Test End lacks a receiver count".into()),
    }
}

fn validate(counts: &Counts, minimum: u16, quiet_cycles: Option<u16>) -> Result<()> {
    let (ends, resets) = quiet_cycles.map_or((4, 1), |cycles| (usize::from(cycles) * 2, cycles));
    if resets == 0
        || counts.quiet_end_counts.len() != ends
        || counts.quiet_end_counts.iter().any(|&count| count != 0)
        || counts.quiet_reset_restarts != resets
    {
        return Err("DTM quiet Test End/Reset restart controls failed or are incomplete".into());
    }
    if counts.pc_silence != Some(0) || counts.esp_silence != Some(0) {
        return Err("DTM silence controls failed or are incomplete".into());
    }
    if !counts.esp_to_pc.is_some_and(|count| count >= minimum)
        || !counts.pc_to_esp.is_some_and(|count| count >= minimum)
    {
        return Err(format!(
            "DTM RF delivery below {minimum}: ESP→PC={:?}, PC→ESP={:?}",
            counts.esp_to_pc, counts.pc_to_esp
        )
        .into());
    }
    Ok(())
}

#[derive(serde::Serialize)]
struct PeripheralCycle {
    start: BluetoothPeripheralEvidence,
    fixture: Option<bluetooth::model::ConnectionReset>,
    completion: Option<BluetoothPeripheralEvidence>,
    failure_snapshot_error: Option<String>,
    encrypted_maintenance: Option<encrypted_maintenance::Observation>,
    restart: Option<BluetoothPeripheralEvidence>,
    maintenance: Option<BluetoothPeripheralEvidence>,
}

impl PeripheralCycle {
    fn record_fixture(
        &mut self,
        result: Result<bluetooth::model::ConnectionReset>,
        snapshot: impl FnOnce() -> Result<BluetoothPeripheralEvidence>,
    ) -> Result<()> {
        match result {
            Ok(fixture) => {
                self.fixture = Some(fixture);
                Ok(())
            }
            Err(error) => {
                match snapshot() {
                    Ok(observed) => self.completion = Some(observed),
                    Err(snapshot_error) => {
                        self.failure_snapshot_error = Some(snapshot_error.to_string())
                    }
                }
                Err(error)
            }
        }
    }
}

#[derive(Clone, Copy)]
struct PeripheralBaseline {
    peripheral_runs: u32,
    peripheral_disconnections: u32,
    connection_complete_events: u32,
    disconnection_complete_events: u32,
    connection_update_complete_events: u32,
    channel_map_update_events: u32,
    target_disconnect_commands: u32,
    target_reset_commands: u32,
    host_event_faults: u32,
    host_acl_received_packets: u32,
    host_acl_queued_packets: u32,
    host_acl_transmitted_packets: u32,
    host_acl_completed_packets: u32,
    host_acl_backpressure_holds: u32,
    host_acl_faults: u32,
}

impl From<&BluetoothPeripheralEvidence> for PeripheralBaseline {
    fn from(evidence: &BluetoothPeripheralEvidence) -> Self {
        Self {
            peripheral_runs: evidence.peripheral_runs,
            peripheral_disconnections: evidence.peripheral_disconnections,
            connection_complete_events: evidence.connection_complete_events,
            disconnection_complete_events: evidence.disconnection_complete_events,
            connection_update_complete_events: evidence.connection_update_complete_events,
            channel_map_update_events: evidence.channel_map_update_events,
            target_disconnect_commands: evidence.target_disconnect_commands,
            target_reset_commands: evidence.target_reset_commands,
            host_event_faults: evidence.host_event_faults,
            host_acl_received_packets: evidence.host_acl_received_packets,
            host_acl_queued_packets: evidence.host_acl_queued_packets,
            host_acl_transmitted_packets: evidence.host_acl_transmitted_packets,
            host_acl_completed_packets: evidence.host_acl_completed_packets,
            host_acl_backpressure_holds: evidence.host_acl_backpressure_holds,
            host_acl_faults: evidence.host_acl_faults,
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct PeripheralConfig {
    pub encrypted: bool,
    pub key_refresh: bool,
    pub encrypted_maintenance: bool,
    pub boots: u8,
    pub connections: u8,
    pub hold_millis: u16,
    pub termination: BluetoothPeripheralTermination,
    pub retire_after: bool,
    pub restart_between_connections: bool,
    pub maintain_between_connections: bool,
    pub calibration_threshold: Option<u8>,
}

pub(crate) fn run_peripheral(
    config: PeripheralConfig,
    output: &Path,
    context: &Context<'_>,
) -> Result<()> {
    let PeripheralConfig {
        encrypted,
        key_refresh,
        encrypted_maintenance,
        boots,
        connections,
        hold_millis,
        termination,
        retire_after,
        restart_between_connections,
        maintain_between_connections,
        calibration_threshold,
    } = config;
    let adapter = context
        .lab
        .bluetooth_adapter
        .ok_or("set [bluetooth] adapter in the local lab configuration")?;
    for boot in 1..=boots {
        let directory = output.join(format!("boot-{boot:03}"));
        context.with_capture(&directory, |capture| {
            let mut cycles = Vec::new();
            let mut retirement = None;
            let result = configure_encryption(capture, config.encrypted, None)
                .and_then(|()| probe_peripheral(capture, adapter, config, &directory, &mut cycles))
                .and_then(|()| {
                    if retire_after {
                        retirement = Some(
                            capture.bluetooth_peripheral(BluetoothPeripheralOperation::Retire)?,
                        );
                    }
                    Ok(())
                });
            crate::evidence::run::atomic_json(
                &directory.join("bluetooth-peripheral.json"),
                &serde_json::json!({
                    "schema": 1,
                    "encrypted": encrypted,
                    "key_refresh": key_refresh,
                    "encrypted_maintenance": encrypted_maintenance,
                    "adapter": adapter.to_string(),
                    "connections": connections,
                    "hold_millis": hold_millis,
                    "termination": termination,
                    "retire_after": retire_after,
                    "restart_between_connections": restart_between_connections,
                    "maintain_between_connections": maintain_between_connections,
                    "calibration_threshold": calibration_threshold,
                    "retirement": retirement,
                    "cycles": cycles,
                    "passed": result.is_ok(),
                    "error": result.as_ref().err().map(ToString::to_string),
                }),
            )?;
            result
        })?;
    }
    Ok(())
}

pub(crate) fn configure_encryption(
    capture: &SerialCapture,
    enabled: bool,
    failure: Option<open_esp_radio_hil_protocol::BluetoothSecurityFailure>,
) -> Result<()> {
    if enabled {
        let caps = capture.request_capabilities(Duration::from_secs(10))?;
        if !caps.features.bluetooth_peripheral {
            return Err("firmware lacks Bluetooth peripheral control".into());
        }
        let configured =
            capture.bluetooth_peripheral(BluetoothPeripheralOperation::EncryptedAcl {
                enabled: true,
                failure,
            })?;
        if !matches!(
            configured.result,
            open_esp_radio_hil_protocol::BluetoothPeripheralResult::EncryptedAclConfigured {
                enabled: true, failure: observed
            } if observed == failure
        ) {
            return Err("encrypted ACL Host was not configured".into());
        }
    }
    Ok(())
}

fn probe_peripheral(
    capture: &SerialCapture,
    adapter: bluetooth::model::Adapter,
    config: PeripheralConfig,
    output: &Path,
    cycles: &mut Vec<PeripheralCycle>,
) -> Result<()> {
    let PeripheralConfig {
        connections,
        hold_millis,
        termination,
        restart_between_connections,
        maintain_between_connections,
        calibration_threshold,
        ..
    } = config;
    let caps = capture.request_capabilities(Duration::from_secs(10))?;
    if !caps.features.bluetooth_peripheral {
        return Err("firmware lacks Bluetooth peripheral control".into());
    }
    if config.encrypted_maintenance && !caps.features.bluetooth_phy_maintenance {
        return Err("firmware lacks automatic PHY maintenance".into());
    }
    let encryption_baseline = capture
        .bluetooth_peripheral(BluetoothPeripheralOperation::Snapshot)?
        .encryption
        .unwrap_or_default();
    for cycle in 1..=connections {
        let start_operation = BluetoothPeripheralOperation::StartAdvertising {
            termination,
            hold_millis,
        };
        let start = capture.bluetooth_peripheral(start_operation)?;
        if start.terminal
            || start.saturated
            || start.host_event_faults != 0
            || start.host_acl_faults != 0
        {
            return Err("Bluetooth peripheral was unhealthy before the connection".into());
        }
        let address = start
            .started_address(start_operation)
            .ok_or("Bluetooth peripheral start omitted its public address")?;
        let maintenance_baseline = start.phy_peripheral_maintenance;
        let baseline = PeripheralBaseline::from(&start);
        cycles.push(PeripheralCycle {
            start,
            fixture: None,
            completion: None,
            failure_snapshot_error: None,
            encrypted_maintenance: None,
            restart: None,
            maintenance: None,
        });
        let directory = output.join(format!("connection-{cycle:03}"));
        let connect = || {
            bluetooth::connect_profile_in(
                &directory,
                adapter,
                bluetooth::model::PeerAddress(address),
                hold_millis,
                termination,
                config.encrypted,
                config.key_refresh,
            )
        };
        let fixture = if config.encrypted_maintenance {
            let entry = cycles.last_mut().expect("cycle was inserted");
            let mut observation = encrypted_maintenance::Observation::default();
            let result = encrypted_maintenance::connect_observed(
                capture,
                &entry.start,
                &mut observation,
                connect,
            );
            entry.encrypted_maintenance = Some(observation);
            result
        } else {
            connect()
        };
        cycles
            .last_mut()
            .expect("cycle was inserted")
            .record_fixture(fixture, || {
                capture.bluetooth_peripheral(BluetoothPeripheralOperation::Snapshot)
            })?;

        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            let snapshot = capture.bluetooth_peripheral(BluetoothPeripheralOperation::Snapshot)?;
            // Preserve the actual failing observation in the workload report.
            cycles.last_mut().expect("cycle was inserted").completion = Some(snapshot.clone());
            let complete = peripheral_cycle_complete(baseline, &snapshot, termination)?;
            if complete && caps.features.bluetooth_phy_maintenance {
                require_active_maintenance(maintenance_baseline, &snapshot)?;
            }
            if complete {
                if config.encrypted {
                    require_encrypted_cycle(
                        cycle,
                        config.key_refresh,
                        encryption_baseline,
                        &snapshot,
                    )?;
                }
                break;
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "Bluetooth peripheral cycle {cycle} did not publish and recover before the deadline"
                )
                .into());
            }
            oer_process::sleep(Duration::from_millis(100))?;
        }
        if maintain_between_connections && cycle < connections {
            let operation = calibration_threshold
                .map_or(BluetoothPeripheralOperation::Maintain, |threshold| {
                    BluetoothPeripheralOperation::Calibrate { threshold }
                });
            let maintained = capture.bluetooth_peripheral(operation)?;
            if calibration_threshold == Some(0) {
                let m = maintained
                    .phy_maintenance
                    .as_ref()
                    .ok_or("calibration measurement missing")?;
                if m.invalid
                    || m.common_calibrations <= u32::from(cycle - 1)
                    || m.bluetooth_calibrations <= u32::from(cycle - 1)
                    || m.latest_rx_quality.is_none()
                    || m.rx_gain.completed == 0
                    || m.tx_dc_pwdet.completed == 0
                {
                    return Err("required real calibration branches were not completed".into());
                }
            }
            let expected = u32::from(cycle);
            if !matches!(maintained.result, open_esp_radio_hil_protocol::BluetoothPeripheralResult::Maintained { cycles, .. } if cycles == expected)
            {
                return Err("Bluetooth maintenance counter did not advance in this boot".into());
            }
            cycles.last_mut().expect("cycle was inserted").maintenance = Some(maintained);
        }
        if restart_between_connections && cycle < connections {
            let restarted = capture.bluetooth_peripheral(BluetoothPeripheralOperation::Restart)?;
            let expected = u32::from(cycle);
            if !matches!(restarted.result, open_esp_radio_hil_protocol::BluetoothPeripheralResult::Restarted { cycles, .. } if cycles == expected)
            {
                return Err("Bluetooth restart counter did not advance in this boot".into());
            }
            cycles.last_mut().expect("cycle was inserted").restart = Some(restarted);
        }
    }
    Ok(())
}

fn peripheral_cycle_complete(
    baseline: PeripheralBaseline,
    current: &BluetoothPeripheralEvidence,
    termination: BluetoothPeripheralTermination,
) -> Result<bool> {
    if termination == BluetoothPeripheralTermination::LegacyPeerPowerOff {
        return Err("historical peer-power-off mode cannot be executed".into());
    }
    if current.terminal || current.saturated {
        return Err("Bluetooth peripheral entered a terminal or saturated state".into());
    }
    if current.host_event_faults != baseline.host_event_faults {
        return Err("Bluetooth Host observed an invalid or out-of-order connection event".into());
    }
    if current.host_acl_faults != baseline.host_acl_faults {
        return Err("Bluetooth Host observed an invalid or unqueueable ACL packet".into());
    }
    let expected_runs = baseline
        .peripheral_runs
        .checked_add(1)
        .ok_or("Bluetooth peripheral run counter was exhausted")?;
    let disconnection_delta = u32::from(termination != BluetoothPeripheralTermination::TargetReset);
    let expected_disconnections = baseline
        .peripheral_disconnections
        .checked_add(disconnection_delta)
        .ok_or("Bluetooth peripheral disconnection counter was exhausted")?;
    let expected_connections = baseline
        .connection_complete_events
        .checked_add(1)
        .ok_or("Bluetooth Host connection-event counter was exhausted")?;
    let expected_host_disconnections = baseline
        .disconnection_complete_events
        .checked_add(disconnection_delta)
        .ok_or("Bluetooth Host disconnection-event counter was exhausted")?;
    let expected_connection_updates = baseline
        .connection_update_complete_events
        .checked_add(1)
        .ok_or("Bluetooth Host connection-update counter was exhausted")?;
    let expected_channel_map_updates = baseline
        .channel_map_update_events
        .checked_add(1)
        .ok_or("Bluetooth Link Layer channel-map-update counter was exhausted")?;
    let expected_target_disconnects = baseline
        .target_disconnect_commands
        .checked_add(u32::from(
            termination == BluetoothPeripheralTermination::TargetDisconnect,
        ))
        .ok_or("Bluetooth target Disconnect counter was exhausted")?;
    let expected_target_resets = baseline
        .target_reset_commands
        .checked_add(u32::from(
            termination == BluetoothPeripheralTermination::TargetReset,
        ))
        .ok_or("Bluetooth target Reset counter was exhausted")?;
    let expected_acl_received = baseline
        .host_acl_received_packets
        .checked_add(BLUETOOTH_PERIPHERAL_ACL_LL_FRAGMENTS.saturating_mul(2))
        .ok_or("Bluetooth Host ACL receive counter was exhausted")?;
    let expected_acl_queued = baseline
        .host_acl_queued_packets
        .checked_add(2)
        .ok_or("Bluetooth Host ACL echo counter was exhausted")?;
    let expected_acl_transmitted = baseline
        .host_acl_transmitted_packets
        .checked_add(2)
        .ok_or("Bluetooth Host ACL transmission-completion counter was exhausted")?;
    let expected_acl_completed = baseline
        .host_acl_completed_packets
        .checked_add(BLUETOOTH_PERIPHERAL_ACL_LL_FRAGMENTS.saturating_mul(2))
        .ok_or("Bluetooth Host ACL completion counter was exhausted")?;
    let expected_backpressure_holds = baseline
        .host_acl_backpressure_holds
        .checked_add(1)
        .ok_or("Bluetooth Host ACL backpressure counter was exhausted")?;
    if current.peripheral_disconnections > expected_disconnections
        || current.connection_complete_events > expected_connections
        || current.disconnection_complete_events > expected_host_disconnections
        || current.connection_update_complete_events > expected_connection_updates
        || current.channel_map_update_events > expected_channel_map_updates
        || current.target_disconnect_commands > expected_target_disconnects
        || current.target_reset_commands > expected_target_resets
        || current.host_acl_received_packets > expected_acl_received
        || current.host_acl_queued_packets > expected_acl_queued
        || current.host_acl_transmitted_packets > expected_acl_transmitted
        || current.host_acl_completed_packets > expected_acl_completed
        || current.host_acl_backpressure_holds > expected_backpressure_holds
    {
        return Err(
            "Bluetooth peripheral emitted an unexpected lifecycle or ACL edge for one cycle".into(),
        );
    }
    let complete = current.peripheral_runs >= expected_runs
        && current.peripheral_disconnections == expected_disconnections
        && current.connection_complete_events == expected_connections
        && current.disconnection_complete_events == expected_host_disconnections
        && current.connection_update_complete_events == expected_connection_updates
        && current.channel_map_update_events == expected_channel_map_updates
        && current.target_disconnect_commands == expected_target_disconnects
        && current.target_reset_commands == expected_target_resets
        && current.host_acl_received_packets == expected_acl_received
        && current.host_acl_queued_packets == expected_acl_queued
        && current.host_acl_transmitted_packets == expected_acl_transmitted
        && current.host_acl_completed_packets == expected_acl_completed
        && current.host_acl_backpressure_holds == expected_backpressure_holds;
    let reason_matches = match termination {
        // Peer Reset may send LL_TERMINATE_IND before stopping. This profile
        // accepts remote-user termination or timeout and records which occurred.
        // Abrupt RF-loss evidence retains its separate strict timeout gate.
        BluetoothPeripheralTermination::PeerReset => {
            matches!(current.last_disconnect_reason, Some(0x08 | 0x13))
        }
        BluetoothPeripheralTermination::PeerRfkill => current.last_disconnect_reason == Some(0x08),
        BluetoothPeripheralTermination::TargetDisconnect => {
            current.last_disconnect_reason == Some(0x16)
        }
        BluetoothPeripheralTermination::TargetReset => true,
        BluetoothPeripheralTermination::LegacyPeerPowerOff => unreachable!(),
    };
    if complete && !reason_matches {
        return Err(
            "Bluetooth Host disconnection reason did not match the termination mode".into(),
        );
    }
    Ok(complete)
}

#[cfg(test)]
mod peripheral_tests {
    #[test]
    fn peer_failure_retains_target_snapshot_without_overwriting_original_error() {
        let mut cycle = super::PeripheralCycle {
            start: evidence(),
            fixture: None,
            completion: None,
            failure_snapshot_error: None,
            encrypted_maintenance: None,
            restart: None,
            maintenance: None,
        };
        let mut observed = evidence();
        observed.last_disconnect_reason = Some(0x3d);
        let error = cycle
            .record_fixture(Err("peer refresh failed".into()), || Ok(observed))
            .unwrap_err();
        assert_eq!(error.to_string(), "peer refresh failed");
        assert_eq!(
            cycle.completion.as_ref().unwrap().last_disconnect_reason,
            Some(0x3d)
        );
        assert!(cycle.fixture.is_none());
        assert!(cycle.failure_snapshot_error.is_none());
        cycle.completion = None;
        let error = cycle
            .record_fixture(Err("peer refresh failed".into()), || {
                Err("serial unavailable".into())
            })
            .unwrap_err();
        assert_eq!(error.to_string(), "peer refresh failed");
        assert!(cycle.completion.is_none());
        assert_eq!(
            cycle.failure_snapshot_error.as_deref(),
            Some("serial unavailable")
        );
    }

    use super::*;
    use open_esp_radio_hil_protocol::BluetoothPeripheralResult;

    pub(super) fn evidence() -> BluetoothPeripheralEvidence {
        BluetoothPeripheralEvidence {
            operation: BluetoothPeripheralOperation::Snapshot,
            result: BluetoothPeripheralResult::Snapshot,
            advertising_runs: 1,
            peripheral_runs: 0,
            peripheral_disconnections: 0,
            phy_peripheral_maintenance: 0,
            phy_maintenance: None,
            calibration_traffic: None,
            acl_backpressure: None,
            encryption: None,
            connection_complete_events: 0,
            disconnection_complete_events: 0,
            connection_update_complete_events: 0,
            channel_map_update_events: 0,
            target_disconnect_commands: 0,
            target_reset_commands: 0,
            host_event_faults: 0,
            host_acl_received_packets: 0,
            host_acl_queued_packets: 0,
            host_acl_transmitted_packets: 0,
            host_acl_completed_packets: 0,
            host_acl_backpressure_holds: 0,
            host_acl_faults: 0,
            last_disconnect_reason: None,
            retries: 0,
            terminal: false,
            saturated: false,
            detail_truncated: false,
            detail: Default::default(),
        }
    }

    #[test]
    fn cycle_requires_both_ordered_host_events_and_radio_recovery() {
        let before = evidence();
        let baseline = PeripheralBaseline::from(&before);
        let mut current = BluetoothPeripheralEvidence {
            peripheral_runs: 1,
            connection_complete_events: 1,
            connection_update_complete_events: 1,
            channel_map_update_events: 1,
            ..before.clone()
        };
        assert!(
            !peripheral_cycle_complete(
                baseline,
                &current,
                BluetoothPeripheralTermination::PeerReset,
            )
            .unwrap()
        );
        current.peripheral_disconnections = 1;
        current.disconnection_complete_events = 1;
        current.host_acl_received_packets = BLUETOOTH_PERIPHERAL_ACL_LL_FRAGMENTS * 2;
        current.host_acl_queued_packets = 2;
        current.host_acl_transmitted_packets = 2;
        current.host_acl_completed_packets = BLUETOOTH_PERIPHERAL_ACL_LL_FRAGMENTS * 2;
        current.host_acl_backpressure_holds = 1;
        current.last_disconnect_reason = Some(0x08);
        assert!(
            peripheral_cycle_complete(
                baseline,
                &current,
                BluetoothPeripheralTermination::PeerReset,
            )
            .unwrap()
        );
        current.host_acl_transmitted_packets = 1;
        assert!(
            !peripheral_cycle_complete(
                baseline,
                &current,
                BluetoothPeripheralTermination::PeerReset,
            )
            .unwrap()
        );
        current.host_acl_transmitted_packets = 2;
        current.host_event_faults = 1;
        assert!(
            peripheral_cycle_complete(
                baseline,
                &current,
                BluetoothPeripheralTermination::PeerReset,
            )
            .is_err()
        );
        current.host_event_faults = 0;
        current.host_acl_faults = 1;
        assert!(
            peripheral_cycle_complete(
                baseline,
                &current,
                BluetoothPeripheralTermination::PeerReset,
            )
            .is_err()
        );
        current.host_acl_faults = 0;
        current.host_acl_queued_packets = 3;
        assert!(
            peripheral_cycle_complete(
                baseline,
                &current,
                BluetoothPeripheralTermination::PeerReset,
            )
            .is_err()
        );
    }

    #[test]
    fn target_disconnect_and_reset_require_distinct_edges() {
        let before = evidence();
        for termination in [
            BluetoothPeripheralTermination::TargetDisconnect,
            BluetoothPeripheralTermination::TargetReset,
        ] {
            let baseline = PeripheralBaseline::from(&before);
            let mut current = BluetoothPeripheralEvidence {
                peripheral_runs: 1,
                connection_complete_events: 1,
                connection_update_complete_events: 1,
                channel_map_update_events: 1,
                host_acl_received_packets: BLUETOOTH_PERIPHERAL_ACL_LL_FRAGMENTS * 2,
                host_acl_queued_packets: 2,
                host_acl_transmitted_packets: 2,
                host_acl_completed_packets: BLUETOOTH_PERIPHERAL_ACL_LL_FRAGMENTS * 2,
                host_acl_backpressure_holds: 1,
                ..before.clone()
            };
            match termination {
                BluetoothPeripheralTermination::TargetDisconnect => {
                    current.peripheral_disconnections = 1;
                    current.disconnection_complete_events = 1;
                    current.target_disconnect_commands = 1;
                    current.last_disconnect_reason = Some(0x16);
                }
                BluetoothPeripheralTermination::TargetReset => {
                    current.target_reset_commands = 1;
                }
                BluetoothPeripheralTermination::PeerReset
                | BluetoothPeripheralTermination::PeerRfkill
                | BluetoothPeripheralTermination::LegacyPeerPowerOff => unreachable!(),
            }
            assert!(peripheral_cycle_complete(baseline, &current, termination).unwrap());
            current.target_disconnect_commands = 0;
            current.target_reset_commands = 0;
            assert!(!peripheral_cycle_complete(baseline, &current, termination).unwrap());
        }
    }

    #[test]
    fn peer_rfkill_requires_supervision_timeout_recovery_without_target_commands() {
        let before = evidence();
        let baseline = PeripheralBaseline::from(&before);
        let mut current = BluetoothPeripheralEvidence {
            peripheral_runs: 1,
            peripheral_disconnections: 1,
            connection_complete_events: 1,
            disconnection_complete_events: 1,
            connection_update_complete_events: 1,
            channel_map_update_events: 1,
            host_acl_received_packets: BLUETOOTH_PERIPHERAL_ACL_LL_FRAGMENTS * 2,
            host_acl_queued_packets: 2,
            host_acl_transmitted_packets: 2,
            host_acl_completed_packets: BLUETOOTH_PERIPHERAL_ACL_LL_FRAGMENTS * 2,
            host_acl_backpressure_holds: 1,
            last_disconnect_reason: Some(0x08),
            ..before
        };
        assert!(
            peripheral_cycle_complete(
                baseline,
                &current,
                BluetoothPeripheralTermination::PeerRfkill,
            )
            .unwrap()
        );
        for reason in [Some(0x08), Some(0x13), Some(0x16), Some(0x3d), None] {
            current.last_disconnect_reason = reason;
            let reset = peripheral_cycle_complete(
                baseline,
                &current,
                BluetoothPeripheralTermination::PeerReset,
            );
            assert_eq!(reset.is_ok(), matches!(reason, Some(0x08 | 0x13)));
            let rf_loss = peripheral_cycle_complete(
                baseline,
                &current,
                BluetoothPeripheralTermination::PeerRfkill,
            );
            assert_eq!(rf_loss.is_ok(), reason == Some(0x08));
        }
    }
    #[test]
    fn automatic_image_requires_new_physical_work_inside_each_acl_cycle() {
        let mut current = evidence();
        assert!(require_active_maintenance(0, &current).is_err());
        current.phy_peripheral_maintenance = 3;
        assert!(require_active_maintenance(3, &current).is_err());
        assert!(require_active_maintenance(4, &current).is_err());
        current.phy_peripheral_maintenance = 4;
        assert!(require_active_maintenance(3, &current).is_err());
        current.phy_maintenance = Some(
            open_esp_radio_hil_protocol::BluetoothPhyMaintenanceEvidence {
                restored: 3,
                run_at_micros: Some(100),
                restoration_deadline_micros: Some(200),
                ..Default::default()
            },
        );
        assert!(require_active_maintenance(3, &current).is_err());
        current.phy_maintenance.as_mut().unwrap().restored = 4;
        require_active_maintenance(3, &current).unwrap();
        current.phy_maintenance.as_mut().unwrap().run_at_micros = None;
        assert!(require_active_maintenance(3, &current).is_err());
        // A subsequent idle transaction needs no RUN. The boot aggregate must
        // still prove every earlier active transaction reached guarded RUN.
        current
            .phy_maintenance
            .as_mut()
            .unwrap()
            .restoration_deadline_micros = None;
        require_active_maintenance(3, &current).unwrap();
        current.phy_maintenance.as_mut().unwrap().restored = 3;
        assert!(require_active_maintenance(3, &current).is_err());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn adapter_replacement_is_not_a_bidirectional_check_of_one_peer() {
        let mut a = bluetooth::model::Check::new(
            bluetooth::model::Adapter(0),
            bluetooth::model::DtmVersion::V2,
        );
        let mut b = bluetooth::model::Check::new(
            bluetooth::model::Adapter(0),
            bluetooth::model::DtmVersion::V2,
        );
        assert!(!same_peer(&a, &b));
        a.address = Some("peer-a".into());
        a.version = Some("firmware-a".into());
        b.address = a.address.clone();
        b.version = a.version.clone();
        assert!(same_peer(&a, &b));
        b.address = Some("peer-b".into());
        assert!(!same_peer(&a, &b));
    }
    #[test]
    fn both_receivers_and_both_silence_controls_are_required() {
        let mut counts = Counts {
            pc_silence: Some(0),
            esp_silence: Some(0),
            esp_to_pc: Some(87),
            pc_to_esp: Some(42),
            quiet_end_counts: vec![0; 4],
            quiet_reset_restarts: 2,
        };
        assert!(validate(&counts, 10, Some(2)).is_ok());
        counts.pc_to_esp = Some(0);
        assert!(validate(&counts, 10, Some(2)).is_err());
        counts.pc_to_esp = Some(42);
        counts.esp_silence = Some(1);
        assert!(validate(&counts, 10, Some(2)).is_err());
        counts.esp_silence = None;
        assert!(validate(&counts, 10, Some(2)).is_err());
    }
    #[test]
    fn rf_counts_cannot_hide_missing_quiet_restart_controls() {
        let mut counts = Counts {
            pc_silence: Some(0),
            esp_silence: Some(0),
            esp_to_pc: Some(87),
            pc_to_esp: Some(42),
            quiet_end_counts: vec![0; 4],
            quiet_reset_restarts: 2,
        };
        assert!(validate(&counts, 10, Some(2)).is_ok());
        counts.quiet_reset_restarts = 1;
        assert!(validate(&counts, 10, Some(2)).is_err());
        counts.quiet_reset_restarts = 2;
        counts.quiet_end_counts.pop();
        assert!(validate(&counts, 10, Some(2)).is_err());
        counts.quiet_end_counts.push(1);
        assert!(validate(&counts, 10, Some(2)).is_err());
    }

    #[test]
    fn stress_gate_requires_every_requested_cycle() {
        let mut counts = Counts {
            pc_silence: Some(0),
            esp_silence: Some(0),
            esp_to_pc: Some(87),
            pc_to_esp: Some(42),
            quiet_end_counts: vec![0; 200],
            quiet_reset_restarts: 100,
        };
        assert!(validate(&counts, 10, Some(100)).is_ok());
        counts.quiet_reset_restarts = 99;
        assert!(validate(&counts, 10, Some(100)).is_err());
        counts.quiet_end_counts = vec![0; 4];
        counts.quiet_reset_restarts = 1;
        assert!(validate(&counts, 10, None).is_ok());
        assert!(validate(&counts, 10, Some(100)).is_err());
        assert!(validate(&counts, 10, Some(0)).is_err());
    }
}

/// Physical completion precedes guarded RUN publication. Live samplers must
/// retain their original deadline while this single restoration is pending.
fn active_maintenance_restoration_pending(current: &BluetoothPeripheralEvidence) -> bool {
    current.phy_maintenance.as_ref().is_some_and(|m| {
        !m.invalid
            && m.restored.checked_add(1) == Some(current.phy_peripheral_maintenance)
            && m.physical_finished_at_micros.is_some()
            && m.restoration_deadline_micros.is_some()
            && m.run_at_micros.is_none()
    })
}

fn require_active_maintenance(baseline: u32, current: &BluetoothPeripheralEvidence) -> Result<()> {
    if current.phy_peripheral_maintenance <= baseline {
        return Err(
            "automatic PHY image completed the ACL cycle without active maintenance".into(),
        );
    }
    let m = current
        .phy_maintenance
        .as_ref()
        .ok_or("active maintenance timing missing")?;
    if m.invalid
        || m.restored != current.phy_peripheral_maintenance
        || (m.restoration_deadline_micros.is_some() && m.run_at_micros.is_none())
    {
        return Err("physical maintenance did not complete its guarded RUN restoration".into());
    }
    Ok(())
}

fn require_encrypted_cycle(
    cycle: u8,
    key_refresh: bool,
    baseline: open_esp_radio_hil_protocol::BluetoothEncryptionEvidence,
    snapshot: &BluetoothPeripheralEvidence,
) -> Result<()> {
    let e = snapshot
        .encryption
        .ok_or("missing target encryption evidence")?;
    let expected = u32::from(cycle);
    if !e.enabled
        || e.encrypted
        || e.key_requests.checked_sub(baseline.key_requests)
            != Some(expected * (1 + u32::from(key_refresh)))
        || e.key_replies.checked_sub(baseline.key_replies)
            != Some(expected * (1 + u32::from(key_refresh)))
        || e.encryption_changes
            .checked_sub(baseline.encryption_changes)
            != Some(expected)
        || e.key_refreshes.checked_sub(baseline.key_refreshes)
            != Some(expected * u32::from(key_refresh))
        || e.negative_replies != baseline.negative_replies
        || e.wrong_key_replies != baseline.wrong_key_replies
        || e.faults != 0
        || e.mic_injections != baseline.mic_injections
        || e.mic_injection_armed
    {
        return Err(format!(
            "encrypted cycle {cycle} missing exact key/encryption/retirement transitions: {e:?}"
        )
        .into());
    }
    Ok(())
}

#[cfg(test)]
mod encrypted_tests {
    use super::*;
    use open_esp_radio_hil_protocol::BluetoothEncryptionEvidence;
    #[test]
    fn recovery_keeps_failure_counters_and_requires_new_successful_encryption() {
        for wrong in [false, true] {
            let baseline = BluetoothEncryptionEvidence {
                enabled: true,
                key_requests: 1,
                key_replies: u32::from(wrong),
                negative_replies: u32::from(!wrong),
                wrong_key_replies: u32::from(wrong),
                ..Default::default()
            };
            let mut snapshot = peripheral_tests::evidence();
            snapshot.encryption = Some(BluetoothEncryptionEvidence {
                key_requests: 2,
                key_replies: baseline.key_replies + 1,
                encryption_changes: 1,
                ..baseline
            });
            assert!(require_encrypted_cycle(1, false, baseline, &snapshot).is_ok());
            assert!(require_encrypted_cycle(1, false, Default::default(), &snapshot).is_err());
            snapshot.encryption.as_mut().unwrap().wrong_key_replies += 1;
            assert!(require_encrypted_cycle(1, false, baseline, &snapshot).is_err());
            snapshot.encryption = Some(baseline);
            assert!(require_encrypted_cycle(1, false, baseline, &snapshot).is_err());
            snapshot.encryption.as_mut().unwrap().key_requests = 0;
            assert!(require_encrypted_cycle(1, false, baseline, &snapshot).is_err());
        }
    }

    #[test]
    fn refresh_gate_requires_both_keys_and_one_refresh_per_connection() {
        let mut snapshot = peripheral_tests::evidence();
        for cycle in [1, 2] {
            let complete = BluetoothEncryptionEvidence {
                enabled: true,
                encrypted: false,
                key_requests: cycle * 2,
                key_replies: cycle * 2,
                negative_replies: 0,
                wrong_key_replies: 0,
                encryption_changes: cycle,
                key_refreshes: cycle,
                faults: 0,
                mic_injections: 0,
                mic_injection_armed: false,
            };
            snapshot.encryption = Some(complete);
            assert!(
                require_encrypted_cycle(cycle as u8, true, Default::default(), &snapshot).is_ok()
            );
            assert!(
                require_encrypted_cycle(cycle as u8, false, Default::default(), &snapshot).is_err()
            );
            for invalid in [
                BluetoothEncryptionEvidence {
                    key_requests: cycle,
                    ..complete
                },
                BluetoothEncryptionEvidence {
                    key_replies: cycle,
                    ..complete
                },
                BluetoothEncryptionEvidence {
                    encryption_changes: cycle * 2,
                    ..complete
                },
                BluetoothEncryptionEvidence {
                    key_refreshes: cycle - 1,
                    ..complete
                },
                BluetoothEncryptionEvidence {
                    encrypted: true,
                    ..complete
                },
                BluetoothEncryptionEvidence {
                    faults: 1,
                    ..complete
                },
            ] {
                snapshot.encryption = Some(invalid);
                assert!(
                    require_encrypted_cycle(cycle as u8, true, Default::default(), &snapshot)
                        .is_err()
                );
            }
        }
    }

    #[test]
    fn encrypted_gate_requires_exact_complete_transitions_after_retirement() {
        let mut snapshot = peripheral_tests::evidence();
        assert!(require_encrypted_cycle(1, false, Default::default(), &snapshot).is_err());
        let complete = BluetoothEncryptionEvidence {
            enabled: true,
            encrypted: false,
            key_requests: 1,
            key_replies: 1,
            negative_replies: 0,
            wrong_key_replies: 0,
            encryption_changes: 1,
            key_refreshes: 0,
            faults: 0,
            mic_injections: 0,
            mic_injection_armed: false,
        };
        snapshot.encryption = Some(complete);
        assert!(require_encrypted_cycle(1, false, Default::default(), &snapshot).is_ok());
        assert!(require_encrypted_cycle(2, false, Default::default(), &snapshot).is_err());
        for invalid in [
            BluetoothEncryptionEvidence {
                enabled: false,
                ..complete
            },
            BluetoothEncryptionEvidence {
                encrypted: true,
                ..complete
            },
            BluetoothEncryptionEvidence {
                key_requests: 2,
                ..complete
            },
            BluetoothEncryptionEvidence {
                key_replies: 0,
                ..complete
            },
            BluetoothEncryptionEvidence {
                encryption_changes: 0,
                ..complete
            },
            BluetoothEncryptionEvidence {
                faults: 1,
                ..complete
            },
        ] {
            snapshot.encryption = Some(invalid);
            assert!(require_encrypted_cycle(1, false, Default::default(), &snapshot).is_err());
        }
    }
}
