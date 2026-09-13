//! Independent RF observations with silence controls in both directions.

use crate::{Result, execution::context::Context, fixture::bluetooth, session::SerialCapture};
use open_esp_radio_hil_protocol::{
    BluetoothDtmOperation as Operation, BluetoothDtmResult, BluetoothPeripheralEvidence,
    BluetoothPeripheralOperation,
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
    host_event_faults: u32,
}

impl From<&BluetoothPeripheralEvidence> for PeripheralBaseline {
    fn from(evidence: &BluetoothPeripheralEvidence) -> Self {
        Self {
            peripheral_runs: evidence.peripheral_runs,
            peripheral_disconnections: evidence.peripheral_disconnections,
            connection_complete_events: evidence.connection_complete_events,
            disconnection_complete_events: evidence.disconnection_complete_events,
            host_event_faults: evidence.host_event_faults,
        }
    }
}

pub(crate) fn run_peripheral(
    boots: u8,
    connections: u8,
    hold_millis: u16,
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
                    "expected_disconnect_reason": 8,
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
    output: &Path,
    cycles: &mut Vec<PeripheralCycle>,
) -> Result<()> {
    let caps = capture.request_capabilities(Duration::from_secs(10))?;
    if !caps.features.bluetooth_peripheral {
        return Err("firmware lacks Bluetooth peripheral control".into());
    }
    for cycle in 1..=connections {
        let start = capture.bluetooth_peripheral(BluetoothPeripheralOperation::StartAdvertising)?;
        if start.terminal || start.saturated || start.host_event_faults != 0 {
            return Err("Bluetooth peripheral was unhealthy before the connection".into());
        }
        let address = start
            .started_address(BluetoothPeripheralOperation::StartAdvertising)
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
        )?;
        cycles.last_mut().expect("cycle was inserted").fixture = Some(fixture);

        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            let snapshot = capture.bluetooth_peripheral(BluetoothPeripheralOperation::Snapshot)?;
            let complete = peripheral_cycle_complete(baseline, &snapshot)?;
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
) -> Result<bool> {
    if current.terminal || current.saturated {
        return Err("Bluetooth peripheral entered a terminal or saturated state".into());
    }
    if current.host_event_faults != baseline.host_event_faults {
        return Err("Bluetooth Host observed an invalid or out-of-order connection event".into());
    }
    let expected_runs = baseline
        .peripheral_runs
        .checked_add(1)
        .ok_or("Bluetooth peripheral run counter was exhausted")?;
    let expected_disconnections = baseline
        .peripheral_disconnections
        .checked_add(1)
        .ok_or("Bluetooth peripheral disconnection counter was exhausted")?;
    let expected_connections = baseline
        .connection_complete_events
        .checked_add(1)
        .ok_or("Bluetooth Host connection-event counter was exhausted")?;
    let expected_host_disconnections = baseline
        .disconnection_complete_events
        .checked_add(1)
        .ok_or("Bluetooth Host disconnection-event counter was exhausted")?;
    if current.peripheral_disconnections > expected_disconnections
        || current.connection_complete_events > expected_connections
        || current.disconnection_complete_events > expected_host_disconnections
    {
        return Err(
            "Bluetooth peripheral emitted more than one lifecycle edge for one cycle".into(),
        );
    }
    let complete = current.peripheral_runs >= expected_runs
        && current.peripheral_disconnections == expected_disconnections
        && current.connection_complete_events == expected_connections
        && current.disconnection_complete_events == expected_host_disconnections;
    if complete && current.last_disconnect_reason != Some(0x08) {
        return Err("Bluetooth Host disconnection reason was not supervision timeout".into());
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
            host_event_faults: 0,
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
            ..before.clone()
        };
        assert!(!peripheral_cycle_complete(baseline, &current).unwrap());
        current.peripheral_disconnections = 1;
        current.disconnection_complete_events = 1;
        current.last_disconnect_reason = Some(0x08);
        assert!(peripheral_cycle_complete(baseline, &current).unwrap());
        current.host_event_faults = 1;
        assert!(peripheral_cycle_complete(baseline, &current).is_err());
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
