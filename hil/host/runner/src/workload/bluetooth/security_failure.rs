//! Initial-key rejection or mismatch, then encrypted recovery in the same HCI epoch.
use super::{PeripheralConfig, configure_encryption, probe_peripheral};
use crate::{Result, execution::context::Context, fixture::bluetooth, session::SerialCapture};
use open_esp_radio_hil_protocol::{
    BluetoothPeripheralEvidence as Evidence, BluetoothPeripheralOperation as Op,
    BluetoothPeripheralTermination as Termination, BluetoothSecurityFailure as Failure,
};
use std::{
    path::Path,
    time::{Duration, Instant},
};

pub(crate) fn run(
    failure: Failure,
    read_version_before_disconnect: bool,
    output: &Path,
    context: &Context<'_>,
) -> Result<()> {
    let adapter = context
        .lab
        .bluetooth_adapter
        .ok_or("missing Bluetooth adapter")?;
    context.with_capture(output, |capture| {
        let mut samples = Vec::new();
        let mut peer = None;
        let mut recovery = Vec::new();
        let mut retirement = None;
        let result = (|| {
            configure_encryption(capture, true, Some(failure))?;
            let op = Op::StartAdvertising {
                termination: Termination::PeerReset,
                hold_millis: 0,
            };
            let started = capture.bluetooth_peripheral(op)?;
            let address = started
                .started_address(op)
                .ok_or("security-failure advertising failed")?;
            samples.push(started);
            peer = Some(bluetooth::security_failure_in(
                &output.join("failed-connection"),
                adapter,
                bluetooth::model::PeerAddress(address),
                failure,
                read_version_before_disconnect,
            )?);
            wait_failure(capture, failure, &mut samples)?;
            probe_peripheral(
                capture,
                adapter,
                PeripheralConfig {
                    encrypted: true,
                    key_refresh: false,
                    boots: 1,
                    connections: 1,
                    hold_millis: 0,
                    termination: Termination::PeerReset,
                    retire_after: true,
                    restart_between_connections: false,
                    maintain_between_connections: false,
                    calibration_threshold: None,
                },
                &output.join("recovery"),
                &mut recovery,
            )?;
            retirement = Some(capture.bluetooth_peripheral(Op::Retire)?);
            Ok(())
        })();
        crate::evidence::run::atomic_json(
            &output.join("bluetooth-security-failure.json"),
            &serde_json::json!({"schema":1, "failure":failure, "read_version_before_disconnect":read_version_before_disconnect, "peer":peer, "samples":samples,
                "recovery":recovery, "retirement":retirement, "passed":result.is_ok(),
                "error":result.as_ref().err().map(ToString::to_string)}),
        )?;
        result
    })
}

fn wait_failure(
    capture: &SerialCapture,
    failure: Failure,
    samples: &mut Vec<Evidence>,
) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        let observed = capture.bluetooth_peripheral(Op::Snapshot)?;
        samples.push(observed);
        if complete_failure(samples.last().expect("sample inserted"), failure)? {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err("target did not retire the failed key connection".into());
        }
        oer_process::sleep(Duration::from_millis(100))?;
    }
}

fn complete_failure(e: &Evidence, failure: Failure) -> Result<bool> {
    let s = e.encryption.ok_or("missing encryption failure evidence")?;
    let missing = failure == Failure::MissingKey;
    if e.terminal
        || e.saturated
        || e.host_event_faults != 0
        || e.host_acl_faults != 0
        || s.faults != 0
        || !s.enabled
        || s.encrypted
        || s.encryption_changes != 0
        || s.key_refreshes != 0
        || e.host_acl_received_packets != 0
        || e.host_acl_queued_packets != 0
        || e.host_acl_transmitted_packets != 0
        || e.host_acl_completed_packets != 0
        || e.host_acl_backpressure_holds != 0
        || e.connection_update_complete_events != 0
        || e.channel_map_update_events != 0
        || e.target_disconnect_commands != 0
        || e.target_reset_commands != 0
        || e.connection_complete_events > 1
        || e.disconnection_complete_events > 1
        || e.peripheral_disconnections > 1
        || s.key_requests > 1
        || s.key_replies > u32::from(!missing)
        || s.negative_replies > u32::from(missing)
        || s.wrong_key_replies > u32::from(!missing)
    {
        return Err(
            "unexpected data, success, fault or lifecycle edge during rejected encryption".into(),
        );
    }
    let complete = e.connection_complete_events == 1
        && e.disconnection_complete_events == 1
        && e.peripheral_disconnections == 1
        && s.key_requests == 1
        && s.key_replies == u32::from(!missing)
        && s.negative_replies == u32::from(missing)
        && s.wrong_key_replies == u32::from(!missing);
    if complete && e.last_disconnect_reason != Some(if missing { 0x13 } else { 0x3d }) {
        return Err("key failure did not produce its exact target disconnect reason".into());
    }
    Ok(complete)
}

#[cfg(test)]
mod tests {
    use super::*;
    use open_esp_radio_hil_protocol::BluetoothEncryptionEvidence;
    #[test]
    fn failure_requires_real_rejection_or_mic_failure_and_zero_application_delivery() {
        for failure in [Failure::MissingKey, Failure::WrongKey] {
            let missing = failure == Failure::MissingKey;
            let mut e = super::super::peripheral_tests::evidence();
            e.connection_complete_events = 1;
            e.disconnection_complete_events = 1;
            e.peripheral_disconnections = 1;
            e.last_disconnect_reason = Some(if missing { 0x13 } else { 0x3d });
            e.encryption = Some(BluetoothEncryptionEvidence {
                enabled: true,
                key_requests: 1,
                key_replies: u32::from(!missing),
                negative_replies: u32::from(missing),
                wrong_key_replies: u32::from(!missing),
                ..Default::default()
            });
            assert!(complete_failure(&e, failure).unwrap());
            let mut bad = e.clone();
            bad.last_disconnect_reason = Some(8);
            assert!(complete_failure(&bad, failure).is_err());
            let mut bad = e.clone();
            bad.host_acl_received_packets = 1;
            assert!(complete_failure(&bad, failure).is_err());
            let mut bad = e.clone();
            bad.encryption.as_mut().unwrap().encryption_changes = 1;
            assert!(complete_failure(&bad, failure).is_err());
            let mut bad = e.clone();
            bad.encryption.as_mut().unwrap().key_requests = 0;
            assert!(!complete_failure(&bad, failure).unwrap());
            let mut bad = e.clone();
            bad.encryption.as_mut().unwrap().faults = 1;
            assert!(complete_failure(&bad, failure).is_err());
        }
    }
}
