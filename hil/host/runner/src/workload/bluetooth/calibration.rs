//! Four Host credits during repeated full calibration on a 7.5-ms plaintext ACL.
use crate::{
    Result,
    execution::context::Context,
    fixture::bluetooth::{att, model::PeerAddress},
    session::SerialCapture,
};
use open_esp_radio_hil_protocol::{
    BluetoothPeripheralEvidence as Evidence, BluetoothPeripheralOperation as Op,
    BluetoothPeripheralResult as Outcome, BluetoothPeripheralTermination,
    bluetooth_calibration_notification,
};
use std::{
    fs,
    io::Write,
    path::Path,
    time::{Duration, Instant},
};

pub(crate) fn run(duration: u16, minimum: u16, output: &Path, context: &Context<'_>) -> Result<()> {
    let adapter = context
        .lab
        .bluetooth_adapter
        .ok_or("missing Bluetooth adapter")?;
    let mut owner = att::Owner::acquire_calibration(adapter, output)?;
    let result = context.with_capture(output, |capture| {
        let mut samples = Vec::<Evidence>::new();
        let probe = exercise(capture, &owner, duration, minimum, output, &mut samples);
        // Release the kernel connection and adapter before restoring thresholds.
        // Failure cleanup is explicit evidence, never an implicit successful run.
        let cleanup = oer_process::cleanup(|| -> Result<()> {
            owner.restore()?;
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                let snapshot = capture.bluetooth_peripheral(Op::Snapshot)?;
                if !snapshot.calibration_traffic.is_some_and(|t| t.connected) { break; }
                if Instant::now() >= deadline { return Err("ACL did not disconnect during fixture cleanup".into()); }
                oer_process::sleep(Duration::from_millis(50))?;
            }
            let disabled = capture.bluetooth_peripheral(Op::CalibrationTraffic { enabled: false })?;
            if disabled.result != (Outcome::CalibrationTrafficConfigured { enabled: false, restored: true }) {
                return Err("original calibration thresholds were not restored".into());
            }
            samples.push(disabled);
            let retired = capture.bluetooth_peripheral(Op::Retire)?;
            if !retired.is_retired(Op::Retire) { return Err("physical cold retirement incomplete".into()); }
            samples.push(retired);
            Ok(())
        });
        crate::evidence::run::atomic_json(&output.join("calibration-traffic.json"), &serde_json::json!({
            "schema":1,"duration_millis":duration,"minimum_calibrations":minimum,
            "samples":samples,"passed":probe.is_ok() && cleanup.is_ok(),
            "error":probe.as_ref().err().map(ToString::to_string),
            "restored":cleanup.is_ok(),"cleanup_error":cleanup.as_ref().err().map(ToString::to_string)
        }))?;
        match (probe, cleanup) { (Ok(()),Ok(())) => Ok(()), (Err(e),Ok(()))|(Ok(()),Err(e))=>Err(e), (Err(e),Err(c))=>Err(format!("{e}; cleanup: {c}").into()) }
    });
    let restore = oer_process::cleanup(|| owner.restore());
    result.and(restore)
}
fn exercise(
    capture: &SerialCapture,
    owner: &att::Owner,
    duration: u16,
    minimum: u16,
    output: &Path,
    samples: &mut Vec<Evidence>,
) -> Result<()> {
    if !capture
        .request_capabilities(Duration::from_secs(10))?
        .features
        .bluetooth_phy_maintenance
    {
        return Err("automatic maintenance image required".into());
    }
    let configured = capture.bluetooth_peripheral(Op::CalibrationTraffic { enabled: true })?;
    if configured.result
        != (Outcome::CalibrationTrafficConfigured {
            enabled: true,
            restored: false,
        })
    {
        return Err("calibration setup rejected".into());
    }
    samples.push(configured);
    let start = Op::StartAdvertising {
        termination: BluetoothPeripheralTermination::PeerReset,
        hold_millis: 0,
    };
    let started = capture.bluetooth_peripheral(start)?;
    let address = PeerAddress(
        started
            .started_address(start)
            .ok_or("advertising address absent")?,
    );
    samples.push(started);
    let peer = owner.connect(address)?;
    peer.send(&[2, 247, 0])?;
    if peer.receive()? != [3, 247, 0] {
        return Err("ATT MTU exchange failed".into());
    }
    let before = wait_credits(capture, 1)?;
    validate_live(&before)?;
    samples.push(before.clone());
    let mut sequence = 0u32;
    let deadline = Instant::now() + Duration::from_millis(u64::from(duration));
    let mut log = fs::File::create(output.join("peer-notifications.jsonl"))?;
    while Instant::now() < deadline {
        capture.bluetooth_peripheral(Op::AclBurst)?;
        for _ in 0..4 {
            let packet = peer.receive()?;
            validate_notification(sequence, &packet)?;
            writeln!(
                log,
                "{}",
                serde_json::json!({"sequence":sequence,"payload":packet})
            )?;
            sequence = sequence
                .checked_add(1)
                .ok_or("notification sequence exhausted")?;
        }
        let sample = wait_credits(capture, 1 + sequence)?;
        validate_live(&sample)?;
        samples.push(sample);
    }
    log.flush()?;
    let after = samples.last().ok_or("no traffic evidence")?;
    validate_calibrations(&before, after, minimum)?;
    if sequence < 4 {
        return Err("no complete four-credit burst".into());
    }
    Ok(())
}
fn validate_notification(sequence: u32, bytes: &[u8]) -> Result<()> {
    if bytes != &bluetooth_calibration_notification(sequence)[4..] {
        return Err(format!(
            "notification {sequence}: loss, duplicate, reordering or payload corruption"
        )
        .into());
    }
    Ok(())
}
fn validate_live(e: &Evidence) -> Result<()> {
    let t = e.calibration_traffic.ok_or("missing traffic evidence")?;
    if t.interval_micros != 7_500 {
        return Err(format!("ACL calibration requires 7500 us, peer selected {} us; no slower-interval fallback is allowed", t.interval_micros).into());
    }
    if !t.enabled
        || !t.connected
        || !t.mtu_exchanged
        || t.faults != 0
        || e.host_event_faults != 0
        || e.host_acl_faults != 0
        || e.terminal
        || e.saturated
        || e.disconnection_complete_events != 0
        || e.connection_update_complete_events != 0
    {
        return Err("ACL traffic connection or Host invariant failed".into());
    }
    Ok(())
}
fn wait_credits(capture: &SerialCapture, expected: u32) -> Result<Evidence> {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let e = capture.bluetooth_peripheral(Op::Snapshot)?;
        let t = e.calibration_traffic.ok_or("missing credit evidence")?;
        validate_live(&e)?;
        if t.completed > expected || t.sent != expected {
            return Err("unexpected HCI credit count".into());
        }
        if t.completed == expected {
            return Ok(e);
        }
        if Instant::now() >= deadline {
            return Err("missing HCI completion credit".into());
        }
        oer_process::sleep(Duration::from_millis(10))?;
    }
}
fn validate_calibrations(before: &Evidence, after: &Evidence, minimum: u16) -> Result<()> {
    // No observation exists before the first physical transaction. This is a
    // zero baseline only when the independent active-completion count is zero.
    let zero = open_esp_radio_hil_protocol::BluetoothPhyMaintenanceEvidence::default();
    let b = match before.phy_maintenance.as_ref() {
        Some(b) => b,
        None if before.phy_peripheral_maintenance == 0 => &zero,
        None => return Err("missing baseline maintenance after active completion".into()),
    };
    let a = after
        .phy_maintenance
        .as_ref()
        .ok_or("missing completed maintenance")?;
    let enough = |old: u32, new: u32| {
        new.checked_sub(old)
            .is_some_and(|n| n >= u32::from(minimum))
    };
    if a.invalid
        || b.invalid
        || !enough(b.common_calibrations, a.common_calibrations)
        || !enough(b.bluetooth_calibrations, a.bluetooth_calibrations)
        || !enough(b.restored, a.restored)
        || !enough(
            before.phy_peripheral_maintenance,
            after.phy_peripheral_maintenance,
        )
        || a.maximum_execution_micros > 20_000
        || a.maximum_restoration_micros > 5_000
        || a.maximum_to_run_micros > 25_000
        || a.latest_rx_quality.is_none()
    {
        return Err(
            "full calibration/restoration evidence is insufficient or exceeds its budget".into(),
        );
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn peer_interval_mismatch_is_explicit_and_not_a_fallback() {
        let mut evidence = super::super::peripheral_tests::evidence();
        evidence.calibration_traffic = Some(
            open_esp_radio_hil_protocol::BluetoothCalibrationTrafficEvidence {
                enabled: true,
                connected: true,
                interval_micros: 45_000,
                ..Default::default()
            },
        );
        let error = validate_live(&evidence).unwrap_err().to_string();
        assert!(error.contains("7500 us"));
        assert!(error.contains("45000 us"));
    }
    #[test]
    fn calibration_gate_requires_new_full_work_and_valid_restoration() {
        use open_esp_radio_hil_protocol::BluetoothPhyMaintenanceEvidence as M;
        let before = super::super::peripheral_tests::evidence();
        let mut after = before.clone();
        after.phy_peripheral_maintenance = 8;
        after.phy_maintenance = Some(M {
            common_calibrations: 8,
            bluetooth_calibrations: 8,
            restored: 8,
            maximum_execution_micros: 16_000,
            maximum_restoration_micros: 2_000,
            maximum_to_run_micros: 18_000,
            latest_rx_quality: Some(Default::default()),
            ..Default::default()
        });
        validate_calibrations(&before, &after, 8).unwrap();
        assert!(validate_calibrations(&after, &after, 8).is_err());
        let mut lost = before.clone();
        lost.phy_peripheral_maintenance = 1;
        assert!(validate_calibrations(&lost, &after, 8).is_err());
        for change in 0..5 {
            let mut broken = after.clone();
            let m = broken.phy_maintenance.as_mut().unwrap();
            match change {
                0 => m.restored = 7,
                1 => m.bluetooth_calibrations = 0,
                2 => m.maximum_restoration_micros = 5001,
                3 => m.invalid = true,
                _ => m.latest_rx_quality = None,
            }
            assert!(validate_calibrations(&before, &broken, 8).is_err());
        }
    }
    #[test]
    fn notification_rejects_loss_reorder_duplicate_and_corruption() {
        let good = bluetooth_calibration_notification(17);
        validate_notification(17, &good[4..]).unwrap();
        assert!(validate_notification(16, &good[4..]).is_err());
        assert!(validate_notification(18, &good[4..]).is_err());
        assert!(validate_notification(17, &good[5..]).is_err());
        let mut damaged = good;
        damaged[63] ^= 1;
        assert!(validate_notification(17, &damaged[4..]).is_err());
    }
}
