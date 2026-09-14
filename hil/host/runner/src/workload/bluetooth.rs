//! Independent RF observations with silence controls in both directions.

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

pub(crate) fn run_peripheral(
    boots: u8,
    connections: u8,
    hold_millis: u16,
    termination: BluetoothPeripheralTermination,
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
            let mut cycles = Vec::new();
            let result = probe_peripheral(
                capture,
                adapter,
                connections,
                hold_millis,
                termination,
                &directory,
                &mut cycles,
            );
            crate::evidence::run::atomic_json(
                &directory.join("bluetooth-peripheral.json"),
                &serde_json::json!({
                    "schema": 1,
                    "adapter": adapter.to_string(),
                    "connections": connections,
                    "hold_millis": hold_millis,
                    "termination": termination,
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

fn probe_peripheral(
    capture: &SerialCapture,
    adapter: bluetooth::model::Adapter,
    connections: u8,
    hold_millis: u16,
    termination: BluetoothPeripheralTermination,
    output: &Path,
    cycles: &mut Vec<PeripheralCycle>,
) -> Result<()> {
    let caps = capture.request_capabilities(Duration::from_secs(10))?;
    if !caps.features.bluetooth_peripheral {
        return Err("firmware lacks Bluetooth peripheral control".into());
    }
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
        let baseline = PeripheralBaseline::from(&start);
        cycles.push(PeripheralCycle {
            start,
            fixture: None,
            completion: None,
        });
        let fixture = bluetooth::connect_reset_in(
            &output.join(format!("connection-{cycle:03}")),
            adapter,
            bluetooth::model::PeerAddress(address),
            hold_millis,
            termination,
        )?;
        cycles.last_mut().expect("cycle was inserted").fixture = Some(fixture);

        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            let snapshot = capture.bluetooth_peripheral(BluetoothPeripheralOperation::Snapshot)?;
            let complete = peripheral_cycle_complete(baseline, &snapshot, termination)?;
            cycles.last_mut().expect("cycle was inserted").completion = Some(snapshot);
            if complete {
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
    let expected_reason = match termination {
        BluetoothPeripheralTermination::PeerReset | BluetoothPeripheralTermination::PeerRfkill => {
            Some(0x08)
        }
        BluetoothPeripheralTermination::TargetDisconnect => Some(0x16),
        BluetoothPeripheralTermination::TargetReset => None,
        BluetoothPeripheralTermination::LegacyPeerPowerOff => unreachable!(),
    };
    if complete && expected_reason.is_some() && current.last_disconnect_reason != expected_reason {
        return Err(
            "Bluetooth Host disconnection reason did not match the termination mode".into(),
        );
    }
    Ok(complete)
}

#[cfg(test)]
mod peripheral_tests {
    use super::*;
    use open_esp_radio_hil_protocol::BluetoothPeripheralResult;

    fn evidence() -> BluetoothPeripheralEvidence {
        BluetoothPeripheralEvidence {
            operation: BluetoothPeripheralOperation::Snapshot,
            result: BluetoothPeripheralResult::Snapshot,
            advertising_runs: 1,
            peripheral_runs: 0,
            peripheral_disconnections: 0,
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
        let current = BluetoothPeripheralEvidence {
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn adapter_replacement_is_not_a_bidirectional_check_of_one_peer() {
        let mut a = bluetooth::model::Check::new(bluetooth::model::Adapter(0));
        let mut b = bluetooth::model::Check::new(bluetooth::model::Adapter(0));
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
