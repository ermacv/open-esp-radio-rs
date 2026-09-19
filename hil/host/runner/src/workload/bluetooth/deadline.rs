//! Real DTM sessions end through the selected autonomous platform reset.
use crate::{Result, execution::context::Context, fixture::bluetooth};
use open_esp_radio_hil_protocol::{BluetoothDtmOperation as Op, ResetReason as Reset};
use std::{path::Path, time::Duration};

pub(crate) fn run(output: &Path, context: &Context<'_>, expected_reset: Reset) -> Result<()> {
    let adapter = context
        .lab
        .bluetooth_adapter
        .ok_or("missing Bluetooth adapter")?;
    let mut failures = Vec::new();
    for (name, operation) in [("tx", Op::Transmit), ("rx", Op::Receive)] {
        let directory = output.join(name);
        std::fs::create_dir_all(&directory)?;
        let result = context.with_capture(&directory, |capture| {
            let features = capture.request_capabilities(Duration::from_secs(10))?.features;
            if !image_matches(expected_reset, features) {
                return Err("DTM reset requires the exact advertised reset mechanism".into());
            }
            capture.bluetooth_dtm(Op::Reset)?;
            // This observation issues no reset or RF command. The ordinary
            // 30-second diagnostic lease is outside this window. The production
            // policy expires 10 seconds after due time; diagnostic MWDT1 expires
            // 10 seconds after DTM start. Allow boot initialization for either.
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
                && fresh.reset_reason != expected_reset
            {
                errors.push("reboot does not match the required platform reset mechanism".into());
            }
            let reset_verified =
                reboot.is_ok() && fresh.as_ref().is_ok_and(|e| e.reset_reason == expected_reset);
            let rf = rf_evidence(
                operation,
                adapter,
                reboot.is_ok(),
                before.as_ref().ok(),
                after.as_ref().ok(),
            );
            crate::evidence::run::atomic_json(
                &directory.join("deadline.json"),
                &serde_json::json!({
                    "schema": 3, "expected_reset": expected_reset, "operation": operation, "started": started,
                    "reboot": reboot.as_ref().ok(),
                    "peer_before": before.as_ref().ok(), "peer_after": after.as_ref().ok(),
                    "fresh": fresh.as_ref().ok(),
                    "host_end_or_reset_before_rf_silence": false, "reset_verified": reset_verified,
                    "rf": rf,
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

/// The peer observes only LE 1M DTM packets on channel zero, not broadband RF
/// energy or the DUT's RX/DMA state. Keep those limits in the sealed result.
#[derive(Debug, serde::Serialize)]
struct RfEvidence {
    peer_identity_verified: bool,
    post_boot_dtm_silence_observed: bool,
    /// None for an RX session, which was never a transmitter.
    tx_dtm_cessation_observed: Option<bool>,
    /// No RX/DMA readback exists in this scenario; silence cannot establish it.
    rx_dma_stop_observed: Option<bool>,
}

fn rf_evidence(
    operation: Op,
    adapter: bluetooth::model::Adapter,
    reboot_observed: bool,
    before: Option<&bluetooth::model::Check>,
    after: Option<&bluetooth::model::Check>,
) -> RfEvidence {
    let before = before.filter(|check| check.passed(adapter, bluetooth::model::DtmVersion::V2));
    let after = after.filter(|check| check.passed(adapter, bluetooth::model::DtmVersion::V2));
    let peer_identity_verified = before
        .zip(after)
        .is_some_and(|(before, after)| super::same_peer(before, after));
    let silence = reboot_observed && after.is_some_and(|check| check.rx_packets == Some(0));
    RfEvidence {
        peer_identity_verified,
        post_boot_dtm_silence_observed: silence,
        tx_dtm_cessation_observed: (operation == Op::Transmit).then_some(
            peer_identity_verified
                && silence
                && before.is_some_and(|check| check.rx_packets.is_some_and(|n| n >= 10)),
        ),
        rx_dma_stop_observed: None,
    }
}

fn image_matches(
    expected: Reset,
    features: open_esp_radio_hil_protocol::FeatureCapabilities,
) -> bool {
    match expected {
        Reset::Software => features.bluetooth_phy_maintenance && !features.bluetooth_watchdog_reset,
        Reset::MainWatchdog1 => {
            features.bluetooth_watchdog_reset && !features.bluetooth_phy_maintenance
        }
        Reset::Other => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peer(count: u16) -> bluetooth::model::Check {
        let mut check = bluetooth::model::Check::new(
            bluetooth::model::Adapter(0),
            bluetooth::model::DtmVersion::V2,
        );
        check.address = Some("peer".into());
        check.version = Some("version".into());
        check.initial_powered = Some(false);
        check.initial_soft_blocked = Some(false);
        check.dtm_v2_advertised = true;
        check.rx_started = true;
        check.rx_packets = Some(count);
        check.tx_started = true;
        check.tx_test_end = true;
        check.restored = true;
        check
    }

    #[test]
    fn diagnostic_v1_cannot_supply_watchdog_v2_rf_evidence() {
        let mut before = peer(80);
        let after = peer(0);
        before.dtm_version = bluetooth::model::DtmVersion::V1;
        before.dtm_v1_advertised = true;
        let evidence = rf_evidence(
            Op::Transmit,
            bluetooth::model::Adapter(0),
            true,
            Some(&before),
            Some(&after),
        );
        assert!(!evidence.peer_identity_verified);
        assert_eq!(evidence.tx_dtm_cessation_observed, Some(false));
        let evidence = rf_evidence(
            Op::Transmit,
            bluetooth::model::Adapter(0),
            true,
            Some(&after),
            Some(&before),
        );
        assert!(!evidence.post_boot_dtm_silence_observed);
        assert_eq!(evidence.tx_dtm_cessation_observed, Some(false));
    }

    #[test]
    fn reset_and_rx_silence_do_not_prove_receiver_or_dma_stop() {
        let quiet = peer(0);
        let evidence = rf_evidence(
            Op::Receive,
            bluetooth::model::Adapter(0),
            true,
            Some(&quiet),
            Some(&quiet),
        );
        assert!(evidence.post_boot_dtm_silence_observed);
        assert_eq!(evidence.tx_dtm_cessation_observed, None);
        assert_eq!(evidence.rx_dma_stop_observed, None);
    }

    #[test]
    fn tx_cessation_requires_positive_control_same_peer_and_complete_checks() {
        let active = peer(80);
        let quiet = peer(0);
        let evaluate = |before, after, reboot| {
            rf_evidence(
                Op::Transmit,
                bluetooth::model::Adapter(0),
                reboot,
                before,
                after,
            )
            .tx_dtm_cessation_observed
        };
        assert_eq!(evaluate(Some(&active), Some(&quiet), true), Some(true));
        assert_eq!(evaluate(Some(&quiet), Some(&quiet), true), Some(false));
        assert_eq!(evaluate(None, Some(&quiet), true), Some(false));
        assert_eq!(evaluate(Some(&active), None, true), Some(false));
        assert_eq!(evaluate(Some(&active), Some(&quiet), false), Some(false));
        assert_eq!(evaluate(Some(&active), Some(&active), true), Some(false));
        let mut invalid = peer(0);
        invalid.address = Some("different-peer".into());
        assert_eq!(evaluate(Some(&active), Some(&invalid), true), Some(false));
        let mut unrestored = peer(0);
        unrestored.restored = false;
        assert_eq!(
            evaluate(Some(&active), Some(&unrestored), true),
            Some(false)
        );
    }

    #[test]
    fn reset_mechanisms_cannot_substitute_for_each_other_or_mix() {
        for automatic in [false, true] {
            for watchdog in [false, true] {
                let features = open_esp_radio_hil_protocol::FeatureCapabilities {
                    bluetooth_phy_maintenance: automatic,
                    bluetooth_watchdog_reset: watchdog,
                    ..Default::default()
                };
                assert_eq!(
                    image_matches(Reset::Software, features),
                    automatic && !watchdog
                );
                assert_eq!(
                    image_matches(Reset::MainWatchdog1, features),
                    watchdog && !automatic
                );
                assert!(!image_matches(Reset::Other, features));
            }
        }
    }
}
