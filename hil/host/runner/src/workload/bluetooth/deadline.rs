//! Real DTM sessions are abandoned by the autonomous hard deadline, not Test End.
use crate::{Result, execution::context::Context, fixture::bluetooth};
use open_esp_radio_hil_protocol::BluetoothDtmOperation as Op;
use std::{path::Path, time::Duration};

pub(crate) fn run(output: &Path, context: &Context<'_>) -> Result<()> {
    let adapter = context
        .lab
        .bluetooth_adapter
        .ok_or("missing Bluetooth adapter")?;
    let mut failures = Vec::new();
    for (name, operation) in [("tx", Op::Transmit), ("rx", Op::Receive)] {
        let directory = output.join(name);
        std::fs::create_dir_all(&directory)?;
        let result = context.with_capture(&directory, |capture| {
            if !capture
                .request_capabilities(Duration::from_secs(10))?
                .features
                .bluetooth_phy_maintenance
            {
                return Err("automatic maintenance image required".into());
            }
            capture.bluetooth_dtm(Op::Reset)?;
            // This observation issues no reset or RF command. The ordinary
            // 30-second diagnostic lease is outside this window; PHY hard deferral
            // is 10 seconds after the original due time. Allow boot initialization.
            capture.expect_reboot(Duration::from_secs(7), Duration::from_secs(18))?;
            let started = capture.bluetooth_dtm(operation)?;
            let before = bluetooth::run_in(&directory.join("peer-before"), adapter);
            // A failed peer observation must not erase independent evidence of
            // the target's autonomous reset. Every failed gate still fails the run.
            let reboot = capture.wait_expected_reboot();
            let after = if reboot.is_ok() {
                // No DUT HCI command precedes this RF silence observation.
                bluetooth::run_in(&directory.join("peer-after"), adapter)
            } else {
                Err("RF silence is unproven without the expected reboot".into())
            };
            let fresh = oer_process::cleanup(|| capture.bluetooth_dtm(Op::Reset));
            let mut errors = Vec::new();
            for (label, error) in [
                ("peer-before", before.as_ref().err()),
                ("reboot", reboot.as_ref().err()),
                ("peer-after", after.as_ref().err()),
                ("fresh HCI", fresh.as_ref().err()),
            ] {
                if let Some(error) = error {
                    errors.push(format!("{label}: {error}"));
                }
            }
            if let Ok(before) = &before {
                if operation == Op::Transmit && !before.rx_packets.is_some_and(|n| n >= 10) {
                    errors.push("DTM TX was not observed before deadline".into());
                }
                if operation == Op::Receive && before.rx_packets != Some(0) {
                    errors.push("RX DTM unexpectedly transmitted".into());
                }
            }
            if let Ok(after) = &after {
                if after.rx_packets != Some(0) {
                    errors.push("DTM packets persist after reboot".into());
                }
                if let Ok(before) = &before
                    && !super::same_peer(before, after)
                {
                    errors.push("peer identity changed".into());
                }
            }
            if let Ok(fresh) = &fresh
                && !fresh.software_reset_boot
            {
                errors.push("reboot was not a platform digital-core software reset".into());
            }
            let reset_verified =
                reboot.is_ok() && fresh.as_ref().is_ok_and(|e| e.software_reset_boot);
            crate::evidence::run::atomic_json(
                &directory.join("deadline.json"),
                &serde_json::json!({
                    "schema": 1, "operation": operation, "started": started,
                    "reboot": reboot.as_ref().ok(),
                    "peer_before": before.as_ref().ok(), "peer_after": after.as_ref().ok(),
                    "fresh": fresh.as_ref().ok(),
                    "host_end_or_reset_before_rf_silence": false, "reset_verified": reset_verified,
                    "passed": errors.is_empty(), "errors": errors
                }),
            )?;
            if errors.is_empty() {
                Ok(())
            } else {
                Err(errors.join("; ").into())
            }
        });
        if let Err(error) = result {
            failures.push(format!("{name}: {error}"));
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("; ").into())
    }
}
